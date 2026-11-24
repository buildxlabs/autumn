//! `AniZip` episode metadata.
//!
//! [`AniZip::media`] fetches and caches the episode data of one media by
//! AniList id: [`Media::episodes`] holds the regular episodes (number,
//! absolute number, title, air date, length, thumbnail URL, overview) keyed
//! by the API's key, specials ("S1", "S2", …), and any OP/ED entries the API
//! lists; [`Media::mappings`] holds the ids the API maps the media to
//! (AniDB and other providers). Requests go through `net` (not the AniList
//! rate limiter) into a stale-first disk cache: [`Aging::Airing`] responses
//! stay fresh for 24 h, [`Aging::Finished`] ones for 7 days.

use std::{
    collections::BTreeMap,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Deserialize, Serialize};
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

/// The episode data of one media, as the `AniZip` API returns it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Media {
    /// Titles per language.
    #[serde(default)]
    pub titles: BTreeMap<String, String>,
    /// Episodes keyed by the entry key: a regular number ("1") or a special
    /// ("S1").
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
    /// Titles per language.
    #[serde(default, rename = "title")]
    pub titles: BTreeMap<String, String>,
    /// The air date.
    #[serde(default, rename = "airDate")]
    pub air_date: Option<String>,
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

impl Episode {
    /// The episode number for regular episodes: the key parsed as a number.
    #[must_use]
    pub fn number(&self) -> Option<u32> {
        self.key.parse().ok()
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
    /// A media `AniZip` does not know returns `Ok(None)`; a cached value is
    /// served when the fetch fails and one exists.
    ///
    /// # Errors
    ///
    /// Returns [`Error::Net`] or [`Error::Decode`] when the request fails,
    /// after `net`'s retries, or the response does not decode.
    pub async fn media(&self, anilist_id: u32, aging: Aging) -> Result<Option<Media>, Error> {
        let key = format!("anizip:media:{anilist_id}");
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
            Err(Error::Net(net::Error::Http { status: 404, .. })) => Ok(None),
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

    /// Fetch the media's response.
    async fn fetch(&self, anilist_id: u32) -> Result<Media, Error> {
        let request = Request::new(
            Method::Get,
            format!("{}/v1/episodes?anilist_id={anilist_id}", self.endpoint),
        );
        let response = self.http.read(request).await?;
        let bytes = response.bytes().await?;
        let mut media: Media = serde_json::from_slice(&bytes).map_err(Error::Decode)?;
        // The API keys `episodes` by the entry key, so fill the field on it.
        for (key, episode) in &mut media.episodes {
            episode.key.clone_from(key);
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

use serde::de::DeserializeOwned;

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

    /// A loopback server serving the recorded response, counting requests.
    struct Mock {
        anizip: AniZip,
        requests: Arc<AtomicUsize>,
        _dir: TempDir,
    }

    impl Mock {
        async fn new() -> Self {
            Self::with_status(StatusCode::OK).await
        }

        async fn with_status(status: StatusCode) -> Self {
            let requests = Arc::new(AtomicUsize::new(0));
            let seen = Arc::clone(&requests);
            let app = Router::new().route(
                "/v1/episodes",
                get(move || {
                    let seen = Arc::clone(&seen);
                    async move {
                        seen.fetch_add(1, Ordering::SeqCst);
                        (status, FIXTURE.to_owned())
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
    async fn media_returns_none_when_ani_zip_does_not_know_the_media() {
        let mock = Mock::with_status(StatusCode::NOT_FOUND).await;
        let media = mock.anizip.media(1234, Aging::Finished).await.unwrap();
        assert!(media.is_none());
    }
}
