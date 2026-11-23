//! Advanced Search state: the full filter set carried in the route, and a
//! paged catalog session that makes one request per committed change (ticket
//! #59, decision #23).

use std::future::Future;

use domain::MediaKind;

/// Result order for Advanced Search.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the AniList filter set is fixed by decision #23"
)]
pub enum SearchSort {
    /// Closest title match first.
    Match,
    /// Most active in the past hour first.
    Trending,
    /// On the most lists first.
    Popularity,
    /// Highest scored first.
    Score,
    /// Most favourited first.
    Favourites,
    /// Newest first.
    StartDate,
    /// Most recently edited first.
    UpdatedAt,
}

/// Release format filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the AniList filter set is fixed by decision #23"
)]
pub enum SearchFormat {
    /// TV series.
    Tv,
    /// Short TV series.
    TvShort,
    /// Film.
    Movie,
    /// Special.
    Special,
    /// Original video animation.
    Ova,
    /// Original net animation.
    Ona,
    /// Music video.
    Music,
    /// Manga, manhwa or manhua.
    Manga,
    /// Light novel.
    Novel,
    /// One-shot manga.
    OneShot,
}

/// Releasing-status filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the AniList filter set is fixed by decision #23"
)]
pub enum SearchStatus {
    /// Still releasing.
    Releasing,
    /// Finished.
    Finished,
    /// Announced but not out.
    NotYetReleased,
    /// Ended early.
    Cancelled,
    /// Paused.
    Hiatus,
}

/// Season filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the AniList filter set is fixed by decision #23"
)]
pub enum SearchSeason {
    /// December to February.
    Winter,
    /// March to May.
    Spring,
    /// June to August.
    Summer,
    /// September to November.
    Fall,
}

/// The full AniList filter set from decision #23. Stored by value in
/// [`Route::Search`], so Back restores the filters along with the scroll
/// offset keyed by that route value.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
#[expect(
    clippy::exhaustive_structs,
    reason = "the AniList filter set is fixed by decision #23 and the route owns it"
)]
pub struct SearchFilters {
    /// Free-text query over titles.
    pub query: Option<String>,
    /// Anime or manga.
    pub kind: Option<MediaKind>,
    /// Result order.
    pub sort: Option<SearchSort>,
    /// Release format.
    pub format: Option<SearchFormat>,
    /// Releasing status.
    pub status: Option<SearchStatus>,
    /// Season first aired.
    pub season: Option<SearchSeason>,
    /// Year of that season.
    pub year: Option<u16>,
    /// Genres to match; AniList's `genre_in` matches any of them.
    pub genres: Vec<String>,
    /// ISO country of origin, such as `JP`.
    pub country: Option<String>,
    /// Whether to show adult media.
    pub adult: bool,
}

/// One result card's catalog text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MediaPreview {
    /// The AniList id, used to mint a local identity when a card is opened.
    pub anilist_id: u32,
    /// The display title.
    pub title: String,
    /// The poster URL, when AniList has one.
    pub poster: Option<String>,
    /// The release format, when known.
    pub format: Option<SearchFormat>,
    /// The mean score, raw 0–100.
    pub score: Option<u8>,
}

/// One page of results.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchPage {
    /// The matching media.
    pub results: Vec<MediaPreview>,
    /// Whether another page follows. AniList's totals are unreliable, so
    /// this is the only paging signal.
    pub has_next_page: bool,
}

/// Why a catalog request failed.
#[derive(Debug, thiserror::Error)]
#[error("catalog search failed: {message}")]
pub struct CatalogError {
    message: String,
}

impl CatalogError {
    /// An error with a message safe to show in the UI.
    pub fn new(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
        }
    }
}

/// The catalog boundary, injected so request counts can be tested with a
/// fake service.
pub trait Catalog {
    /// Fetches page `page` (counting from 1) for `filters`.
    fn search(
        &self,
        filters: &SearchFilters,
        page: u32,
    ) -> impl Future<Output = Result<SearchPage, CatalogError>> + Send;
}

/// The observable state of Advanced Search, with no GPUI dependency.
#[derive(Debug, Clone)]
pub struct SearchSession<C> {
    catalog: C,
    filters: SearchFilters,
    results: Vec<MediaPreview>,
    has_next: bool,
    page: u32,
}

impl<C> SearchSession<C> {
    /// A session over `catalog` starting with `filters`. No request is made
    /// until [`Self::apply`] is called.
    pub fn new(catalog: C, filters: SearchFilters) -> Self {
        Self {
            catalog,
            filters,
            results: Vec::new(),
            has_next: false,
            page: 0,
        }
    }

    /// The committed filters, identical to the ones in the route.
    pub fn filters(&self) -> &SearchFilters {
        &self.filters
    }

    /// Everything loaded so far.
    pub fn results(&self) -> &[MediaPreview] {
        &self.results
    }

    /// The last loaded page, or 0 before the first request.
    pub fn page(&self) -> u32 {
        self.page
    }

    /// Whether a further page is available.
    pub fn has_next(&self) -> bool {
        self.has_next
    }
}

impl<C: Catalog> SearchSession<C> {
    /// Commits `filters`, replacing the results. An unchanged filter set is
    /// a no-op, so a re-render or a repeated click costs no request.
    ///
    /// # Errors
    ///
    /// Returns the catalog's error; the session keeps the old results.
    pub async fn apply(&mut self, filters: SearchFilters) -> Result<(), CatalogError> {
        if filters == self.filters && self.page > 0 {
            return Ok(());
        }
        // Fetch before mutating: a failed request leaves the last good
        // results and filters on screen.
        let page = self.catalog.search(&filters, 1).await?;
        self.filters = filters;
        self.results = page.results;
        self.has_next = page.has_next_page;
        self.page = 1;
        Ok(())
    }

    /// Loads one more page. A no-op when AniList reported no further page.
    ///
    /// # Errors
    ///
    /// Returns the catalog's error; the session keeps the old results.
    pub async fn next_page(&mut self) -> Result<(), CatalogError> {
        if !self.has_next {
            return Ok(());
        }
        let next = self.page + 1;
        let page = self.catalog.search(&self.filters, next).await?;
        self.results.extend(page.results);
        self.has_next = page.has_next_page;
        self.page = next;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{
        Arc,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    };

    use gpui_kit::{Point, point, px};
    use tokio::task::yield_now;

    use super::{
        Catalog, CatalogError, MediaPreview, SearchFilters, SearchPage, SearchSession, SearchSort,
    };
    use crate::{Route, RouterState};

    struct FakeCatalog {
        hits: Arc<AtomicUsize>,
        pages: Arc<std::sync::Mutex<Vec<(SearchFilters, u32)>>>,
        last_page: u32,
        fail: Arc<AtomicBool>,
    }

    impl Catalog for FakeCatalog {
        async fn search(
            &self,
            filters: &SearchFilters,
            page: u32,
        ) -> Result<SearchPage, CatalogError> {
            yield_now().await;
            if self.fail.load(Ordering::SeqCst) {
                return Err(CatalogError::new("offline"));
            }
            self.hits.fetch_add(1, Ordering::SeqCst);
            self.pages
                .lock()
                .map_err(|_| CatalogError::new("poisoned fake"))?
                .push((filters.clone(), page));
            Ok(SearchPage {
                results: vec![MediaPreview {
                    anilist_id: page,
                    title: format!("Result {page}"),
                    poster: None,
                    format: None,
                    score: None,
                }],
                has_next_page: page < self.last_page,
            })
        }
    }

    fn catalog(last_page: u32) -> (FakeCatalog, Arc<AtomicUsize>) {
        let hits = Arc::new(AtomicUsize::new(0));
        (
            FakeCatalog {
                hits: Arc::clone(&hits),
                pages: Arc::new(std::sync::Mutex::new(Vec::new())),
                last_page,
                fail: Arc::new(AtomicBool::new(false)),
            },
            hits,
        )
    }

    fn filters(query: &str) -> SearchFilters {
        SearchFilters {
            query: Some(query.to_owned()),
            ..SearchFilters::default()
        }
    }

    #[tokio::test]
    async fn a_committed_filter_change_costs_exactly_one_request() {
        let (fake, hits) = catalog(3);
        let mut session = SearchSession::new(fake, SearchFilters::default());

        session.apply(filters("frieren")).await.expect("first page");
        assert_eq!(hits.load(Ordering::SeqCst), 1, "one request per filter set");

        session
            .apply(filters("frieren"))
            .await
            .expect("unchanged filters");
        assert_eq!(hits.load(Ordering::SeqCst), 1, "a repeat costs no request");

        let mut sorted = filters("frieren");
        sorted.sort = Some(SearchSort::Score);
        session.apply(sorted).await.expect("changed filter");
        assert_eq!(hits.load(Ordering::SeqCst), 2, "one request per change");
        assert_eq!(session.page(), 1, "a new filter starts at page one");
        assert_eq!(session.results().len(), 1, "old results were replaced");
    }

    #[tokio::test]
    async fn paging_costs_one_request_per_page_and_stops_at_the_end() {
        let (fake, hits) = catalog(2);
        let mut session = SearchSession::new(fake, SearchFilters::default());
        session.apply(filters("x")).await.expect("first page");

        session.next_page().await.expect("second page");
        assert_eq!(hits.load(Ordering::SeqCst), 2, "one request for page two");
        assert_eq!(session.results().len(), 2, "pages are appended");
        assert!(!session.has_next(), "the last page has no successor");

        session.next_page().await.expect("no third page");
        assert_eq!(hits.load(Ordering::SeqCst), 2, "no request past the end");
    }

    #[tokio::test]
    async fn a_failed_request_keeps_the_last_good_results() {
        let (fake, hits) = catalog(3);
        let fail = Arc::clone(&fake.fail);
        let mut session = SearchSession::new(fake, SearchFilters::default());
        session.apply(filters("frieren")).await.expect("first page");
        let good = session.results().to_vec();

        fail.store(true, Ordering::SeqCst);
        session
            .apply(filters("vinland"))
            .await
            .expect_err("catalog is offline");
        assert_eq!(session.results(), good.as_slice(), "old results stay put");
        assert_eq!(session.filters(), &filters("frieren"), "old filters stay");
        assert_eq!(
            hits.load(Ordering::SeqCst),
            1,
            "only the successful call hit"
        );
    }

    #[test]
    fn back_restores_the_filters_and_the_scroll_offset() {
        let first = filters("frieren");
        let second = filters("vinland");
        let mut router = RouterState::new(Route::Home);
        router.push(Route::Search(first.clone()), Point::default());
        router.push(Route::Search(second), point(px(320.), px(0.)));

        let restore = router.back(Point::default()).expect("a previous route");
        assert_eq!(restore, point(px(320.), px(0.)), "scroll comes back");
        assert_eq!(
            router.current(),
            &Route::Search(first),
            "the filters come back in the route"
        );
    }
}
