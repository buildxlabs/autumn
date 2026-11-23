//! The Discover screen's data layer: eight sections, each stale-first and
//! independently recoverable (#62, decisions #23 and #31).

use std::{
    future::Future,
    pin::Pin,
    rc::Rc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use domain::{MediaKind, SectionFetch};
use futures::future::join_all;
use gpui_kit::{
    App, AppContext, Context, Entity, InteractiveElement, IntoElement, ParentElement, Render,
    Resource, Styled, Task, TestSupportExt, Window,
    component::{ActiveTheme, button::Button, carousel::CarouselState},
    div, px,
};
use metadata::{
    AIRING_TTL, AiringEpisode, AniList, CATALOG_TTL, Cached, CatalogCache, CountryTrends, Error,
    Lookup, MediaSummary, Refresh,
};
use ui_components::{
    CarouselWheel, LruImageCache, MEDIA_CARD_HEIGHT, MediaCardView, MediaCarousel,
    MediaFormat as CardFormat,
    hero::{HeroBannerData, hero_banner},
};
use ui_state::bridge::{self, BridgeError};

/// One carousel for every section but the hero, with the country section
/// expanded into three rows.
const CAROUSEL_COUNT: usize = DiscoverSection::ORDER.len() + 1;

/// One cached request behind a Discover section.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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
    pub(crate) fn cache_key(self, user: &str) -> String {
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
    pub(crate) const fn ttl(self) -> Duration {
        match self {
            Self::AiringSchedule => AIRING_TTL,
            _ => CATALOG_TTL,
        }
    }

    /// The display section this key feeds.
    const fn section(self) -> DiscoverSection {
        match self {
            Self::Trending => DiscoverSection::Trending,
            Self::Popular => DiscoverSection::Popular,
            Self::UpcomingSeason => DiscoverSection::UpcomingSeason,
            Self::TrendingMovies => DiscoverSection::TrendingMovies,
            Self::AiringSchedule => DiscoverSection::AiringSchedule,
            Self::MissedSequels => DiscoverSection::MissedSequels,
            Self::TrendingManga => DiscoverSection::TrendingManga,
            Self::Japan | Self::Korea | Self::China => DiscoverSection::TrendingByCountry,
        }
    }
}

/// The eight carousels of the Discover screen, in display order (#23).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum DiscoverSection {
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
    pub(crate) const ORDER: [Self; 8] = [
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
    pub(crate) const fn title(self) -> &'static str {
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
pub(crate) enum SectionEntries {
    /// Media, from any of the media carousels.
    Media(Vec<MediaSummary>),
    /// Upcoming episode releases.
    Schedule(Vec<AiringEpisode>),
    /// The JP, KR and CN carousels.
    Countries(CountryTrends),
}

/// One value to show, with the fetch state needed to render it.
#[derive(Debug, Clone, PartialEq, Eq)]
struct SectionValue {
    entries: SectionEntries,
    updated_at: SystemTime,
    offline: bool,
}

/// A value served now and, when it is missing or stale, a fetch to run in
/// the background. The future is `'static` so the UI can run it on the bridge.
pub struct LoadedSection {
    value: SectionValue,
    refresh: Option<SectionRefresh>,
}

impl std::fmt::Debug for LoadedSection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoadedSection")
            .field("value", &self.value)
            .field("has_refresh", &self.refresh.is_some())
            .finish()
    }
}

/// A pending refresh, applied to its section when it lands.
type SectionRefresh = Pin<Box<dyn Future<Output = Result<SectionValue, Error>> + Send>>;

/// The cached catalog requests Discover needs. Implemented by the real
/// [`metadata::CatalogCache`] wrapper and by fakes in tests.
pub trait DiscoverCatalog: Send + Sync {
    /// Serve one request's cached value at once, with a pending refresh.
    ///
    /// # Errors
    ///
    /// When there is no cached value and the fetch fails.
    fn load(
        &self,
        key: DiscoverKey,
        user: &str,
    ) -> impl Future<Output = Result<LoadedSection, Error>> + Send;
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
    ) -> Result<LoadedSection, Error> {
        let lookup = self
            .cache
            .lookup(key.cache_key(user), key.ttl(), fetch)
            .await;
        Ok(from_lookup(lookup, key, SectionEntries::Media))
    }

    async fn schedule(
        &self,
        key: DiscoverKey,
        user: &str,
        fetch: impl Future<Output = Result<Vec<AiringEpisode>, Error>> + Send + 'static,
    ) -> Result<LoadedSection, Error> {
        let lookup = self
            .cache
            .lookup(key.cache_key(user), key.ttl(), fetch)
            .await;
        Ok(from_lookup(lookup, key, SectionEntries::Schedule))
    }
}

/// Show a cached value at once; leave a miss or a stale value's fetch to
/// run in the background and be applied when it lands.
fn from_lookup<T>(
    lookup: Lookup<T>,
    key: DiscoverKey,
    wrap: fn(T) -> SectionEntries,
) -> LoadedSection
where
    T: Send + 'static,
{
    match lookup {
        Lookup::Cached { cached, refresh } => LoadedSection {
            value: cached_value(cached, wrap),
            refresh: refresh.map(|refresh| pending_value(refresh, wrap)),
        },
        Lookup::Miss(fetch) => LoadedSection {
            value: empty_value(key),
            refresh: Some(pending_value(fetch, wrap)),
        },
    }
}

fn cached_value<T>(cached: Cached<T>, wrap: fn(T) -> SectionEntries) -> SectionValue
where
    T: Send + 'static,
{
    SectionValue {
        entries: wrap(cached.value),
        updated_at: cached.updated_at,
        offline: cached.offline,
    }
}

fn pending_value<T>(refresh: Refresh<T>, wrap: fn(T) -> SectionEntries) -> SectionRefresh
where
    T: Send + 'static,
{
    Box::pin(async move {
        let cached = refresh.await?;
        Ok(cached_value(cached, wrap))
    })
}

impl DiscoverCatalog for RealDiscoverCatalog {
    async fn load(&self, key: DiscoverKey, user: &str) -> Result<LoadedSection, Error> {
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
pub(crate) struct Section {
    /// Which carousel this is.
    pub(crate) kind: DiscoverSection,
    /// The media to render; empty when the fetch failed with no cache.
    pub(crate) entries: SectionEntries,
    /// The outcome of this section's fetch, independent of the others.
    pub(crate) state: SectionFetch,
    /// Each country's fetch state, in JP/KR/CN order.
    country_states: [SectionFetch; 3],
}

impl Section {
    /// A section for the shell's fake loader in tests.
    #[cfg(test)]
    pub(crate) fn new(kind: DiscoverSection, entries: SectionEntries, state: SectionFetch) -> Self {
        Self {
            kind,
            entries,
            state,
            country_states: std::array::from_fn(|_| SectionFetch::Ready),
        }
    }
}

/// The eight Discover sections, loaded independently.
#[derive(Debug, Default, Clone)]
pub struct DiscoverStore {
    sections: Vec<Section>,
}

/// A cached snapshot and the refreshes still in flight.
#[derive(Debug, Default)]
pub struct DiscoverUpdate {
    store: DiscoverStore,
    pending: Vec<PendingRefresh>,
}

impl DiscoverUpdate {
    /// A snapshot with nothing to refresh, for loaders serving cached data.
    #[must_use]
    pub fn new(store: DiscoverStore) -> Self {
        Self {
            store,
            pending: Vec::new(),
        }
    }
}

/// One refresh waiting to be applied to its key's section.
struct PendingRefresh {
    key: DiscoverKey,
    refresh: SectionRefresh,
}

impl std::fmt::Debug for PendingRefresh {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingRefresh")
            .field("key", &self.key)
            .finish_non_exhaustive()
    }
}

impl DiscoverStore {
    /// A store with ready-made sections, for the shell's fake loader in tests.
    #[cfg(test)]
    pub(crate) fn from_sections(sections: Vec<Section>) -> Self {
        Self { sections }
    }

    /// The sections in display order.
    pub(crate) fn sections(&self) -> &[Section] {
        &self.sections
    }

    /// Publish every cached section at once, keeping each section's error to
    /// itself. Lookups are local disk reads; they are started together, and
    /// a miss or stale fetch is left pending for the caller to apply when it
    /// lands, so one slow section cannot hold up the others.
    pub async fn load<C: DiscoverCatalog>(catalog: &C, user: Option<&str>) -> DiscoverUpdate {
        let keys: Vec<(DiscoverSection, DiscoverKey)> = DiscoverSection::ORDER
            .into_iter()
            .flat_map(|kind| match kind {
                DiscoverSection::TrendingByCountry => vec![
                    (kind, DiscoverKey::Japan),
                    (kind, DiscoverKey::Korea),
                    (kind, DiscoverKey::China),
                ],
                _ => vec![(kind, key_of(kind))],
            })
            .collect();
        let lookups = keys
            .iter()
            .map(|(_, key)| load_key(catalog, user, *key))
            .collect::<Vec<_>>();
        let results = join_all(lookups).await;

        let mut sections = Vec::with_capacity(DiscoverSection::ORDER.len());
        let mut pending = Vec::new();
        let mut countries = Vec::with_capacity(3);
        for ((kind, key), loaded) in keys.into_iter().zip(results) {
            if let Some(refresh) = loaded.refresh {
                pending.push(PendingRefresh { key, refresh });
            }
            if kind == DiscoverSection::TrendingByCountry {
                countries.push((key, loaded.value, loaded.state));
                continue;
            }
            sections.push(Section {
                kind,
                entries: loaded.value.entries,
                state: loaded.state,
                country_states: std::array::from_fn(|_| SectionFetch::Ready),
            });
        }
        if !countries.is_empty() {
            let mut trends = CountryTrends::default();
            let (mut japan, mut korea, mut china) = (
                SectionFetch::Ready,
                SectionFetch::Ready,
                SectionFetch::Ready,
            );
            for (key, value, state) in countries {
                match key {
                    DiscoverKey::Japan => {
                        trends.japan = media_only(value.entries);
                        japan = state;
                    }
                    DiscoverKey::Korea => {
                        trends.korea = media_only(value.entries);
                        korea = state;
                    }
                    _ => {
                        trends.china = media_only(value.entries);
                        china = state;
                    }
                }
            }
            let country_states = [japan, korea, china];
            sections.push(Section {
                kind: DiscoverSection::TrendingByCountry,
                entries: SectionEntries::Countries(trends),
                state: country_state(&country_states),
                country_states,
            });
        }
        DiscoverUpdate {
            store: Self { sections },
            pending,
        }
    }

    /// Apply one refresh outcome to its section, leaving the others alone.
    fn apply(&mut self, key: DiscoverKey, value: Option<SectionValue>, error: Option<&str>) {
        let state = match (error, &value) {
            (Some(message), _) => SectionFetch::Failed(message.to_owned()),
            (None, Some(value)) => value_state(value),
            (None, None) => SectionFetch::Ready,
        };
        let Some(target) = self
            .sections
            .iter_mut()
            .find(|section| section.kind == key.section())
        else {
            return;
        };
        if let (
            DiscoverKey::Japan | DiscoverKey::Korea | DiscoverKey::China,
            SectionEntries::Countries(countries),
        ) = (key, &mut target.entries)
        {
            let slot = match key {
                DiscoverKey::Japan => &mut countries.japan,
                DiscoverKey::Korea => &mut countries.korea,
                _ => &mut countries.china,
            };
            if let Some(SectionValue {
                entries: SectionEntries::Media(media),
                ..
            }) = value
            {
                *slot = media;
            }
            let index = match key {
                DiscoverKey::Japan => 0,
                DiscoverKey::Korea => 1,
                _ => 2,
            };
            if let Some(slot) = target.country_states.get_mut(index) {
                *slot = state;
            }
            target.state = country_state(&target.country_states);
        } else {
            if let Some(value) = value {
                target.entries = value.entries;
            }
            target.state = state;
        }
    }
}

/// One key's result, before it is placed in a display section.
struct KeyLoad {
    value: SectionValue,
    refresh: Option<SectionRefresh>,
    state: SectionFetch,
}

/// Load one key; a failed request is data for that section, not an error
/// that stops the other seven from rendering. A guest skips the one request
/// that reads their personal collection.
async fn load_key<C: DiscoverCatalog>(
    catalog: &C,
    user: Option<&str>,
    key: DiscoverKey,
) -> KeyLoad {
    if key == DiscoverKey::MissedSequels && user.is_none() {
        return KeyLoad {
            value: empty_value(key),
            refresh: None,
            state: SectionFetch::Ready,
        };
    }
    match catalog.load(key, user.unwrap_or_default()).await {
        Ok(loaded) => {
            let state = value_state(&loaded.value);
            KeyLoad {
                value: loaded.value,
                refresh: loaded.refresh,
                state,
            }
        }
        Err(error) => KeyLoad {
            value: empty_value(key),
            refresh: None,
            state: SectionFetch::Failed(error.to_string()),
        },
    }
}

fn key_of(section: DiscoverSection) -> DiscoverKey {
    match section {
        DiscoverSection::Trending => DiscoverKey::Trending,
        DiscoverSection::Popular => DiscoverKey::Popular,
        DiscoverSection::UpcomingSeason => DiscoverKey::UpcomingSeason,
        DiscoverSection::TrendingMovies => DiscoverKey::TrendingMovies,
        DiscoverSection::AiringSchedule => DiscoverKey::AiringSchedule,
        DiscoverSection::MissedSequels => DiscoverKey::MissedSequels,
        DiscoverSection::TrendingManga => DiscoverKey::TrendingManga,
        DiscoverSection::TrendingByCountry => DiscoverKey::Japan,
    }
}

fn empty_value(key: DiscoverKey) -> SectionValue {
    let entries = if key == DiscoverKey::AiringSchedule {
        SectionEntries::Schedule(Vec::new())
    } else {
        SectionEntries::Media(Vec::new())
    };
    SectionValue {
        entries,
        updated_at: UNIX_EPOCH,
        offline: false,
    }
}

fn value_state(value: &SectionValue) -> SectionFetch {
    if value.offline {
        SectionFetch::Offline {
            last_updated: Some(value.updated_at),
        }
    } else {
        SectionFetch::Ready
    }
}

/// The country section shows what it has and reports the first failure.
fn country_state(states: &[SectionFetch; 3]) -> SectionFetch {
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

/// Loads the eight sections for one account. The composition root builds
/// this around the stale-first catalog cache; tests use a fake.
pub type DiscoverLoader =
    Rc<dyn Fn(Option<String>, &App) -> Task<Result<DiscoverUpdate, BridgeError>>>;

/// The action run when a carousel card's Open menu item is used, with the
/// card's AniList id so a later Entry screen can navigate to it.
type CardAction = Rc<dyn Fn(u32, &mut Window, &mut App)>;

/// The Discover screen: a hero and seven carousels, each rendering its own
/// fetch outcome so a failed section cannot hide the others.
pub struct DiscoverView {
    store: DiscoverStore,
    loader: DiscoverLoader,
    user: Option<String>,
    task: Option<Task<()>>,
    refresh_tasks: Vec<Task<()>>,
    error: Option<String>,
    cache: Entity<LruImageCache>,
    carousel_states: Vec<Entity<CarouselState>>,
    wheels: Vec<CarouselWheel>,
    /// Optional seam for the Entry screen to open a card's media later.
    on_open: Option<CardAction>,
}

impl std::fmt::Debug for DiscoverView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiscoverView")
            .field("sections", &self.store.sections().len())
            .field("user", &self.user)
            .finish_non_exhaustive()
    }
}

impl DiscoverView {
    /// A view that loads `user`'s catalog through `loader`. `None` is a guest,
    /// whose personal sections are not requested. The cache is shared with
    /// the shell's other images.
    #[must_use]
    pub fn new(
        user: Option<String>,
        loader: DiscoverLoader,
        cache: Entity<LruImageCache>,
        cx: &mut App,
    ) -> Entity<Self> {
        let carousel_states = (0..CAROUSEL_COUNT)
            .map(|_| cx.new(|_| CarouselState::new(0)))
            .collect();
        let wheels = (0..CAROUSEL_COUNT)
            .map(|_| CarouselWheel::default())
            .collect();
        cx.new(|_| Self {
            store: DiscoverStore::default(),
            loader,
            user,
            task: None,
            refresh_tasks: Vec::new(),
            error: None,
            cache,
            carousel_states,
            wheels,
            on_open: None,
        })
    }

    /// Changes the account whose personal sections are loaded. Returns
    /// whether the name changed, so the caller reloads only when it has.
    pub fn set_user(&mut self, user: Option<String>) -> bool {
        if self.user == user {
            return false;
        }
        self.user = user;
        true
    }

    /// Sets the action run when a carousel card's Open menu item is used.
    /// The Entry screen can attach its navigation here when it lands.
    pub fn set_on_open(&mut self, on_open: impl Fn(u32, &mut Window, &mut App) + 'static) {
        self.on_open = Some(Rc::new(on_open));
    }

    /// Starts a load, replacing and so cancelling any older one. The cached
    /// values land at once; each stale section refreshes on its own task.
    pub fn load(&mut self, cx: &mut Context<'_, Self>) {
        let user = self.user.clone();
        let task = (self.loader)(user, cx);
        self.error = None;
        self.task = Some(cx.spawn(async move |view, cx| match task.await {
            Ok(DiscoverUpdate { store, pending }) => {
                let mut refresh_tasks = Vec::with_capacity(pending.len());
                for PendingRefresh { key, refresh } in pending {
                    let refresh_task = cx.update(|cx| bridge::run(cx, refresh));
                    let weak = view.clone();
                    refresh_tasks.push(cx.spawn(async move |cx| {
                        let outcome = match refresh_task.await {
                            Ok(Ok(value)) => (Some(value), None),
                            Ok(Err(error)) => (None, Some(error.to_string())),
                            Err(error) => (None, Some(error.to_string())),
                        };
                        let _ = weak.update(cx, |view, cx| {
                            view.store.apply(key, outcome.0, outcome.1.as_deref());
                            view.sync_carousel_counts(cx);
                            cx.notify();
                        });
                    }));
                }
                let _ = view.update(cx, |view, cx| view.adopt(store, refresh_tasks, cx));
            }
            Err(error) => {
                let _ = view.update(cx, |view, cx| {
                    view.error = Some(error.to_string());
                    view.task = None;
                    view.refresh_tasks.clear();
                    cx.notify();
                });
            }
        }));
        cx.notify();
    }

    fn adopt(
        &mut self,
        store: DiscoverStore,
        refresh_tasks: Vec<Task<()>>,
        cx: &mut Context<'_, Self>,
    ) {
        self.store = store;
        self.refresh_tasks = refresh_tasks;
        self.task = None;
        self.sync_carousel_counts(cx);
        cx.notify();
    }

    /// A carousel built with no items never enabled its arrows. Tell each
    /// state how many cards arrived so paging and selection work.
    fn sync_carousel_counts(&mut self, cx: &mut Context<'_, Self>) {
        let counts = self.carousel_counts();
        for (index, count) in counts.into_iter().enumerate() {
            if let Some(state) = self.carousel_states.get(index) {
                cx.update_entity(state, |state, state_cx| {
                    state.set_item_count(count, state_cx);
                });
            }
        }
    }

    /// Card counts in the same order as [`Self::render`] emits carousels.
    fn carousel_counts(&self) -> Vec<usize> {
        let mut counts = Vec::new();
        for section in self.store.sections() {
            match section.kind {
                DiscoverSection::Trending => {}
                DiscoverSection::TrendingByCountry => {
                    if let SectionEntries::Countries(trends) = &section.entries {
                        counts.extend([trends.japan.len(), trends.korea.len(), trends.china.len()]);
                    }
                }
                _ => counts.push(entry_len(&section.entries)),
            }
        }
        counts
    }

    fn carousel(
        &self,
        id: &str,
        title: &str,
        entries: &SectionEntries,
        state: &SectionFetch,
        index: usize,
    ) -> impl IntoElement {
        let body = if let SectionFetch::Failed(message) = state {
            div()
                .h(px(MEDIA_CARD_HEIGHT))
                .child(format!("Could not load {title}: {message}"))
                .into_any_element()
        } else {
            let cards = card_views(entries);
            if cards.is_empty() {
                div()
                    .h(px(MEDIA_CARD_HEIGHT))
                    .child("Nothing to show yet")
                    .into_any_element()
            } else if let (Some(carousel_state), Some(wheel)) =
                (self.carousel_states.get(index), self.wheels.get(index))
            {
                let mut carousel = MediaCarousel::new(
                    id,
                    carousel_state.clone(),
                    cards,
                    self.cache.clone(),
                    wheel.clone(),
                );
                if let Some(on_open) = &self.on_open {
                    let on_open = Rc::clone(on_open);
                    let ids: Rc<[u32]> = anilist_ids(entries).into();
                    carousel = carousel.on_open(move |index, window, cx| {
                        if let Some(id) = ids.get(index) {
                            on_open(*id, window, cx);
                        }
                    });
                }
                carousel.into_any_element()
            } else {
                div().into_any_element()
            }
        };
        div()
            .flex()
            .flex_col()
            .gap_2()
            .child(div().text_xl().child(title.to_owned()))
            .child(body)
    }
}

fn title_of(media: &MediaSummary) -> String {
    media
        .title
        .english
        .clone()
        .or_else(|| media.title.romaji.clone())
        .or_else(|| media.title.native.clone())
        .unwrap_or_default()
}

fn media_card(media: &MediaSummary) -> MediaCardView {
    let format = match media.kind {
        Some(MediaKind::Anime) => Some(CardFormat::Anime),
        Some(MediaKind::Manga) => Some(CardFormat::Manga),
        _ => None,
    };
    MediaCardView {
        poster: media
            .poster
            .as_ref()
            .map(|url| Resource::Uri(url.clone().into())),
        title: title_of(media).into(),
        format,
        score: media.mean_score.map(f32::from),
        progress: 0,
        total: media.episodes.or(media.chapters),
        detail: media.genres.first().cloned().unwrap_or_default().into(),
    }
}

fn card_views(entries: &SectionEntries) -> Rc<[MediaCardView]> {
    match entries {
        SectionEntries::Media(media) => media.iter().map(media_card).collect(),
        SectionEntries::Schedule(schedule) => schedule
            .iter()
            .map(|entry| media_card(&entry.media))
            .collect(),
        SectionEntries::Countries(_) => Rc::from([]),
    }
}

fn entry_len(entries: &SectionEntries) -> usize {
    match entries {
        SectionEntries::Media(media) => media.len(),
        SectionEntries::Schedule(schedule) => schedule.len(),
        SectionEntries::Countries(_) => 0,
    }
}

fn anilist_ids(entries: &SectionEntries) -> Vec<u32> {
    match entries {
        SectionEntries::Media(media) => media.iter().map(|item| item.anilist_id).collect(),
        SectionEntries::Schedule(schedule) => {
            schedule.iter().map(|item| item.media.anilist_id).collect()
        }
        SectionEntries::Countries(_) => Vec::new(),
    }
}

fn media_only(entries: SectionEntries) -> Vec<MediaSummary> {
    match entries {
        SectionEntries::Media(media) => media,
        _ => Vec::new(),
    }
}

fn first_media(entries: &SectionEntries) -> Option<&MediaSummary> {
    match entries {
        SectionEntries::Media(media) => media.first(),
        SectionEntries::Schedule(schedule) => schedule.first().map(|entry| &entry.media),
        SectionEntries::Countries(trends) => trends.japan.first(),
    }
}

impl Render for DiscoverView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<'_, Self>) -> impl IntoElement {
        let colors = cx.theme().colors;
        let mut index = 0usize;
        let mut sections = Vec::new();
        if let Some(message) = &self.error {
            sections.push(
                div()
                    .id("discover-error")
                    .test_support()
                    .child(format!("Could not load Discover: {message}"))
                    .into_any_element(),
            );
        }
        for section in self.store.sections() {
            match section.kind {
                DiscoverSection::Trending => {
                    if let Some(media) = first_media(&section.entries) {
                        let data = HeroBannerData {
                            title: title_of(media).into(),
                            synopsis: String::new().into(),
                            banner: media.banner.clone(),
                        };
                        sections.push(
                            div()
                                .id("discover-hero")
                                .test_support()
                                .child(hero_banner(
                                    "discover-hero",
                                    data,
                                    &self.cache,
                                    colors.accent,
                                    colors.background,
                                    div().child(
                                        Button::new("discover-hero-details").label("Details"),
                                    ),
                                ))
                                .into_any_element(),
                        );
                    }
                }
                DiscoverSection::TrendingByCountry => {
                    if let SectionEntries::Countries(trends) = &section.entries {
                        for (label, media) in [
                            ("Japan", &trends.japan),
                            ("South Korea", &trends.korea),
                            ("China", &trends.china),
                        ] {
                            // A failed country keeps its failure to itself:
                            // the other two still render their media.
                            let state = if media.is_empty() {
                                section.state.clone()
                            } else {
                                SectionFetch::Ready
                            };
                            sections.push(
                                self.carousel(
                                    &format!("discover-country-{label}"),
                                    label,
                                    &SectionEntries::Media(media.clone()),
                                    &state,
                                    index,
                                )
                                .into_any_element(),
                            );
                            index += 1;
                        }
                    }
                }
                _ => {
                    sections.push(
                        self.carousel(
                            &format!("discover-section-{}", section.kind.title()),
                            section.kind.title(),
                            &section.entries,
                            &section.state,
                            index,
                        )
                        .into_any_element(),
                    );
                    index += 1;
                }
            }
        }
        div()
            .id("discover-view")
            .test_support()
            .flex()
            .flex_col()
            .gap_5()
            .p_6()
            .children(sections)
    }
}

/// A media card for tests in this crate, including the shell's fake loader.
#[cfg(test)]
pub(crate) fn test_media(name: &str) -> MediaSummary {
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

#[cfg(test)]
mod tests {
    use std::{
        collections::{HashMap, HashSet},
        sync::{Arc, Mutex},
    };

    use gpui_kit::{TestAppContext, test::TestWindowExt};

    use super::*;

    /// Whether a fetch is the first load or a refresh of a stale value.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum FetchKind {
        Initial,
        Refresh,
    }

    /// A catalog that counts fetches and models each key's TTL with a
    /// movable clock, so the six-hour boundary is observable.
    #[derive(Debug, Clone, Default)]
    struct FakeCatalog {
        fetches: Arc<std::sync::atomic::AtomicUsize>,
        failing: Arc<Mutex<HashSet<DiscoverKey>>>,
        failing_refresh: Arc<Mutex<HashSet<DiscoverKey>>>,
        fetched_at: Arc<Mutex<HashMap<DiscoverKey, Duration>>>,
        now: Arc<Mutex<Duration>>,
    }

    impl FakeCatalog {
        fn failing(key: DiscoverKey) -> Self {
            Self {
                failing: Arc::new(Mutex::new(HashSet::from([key]))),
                ..Self::default()
            }
        }

        fn fail_refresh(&self, key: DiscoverKey) {
            self.failing_refresh
                .lock()
                .expect("lock poisoned")
                .insert(key);
        }

        fn fetches(&self) -> usize {
            self.fetches.load(std::sync::atomic::Ordering::SeqCst)
        }

        fn advance(&self, by: Duration) {
            *self.now.lock().expect("lock poisoned") += by;
        }

        fn entries(key: DiscoverKey) -> SectionValue {
            let entries = if key == DiscoverKey::AiringSchedule {
                SectionEntries::Schedule(Vec::new())
            } else {
                SectionEntries::Media(vec![test_media(&key.cache_key("viewer"))])
            };
            SectionValue {
                entries,
                updated_at: UNIX_EPOCH,
                offline: false,
            }
        }

        fn fetch(
            &self,
            key: DiscoverKey,
            now: Duration,
            kind: FetchKind,
        ) -> Result<SectionValue, Error> {
            self.fetches
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            if self.failing.lock().expect("lock poisoned").contains(&key)
                || (kind == FetchKind::Refresh
                    && self
                        .failing_refresh
                        .lock()
                        .expect("lock poisoned")
                        .contains(&key))
            {
                return Err(Error::Api {
                    status: None,
                    message: format!("{key:?} failed"),
                });
            }
            self.fetched_at
                .lock()
                .expect("lock poisoned")
                .insert(key, now);
            Ok(Self::entries(key))
        }
    }

    impl DiscoverCatalog for FakeCatalog {
        async fn load(&self, key: DiscoverKey, _user: &str) -> Result<LoadedSection, Error> {
            std::future::ready(()).await;
            let now = *self.now.lock().expect("lock poisoned");
            let fetched_at = self
                .fetched_at
                .lock()
                .expect("lock poisoned")
                .get(&key)
                .copied();
            match fetched_at {
                None => {
                    let catalog = self.clone();
                    Ok(LoadedSection {
                        value: empty_value(key),
                        refresh: Some(Box::pin(async move {
                            catalog.fetch(key, now, FetchKind::Initial)
                        })),
                    })
                }
                Some(at) if now.saturating_sub(at) < key.ttl() => Ok(LoadedSection {
                    value: Self::entries(key),
                    refresh: None,
                }),
                Some(_) => {
                    let catalog = self.clone();
                    Ok(LoadedSection {
                        value: Self::entries(key),
                        refresh: Some(Box::pin(async move {
                            catalog.fetch(key, now, FetchKind::Refresh)
                        })),
                    })
                }
            }
        }
    }

    /// Apply refreshes one at a time, as the UI does when each lands.
    async fn settle(update: DiscoverUpdate) -> DiscoverStore {
        let mut store = update.store;
        for pending in update.pending {
            match pending.refresh.await {
                Ok(value) => store.apply(pending.key, Some(value), None),
                Err(error) => {
                    let message = error.to_string();
                    store.apply(pending.key, None, Some(&message));
                }
            }
        }
        store
    }

    #[test]
    fn card_views_map_a_catalog_summary_to_a_media_card() {
        let mut media = test_media("A title");
        media.poster = Some("https://example.test/poster.jpg".to_owned());
        let views = card_views(&SectionEntries::Media(vec![media]));
        let card = views.first().expect("one card");
        assert_eq!(
            card.title,
            String::from("A title"),
            "the title is shown on the card"
        );
        assert!(
            card.poster.is_some(),
            "a poster URL becomes an image resource"
        );
    }

    #[test]
    fn anilist_ids_follow_the_card_order() {
        let mut first = test_media("First");
        first.anilist_id = 42;
        let mut second = test_media("Second");
        second.anilist_id = 7;
        let ids = anilist_ids(&SectionEntries::Media(vec![first, second]));
        assert_eq!(ids, [42, 7], "Open receives the card's AniList id");
    }

    #[tokio::test]
    async fn the_eight_sections_render_in_ticket_order() {
        let store = DiscoverStore::load(&FakeCatalog::default(), Some("viewer"))
            .await
            .store;

        let got: Vec<DiscoverSection> = store.sections().iter().map(|s| s.kind).collect();
        assert_eq!(
            got,
            DiscoverSection::ORDER,
            "sections must be in ticket order"
        );
    }

    #[tokio::test]
    async fn one_failing_section_leaves_the_others_rendered() {
        let catalog = FakeCatalog::failing(DiscoverKey::Popular);
        let store = settle(DiscoverStore::load(&catalog, Some("viewer")).await).await;

        let popular = store
            .sections()
            .iter()
            .find(|s| s.kind == DiscoverSection::Popular)
            .expect("the Popular section");
        assert!(matches!(popular.state, SectionFetch::Failed(_)));
        assert_eq!(popular.entries, SectionEntries::Media(Vec::new()));

        for section in store.sections() {
            if section.kind == DiscoverSection::Popular {
                continue;
            }
            assert_eq!(
                section.state,
                SectionFetch::Ready,
                "{} must render",
                section.kind.title()
            );
        }
    }

    #[tokio::test]
    async fn one_failed_country_does_not_hide_the_other_two() {
        let catalog = FakeCatalog::failing(DiscoverKey::Japan);
        let store = settle(DiscoverStore::load(&catalog, Some("viewer")).await).await;
        let country = store
            .sections()
            .iter()
            .find(|section| section.kind == DiscoverSection::TrendingByCountry)
            .expect("the country section");
        assert!(
            matches!(country.entries, SectionEntries::Countries(_)),
            "the country section holds three carousels"
        );
        let SectionEntries::Countries(trends) = &country.entries else {
            return;
        };
        assert!(trends.japan.is_empty(), "Japan failed");
        assert!(!trends.korea.is_empty(), "Korea still renders");
        assert!(!trends.china.is_empty(), "China still renders");
    }

    #[tokio::test]
    async fn a_second_visit_within_six_hours_makes_no_requests() {
        let catalog = FakeCatalog::default();
        let first = settle(DiscoverStore::load(&catalog, Some("viewer")).await).await;
        let requests = catalog.fetches();
        assert!(requests > 0, "the first visit fetches the catalog");

        // Well inside even the shortest Discover TTL (the one-hour schedule).
        catalog.advance(Duration::from_mins(30));
        let second = settle(DiscoverStore::load(&catalog, Some("viewer")).await).await;
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
        let third = settle(DiscoverStore::load(&catalog, Some("viewer")).await).await;
        assert!(
            catalog.fetches() > requests,
            "once every section is stale, its refresh fetches again"
        );
        assert_eq!(
            first.sections(),
            third.sections(),
            "the values are still shown"
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
    async fn a_guest_never_requests_their_personal_collection() {
        let catalog = FakeCatalog::failing(DiscoverKey::MissedSequels);
        let guest = DiscoverStore::load(&catalog, None).await.store;
        let sequels = guest
            .sections()
            .iter()
            .find(|section| section.kind == DiscoverSection::MissedSequels)
            .expect("the Missed Sequels section");
        assert_eq!(
            sequels.state,
            SectionFetch::Ready,
            "a guest is not sent an AniList request"
        );
        assert_eq!(sequels.entries, SectionEntries::Media(Vec::new()));

        let signed_in = settle(DiscoverStore::load(&catalog, Some("viewer")).await).await;
        let sequels = signed_in
            .sections()
            .iter()
            .find(|section| section.kind == DiscoverSection::MissedSequels)
            .expect("the Missed Sequels section");
        assert!(
            matches!(sequels.state, SectionFetch::Failed(_)),
            "an account with no cached sequels does make the request"
        );
    }

    #[tokio::test]
    async fn cached_values_are_published_before_their_refresh_lands() {
        let catalog = FakeCatalog::default();
        // Seed every key, then let them all go stale and fail one refresh.
        let _ = settle(DiscoverStore::load(&catalog, Some("viewer")).await).await;
        catalog.advance(Duration::from_hours(7));
        catalog.fail_refresh(DiscoverKey::TrendingManga);
        let update = DiscoverStore::load(&catalog, Some("viewer")).await;
        assert!(
            !update.pending.is_empty(),
            "stale sections come with refreshes"
        );

        let manga = update
            .store
            .sections()
            .iter()
            .find(|section| section.kind == DiscoverSection::TrendingManga)
            .expect("the manga section");
        assert_ne!(
            manga.entries,
            SectionEntries::Media(Vec::new()),
            "the cached value is on screen at once"
        );
        assert_eq!(manga.state, SectionFetch::Ready);

        let store = settle(update).await;
        let manga = store
            .sections()
            .iter()
            .find(|section| section.kind == DiscoverSection::TrendingManga)
            .expect("the manga section");
        assert!(matches!(manga.state, SectionFetch::Failed(_)));
        assert_ne!(
            manga.entries,
            SectionEntries::Media(Vec::new()),
            "a failed refresh keeps the stale value"
        );
    }

    #[tokio::test]
    async fn applying_one_refresh_leaves_the_other_sections_alone() {
        let catalog = FakeCatalog::default();
        let _ = settle(DiscoverStore::load(&catalog, Some("viewer")).await).await;
        catalog.advance(Duration::from_hours(7));
        let mut update = DiscoverStore::load(&catalog, Some("viewer")).await;
        let before = update.store.sections().to_vec();
        let pending = update.pending.remove(0);
        update
            .store
            .apply(pending.key, None, Some("refresh failed"));

        let changed = before
            .iter()
            .zip(update.store.sections())
            .filter(|(old, new)| old != new)
            .count();
        assert_eq!(changed, 1, "only the refreshed section changed");
    }

    #[gpui_kit::test]
    fn a_carousel_card_has_non_zero_bounds(cx: &mut TestAppContext) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            ui_theme::init(cx);
        });
        let store = DiscoverStore {
            sections: vec![Section {
                kind: DiscoverSection::Popular,
                entries: SectionEntries::Media(vec![test_media("A title")]),
                state: SectionFetch::Ready,
                country_states: std::array::from_fn(|_| SectionFetch::Ready),
            }],
        };
        let loader: DiscoverLoader = Rc::new(move |_, _| {
            Task::ready(Ok::<_, BridgeError>(DiscoverUpdate::new(store.clone())))
        });
        let (window, view) = cx
            .update(|cx| {
                gpui_kit::open_window(gpui_kit::WindowOptions::default(), cx, |_, cx| {
                    DiscoverView::new(None, loader, LruImageCache::new(4, cx), cx)
                })
            })
            .expect("the test platform opens windows");
        cx.update(|cx| view.update(cx, DiscoverView::load));
        cx.run_until_parked();
        cx.update_window(window, |_, window, cx| window.render_frame(cx))
            .expect("the test window");

        let bounds = cx
            .update_window(window, |_, window, _| {
                window
                    .find("discover-section-Popular Anime-card-0")
                    .bounds()
            })
            .expect("the test window");
        assert!(
            bounds.size.width > px(0.) && bounds.size.height > px(0.),
            "the loaded card is laid out: {bounds:?}"
        );
        let carousel_state = cx.read(|app| {
            view.read(app)
                .carousel_states
                .first()
                .cloned()
                .expect("the first carousel")
        });
        let item_count = cx.read(|app| carousel_state.read(app).item_count());
        assert_eq!(item_count, 1, "the carousel knows a card arrived");
    }

    /// Opens a Discover view with a real bridge so pending fetches run.
    fn open_view(
        cx: &mut TestAppContext,
        loader: DiscoverLoader,
    ) -> (
        tokio::runtime::Runtime,
        gpui_kit::AnyWindowHandle,
        Entity<DiscoverView>,
    ) {
        cx.update(|cx| {
            gpui_kit::init(cx);
            ui_theme::init(cx);
        });
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("a test Tokio runtime");
        cx.update(|cx| ui_state::bridge::init(cx, runtime.handle().clone()));
        let (window, view) = cx
            .update(|cx| {
                gpui_kit::open_window(gpui_kit::WindowOptions::default(), cx, |_, cx| {
                    DiscoverView::new(None, loader, LruImageCache::new(4, cx), cx)
                })
            })
            .expect("the test platform opens windows");
        (runtime, window, view)
    }

    fn render(cx: &mut TestAppContext, window: gpui_kit::AnyWindowHandle) {
        cx.update_window(window, |_, window, cx| window.render_frame(cx))
            .expect("the test window");
    }

    fn shows(cx: &mut TestAppContext, window: gpui_kit::AnyWindowHandle, id: &'static str) -> bool {
        cx.update_window(window, |_, window, _| window.try_find(id).is_some())
            .expect("the test window")
    }

    #[gpui_kit::test]
    fn a_refresh_updates_the_carousel_item_count(cx: &mut TestAppContext) {
        let store = DiscoverStore::from_sections(vec![Section::new(
            DiscoverSection::Popular,
            SectionEntries::Media(vec![test_media("Before")]),
            SectionFetch::Ready,
        )]);
        let refreshed = SectionValue {
            entries: SectionEntries::Media(vec![
                test_media("One"),
                test_media("Two"),
                test_media("Three"),
            ]),
            updated_at: UNIX_EPOCH,
            offline: false,
        };
        let loader: DiscoverLoader = Rc::new(move |_, _| {
            let value = refreshed.clone();
            let mut update = DiscoverUpdate::new(store.clone());
            update.pending.push(PendingRefresh {
                key: DiscoverKey::Popular,
                refresh: Box::pin(async move { Ok(value) }),
            });
            Task::ready(Ok::<_, BridgeError>(update))
        });
        let (_runtime, window, view) = open_view(cx, loader);
        cx.update(|cx| view.update(cx, DiscoverView::load));
        cx.run_until_parked();
        render(cx, window);

        let carousel_state = cx.read(|app| {
            view.read(app)
                .carousel_states
                .first()
                .cloned()
                .expect("the first carousel")
        });
        let item_count = cx.read(|app| carousel_state.read(app).item_count());
        assert_eq!(item_count, 3, "the count follows the refreshed cards");
        assert!(
            shows(cx, window, "discover-section-Popular Anime-card-2"),
            "the third refreshed card renders"
        );
    }

    /// Every key but one is cached; the remaining miss waits on a gate.
    #[derive(Clone)]
    struct GatedCatalog {
        cached: Arc<HashSet<DiscoverKey>>,
        gate: Arc<Mutex<Option<tokio::sync::oneshot::Receiver<()>>>>,
    }

    impl DiscoverCatalog for GatedCatalog {
        async fn load(&self, key: DiscoverKey, _user: &str) -> Result<LoadedSection, Error> {
            std::future::ready(()).await;
            if self.cached.contains(&key) {
                return Ok(LoadedSection {
                    value: SectionValue {
                        entries: SectionEntries::Media(vec![test_media("Cached")]),
                        updated_at: UNIX_EPOCH,
                        offline: false,
                    },
                    refresh: None,
                });
            }
            let receiver = self
                .gate
                .lock()
                .expect("test lock poisoned")
                .take()
                .expect("one gated miss");
            Ok(LoadedSection {
                value: empty_value(key),
                refresh: Some(Box::pin(async move {
                    receiver.await.expect("the gate was not dropped");
                    Ok(SectionValue {
                        entries: SectionEntries::Media(vec![test_media("Sequel")]),
                        updated_at: UNIX_EPOCH,
                        offline: false,
                    })
                })),
            })
        }
    }

    #[tokio::test]
    async fn a_cached_section_is_published_before_a_slow_miss() {
        let (gate, receiver) = tokio::sync::oneshot::channel::<()>();
        let cached: HashSet<DiscoverKey> = DiscoverSection::ORDER
            .into_iter()
            .flat_map(|section| match section {
                DiscoverSection::TrendingByCountry => {
                    vec![DiscoverKey::Japan, DiscoverKey::Korea, DiscoverKey::China]
                }
                _ => vec![key_of(section)],
            })
            .filter(|key| *key != DiscoverKey::MissedSequels)
            .collect();
        let catalog = GatedCatalog {
            cached: Arc::new(cached),
            gate: Arc::new(Mutex::new(Some(receiver))),
        };

        let update = DiscoverStore::load(&catalog, Some("viewer")).await;
        let popular = update
            .store
            .sections()
            .iter()
            .find(|section| section.kind == DiscoverSection::Popular)
            .expect("the cached section");
        assert_ne!(
            popular.entries,
            SectionEntries::Media(Vec::new()),
            "the cached section is available before the miss resolves"
        );
        let missed = update
            .store
            .sections()
            .iter()
            .find(|section| section.kind == DiscoverSection::MissedSequels)
            .expect("the missing section");
        assert_eq!(missed.entries, SectionEntries::Media(Vec::new()));
        assert_eq!(update.pending.len(), 1, "only the miss is pending");

        gate.send(()).expect("the test sends one gate release");
        let store = settle(update).await;
        let missed = store
            .sections()
            .iter()
            .find(|section| section.kind == DiscoverSection::MissedSequels)
            .expect("the missing section");
        assert_ne!(
            missed.entries,
            SectionEntries::Media(Vec::new()),
            "the miss is applied when it lands"
        );
    }
}
