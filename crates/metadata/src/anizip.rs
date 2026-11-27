//! `AniZip` episode metadata.
//!
//! [`AniZip::media`] fetches and caches the episode data of one media by
//! AniList id: [`Media::episodes`] holds numbered episodes, specials ("S1",
//! "S2", …) and the opening/ending entries the API lists ("O1", "E1", …),
//! keyed by the API's key. Each entry has its season and absolute numbers,
//! its AniDB episode id (AniDB eid), titles, air date, length, thumbnail
//! URL and overview; [`Media::mappings`]
//! holds the ids the API maps the media to (AniDB and other providers).
//! Requests go through `net` (not the AniList rate limiter) into a
//! stale-first disk cache: [`Aging::Airing`] responses stay fresh for 24 h,
//! [`Aging::Finished`] ones for 7 days. A stale response is returned at once
//! when the refresh fails; a caller wanting background refresh can await the
//! fetch elsewhere, since this client serves metadata on demand.

use std::{
    collections::BTreeMap,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize, de::DeserializeOwned};
use tracing::warn;

use net::{Method, Request};

use crate::Error;

/// The `AniZip` endpoint.
pub const ANIZIP_ENDPOINT: &str = "https://api.ani.zip";

/// How long a cached response stays fresh: aired shows for 24 h, finished
/// ones for 7 days.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum Aging {
    /// Airing media: fresh for 24 h.
    Airing,
    /// Finished media: fresh for 7 days.
    Finished,
}

impl Aging {
    /// The cache TTL of the aging.
    #[must_use]
    pub const fn ttl(self) -> Duration {
        match self {
            Self::Airing => Duration::from_hours(24),
            Self::Finished => Duration::from_hours(24 * 7),
        }
    }
}

/// The kind of an entry in [`Media::episodes`], from its API key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum EntryKind {
    /// A numbered episode.
    Regular,
    /// A special, keyed `S1`, `S2`, …
    Special,
    /// An opening, keyed `O1`, `O2`, …
    Opening,
    /// An ending, keyed `E1`, `E2`, …
    Ending,
    /// A non-credits version, keyed `NC1`, …
    NonCredit,
    /// A key this client does not classify.
    Other,
}

/// The episode data of one media, as the `AniZip` API returns it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Media {
    /// Titles per language.
    #[serde(default)]
    pub titles: BTreeMap<String, String>,
    /// Entries keyed by the API's key: a number ("9"), a special ("S1") or a
    /// theme ("O1").
    #[serde(default)]
    pub episodes: BTreeMap<String, Episode>,
    /// The number of regular episodes.
    #[serde(default, rename = "episodeCount")]
    pub episode_count: Option<u32>,
    /// The number of special episodes.
    #[serde(default, rename = "specialCount")]
    pub special_count: Option<u32>,
    /// The ids the `API` maps the media to.
    #[serde(default)]
    pub mappings: Option<Mappings>,
}

/// One entry of the `AniZip` episode map.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Episode {
    /// The entry's key in the API's `episodes` map: the episode number
    /// ("1") or a special ("S1").
    #[serde(default)]
    pub key: String,
    /// The absolute episode number, when the API lists one.
    #[serde(default, rename = "absoluteEpisodeNumber")]
    pub absolute_number: Option<u32>,
    /// The season number, when the API lists one. Season 0 holds specials.
    #[serde(default, rename = "seasonNumber")]
    pub season_number: Option<u32>,
    /// The episode number within that season, when the API lists one.
    #[serde(default, rename = "episodeNumber")]
    pub season_episode: Option<u32>,
    /// The AniDB episode id (**AniDB eid**), when the API lists one.
    #[serde(default, rename = "anidbEid")]
    pub anidb_eid: Option<u32>,
    /// Titles per language.
    #[serde(default, rename = "title")]
    pub titles: BTreeMap<String, String>,
    /// The air date, from either spelling `AniZip` uses.
    #[serde(default, rename = "airDate")]
    pub air_date: Option<String>,
    /// The lower-case spelling some entries use instead of `airDate`.
    #[serde(default)]
    airdate: Option<String>,
    /// Length in minutes.
    #[serde(default)]
    pub length: Option<u32>,
    /// A thumbnail URL.
    #[serde(default)]
    pub image: Option<String>,
    /// The overview.
    #[serde(default)]
    pub overview: Option<String>,
}

impl Media {
    /// The entries of `kind`, in the API's key order.
    pub fn entries(&self, kind: EntryKind) -> impl Iterator<Item = &Episode> {
        self.episodes
            .values()
            .filter(move |episode| episode.kind() == kind)
    }
}

impl Episode {
    /// The episode number for regular episodes: the key parsed as a number.
    #[must_use]
    pub fn number(&self) -> Option<u32> {
        self.key.parse().ok()
    }

    /// Whether this is a numbered episode, a special or a theme entry.
    #[must_use]
    pub fn kind(&self) -> EntryKind {
        let key = self.key.as_str();
        if key.parse::<u32>().is_ok() {
            return EntryKind::Regular;
        }
        if key.starts_with("NC") {
            return EntryKind::NonCredit;
        }
        match key.as_bytes().first() {
            Some(b'S') => EntryKind::Special,
            Some(b'O') => EntryKind::Opening,
            Some(b'E') => EntryKind::Ending,
            _ => EntryKind::Other,
        }
    }

    /// The air date under either key the API uses.
    #[must_use]
    pub fn aired(&self) -> Option<&str> {
        self.air_date.as_deref().or(self.airdate.as_deref())
    }
}

/// The ids the `AniZip` response maps the media to (`mappings`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mappings {
    /// An AniList media id.
    pub anilist_id: Option<u32>,
    /// A Kitsu id.
    pub kitsu_id: Option<u32>,
    /// A MyAnimeList id.
    pub mal_id: Option<u32>,
    /// An AniDB anime id.
    pub anidb_id: Option<u32>,
    /// An `AniSearch` id.
    pub anisearch_id: Option<u32>,
    /// A `LiveChart` id.
    pub livechart_id: Option<u32>,
    /// A TVDB id.
    pub thetvdb_id: Option<u32>,
    /// An Animeplanet id.
    pub animeplanet_id: Option<String>,
    /// An `IMDb` id.
    pub imdb_id: Option<String>,
    /// A TMDB id.
    pub themoviedb_id: Option<String>,
    /// The `AniZip` `type` (TV, MOVIE, …).
    #[serde(rename = "type")]
    pub kind: Option<String>,
}

/// The `AniZip` client: fetch and cache episode data by AniList id.
///
/// Requests go through `net` (not the AniList rate limiter). Cloning shares
/// the HTTP client and the cache directory.
#[derive(Debug, Clone)]
pub struct AniZip {
    http: net::Client,
    disk: net::PosterCache,
    endpoint: String,
}

impl AniZip {
    /// A client for the public `AniZip` endpoint.
    #[must_use]
    pub fn new(http: net::Client, disk: net::PosterCache) -> Self {
        Self::with_endpoint(http, disk, ANIZIP_ENDPOINT)
    }

    /// A client for another endpoint, such as a loopback mock in tests.
    #[must_use]
    pub fn with_endpoint(
        http: net::Client,
        disk: net::PosterCache,
        endpoint: impl Into<String>,
    ) -> Self {
        Self {
            http,
            disk,
            endpoint: endpoint.into(),
        }
    }

    /// The episode data of the media with the AniList id `anilist_id`,
    /// cached for `aging.ttl()`.
    ///
    /// A media `AniZip` does not know returns `Ok(None)` when there is no
    /// cached value; a cached value is served when the fetch fails.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Net`] or [`Error::Decode`] when the request fails,
    /// after `net`'s retries, or the response does not decode. With a cached
    /// response, a failed fetch serves that value instead of the error.
    pub async fn media(&self, anilist_id: u32, aging: Aging) -> Result<Option<Media>, Error> {
        let key = format!("v1:anizip:media:{anilist_id}");
        let stored: Option<Stored<Media>> = read_stored(&self.disk, &key).await;
        if let Some(stored) = &stored
            && !is_stale(stored.updated_at, aging.ttl())
        {
            return Ok(Some(stored.value.clone()));
        }
        match self.fetch(anilist_id).await {
            Ok(media) => {
                store_stored(&self.disk, &key, &media).await;
                Ok(Some(media))
            }
            Err(Error::Net(net::Error::Http { status: 404, .. })) if stored.is_none() => Ok(None),
            Err(error) => {
                if let Some(stored) = stored {
                    warn!(%error, "anizip: serving the stale response");
                    Ok(Some(stored.value))
                } else {
                    Err(error)
                }
            }
        }
    }

    /// The AniDB episode id (**AniDB eid**) of one episode: the episode-level
    /// **Cross-reference** the `AniZip` response lists as `anidbEid`.
    ///
    /// `anilist_id` is the AniList media id; `episode_key` is the `AniZip`
    /// key in [`Media::episodes`] ("1", "S1", "O1", …). `None` when the
    /// response has no such key or lists no eid for it, or when no response
    /// is available. Served from the same cache as [`AniZip::media`].
    pub async fn anidb_eid(&self, anilist_id: u32, episode_key: &str, aging: Aging) -> Option<u32> {
        let media = self.media(anilist_id, aging).await.ok().flatten()?;
        media.episodes.get(episode_key)?.anidb_eid
    }

    /// Fetch the media's response.
    async fn fetch(&self, anilist_id: u32) -> Result<Media, Error> {
        let request = Request::new(
            Method::Get,
            format!("{}/v1/episodes?anilist_id={anilist_id}", self.endpoint),
        );
        let response = self.http.read(request).await?;
        let bytes = response.bytes().await?;
        let mut media: Media = serde_json::from_slice(&bytes).map_err(Error::Decode)?;
        for (key, episode) in &mut media.episodes {
            // The API keys `episodes` by the entry key, so fill the field.
            episode.key.clone_from(key);
            // Some entries spell the air date `airdate`, with no `airDate`.
            if episode.air_date.is_none() {
                episode.air_date = episode.airdate.take();
            }
        }
        Ok(media)
    }
}

/// A cached value and when it was stored.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Stored<T> {
    /// When the value was stored.
    updated_at: SystemTime,
    /// The value.
    value: T,
}

/// Read the cached media under `key`, if any.
async fn read_stored<T: DeserializeOwned>(disk: &net::PosterCache, key: &str) -> Option<Stored<T>> {
    let bytes = disk.read_metadata(key).await.ok().flatten()?;
    let (seconds, value): (u64, T) = rmp_serde::from_slice(&bytes).ok()?;
    Some(Stored {
        updated_at: UNIX_EPOCH + Duration::from_secs(seconds),
        value,
    })
}

/// Store the media under `key`. A write failure is logged: the value is
/// still usable.
async fn store_stored<T: Serialize>(disk: &net::PosterCache, key: &str, value: &T) {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs());
    if let Ok(bytes) = rmp_serde::to_vec_named(&(seconds, value))
        && let Err(error) = disk.write_metadata(key, bytes).await
    {
        warn!(%error, "anizip: cache write failed");
    }
}

/// Whether the stored value is older than `ttl`. A time in the future
/// (the clock moved back) counts as stale.
fn is_stale(updated_at: SystemTime, ttl: Duration) -> bool {
    SystemTime::now()
        .duration_since(updated_at)
        .map_or(true, |age| age >= ttl)
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };

    use axum::{Router, http::StatusCode, routing::get};
    use tempfile::TempDir;
    use tokio::net::TcpListener;

    use super::*;

    /// The recorded response of `api.ani.zip` for Frieren, trimmed to a few
    /// episodes (the full one is ~54 KB).
    const FIXTURE: &str = include_str!("../tests/fixtures/anizip_frieren.json");
    /// A trimmed One Piece response, for multi-season numbers and entry kinds.
    const OFFSET_FIXTURE: &str = include_str!("../tests/fixtures/anizip_onepiece.json");

    /// A loopback server serving the recorded response, counting requests.
    struct Mock {
        anizip: AniZip,
        requests: Arc<AtomicUsize>,
        _dir: TempDir,
    }

    impl Mock {
        async fn new() -> Self {
            Self::with_body(StatusCode::OK, FIXTURE).await
        }

        async fn with_status(status: StatusCode) -> Self {
            Self::with_body(status, FIXTURE).await
        }

        async fn with_body(status: StatusCode, body: &'static str) -> Self {
            let requests = Arc::new(AtomicUsize::new(0));
            let seen = Arc::clone(&requests);
            let app = Router::new().route(
                "/v1/episodes",
                get(move || {
                    let seen = Arc::clone(&seen);
                    async move {
                        seen.fetch_add(1, Ordering::SeqCst);
                        (status, body.to_owned())
                    }
                }),
            );
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            tokio::spawn(async move {
                axum::serve(listener, app).await.unwrap();
            });
            let dir = TempDir::new().unwrap();
            Self {
                anizip: AniZip::with_endpoint(
                    net::Client::builder().build().unwrap(),
                    net::PosterCache::new(dir.path()).unwrap(),
                    format!("http://{address}"),
                ),
                requests,
                _dir: dir,
            }
        }

        fn requests(&self) -> usize {
            self.requests.load(Ordering::SeqCst)
        }
    }

    #[test]
    fn the_anizip_fixture_decodes_anidb_eids() {
        let media: Media = serde_json::from_str(FIXTURE).unwrap();

        let first = media.episodes.get("1").unwrap();
        assert_eq!(first.anidb_eid, Some(271_418));
        let special = media.episodes.get("S1").unwrap();
        assert_eq!(special.anidb_eid, Some(273_428));
    }

    #[test]
    fn a_payload_without_anidb_eid_still_parses() {
        let episode: Episode = serde_json::from_str("{}").unwrap();
        assert_eq!(episode.anidb_eid, None);
    }

    #[test]
    fn the_anizip_fixture_decodes_titles_and_episodes() {
        let media: Media = serde_json::from_str(FIXTURE).unwrap();

        assert_eq!(
            media.titles.get("x-jat").map(String::as_str),
            Some("Sousou no Frieren")
        );
        assert!(media.episodes.contains_key("1"));
        assert!(media.episodes.contains_key("S1"));
        let episode = media.episodes.get("1").unwrap();
        assert_eq!(episode.absolute_number, Some(1));
        assert_eq!(episode.air_date.as_deref(), Some("2023-09-29"));
        assert_eq!(episode.length, Some(30));
        assert!(episode.titles.contains_key("ja"));
        let mappings = media.mappings.unwrap();
        assert_eq!(mappings.anilist_id, Some(154_587));
        assert_eq!(mappings.mal_id, Some(52_991));
        assert_eq!(mappings.anidb_id, Some(17_617));
        assert_eq!(mappings.kitsu_id, Some(46_474));
        assert_eq!(mappings.kind.as_deref(), Some("TV"));
        assert_eq!(
            mappings.animeplanet_id.as_deref(),
            Some("frieren-beyond-journeys-end")
        );
        assert_eq!(mappings.imdb_id.as_deref(), Some("tt22248376"));
        assert_eq!(mappings.themoviedb_id.as_deref(), Some("209867"));
    }

    #[tokio::test]
    async fn the_multi_season_fixture_decodes_offsets_and_entry_kinds() {
        let mock = Mock::with_body(StatusCode::OK, OFFSET_FIXTURE).await;

        let media = mock
            .anizip
            .media(21, Aging::Finished)
            .await
            .unwrap()
            .unwrap();

        let ninth = media.episodes.get("9").unwrap();
        assert_eq!(ninth.season_number, Some(2));
        assert_eq!(ninth.season_episode, Some(1));
        assert_eq!(ninth.absolute_number, Some(9));
        assert_eq!(ninth.kind(), EntryKind::Regular);
        let special = media.episodes.get("S1").unwrap();
        assert_eq!(special.kind(), EntryKind::Special);
        assert_eq!(special.season_number, Some(0));
        assert_eq!(special.absolute_number, None);
        let opening = media.episodes.get("O1").unwrap();
        assert_eq!(opening.kind(), EntryKind::Opening);
        assert_eq!(
            opening.aired(),
            Some("2020-03-16"),
            "the lower-case airdate is kept"
        );
        assert_eq!(
            media.episodes.get("100").unwrap().aired(),
            Some("2002-02-10")
        );
        assert_eq!(media.entries(EntryKind::Regular).count(), 2);
        assert_eq!(media.entries(EntryKind::Special).count(), 1);
        assert_eq!(media.entries(EntryKind::Opening).count(), 1);
        assert_eq!(media.episode_count, Some(1184));
        assert_eq!(media.special_count, Some(51));
        let mappings = media.mappings.unwrap();
        assert_eq!(mappings.anilist_id, Some(21));
        assert_eq!(mappings.anidb_id, Some(69));
    }

    #[tokio::test]
    async fn an_unreadable_cached_blob_is_a_miss() {
        let mock = Mock::new().await;
        mock.anizip
            .disk
            .write_metadata("v1:anizip:media:154587", b"not msgpack".to_vec())
            .await
            .unwrap();

        let media = mock
            .anizip
            .media(154_587, Aging::Finished)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(mock.requests(), 1, "the bad blob was treated as a miss");
        assert_eq!(
            media.titles.get("x-jat").map(String::as_str),
            Some("Sousou no Frieren")
        );
    }

    #[tokio::test]
    async fn media_fetches_once_and_the_second_call_makes_no_request() {
        let mock = Mock::new().await;
        let first = mock.anizip.media(154_587, Aging::Finished).await.unwrap();
        let media = first.as_ref().unwrap();
        assert_eq!(media.episodes.get("1").and_then(Episode::number), Some(1));
        assert_eq!(media.episodes.get("1").unwrap().key, "1");
        assert_eq!(mock.requests(), 1);

        let second = mock.anizip.media(154_587, Aging::Finished).await.unwrap();
        assert_eq!(second, first);
        assert_eq!(
            mock.requests(),
            1,
            "the second call is served from the cache"
        );
    }

    #[tokio::test]
    async fn anidb_eid_covers_a_regular_episode_a_special_and_a_missing_key() {
        let mock = Mock::new().await;

        assert_eq!(
            mock.anizip.anidb_eid(154_587, "1", Aging::Finished).await,
            Some(271_418),
            "a regular episode's AniDB eid"
        );
        assert_eq!(
            mock.anizip.anidb_eid(154_587, "S1", Aging::Finished).await,
            Some(273_428),
            "a special's AniDB eid"
        );
        assert_eq!(
            mock.anizip.anidb_eid(154_587, "999", Aging::Finished).await,
            None,
            "a missing key"
        );
        assert_eq!(mock.requests(), 1, "the response was cached");
    }

    #[tokio::test]
    async fn anidb_eid_is_none_when_ani_zip_does_not_know_the_media() {
        let mock = Mock::with_status(StatusCode::NOT_FOUND).await;
        assert_eq!(
            mock.anizip.anidb_eid(1234, "1", Aging::Finished).await,
            None
        );
    }

    /// Seed a cached response of `age`, as an earlier fetch would have left it.
    async fn seed(disk: &net::PosterCache, id: u32, media: &Media, age: Duration) {
        let seconds = (SystemTime::now() - age)
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let key = format!("v1:anizip:media:{id}");
        let bytes = rmp_serde::to_vec_named(&(seconds, media)).unwrap();
        disk.write_metadata(&key, bytes).await.unwrap();
    }

    #[tokio::test]
    async fn a_stale_response_is_refreshed() {
        let mock = Mock::new().await;
        let media: Media = serde_json::from_str(FIXTURE).unwrap();
        seed(&mock.anizip.disk, 154_587, &media, Aging::Airing.ttl() * 2).await;

        let refreshed = mock
            .anizip
            .media(154_587, Aging::Airing)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(mock.requests(), 1, "a stale response is fetched again");
        assert_eq!(
            refreshed.titles.get("x-jat").map(String::as_str),
            Some("Sousou no Frieren")
        );
        assert_eq!(
            mock.anizip
                .media(154_587, Aging::Airing)
                .await
                .unwrap()
                .unwrap(),
            refreshed,
            "the refreshed response was stored"
        );
        assert_eq!(mock.requests(), 1);
    }

    #[tokio::test]
    async fn the_ttl_depends_on_whether_the_media_is_airing() {
        let mock = Mock::new().await;
        let media: Media = serde_json::from_str(FIXTURE).unwrap();
        let age = Duration::from_hours(48);
        seed(&mock.anizip.disk, 154_587, &media, age).await;

        mock.anizip
            .media(154_587, Aging::Finished)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(mock.requests(), 0, "two days is fresh for finished media");

        mock.anizip
            .media(154_587, Aging::Airing)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(mock.requests(), 1, "two days is stale for airing media");
    }

    #[tokio::test]
    async fn a_failed_fetch_keeps_the_stale_response() {
        let mock = Mock::with_status(StatusCode::NOT_FOUND).await;
        let media: Media = serde_json::from_str(FIXTURE).unwrap();
        seed(&mock.anizip.disk, 154_587, &media, Aging::Airing.ttl() * 2).await;

        let served = mock
            .anizip
            .media(154_587, Aging::Airing)
            .await
            .unwrap()
            .unwrap();

        assert_eq!(served, media, "the stale response is served on a 404");
    }

    #[tokio::test]
    async fn media_returns_none_when_ani_zip_does_not_know_the_media() {
        let mock = Mock::with_status(StatusCode::NOT_FOUND).await;
        let media = mock.anizip.media(1234, Aging::Finished).await.unwrap();
        assert!(media.is_none());
    }
}
