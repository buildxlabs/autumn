//! The Discover screen's data layer: eight sections, each stale-first and
//! independently recoverable (#62, decisions #23 and #31).

use std::{future::Future, time::Duration};

use domain::SectionFetch;
use metadata::{AiringEpisode, AniList, CatalogCache, CountryTrends, Error, Load, MediaSummary};

/// How long a catalog section stays fresh before it is refreshed (#31).
pub const CATALOG_TTL: Duration = Duration::from_hours(6);
/// How long the airing schedule stays fresh before it is refreshed (#31).
pub const AIRING_TTL: Duration = Duration::from_hours(1);

/// One cached request behind a Discover section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the cache keys are this crate's contract with its catalog service"
)]
pub enum DiscoverKey {
    /// Trending Anime, the hero.
    Trending,
    /// Popular Anime.
    Popular,
    /// Upcoming Season.
    UpcomingSeason,
    /// Trending Movies.
    TrendingMovies,
    /// Airing Schedule.
    AiringSchedule,
    /// Missed Sequels.
    MissedSequels,
    /// Trending Manga.
    TrendingManga,
    /// Trending Anime from Japan.
    Japan,
    /// Trending Anime from South Korea.
    Korea,
    /// Trending Anime from China.
    China,
}

impl DiscoverKey {
    /// The stable cache key for this request. Missed Sequels depends on the
    /// account, so it is keyed by user; no other key is.
    #[must_use]
    pub fn cache_key(self, user: &str) -> String {
        let name = match self {
            Self::Trending => "trending",
            Self::Popular => "popular",
            Self::UpcomingSeason => "upcoming",
            Self::TrendingMovies => "movies",
            Self::AiringSchedule => "airing",
            Self::MissedSequels => return format!("discover-section:missed:{user}"),
            Self::TrendingManga => "manga",
            Self::Japan => "country:JP",
            Self::Korea => "country:KR",
            Self::China => "country:CN",
        };
        format!("discover-section:{name}")
    }

    /// How long this request stays fresh.
    #[must_use]
    pub const fn ttl(self) -> Duration {
        match self {
            Self::AiringSchedule => AIRING_TTL,
            _ => CATALOG_TTL,
        }
    }
}

/// The eight carousels of the Discover screen, in display order (#23).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the screen renders every section, so a new one must be handled here"
)]
pub enum DiscoverSection {
    /// Trending Anime, shown as the hero.
    Trending,
    /// Popular Anime.
    Popular,
    /// Upcoming Season.
    UpcomingSeason,
    /// Trending Movies.
    TrendingMovies,
    /// Airing Schedule.
    AiringSchedule,
    /// Missed Sequels.
    MissedSequels,
    /// Trending Manga.
    TrendingManga,
    /// Trending by Country: JP, KR and CN.
    TrendingByCountry,
}

impl DiscoverSection {
    /// The sections in the order the ticket lists them.
    pub const ORDER: [Self; 8] = [
        Self::Trending,
        Self::Popular,
        Self::UpcomingSeason,
        Self::TrendingMovies,
        Self::AiringSchedule,
        Self::MissedSequels,
        Self::TrendingManga,
        Self::TrendingByCountry,
    ];

    /// The heading of this section.
    #[must_use]
    pub const fn title(self) -> &'static str {
        match self {
            Self::Trending => "Trending Anime",
            Self::Popular => "Popular Anime",
            Self::UpcomingSeason => "Upcoming Season",
            Self::TrendingMovies => "Trending Movies",
            Self::AiringSchedule => "Airing Schedule",
            Self::MissedSequels => "Missed Sequels",
            Self::TrendingManga => "Trending Manga",
            Self::TrendingByCountry => "Trending by Country",
        }
    }
}

/// The media of one section.
#[derive(Debug, Clone, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "each section knows which of these three shapes it renders"
)]
pub enum SectionEntries {
    /// Media, from any of the media carousels.
    Media(Vec<MediaSummary>),
    /// Upcoming episode releases.
    Schedule(Vec<AiringEpisode>),
    /// The JP, KR and CN carousels.
    Countries(CountryTrends),
}

/// One cached request's result, with the fetch state needed to render it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectionLoad {
    /// The value to show, stale if a refresh is in flight or failed.
    pub entries: SectionEntries,
    /// When AniList returned this value.
    pub updated_at: std::time::SystemTime,
    /// Whether the last refresh could not reach AniList.
    pub offline: bool,
    /// A refresh failed with a real answer, such as an API error. The
    /// cached value is still shown, but the section can report the problem.
    pub refresh_error: Option<String>,
}

/// The cached catalog requests Discover needs. Implemented by the real
/// [`metadata::CatalogCache`] wrapper and by fakes in tests.
pub trait DiscoverCatalog: Send + Sync {
    /// Serve one request stale-first, fetching when there is no fresh value.
    ///
    /// # Errors
    ///
    /// When there is no cached value and the fetch fails.
    fn load(
        &self,
        key: DiscoverKey,
        user: &str,
    ) -> impl Future<Output = Result<SectionLoad, Error>> + Send;
}

/// The [`DiscoverCatalog`] backed by the stale-first metadata cache.
#[derive(Debug, Clone)]
pub struct RealDiscoverCatalog {
    cache: CatalogCache,
    anilist: AniList,
}

impl RealDiscoverCatalog {
    /// Cache `anilist`'s Discover responses in `cache`.
    #[must_use]
    pub fn new(cache: CatalogCache, anilist: AniList) -> Self {
        Self { cache, anilist }
    }

    async fn media(
        &self,
        key: DiscoverKey,
        user: &str,
        fetch: impl Future<Output = Result<Vec<MediaSummary>, Error>> + Send + 'static,
    ) -> Result<SectionLoad, Error> {
        let load = self
            .cache
            .section(key.cache_key(user), key.ttl(), fetch)
            .await?;
        Ok(resolve(load, SectionEntries::Media).await)
    }

    async fn schedule(
        &self,
        key: DiscoverKey,
        user: &str,
        fetch: impl Future<Output = Result<Vec<AiringEpisode>, Error>> + Send + 'static,
    ) -> Result<SectionLoad, Error> {
        let load = self
            .cache
            .section(key.cache_key(user), key.ttl(), fetch)
            .await?;
        Ok(resolve(load, SectionEntries::Schedule).await)
    }
}

/// Use the cached value at once, awaiting a refresh when one is due. A
/// failed refresh keeps the stale value on screen and reports the error so
/// only that section shows a recovery affordance.
async fn resolve<T>(load: Load<T>, wrap: impl FnOnce(T) -> SectionEntries) -> SectionLoad {
    let Load { cached, refresh } = load;
    let (cached, refresh_error) = match refresh {
        Some(refresh) => match refresh.await {
            Ok(cached) => (cached, None),
            Err(error) => (cached, Some(error.to_string())),
        },
        None => (cached, None),
    };
    SectionLoad {
        entries: wrap(cached.value),
        updated_at: cached.updated_at,
        offline: cached.offline,
        refresh_error,
    }
}

impl DiscoverCatalog for RealDiscoverCatalog {
    async fn load(&self, key: DiscoverKey, user: &str) -> Result<SectionLoad, Error> {
        let anilist = self.anilist.clone();
        match key {
            DiscoverKey::Trending => {
                self.media(key, user, async move { anilist.trending().await })
                    .await
            }
            DiscoverKey::Popular => {
                self.media(key, user, async move { anilist.popular().await })
                    .await
            }
            DiscoverKey::UpcomingSeason => {
                self.media(key, user, async move { anilist.upcoming_season().await })
                    .await
            }
            DiscoverKey::TrendingMovies => {
                self.media(key, user, async move { anilist.trending_movies().await })
                    .await
            }
            DiscoverKey::AiringSchedule => {
                self.schedule(key, user, async move { anilist.airing_schedule().await })
                    .await
            }
            DiscoverKey::MissedSequels => {
                let name = user.to_owned();
                self.media(
                    key,
                    user,
                    async move { anilist.missed_sequels(&name).await },
                )
                .await
            }
            DiscoverKey::TrendingManga => {
                self.media(key, user, async move { anilist.trending_manga().await })
                    .await
            }
            DiscoverKey::Japan | DiscoverKey::Korea | DiscoverKey::China => {
                let country = match key {
                    DiscoverKey::Japan => "JP",
                    DiscoverKey::Korea => "KR",
                    _ => "CN",
                };
                self.media(key, user, async move {
                    anilist.trending_by_country(country).await
                })
                .await
            }
        }
    }
}

/// One rendered section and the outcome of its fetch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Section {
    /// Which carousel this is.
    pub section: DiscoverSection,
    /// The media to render; empty when the fetch failed with no cache.
    pub entries: SectionEntries,
    /// The outcome of this section's fetch, independent of the others.
    pub state: SectionFetch,
}

/// The eight Discover sections, loaded independently.
#[derive(Debug, Default)]
pub struct DiscoverStore {
    sections: Vec<Section>,
}

impl DiscoverStore {
    /// The sections in display order.
    #[must_use]
    pub fn sections(&self) -> &[Section] {
        &self.sections
    }

    /// Load every section, keeping each section's error to itself.
    pub async fn load<C: DiscoverCatalog>(catalog: &C, user: &str) -> Self {
        let mut sections = Vec::with_capacity(DiscoverSection::ORDER.len());
        for section in DiscoverSection::ORDER {
            sections.push(load_section(catalog, user, section).await);
        }
        Self { sections }
    }
}

/// One key's result, before it is placed in a display section.
struct KeyLoad {
    entries: SectionEntries,
    state: SectionFetch,
}

/// Load one key; a failed request is data for that section, not an error
/// that stops the other seven from rendering.
async fn load_key<C: DiscoverCatalog>(catalog: &C, user: &str, key: DiscoverKey) -> KeyLoad {
    match catalog.load(key, user).await {
        Ok(load) => {
            let state = if let Some(message) = load.refresh_error {
                SectionFetch::Failed(message)
            } else if load.offline {
                SectionFetch::Offline {
                    last_updated: Some(load.updated_at),
                }
            } else {
                SectionFetch::Ready
            };
            KeyLoad {
                entries: load.entries,
                state,
            }
        }
        Err(error) => KeyLoad {
            entries: if key == DiscoverKey::AiringSchedule {
                SectionEntries::Schedule(Vec::new())
            } else {
                SectionEntries::Media(Vec::new())
            },
            state: SectionFetch::Failed(error.to_string()),
        },
    }
}

fn media_of(load: KeyLoad) -> Vec<MediaSummary> {
    match load.entries {
        SectionEntries::Media(media) => media,
        _ => Vec::new(),
    }
}

/// The country section shows what it has and reports the first failure.
fn country_state(states: [&SectionFetch; 3]) -> SectionFetch {
    for state in states {
        if let SectionFetch::Failed(message) = state {
            return SectionFetch::Failed(message.clone());
        }
    }
    if states
        .iter()
        .any(|state| matches!(state, SectionFetch::Offline { .. }))
    {
        return SectionFetch::Offline { last_updated: None };
    }
    SectionFetch::Ready
}

async fn load_section<C: DiscoverCatalog>(
    catalog: &C,
    user: &str,
    section: DiscoverSection,
) -> Section {
    let key = match section {
        DiscoverSection::Trending => DiscoverKey::Trending,
        DiscoverSection::Popular => DiscoverKey::Popular,
        DiscoverSection::UpcomingSeason => DiscoverKey::UpcomingSeason,
        DiscoverSection::TrendingMovies => DiscoverKey::TrendingMovies,
        DiscoverSection::AiringSchedule => DiscoverKey::AiringSchedule,
        DiscoverSection::MissedSequels => DiscoverKey::MissedSequels,
        DiscoverSection::TrendingManga => DiscoverKey::TrendingManga,
        DiscoverSection::TrendingByCountry => {
            let japan = load_key(catalog, user, DiscoverKey::Japan).await;
            let korea = load_key(catalog, user, DiscoverKey::Korea).await;
            let china = load_key(catalog, user, DiscoverKey::China).await;
            let state = country_state([&japan.state, &korea.state, &china.state]);
            return Section {
                section,
                entries: SectionEntries::Countries(CountryTrends {
                    japan: media_of(japan),
                    korea: media_of(korea),
                    china: media_of(china),
                }),
                state,
            };
        }
    };
    let loaded = load_key(catalog, user, key).await;
    Section {
        section,
        entries: loaded.entries,
        state: loaded.state,
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::{HashMap, HashSet},
        sync::{
            Mutex,
            atomic::{AtomicUsize, Ordering},
        },
        time::SystemTime,
    };

    use super::*;

    /// A catalog that counts fetches and models each key's TTL with a
    /// movable clock, so the six-hour boundary is observable.
    #[derive(Debug, Default)]
    struct FakeCatalog {
        fetches: AtomicUsize,
        failing: HashSet<DiscoverKey>,
        failing_refresh: HashSet<DiscoverKey>,
        fetched_at: Mutex<HashMap<DiscoverKey, Duration>>,
        now: Mutex<Duration>,
    }

    impl FakeCatalog {
        fn failing(key: DiscoverKey) -> Self {
            Self {
                failing: HashSet::from([key]),
                ..Self::default()
            }
        }

        fn failing_refresh(key: DiscoverKey) -> Self {
            Self {
                failing_refresh: HashSet::from([key]),
                ..Self::default()
            }
        }

        fn fetches(&self) -> usize {
            self.fetches.load(Ordering::SeqCst)
        }

        fn advance(&self, by: Duration) {
            *self.now.lock().unwrap() += by;
        }

        fn entries(&self, key: DiscoverKey) -> SectionLoad {
            let entries = if key == DiscoverKey::AiringSchedule {
                SectionEntries::Schedule(Vec::new())
            } else {
                SectionEntries::Media(vec![media_summary(&key.cache_key("viewer"))])
            };
            SectionLoad {
                entries,
                updated_at: SystemTime::UNIX_EPOCH,
                offline: false,
                refresh_error: self
                    .failing_refresh
                    .contains(&key)
                    .then(|| "refresh failed".to_owned()),
            }
        }

        async fn serve(&self, key: DiscoverKey) -> Result<SectionLoad, Error> {
            std::future::ready(()).await;
            let now = *self.now.lock().unwrap();
            if let Some(fetched_at) = self.fetched_at.lock().unwrap().get(&key)
                && now.saturating_sub(*fetched_at) < key.ttl()
            {
                return Ok(self.entries(key));
            }
            self.fetches.fetch_add(1, Ordering::SeqCst);
            if self.failing.contains(&key) {
                return Err(Error::Api {
                    status: None,
                    message: format!("{key:?} failed"),
                });
            }
            self.fetched_at.lock().unwrap().insert(key, now);
            Ok(self.entries(key))
        }
    }

    impl DiscoverCatalog for FakeCatalog {
        async fn load(&self, key: DiscoverKey, _user: &str) -> Result<SectionLoad, Error> {
            self.serve(key).await
        }
    }

    fn media_summary(name: &str) -> MediaSummary {
        MediaSummary {
            anilist_id: 1,
            mal_id: None,
            kind: None,
            title: metadata::MediaTitle {
                romaji: Some(name.to_owned()),
                english: None,
                native: None,
            },
            poster: None,
            banner: None,
            format: None,
            status: None,
            season: None,
            season_year: None,
            episodes: None,
            chapters: None,
            mean_score: None,
            genres: Vec::new(),
            is_adult: false,
        }
    }

    #[tokio::test]
    async fn the_eight_sections_render_in_ticket_order() {
        let store = DiscoverStore::load(&FakeCatalog::default(), "viewer").await;

        let got: Vec<DiscoverSection> = store.sections().iter().map(|s| s.section).collect();
        assert_eq!(
            got,
            DiscoverSection::ORDER,
            "sections must be in ticket order"
        );
    }

    #[tokio::test]
    async fn one_failing_section_leaves_the_others_rendered() {
        let catalog = FakeCatalog::failing(DiscoverKey::Popular);
        let store = DiscoverStore::load(&catalog, "viewer").await;

        let popular = store
            .sections()
            .iter()
            .find(|s| s.section == DiscoverSection::Popular)
            .unwrap();
        assert!(matches!(popular.state, SectionFetch::Failed(_)));
        assert_eq!(popular.entries, SectionEntries::Media(Vec::new()));

        for section in store.sections() {
            if section.section == DiscoverSection::Popular {
                continue;
            }
            assert_eq!(
                section.state,
                SectionFetch::Ready,
                "{} must render",
                section.section.title()
            );
        }
    }

    #[tokio::test]
    async fn a_second_visit_within_six_hours_makes_no_requests() {
        let catalog = FakeCatalog::default();
        let first = DiscoverStore::load(&catalog, "viewer").await;
        let requests = catalog.fetches();
        assert!(requests > 0, "the first visit fetches the catalog");

        // Well inside even the shortest Discover TTL (the one-hour schedule).
        catalog.advance(Duration::from_mins(30));
        let second = DiscoverStore::load(&catalog, "viewer").await;
        assert_eq!(
            catalog.fetches(),
            requests,
            "a fresh cache makes no requests"
        );
        assert_eq!(
            first.sections(),
            second.sections(),
            "both visits show the same sections"
        );

        catalog.advance(Duration::from_hours(7));
        let _third = DiscoverStore::load(&catalog, "viewer").await;
        assert!(
            catalog.fetches() > requests,
            "once every section is stale, the catalog is fetched again"
        );
    }

    #[test]
    fn missed_sequels_is_cached_for_one_account_only() {
        assert_ne!(
            DiscoverKey::MissedSequels.cache_key("first"),
            DiscoverKey::MissedSequels.cache_key("second"),
            "two accounts must not share their sequels"
        );
        assert_eq!(
            DiscoverKey::Popular.cache_key("first"),
            DiscoverKey::Popular.cache_key("second"),
            "the catalog is the same for every account"
        );
    }

    #[tokio::test]
    async fn a_failed_refresh_keeps_the_stale_value_but_reports_its_section() {
        let catalog = FakeCatalog::failing_refresh(DiscoverKey::TrendingManga);
        let store = DiscoverStore::load(&catalog, "viewer").await;

        let manga = store
            .sections()
            .iter()
            .find(|section| section.section == DiscoverSection::TrendingManga)
            .unwrap();
        assert!(matches!(manga.state, SectionFetch::Failed(_)));
        assert_ne!(
            manga.entries,
            SectionEntries::Media(Vec::new()),
            "the stale value stays on screen"
        );
        for section in store.sections() {
            if section.section != DiscoverSection::TrendingManga {
                assert_eq!(section.state, SectionFetch::Ready);
            }
        }
    }
}
