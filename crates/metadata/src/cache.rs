//! A stale-first disk cache for catalog responses (decision #31).
//!
//! Each response is a `MessagePack` blob in `<cache>/metadata/`, written
//! through [`net::PosterCache`] so blobs and posters share one size cap.
//! A cached value is returned at once; when it is older than its time to
//! live, a [`Refresh`] comes with it. Only what the user views is cached:
//! AniList's terms forbid crawling.

use std::{
    fmt,
    future::Future,
    pin::Pin,
    task::{Context, Poll},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use serde::{Serialize, de::DeserializeOwned};
use tracing::warn;

use crate::{AiringEpisode, AniList, DiscoverLoad, Error, HomeLoad, MediaDetail};

/// How long an airing schedule stays fresh.
const AIRING_TTL: Duration = Duration::from_hours(1);
/// How long Home and Discover stay fresh.
const CATALOG_TTL: Duration = Duration::from_hours(6);
/// How long an Entry page stays fresh.
const ENTRY_TTL: Duration = Duration::from_hours(24);

/// Bumped when a cached type changes meaning, so old blobs are never read.
/// Blobs store field names, so adding a field with a serde default needs
/// no bump; a missing required field makes the blob a miss.
const KEY_VERSION: u32 = 1;

/// AniList catalog queries behind the stale-first disk cache.
///
/// Cloning shares the HTTP client and the cache directory.
#[derive(Debug, Clone)]
pub struct CatalogCache {
    anilist: AniList,
    disk: net::PosterCache,
}

/// A catalog value and when it was fetched from AniList.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cached<T> {
    /// The value.
    pub value: T,
    /// When AniList returned it: the "last updated" time.
    pub updated_at: SystemTime,
    /// Whether a refresh failed to reach AniList, so this value is all
    /// there is until the network returns.
    pub offline: bool,
}

/// What a cached query returns at once.
#[derive(Debug)]
pub struct Load<T> {
    /// The value to show now: from disk, or just fetched on a cache miss.
    pub cached: Cached<T>,
    /// Present when [`Load::cached`] is stale: await it for the refreshed
    /// value. Dropping it cancels the refresh.
    pub refresh: Option<Refresh<T>>,
}

/// A pending refresh of a stale value.
///
/// It resolves to the new value, now stored on disk, or, when AniList
/// can't be reached ([`Error::Net`]), to the stale value marked
/// [`Cached::offline`].
///
/// # Errors
///
/// Resolves to [`Error::Api`] or [`Error::Decode`] when AniList answers
/// with an error or an unexpected shape; keep showing the stale value.
pub struct Refresh<T>(Pin<Box<dyn Future<Output = Result<Cached<T>, Error>> + Send>>);

impl<T> Future for Refresh<T> {
    type Output = Result<Cached<T>, Error>;

    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        self.0.as_mut().poll(cx)
    }
}

impl<T> fmt::Debug for Refresh<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Refresh").finish_non_exhaustive()
    }
}

impl CatalogCache {
    /// Cache `anilist`'s responses in `disk`'s `metadata/` directory.
    #[must_use]
    pub fn new(anilist: AniList, disk: net::PosterCache) -> Self {
        Self { anilist, disk }
    }

    /// [`AniList::home_load`], fresh for 6 hours.
    ///
    /// # Errors
    ///
    /// Only on a cache miss: the fetch's error.
    pub async fn home(&self) -> Result<Load<HomeLoad>, Error> {
        let anilist = self.anilist.clone();
        self.load("home".to_owned(), CATALOG_TTL, async move {
            anilist.home_load().await
        })
        .await
    }

    /// [`AniList::discover_load`], fresh for 6 hours.
    ///
    /// # Errors
    ///
    /// Only on a cache miss: the fetch's error.
    pub async fn discover(&self, missed_sequels_user: &str) -> Result<Load<DiscoverLoad>, Error> {
        let anilist = self.anilist.clone();
        let user = missed_sequels_user.to_owned();
        let key = format!("discover:{user}");
        self.load(key, CATALOG_TTL, async move {
            anilist.discover_load(&user).await
        })
        .await
    }

    /// [`AniList::airing_schedule`], fresh for 1 hour.
    ///
    /// # Errors
    ///
    /// Only on a cache miss: the fetch's error.
    pub async fn airing_schedule(&self) -> Result<Load<Vec<AiringEpisode>>, Error> {
        let anilist = self.anilist.clone();
        self.load("airing_schedule".to_owned(), AIRING_TTL, async move {
            anilist.airing_schedule().await
        })
        .await
    }

    /// [`AniList::media`] for the Entry page, fresh for 24 hours.
    ///
    /// # Errors
    ///
    /// Only on a cache miss: the fetch's error.
    pub async fn media(&self, anilist_id: u32) -> Result<Load<MediaDetail>, Error> {
        let anilist = self.anilist.clone();
        self.load(format!("media:{anilist_id}"), ENTRY_TTL, async move {
            anilist.media(anilist_id).await
        })
        .await
    }

    /// Serve `key` from disk, fetching with `fetch` on a miss and
    /// refreshing with it once the entry is older than `ttl`.
    async fn load<T, Fut>(&self, key: String, ttl: Duration, fetch: Fut) -> Result<Load<T>, Error>
    where
        T: Serialize + DeserializeOwned + Clone + Send + Sync + 'static,
        Fut: Future<Output = Result<T, Error>> + Send + 'static,
    {
        let key = format!("v{KEY_VERSION}:{key}");
        let Some(stale) = self.read::<T>(&key).await else {
            let value = fetch.await?;
            return Ok(Load {
                cached: self.store(&key, value).await,
                refresh: None,
            });
        };
        if !is_stale(stale.updated_at, ttl) {
            return Ok(Load {
                cached: stale,
                refresh: None,
            });
        }
        let cache = self.clone();
        let refresh = Refresh(Box::pin({
            let stale = stale.clone();
            async move {
                match fetch.await {
                    Ok(value) => Ok(cache.store(&key, value).await),
                    Err(Error::Net(error)) if unreachable(&error) => {
                        warn!(%error, "AniList unreachable; serving the cached value");
                        Ok(Cached {
                            offline: true,
                            ..stale
                        })
                    }
                    Err(error) => Err(error),
                }
            }
        }));
        Ok(Load {
            cached: stale,
            refresh: Some(refresh),
        })
    }

    /// The entry under `key`. A missing, unreadable or outdated blob is a
    /// miss.
    async fn read<T: DeserializeOwned>(&self, key: &str) -> Option<Cached<T>> {
        let bytes = match self.disk.read_metadata(key).await {
            Ok(bytes) => bytes?,
            Err(error) => {
                warn!(%error, "catalog cache read failed");
                return None;
            }
        };
        match rmp_serde::from_slice::<(u64, T)>(&bytes) {
            Ok((seconds, value)) => Some(Cached {
                value,
                updated_at: UNIX_EPOCH + Duration::from_secs(seconds),
                offline: false,
            }),
            Err(error) => {
                warn!(%error, "dropping an unreadable catalog cache entry");
                None
            }
        }
    }

    /// Store `value` as fetched now. A failed write is logged: the value
    /// is still good to show.
    async fn store<T: Serialize>(&self, key: &str, value: T) -> Cached<T> {
        // Whole seconds, as on disk, so a reload reads back the same time.
        let updated_at = UNIX_EPOCH
            + Duration::from_secs(
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .map_or(0, |since| since.as_secs()),
            );
        self.store_at(key, &value, updated_at).await;
        Cached {
            value,
            updated_at,
            offline: false,
        }
    }

    async fn store_at<T: Serialize>(&self, key: &str, value: &T, updated_at: SystemTime) {
        let seconds = updated_at
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        let result = match rmp_serde::to_vec_named(&(seconds, value)) {
            Ok(bytes) => self.disk.write_metadata(key, bytes).await,
            Err(error) => {
                warn!(%error, "could not encode a catalog cache entry");
                return;
            }
        };
        if let Err(error) = result {
            warn!(%error, "catalog cache write failed");
        }
    }
}

/// Whether a failed request means AniList can't be reached right now, so
/// the cached value should be shown as offline. Other HTTP errors, such as
/// a 404 for a removed media, are real answers and pass through.
fn unreachable(error: &net::Error) -> bool {
    match error {
        net::Error::Transport { .. }
        | net::Error::Body { .. }
        | net::Error::RateLimited { .. }
        | net::Error::AniListUnavailable => true,
        net::Error::Http { status, .. } => *status >= 500,
        _ => false,
    }
}

/// Whether an entry fetched at `updated_at` has outlived `ttl`. A time in
/// the future (the clock moved back) counts as stale.
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

    use super::*;

    const KEY: &str = "test";
    const TTL: Duration = Duration::from_hours(1);

    struct Fixture {
        cache: CatalogCache,
        fetches: Arc<AtomicUsize>,
        _dir: tempfile::TempDir,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            let disk = net::PosterCache::new(dir.path()).unwrap();
            let http = net::Client::builder().max_retries(0).build().unwrap();
            Self {
                cache: CatalogCache::new(AniList::new(http), disk),
                fetches: Arc::new(AtomicUsize::new(0)),
                _dir: dir,
            }
        }

        /// A fetch that counts itself and returns `value`.
        fn online(&self, value: &str) -> impl Future<Output = Result<String, Error>> + use<> {
            let fetches = Arc::clone(&self.fetches);
            let value = value.to_owned();
            async move {
                fetches.fetch_add(1, Ordering::SeqCst);
                Ok(value)
            }
        }

        /// A fetch that counts itself and fails to connect, as with no
        /// network.
        fn offline(&self) -> impl Future<Output = Result<String, Error>> + use<> {
            let fetches = Arc::clone(&self.fetches);
            async move {
                fetches.fetch_add(1, Ordering::SeqCst);
                let closed = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
                let address = closed.local_addr().unwrap();
                drop(closed);
                let http = net::Client::builder().max_retries(0).build().unwrap();
                let anilist = AniList::with_endpoint(http, format!("http://{address}/"));
                anilist.trending().await.map(|_| String::new())
            }
        }

        async fn seed(&self, value: &str, age: Duration) -> SystemTime {
            let updated_at = UNIX_EPOCH
                + Duration::from_secs(
                    (SystemTime::now() - age)
                        .duration_since(UNIX_EPOCH)
                        .unwrap()
                        .as_secs(),
                );
            let key = format!("v{KEY_VERSION}:{KEY}");
            self.cache
                .store_at(&key, &value.to_owned(), updated_at)
                .await;
            updated_at
        }

        async fn load(
            &self,
            fetch: impl Future<Output = Result<String, Error>> + Send + 'static,
        ) -> Result<Load<String>, Error> {
            self.cache.load(KEY.to_owned(), TTL, fetch).await
        }

        fn fetches(&self) -> usize {
            self.fetches.load(Ordering::SeqCst)
        }
    }

    #[tokio::test]
    async fn a_fresh_hit_makes_no_request() {
        let fixture = Fixture::new();
        let updated_at = fixture.seed("cached", Duration::from_secs(60)).await;

        let load = fixture.load(fixture.online("new")).await.unwrap();

        assert_eq!(fixture.fetches(), 0);
        assert!(load.refresh.is_none());
        assert_eq!(
            load.cached,
            Cached {
                value: "cached".to_owned(),
                updated_at,
                offline: false,
            }
        );
    }

    #[tokio::test]
    async fn a_stale_hit_returns_the_cached_value_then_the_refreshed_one() {
        let fixture = Fixture::new();
        fixture.seed("cached", TTL * 2).await;

        let load = fixture.load(fixture.online("new")).await.unwrap();
        assert_eq!(load.cached.value, "cached");
        assert_eq!(fixture.fetches(), 0, "the refresh waits to be awaited");

        let refreshed = load.refresh.unwrap().await.unwrap();
        assert_eq!(refreshed.value, "new");
        assert!(!refreshed.offline);
        assert_eq!(fixture.fetches(), 1);

        let next = fixture.load(fixture.online("newer")).await.unwrap();
        assert_eq!(next.cached.value, "new", "the refresh was stored");
        assert!(next.refresh.is_none(), "and is fresh");
    }

    #[tokio::test]
    async fn offline_returns_the_cached_value_with_its_timestamp() {
        let fixture = Fixture::new();
        let updated_at = fixture.seed("cached", TTL * 2).await;

        let load = fixture.load(fixture.offline()).await.unwrap();
        let refreshed = load.refresh.unwrap().await.unwrap();

        assert_eq!(fixture.fetches(), 1);
        assert_eq!(
            refreshed,
            Cached {
                value: "cached".to_owned(),
                updated_at,
                offline: true,
            }
        );
    }

    #[tokio::test]
    async fn an_outage_also_serves_the_cached_value_offline() {
        let fixture = Fixture::new();
        fixture.seed("cached", TTL * 2).await;

        let load = fixture
            .load(async { Err(net::Error::AniListUnavailable.into()) })
            .await
            .unwrap();
        let refreshed = load.refresh.unwrap().await.unwrap();

        assert!(refreshed.offline);
        assert_eq!(refreshed.value, "cached");
    }

    #[tokio::test]
    async fn a_client_error_on_refresh_is_not_offline() {
        let fixture = Fixture::new();
        fixture.seed("cached", TTL * 2).await;

        let load = fixture
            .load(async {
                Err(net::Error::Http {
                    host: "graphql.anilist.co".to_owned(),
                    status: 404,
                }
                .into())
            })
            .await
            .unwrap();
        let refreshed = load.refresh.unwrap().await;

        assert!(
            matches!(
                refreshed,
                Err(Error::Net(net::Error::Http { status: 404, .. }))
            ),
            "{refreshed:?}"
        );
    }

    #[tokio::test]
    async fn a_fetched_value_reads_back_with_the_same_timestamp() {
        let fixture = Fixture::new();

        let first = fixture.load(fixture.online("new")).await.unwrap();
        let second = fixture.load(fixture.online("newer")).await.unwrap();

        assert_eq!(second.cached, first.cached);
    }

    #[tokio::test]
    async fn no_cache_and_no_network_is_an_error() {
        let fixture = Fixture::new();

        let result = fixture.load(fixture.offline()).await;

        assert!(matches!(result, Err(Error::Net(_))), "{result:?}");
    }

    #[tokio::test]
    async fn a_miss_fetches_and_stores_the_value() {
        let fixture = Fixture::new();

        let first = fixture.load(fixture.online("new")).await.unwrap();
        let second = fixture.load(fixture.online("newer")).await.unwrap();

        assert_eq!(first.cached.value, "new");
        assert!(first.refresh.is_none());
        assert_eq!(second.cached.value, "new");
        assert_eq!(fixture.fetches(), 1);
    }

    #[test]
    fn catalog_loads_round_trip_through_a_blob() {
        use crate::{
            CountryTrends, MediaFormat, MediaSeason, MediaStatus, MediaSummary, MediaTitle,
        };

        let media = MediaSummary {
            anilist_id: 154_587,
            mal_id: Some(52_991),
            kind: Some(domain::MediaKind::Anime),
            title: MediaTitle {
                romaji: Some("Sousou no Frieren".to_owned()),
                english: None,
                native: Some("葬送のフリーレン".to_owned()),
            },
            poster: Some("https://example.invalid/p.jpg".to_owned()),
            banner: None,
            format: Some(MediaFormat::Tv),
            status: Some(MediaStatus::Other("NOT_YET_KNOWN".to_owned())),
            season: Some(MediaSeason::Fall),
            season_year: Some(2023),
            episodes: Some(28),
            chapters: None,
            mean_score: Some(90),
            genres: vec!["Adventure".to_owned()],
            is_adult: false,
        };
        let episode = AiringEpisode {
            media: media.clone(),
            episode: 3,
            airing_at: 1_700_000_000,
            time_until_airing: -60,
        };
        let discover = DiscoverLoad {
            trending: vec![media.clone()],
            popular: Vec::new(),
            upcoming_season: Vec::new(),
            trending_movies: Vec::new(),
            airing_schedule: vec![episode.clone()],
            missed_sequels: Vec::new(),
            trending_manga: Vec::new(),
            trending_by_country: CountryTrends {
                japan: vec![media.clone()],
                ..CountryTrends::default()
            },
        };
        let home = HomeLoad {
            trending: vec![media.clone()],
            aired_recently: vec![episode],
            anime_carousel: vec![media],
        };

        let bytes = rmp_serde::to_vec_named(&(7u64, &discover)).unwrap();
        let (seconds, decoded): (u64, DiscoverLoad) = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!((seconds, decoded), (7, discover));
        let bytes = rmp_serde::to_vec_named(&(7u64, &home)).unwrap();
        let (_, decoded): (u64, HomeLoad) = rmp_serde::from_slice(&bytes).unwrap();
        assert_eq!(decoded, home);
    }

    #[tokio::test]
    async fn an_unreadable_entry_is_a_miss() {
        let fixture = Fixture::new();
        let key = format!("v{KEY_VERSION}:{KEY}");
        fixture
            .cache
            .disk
            .write_metadata(&key, b"not msgpack".to_vec())
            .await
            .unwrap();

        let load = fixture.load(fixture.online("new")).await.unwrap();

        assert_eq!(load.cached.value, "new");
        assert_eq!(fixture.fetches(), 1);
    }
}
