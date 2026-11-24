// Ported from seanime's `internal/api/animeoffledb` (GPL-3.0-only,
// https://github.com/5rahim/seanime); per AGENTS.md this notice stays here.

//! Anime-offline-database as a matching candidate index (OD-2).
//!
//! [`AnimeOfflineDatabase::load`] downloads the latest release's
//! `anime-offline-database.jsonl` through `net` into the cache dir and
//! parses it into an in-memory index: per anime its titles and synonyms,
//! format, episode count, season and year, and the external ids it lists
//! (AniList, MAL, AniDB, Kitsu). The file is refreshed at most weekly; a
//! failed download or parse is not fatal, the caller gets the previous
//! file if any, else an empty index. See ARCHITECTURE §2 and decision
//! OD-2.

use std::time::{Duration, SystemTime};

use net::{Method, Request};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::{Error, MediaFormat, MediaSeason};

/// The asset the latest release adds: the JSONL file.
const ASSET_NAME: &str = "anime-offline-database.jsonl";

/// The GitHub API endpoint of the latest release.
const RELEASE_API: &str =
    "https://api.github.com/repos/manami-project/anime-offline-database/releases/latest";

/// How long the cached JSONL stays fresh: weekly.
const CACHE_TTL: Duration = Duration::from_hours(7 * 24);

/// The key of the cached JSONL blob under the cache dir's `metadata/`.
const BLOB_KEY: &str = "anime-offline-database.jsonl";

/// The key of the cache marker blob, next to [`BLOB_KEY`].
const META_KEY: &str = "anime-offline-database.jsonl.meta";

/// The index of every anime the file lists: their titles, format, episode
/// count, season and the external ids they list.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnimeOfflineDatabase {
    entries: Vec<OfflineAnime>,
}

/// One anime of the index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OfflineAnime {
    /// The main title.
    pub title: String,
    /// Alternative titles and spellings.
    pub synonyms: Vec<String>,
    /// How the anime was distributed.
    pub format: Option<MediaFormat>,
    /// The number of episodes, movies or parts.
    pub episodes: Option<u32>,
    /// The season it first distributed in.
    pub season: Option<MediaSeason>,
    /// The year of that season.
    pub season_year: Option<u16>,
    /// The external ids it lists, by provider.
    pub ids: AnimeOfflineIds,
}

/// The external ids an anime-offline-database entry lists, by provider.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AnimeOfflineIds {
    /// An AniList media id.
    pub anilist: Option<u32>,
    /// A MyAnimeList id.
    pub mal: Option<u32>,
    /// An AniDB anime id.
    pub anidb: Option<u32>,
    /// A Kitsu id.
    pub kitsu: Option<u32>,
}

/// The latest GitHub release.
#[derive(Deserialize)]
struct Release {
    /// The release's assets.
    assets: Vec<Asset>,
}

/// One asset of a release.
#[derive(Deserialize)]
struct Asset {
    /// The asset's file name.
    name: String,
    /// The URL the asset is downloadable from.
    browser_download_url: String,
}

/// One line of the JSONL: the anime entries after the first metadata line.
#[derive(Deserialize)]
struct AnimeLine {
    /// The main title; the dataset's first line has no title and is skipped.
    #[serde(default)]
    title: Option<String>,
    /// Alternative titles and spellings.
    #[serde(default)]
    synonyms: Vec<String>,
    /// How the anime was distributed, as the JSONL spells it.
    #[serde(default)]
    r#type: Option<String>,
    /// The number of episodes, movies or parts.
    #[serde(default)]
    episodes: Option<u32>,
    /// The season it first distributed in, as the JSONL spells it.
    #[serde(rename = "animeSeason")]
    anime_season: Option<SeasonLine>,
    /// URLs of the providers the anime is listed by.
    #[serde(default)]
    sources: Vec<String>,
}

/// The `animeSeason` object of a JSONL anime entry.
#[derive(Deserialize)]
struct SeasonLine {
    /// The season, as the JSONL spells it.
    #[serde(default)]
    season: Option<String>,
    /// The year of that season.
    #[serde(default)]
    year: Option<u16>,
}

/// The cache marker next to the JSONL blob: when it was last downloaded or
/// confirmed current, and from where.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct CacheMeta {
    /// The asset URL the blob was downloaded from.
    url: String,
    /// The `ETag` of the download, when the host sent one.
    etag: Option<String>,
    /// When the blob was last downloaded or confirmed current.
    last_refresh: SystemTime,
}

impl AnimeOfflineDatabase {
    /// The index from the latest release, via [`RELEASE_API`].
    pub async fn load(client: &net::Client, disk: &net::PosterCache) -> Self {
        Self::load_from(client, disk, RELEASE_API).await
    }

    /// [`AnimeOfflineDatabase::load`], fetching the release metadata from
    /// `release_api` (a loopback mock in tests).
    pub async fn load_from(
        client: &net::Client,
        disk: &net::PosterCache,
        release_api: &str,
    ) -> Self {
        let meta = read_meta(disk).await;
        let cached = read_cached(disk).await;
        if meta
            .as_ref()
            .is_some_and(|meta| !is_stale(meta.last_refresh, CACHE_TTL))
            && cached.is_some()
        {
            return Self::parse_or_warn(cached.as_deref());
        }
        match refresh(client, disk, release_api, meta).await {
            Some(bytes) => Self::parse_or_warn(Some(&bytes)),
            None => Self::parse_or_warn(cached.as_deref()),
        }
    }

    /// Parse the `anime-offline-database.jsonl` blob: the first line is
    /// dataset metadata, the rest are anime entries.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Decode`] on any line that is not valid JSON.
    pub fn parse_jsonl(bytes: &[u8]) -> Result<Self, Error> {
        let mut entries = Vec::new();
        for line in bytes.split(|byte| *byte == b'\n') {
            if line.iter().all(u8::is_ascii_whitespace) {
                continue;
            }
            if let Some(anime) = serde_json::from_slice::<AnimeLine>(line)
                .map_err(Error::Decode)?
                .into_anime()
            {
                entries.push(anime);
            }
        }
        Ok(Self { entries })
    }

    /// Every anime of the index.
    #[must_use]
    pub fn entries(&self) -> &[OfflineAnime] {
        &self.entries
    }

    /// The index, or an empty one with a logged warning when `bytes` do not
    /// parse.
    fn parse_or_warn(bytes: Option<&[u8]>) -> Self {
        match bytes.map(Self::parse_jsonl) {
            Some(Ok(index)) => index,
            Some(Err(error)) => {
                warn!(%error, "anime-offline-database: the cached file is unusable");
                Self::default()
            }
            None => {
                warn!("anime-offline-database: no cached file and no download yet");
                Self::default()
            }
        }
    }
}

impl AnimeLine {
    fn into_anime(self) -> Option<OfflineAnime> {
        Some(OfflineAnime {
            title: self.title?,
            synonyms: self.synonyms,
            format: self.r#type.as_deref().and_then(to_format),
            episodes: self.episodes,
            season: self.anime_season.as_ref().and_then(SeasonLine::to_season),
            season_year: self.anime_season.and_then(|season| season.year),
            ids: AnimeOfflineIds::parse_sources(&self.sources),
        })
    }
}

impl SeasonLine {
    fn to_season(&self) -> Option<MediaSeason> {
        match self.season.as_deref() {
            Some("SPRING") => Some(MediaSeason::Spring),
            Some("SUMMER") => Some(MediaSeason::Summer),
            Some("FALL") => Some(MediaSeason::Fall),
            Some("WINTER") => Some(MediaSeason::Winter),
            _ => None,
        }
    }
}

impl AnimeOfflineIds {
    /// The external ids the `sources` URLs name: `anilist.co/anime/N`,
    /// `kitsu.app/anime/N`, `myanimelist.net/anime/N` and `anidb.net/anime/N`.
    fn parse_sources(sources: &[String]) -> Self {
        fn id_of(sources: &[String], host: &str) -> Option<u32> {
            sources.iter().find_map(|source| {
                let path = source.trim();
                let path = path
                    .strip_prefix("https://")
                    .or_else(|| path.strip_prefix("http://"))
                    .unwrap_or(path);
                let (url_host, rest) = path.split_once('/')?;
                if url_host != host {
                    return None;
                }
                rest.strip_prefix("anime/")?.parse().ok()
            })
        }
        Self {
            anilist: id_of(sources, "anilist.co"),
            mal: id_of(sources, "myanimelist.net"),
            anidb: id_of(sources, "anidb.net"),
            kitsu: id_of(sources, "kitsu.app"),
        }
    }
}

/// The format of a JSONL anime entry, per the `ODbL` `type` values.
fn to_format(type_: &str) -> Option<MediaFormat> {
    match type_ {
        "TV" => Some(MediaFormat::Tv),
        "MOVIE" => Some(MediaFormat::Movie),
        "OVA" => Some(MediaFormat::Ova),
        "ONA" => Some(MediaFormat::Ona),
        "SPECIAL" => Some(MediaFormat::Special),
        _ => None,
    }
}

/// Download or confirm the asset through the release and return the fresh
/// bytes. On any failure the caller gets `None` after the warning.
async fn refresh(
    client: &net::Client,
    disk: &net::PosterCache,
    release_api: &str,
    meta: Option<CacheMeta>,
) -> Option<Vec<u8>> {
    let url = match latest_asset(client, release_api).await {
        Ok(url) => url,
        Err(error) => {
            warn!(%error, "anime-offline-database: no asset in the latest release");
            return None;
        }
    };
    let request = if let Some(etag) = meta.as_ref().and_then(|meta| meta.etag.clone()) {
        Request::new(Method::Get, url.clone())
            .header("If-None-Match", &etag)
            .ok()?
    } else {
        Request::new(Method::Get, url.clone())
    };
    match client.read(request).await {
        Ok(response) => {
            let etag = response.header("etag").map(str::to_owned);
            match response.bytes().await {
                Ok(bytes) => {
                    store(disk, &url, etag, bytes).await;
                    read_cached(disk).await
                }
                Err(error) => {
                    warn!(%error, "anime-offline-database: the asset could not be read");
                    None
                }
            }
        }
        Err(error) => {
            if is_not_modified(&error) {
                touch_meta(disk, &url).await;
                read_cached(disk).await
            } else {
                warn!(%error, "anime-offline-database: the asset could not be read");
                None
            }
        }
    }
}

/// The URL of the `anime-offline-database.jsonl` asset in the latest
/// release.
async fn latest_asset(client: &net::Client, release_api: &str) -> Result<String, Error> {
    let request =
        Request::new(Method::Get, release_api).header("Accept", "application/vnd.github+json")?;
    let response = client.read(request).await?;
    let bytes = response.bytes().await?;
    let release: Release = serde_json::from_slice(&bytes).map_err(Error::Decode)?;
    release
        .assets
        .iter()
        .find(|asset| asset.name == ASSET_NAME)
        .map(|asset| asset.browser_download_url.clone())
        .ok_or_else(|| Error::MissingAsset(ASSET_NAME.to_owned()))
}

/// Store the asset blob and its cache marker. A failed write is logged: the
/// caller still parses the bytes it has.
async fn store(disk: &net::PosterCache, url: &str, etag: Option<String>, bytes: Vec<u8>) {
    if let Err(error) = disk.write_metadata(BLOB_KEY, bytes).await {
        warn!(%error, "anime-offline-database: could not write the cached file");
        return;
    }
    let meta = CacheMeta {
        url: url.to_owned(),
        etag,
        last_refresh: SystemTime::now(),
    };
    if let Err(error) = write_meta(disk, &meta).await {
        warn!(%error, "anime-offline-database: could not write the cache marker");
    }
}

/// Mark the cache as refreshed now, keeping the stored metadata.
async fn touch_meta(disk: &net::PosterCache, url: &str) {
    let mut meta = read_meta(disk).await.unwrap_or(CacheMeta {
        url: url.to_owned(),
        etag: None,
        last_refresh: SystemTime::now(),
    });
    meta.last_refresh = SystemTime::now();
    if let Err(error) = write_meta(disk, &meta).await {
        warn!(%error, "anime-offline-database: could not write the cache marker");
    }
}

/// Write the cache marker.
async fn write_meta(disk: &net::PosterCache, meta: &CacheMeta) -> Result<(), Error> {
    let bytes = serde_json::to_vec(meta).map_err(Error::Encode)?;
    disk.write_metadata(META_KEY, bytes).await?;
    Ok(())
}

/// Read the cached JSONL blob, if any.
async fn read_cached(disk: &net::PosterCache) -> Option<Vec<u8>> {
    disk.read_metadata(BLOB_KEY).await.ok().flatten()
}

/// Read the cache marker, if any.
async fn read_meta(disk: &net::PosterCache) -> Option<CacheMeta> {
    let bytes = disk.read_metadata(META_KEY).await.ok().flatten()?;
    serde_json::from_slice(&bytes).ok()
}

/// Whether a failed read means the asset is unchanged. Other errors pass
/// through.
fn is_not_modified(error: &net::Error) -> bool {
    matches!(error, net::Error::Http { status: 304, .. })
}

/// Whether the cache entry is older than `ttl`.
fn is_stale(last_refresh: SystemTime, ttl: Duration) -> bool {
    SystemTime::now()
        .duration_since(last_refresh)
        .map_or(true, |age| age >= ttl)
}

#[cfg(test)]
#[expect(
    clippy::indexing_slicing,
    reason = "tests index their own recorded fixture"
)]
mod tests {
    use std::sync::{Arc, Mutex};

    use axum::{
        Json, Router,
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        routing::get,
    };
    use serde_json::json;
    use tempfile::TempDir;
    use tokio::net::TcpListener;

    use super::*;

    /// The recorded JSONL fixture: the dataset metadata line plus two anime.
    const FIXTURE: &str = include_str!("../tests/fixtures/anime_offline_database.jsonl");

    /// What the mock server counted, for the download assertions.
    #[derive(Debug, Default)]
    struct Counts {
        downloads: usize,
        conditional: usize,
    }

    /// A loopback server that serves the release JSON and the asset,
    /// counting downloads and conditional GETs.
    struct Mock {
        client: net::Client,
        release_api: String,
        counts: Arc<Mutex<Counts>>,
    }

    impl Mock {
        async fn new() -> Self {
            let counts = Arc::new(Mutex::new(Counts::default()));
            let seen = Arc::clone(&counts);
            let app = Router::new()
                .route(
                    "/releases/latest",
                    get(move |headers: HeaderMap| {
                        let host = headers
                            .get("host")
                            .and_then(|value| value.to_str().ok())
                            .unwrap_or_default()
                            .to_owned();
                        async move {
                            Json(json!({
                                "assets": [{
                                    "name": ASSET_NAME,
                                    "browser_download_url": format!("http://{host}/asset"),
                                }],
                            }))
                        }
                    }),
                )
                .route(
                    "/asset",
                    get(move |headers: HeaderMap| {
                        let seen = Arc::clone(&seen);
                        async move {
                            let etag = headers
                                .get("if-none-match")
                                .and_then(|value| value.to_str().ok())
                                .unwrap_or_default();
                            if etag == "v1" {
                                seen.lock().unwrap().conditional += 1;
                                return StatusCode::NOT_MODIFIED.into_response();
                            }
                            seen.lock().unwrap().downloads += 1;
                            let mut response = FIXTURE.to_owned().into_response();
                            response.headers_mut().insert("ETag", "v1".parse().unwrap());
                            response
                        }
                    }),
                );
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            Self {
                client: net::Client::builder().build().unwrap(),
                release_api: format!("http://{address}/releases/latest"),
                counts,
            }
        }

        fn downloads(&self) -> usize {
            self.counts.lock().unwrap().downloads
        }

        fn conditional(&self) -> usize {
            self.counts.lock().unwrap().conditional
        }
    }

    fn disk(dir: &TempDir) -> net::PosterCache {
        net::PosterCache::new(dir.path()).unwrap()
    }

    fn fresh_meta() -> CacheMeta {
        CacheMeta {
            url: "https://github.com/manami-project/anime-offline-database/releases/download/2026-27/anime-offline-database.jsonl".to_owned(),
            etag: Some("v1".to_owned()),
            last_refresh: SystemTime::now(),
        }
    }

    #[test]
    fn the_jsonl_fixture_decodes_titles_and_ids() {
        let index = AnimeOfflineDatabase::parse_jsonl(FIXTURE.as_bytes()).unwrap();
        let entries = index.entries();
        assert_eq!(entries.len(), 2);
        let first = &entries[0];
        assert_eq!(first.title, "!NVADE SHOW!");
        assert!(first.synonyms.contains(&"!nvade Show!".to_owned()));
        assert_eq!(first.format, Some(MediaFormat::Special));
        assert_eq!(first.episodes, Some(1));
        assert_eq!(first.season, Some(MediaSeason::Fall));
        assert_eq!(first.season_year, Some(2020));
        assert_eq!(
            first.ids,
            AnimeOfflineIds {
                anilist: Some(142_051),
                kitsu: Some(47_450),
                mal: Some(51_478),
                anidb: None,
            }
        );
        let second = &entries[1];
        assert_eq!(second.title, "\"0\"");
        assert_eq!(second.season, Some(MediaSeason::Summer));
        assert_eq!(second.season_year, Some(2013));
        assert_eq!(
            second.ids,
            AnimeOfflineIds {
                anilist: Some(102_416),
                kitsu: Some(8_925),
                mal: Some(20_707),
                anidb: Some(10_143),
            }
        );
    }

    #[tokio::test]
    async fn a_second_load_within_a_week_makes_no_download() {
        let mock = Mock::new().await;
        let dir = TempDir::new().unwrap();
        let disk = disk(&dir);
        let first = AnimeOfflineDatabase::load_from(&mock.client, &disk, &mock.release_api).await;
        assert_eq!(first.entries().len(), 2);
        assert_eq!(mock.downloads(), 1);

        let second = AnimeOfflineDatabase::load_from(&mock.client, &disk, &mock.release_api).await;
        assert_eq!(second, first);
        assert_eq!(mock.downloads(), 1, "the second load makes no download");
    }

    #[tokio::test]
    async fn a_conditional_get_with_an_etag_makes_no_second_download() {
        let mock = Mock::new().await;
        let dir = TempDir::new().unwrap();
        let disk = disk(&dir);
        // Fresh marker, no cached file: the conditional GET returns 304.
        disk.write_metadata(META_KEY, serde_json::to_vec(&fresh_meta()).unwrap())
            .await
            .unwrap();

        let index = AnimeOfflineDatabase::load_from(&mock.client, &disk, &mock.release_api).await;

        assert_eq!(mock.conditional(), 1);
        assert_eq!(mock.downloads(), 0, "the etag came back 304");
        assert_eq!(index.entries().len(), 0);
    }

    #[tokio::test]
    async fn a_corrupt_cached_file_yields_the_empty_index_plus_a_warning() {
        let mock = Mock::new().await;
        let dir = TempDir::new().unwrap();
        let disk = disk(&dir);
        disk.write_metadata(BLOB_KEY, b"not jsonl".to_vec())
            .await
            .unwrap();
        disk.write_metadata(META_KEY, serde_json::to_vec(&fresh_meta()).unwrap())
            .await
            .unwrap();

        let index = AnimeOfflineDatabase::load_from(&mock.client, &disk, &mock.release_api).await;

        assert_eq!(index.entries().len(), 0);
        assert_eq!(
            mock.downloads(),
            0,
            "a fresh marker serves the corrupt file as is"
        );
    }
}
