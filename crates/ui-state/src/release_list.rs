//! The manual **release list** sheet (#221, #182 Q5–Q7, Q9).
//!
//! A right-side sheet over the indexer results for one episode. The view
//! model here is pure and tested: hard-filter status against a profile,
//! sortable columns (rank by default, ★ Best on the top passing row),
//! client-side chips, inline batch expansion, and a session-level `no peers`
//! mark on a release that failed to start. Searching and file listing go
//! through the engine (`render()` stays pure).

use std::{
    collections::{BTreeMap, BTreeSet},
    sync::Arc,
};

use domain::{MediaId, PlaybackDetails, StreamSource, TorrentFileRow};
use engine::EngineServices;
use gpui_kit::{Context, Task};
use search::{EpisodeTarget, Indexer, Torrent, rank_with_status};
use settings::AutoSelectProfile;

/// The `no peers` status of a release that failed to start this session.
pub const NO_PEERS: &str = "no peers";
/// One result row in the release list.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReleaseRow {
    /// The underlying indexer result, for streaming and file listing.
    pub torrent: Torrent,
    /// Lowercase v1 infohash, the row key.
    pub infohash: String,
    /// Release name.
    pub name: String,
    /// Origin indexer.
    pub indexer: Indexer,
    /// `1080p` etc, when known.
    pub resolution: Option<u32>,
    /// Video codec label, when known.
    pub codec: Option<String>,
    /// Release group, when known.
    pub group: Option<String>,
    /// Seeders, when known.
    pub seeders: Option<u32>,
    /// Total size in bytes, when known.
    pub size: Option<u64>,
    /// Published Unix seconds, when known.
    pub published: Option<u64>,
    /// Whether this is a batch (range, `Batch`, `Complete`, volume).
    pub batch: bool,
    /// `Batch 1–8` range label, when the name states one.
    pub batch_range: Option<String>,
    /// Whether it passes every hard filter of the active profile.
    pub passes: bool,
    /// Hard-filter reason when failing (`min seeders`, `excluded term`,
    /// `size`, `wrong episode`, `wrong season`, `not an episode`, `only`,
    /// `never`, `require …`), `no peers`, or `None` when passing.
    pub status: Option<String>,
    /// Whether this release was marked `no peers` this app session.
    pub no_peers: bool,
}

/// A sortable column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum SortColumn {
    /// Auto-select rank (default); passing first, ★ Best on top.
    #[default]
    Rank,
    /// Release name.
    Release,
    /// Resolution.
    Resolution,
    /// Video codec.
    Codec,
    /// Release group.
    Group,
    /// Seeders.
    Seeders,
    /// Size.
    Size,
    /// Published date.
    Date,
    /// Batch vs single.
    Batch,
    /// Status.
    Status,
}

/// Sort direction; a second header click reverses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum SortDirection {
    /// Smallest first / A→Z.
    #[default]
    Ascending,
    /// Largest first / Z→A.
    Descending,
}

/// The current sort; `column` clicked again flips `direction`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct ReleaseSort {
    /// The sorted column.
    pub column: SortColumn,
    /// The direction.
    pub direction: SortDirection,
}

impl ReleaseSort {
    /// Header click: pick a column, or reverse it if already active. `Rank`
    /// always returns to rank-ascending.
    #[must_use]
    pub fn clicked(self, column: SortColumn) -> Self {
        if column == SortColumn::Rank {
            return Self {
                column: SortColumn::Rank,
                direction: SortDirection::Ascending,
            };
        }
        if self.column == column {
            let direction = match self.direction {
                SortDirection::Ascending => SortDirection::Descending,
                SortDirection::Descending => SortDirection::Ascending,
            };
            Self { column, direction }
        } else {
            Self {
                column,
                direction: SortDirection::Ascending,
            }
        }
    }
}

/// Client-side `All / Single / Batch`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum KindFilter {
    /// Everything.
    #[default]
    All,
    /// Only single-episode releases.
    Single,
    /// Only batches.
    Batch,
}

/// Client-side chips; never trigger a search. Reset on open.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReleaseFilters {
    /// All / Single / Batch.
    pub kind: KindFilter,
    /// Selected resolutions; empty = all. Only values present are offered.
    pub resolutions: BTreeSet<u32>,
    /// Selected indexers; empty = all.
    pub indexers: BTreeSet<String>,
    /// `Show filtered-out` (default on): keep failing rows, dimmed.
    pub show_filtered_out: bool,
}

impl ReleaseFilters {
    /// Fresh filters for a newly opened sheet: `Show filtered-out` on.
    #[must_use]
    pub fn new() -> Self {
        Self {
            kind: KindFilter::All,
            resolutions: BTreeSet::new(),
            indexers: BTreeSet::new(),
            show_filtered_out: true,
        }
    }
}

/// Resolutions present in the results, ascending.
#[must_use]
pub fn available_resolutions(rows: &[ReleaseRow]) -> Vec<u32> {
    rows.iter()
        .filter_map(|row| row.resolution)
        .collect::<BTreeSet<u32>>()
        .into_iter()
        .collect()
}

/// Indexer labels present in the results, in a stable order.
#[must_use]
pub fn available_indexers(rows: &[ReleaseRow]) -> Vec<String> {
    rows.iter()
        .map(|row| row.indexer.label().to_owned())
        .collect::<BTreeSet<String>>()
        .into_iter()
        .collect()
}

/// The rows shown: client-side filters applied, then sorted. Failing rows
/// are dropped when `show_filtered_out` is off; otherwise they sit below
/// passing ones under rank sort.
#[must_use]
pub fn visible_rows<'a>(
    rows: &'a [ReleaseRow],
    filters: &ReleaseFilters,
    sort: ReleaseSort,
) -> Vec<&'a ReleaseRow> {
    let mut visible: Vec<&ReleaseRow> = rows
        .iter()
        .filter(|row| filters.show_filtered_out || row.passes)
        .filter(|row| match filters.kind {
            KindFilter::All => true,
            KindFilter::Single => !row.batch,
            KindFilter::Batch => row.batch,
        })
        .filter(|row| {
            filters.resolutions.is_empty()
                || row
                    .resolution
                    .is_some_and(|resolution| filters.resolutions.contains(&resolution))
        })
        .filter(|row| filters.indexers.is_empty() || filters.indexers.contains(row.indexer.label()))
        .collect();
    sort_rows(&mut visible, sort);
    visible
}

/// Sorts in place. Passing rows always sit above failing ones; `Rank`
/// preserves the pre-ranked order. `Descending` reverses within a group.
pub fn sort_rows(rows: &mut Vec<&ReleaseRow>, sort: ReleaseSort) {
    // Stable: rank order is the incoming order; apply a stable sort.
    let cmp = |a: &&ReleaseRow, b: &&ReleaseRow| compare(a, b, sort);
    rows.sort_by(|a, b| {
        b.passes
            .cmp(&a.passes)
            .then_with(|| cmp(a, b))
            .then_with(|| a.infohash.cmp(&b.infohash))
    });
}

fn compare(a: &ReleaseRow, b: &ReleaseRow, sort: ReleaseSort) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    let base = match sort.column {
        SortColumn::Rank => Ordering::Equal,
        SortColumn::Release => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        SortColumn::Resolution => a.resolution.cmp(&b.resolution),
        SortColumn::Codec => a.codec.cmp(&b.codec),
        SortColumn::Group => a
            .group
            .as_deref()
            .unwrap_or("")
            .to_lowercase()
            .cmp(&b.group.as_deref().unwrap_or("").to_lowercase()),
        SortColumn::Seeders => a.seeders.cmp(&b.seeders),
        SortColumn::Size => a.size.cmp(&b.size),
        SortColumn::Date => a.published.cmp(&b.published),
        SortColumn::Batch => a.batch.cmp(&b.batch),
        SortColumn::Status => a.status.cmp(&b.status),
    };
    match sort.direction {
        SortDirection::Ascending => base,
        SortDirection::Descending => base.reverse(),
    }
}

/// The index of the top passing row (★ Best), if any.
#[must_use]
pub fn best_index(rows: &[&ReleaseRow]) -> Option<usize> {
    rows.iter().position(|row| row.passes && !row.no_peers)
}

/// Builds rows from raw results, ranked against `target` + `profile`, with
/// session `no_peers` infohashes marked. Failing rows are kept.
#[must_use]
pub fn build_rows(
    results: Vec<Torrent>,
    target: &EpisodeTarget,
    profile: &AutoSelectProfile,
    no_peers: &BTreeSet<String>,
) -> Vec<ReleaseRow> {
    rank_with_status(results, target, profile)
        .into_iter()
        .map(|ranked| {
            let torrent = ranked.torrent;
            let release = release_parser::parse(&torrent.name);
            let infohash = torrent.infohash.to_ascii_lowercase();
            let no_peers = no_peers.contains(&infohash);
            let batch_range = if release.batch || torrent.is_batch {
                batch_range_label(&release)
            } else {
                None
            };
            let batch = release.batch || torrent.is_batch;
            let (passes, status) = if no_peers {
                (ranked.passes, Some(NO_PEERS.to_owned()))
            } else {
                (ranked.passes, ranked.status)
            };
            ReleaseRow {
                name: torrent.name.clone(),
                indexer: torrent.indexer,
                resolution: release.resolution,
                codec: release.video_codec.map(codec_label),
                group: release.group,
                seeders: torrent.seeders,
                size: torrent.size,
                published: torrent.published,
                batch,
                batch_range,
                passes,
                status,
                no_peers,
                torrent,
                infohash,
            }
        })
        .collect()
}

/// Re-ranks current results with a one-shot profile, keeping `no_peers`.
#[must_use]
pub fn re_rank(
    raw: &[Torrent],
    target: &EpisodeTarget,
    profile: &AutoSelectProfile,
    no_peers: &BTreeSet<String>,
) -> Vec<ReleaseRow> {
    build_rows(raw.to_vec(), target, profile, no_peers)
}

fn batch_range_label(release: &release_parser::Release) -> Option<String> {
    match (release.episode, release.last_episode) {
        (Some(first), Some(last)) => Some(format!("Batch {first}–{last}")),
        _ => None,
    }
}

/// A `VideoCodec` as a short label.
#[must_use]
pub fn codec_label(codec: release_parser::VideoCodec) -> String {
    use release_parser::VideoCodec::*;
    match codec {
        H264 => "H.264",
        H265 => "HEVC",
        Av1 => "AV1",
        Xvid => "Xvid",
        Vp9 => "VP9",
        Mpeg2 => "MPEG-2",
        _ => return "?".to_owned(),
    }
    .to_owned()
}

/// One file inside an expanded batch (#182 Q7).
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct BatchFile {
    /// File index inside the torrent.
    pub index: usize,
    /// `/`-separated path inside the torrent.
    pub path: String,
    /// The file name.
    pub name: String,
    /// Size in bytes.
    pub size: u64,
    /// Parsed episode number within this media, when known.
    pub episode: Option<u32>,
    /// Whether this is the requested episode's file.
    pub is_requested_episode: bool,
    /// Whether it can be played (not NCOP/NCED/sample/extra, a video).
    pub playable: bool,
    /// Whether it is preselected with ▶ Stream.
    pub preselected: bool,
}

/// State of an expanded batch's file list.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum BatchState {
    /// File list being fetched.
    #[default]
    Loading,
    /// Loaded file list.
    Loaded(Vec<BatchFile>),
    /// The file list could not be fetched.
    Failed,
}

/// Builds batch files in order from a torrent file list. The requested
/// episode is preselected; NCOP/NCED/sample/extra files are dimmed.
#[must_use]
pub fn batch_files(rows: Vec<TorrentFileRow>, target: &EpisodeTarget) -> Vec<BatchFile> {
    let entries: Vec<(usize, &str)> = rows
        .iter()
        .map(|row| (row.index, row.path.as_str()))
        .collect();
    let preselected = search::pick_file(entries.iter().copied(), target);
    let mut files: Vec<BatchFile> = rows
        .into_iter()
        .map(|row| {
            let name = row
                .path
                .rsplit(['/', '\\'])
                .next()
                .unwrap_or(&row.path)
                .to_owned();
            let release = release_parser::parse(&name);
            let episode = match release.kind {
                release_parser::EpisodeKind::Episode => release.episode,
                _ => None,
            };
            let is_requested_episode =
                search::matches(&release, target) && release.last_episode.is_none();
            let playable = search::is_playable_file(&row.path);
            BatchFile {
                index: row.index,
                name,
                path: row.path,
                size: row.len,
                episode,
                is_requested_episode,
                playable,
                preselected: false,
            }
        })
        .collect();
    files.sort_by_key(|file| file.index);
    if let Some(preselected) = preselected
        && let Some(file) = files.iter_mut().find(|file| file.index == preselected)
    {
        file.preselected = true;
    } else if let Some(file) = files.iter_mut().find(|file| file.is_requested_episode) {
        file.preselected = true;
    }
    files
}

/// The episode a picked file plays as: its parsed episode, else `requested`.
#[must_use]
pub fn file_episode(path: &str, requested: u32) -> u32 {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let release = release_parser::parse(name);
    match release.kind {
        release_parser::EpisodeKind::Episode => release.episode.unwrap_or(requested),
        _ => requested,
    }
}

/// Re-exported `BTreeMap` key type for expanded batches.
pub type BatchMap = BTreeMap<String, BatchState>;

/// The torrent file list when a torrent already carries it (`AnimeTosho`),
/// else empty.
#[must_use]
pub fn indexer_files(torrent: &Torrent) -> Vec<TorrentFileRow> {
    torrent
        .files
        .iter()
        .enumerate()
        .map(|(index, path)| TorrentFileRow::new(index, path.clone(), 0, false))
        .collect()
}

/// The episode + media context a sheet is opened for.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct ReleaseListContext {
    /// The media.
    pub media: MediaId,
    /// The AniList id.
    pub anilist_id: u32,
    /// The requested episode number `N`.
    pub episode: u32,
    /// The episode title, when known.
    pub episode_title: Option<String>,
    /// This media's season `M`.
    pub season: u32,
    /// `AniZip` absolute number when it differs from `episode`.
    pub absolute_episode: Option<u32>,
    /// The indexer search for this episode.
    pub search: search::SearchRequest,
    /// The file-match target.
    pub target: EpisodeTarget,
    /// The media title, for HUD labels.
    pub media_title: String,
    /// The media poster, for HUD labels.
    pub poster: Option<String>,
    /// The effective saved profile (before any one-shot override).
    pub profile: AutoSelectProfile,
    /// Every saved profile, for **Profile ▾**.
    pub profiles: Vec<AutoSelectProfile>,
    /// `#181`'s first query, prefilled in the editable query box.
    pub prefill_query: String,
    /// An auto-select failure reason, shown in the header.
    pub failure_reason: Option<String>,
}

/// The release list sheet's store. One instance, reused; `open` resets
/// per-sheet state while `no_peers` persists for the app session.
pub struct ReleaseListStore {
    services: Arc<dyn EngineServices>,
    playback: gpui_kit::Entity<crate::PlaybackStore>,
    context: Option<ReleaseListContext>,
    /// Editable query; `prefill_query` on open.
    query: String,
    /// Raw results from the last search, for re-ranking.
    raw_results: Vec<Torrent>,
    /// Ranked rows.
    rows: Vec<ReleaseRow>,
    /// One-shot profile override; not saved.
    profile_override: Option<AutoSelectProfile>,
    sort: ReleaseSort,
    filters: ReleaseFilters,
    /// Expanded batch infohash (one at a time).
    expanded: Option<String>,
    /// Per-infohash batch file-list state.
    batches: BTreeMap<String, BatchState>,
    /// Row-level amber message, e.g. no episode-N file.
    amber: BTreeMap<String, String>,
    /// Session-level `no peers` infohashes.
    no_peers: BTreeSet<String>,
    /// A search is in flight.
    searching: bool,
    /// Last search error.
    error: Option<String>,
    task: Option<Task<()>>,
    batch_task: Option<Task<()>>,
    stream_task: Option<Task<()>>,
}

impl std::fmt::Debug for ReleaseListStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReleaseListStore")
            .field("episode", &self.context.as_ref().map(|c| c.episode))
            .field("results", &self.rows.len())
            .field("searching", &self.searching)
            .finish_non_exhaustive()
    }
}

impl ReleaseListStore {
    /// A store over `services`, playing through `playback`.
    #[must_use]
    pub fn new(
        services: Arc<dyn EngineServices>,
        playback: gpui_kit::Entity<crate::PlaybackStore>,
    ) -> Self {
        Self {
            services,
            playback,
            context: None,
            query: String::new(),
            raw_results: Vec::new(),
            rows: Vec::new(),
            profile_override: None,
            sort: ReleaseSort::default(),
            filters: ReleaseFilters::new(),
            expanded: None,
            batches: BTreeMap::new(),
            amber: BTreeMap::new(),
            no_peers: BTreeSet::new(),
            searching: false,
            error: None,
            task: None,
            batch_task: None,
            stream_task: None,
        }
    }

    /// Opens the sheet for `context`, resetting per-sheet state and running
    /// the initial search with `context.prefill_query`. `no_peers` persists.
    pub fn open(&mut self, context: ReleaseListContext, cx: &mut Context<'_, Self>) {
        self.query.clone_from(&context.prefill_query);
        self.context = Some(context);
        self.raw_results = Vec::new();
        self.rows = Vec::new();
        self.profile_override = None;
        self.sort = ReleaseSort::default();
        self.filters = ReleaseFilters::new();
        self.expanded = None;
        self.batches = BTreeMap::new();
        self.amber = BTreeMap::new();
        self.error = None;
        self.task = None;
        self.batch_task = None;
        self.stream_task = None;
        cx.notify();
        let keyword = self.query.clone();
        self.run_search(keyword, false, cx);
    }

    /// The context the sheet is open for.
    #[must_use]
    pub fn context(&self) -> Option<&ReleaseListContext> {
        self.context.as_ref()
    }

    /// The editable query.
    #[must_use]
    pub fn query(&self) -> &str {
        &self.query
    }

    /// Sets the editable query (does not search).
    pub fn set_query(&mut self, query: String, cx: &mut Context<'_, Self>) {
        self.query = query;
        cx.notify();
    }

    /// Enter: one keyword search on both indexers. `refresh` (**Refresh**)
    /// bypasses the 60 s cache.
    #[expect(
        clippy::fn_params_excessive_bools,
        reason = "refresh bypasses the cache"
    )]
    pub fn search(&mut self, refresh: bool, cx: &mut Context<'_, Self>) {
        let keyword = self.query.clone();
        self.run_search(keyword, refresh, cx);
    }

    #[expect(
        clippy::fn_params_excessive_bools,
        reason = "refresh bypasses the cache"
    )]
    fn run_search(&mut self, keyword: String, refresh: bool, cx: &mut Context<'_, Self>) {
        let Some(ctx) = self.context.clone() else {
            return;
        };
        let services = Arc::clone(&self.services);
        self.searching = true;
        self.error = None;
        cx.notify();
        self.task = Some(cx.spawn(async move |store, cx| {
            let result: Result<Vec<Torrent>, String> = cx
                .update(|app| crate::bridge::run(app, services.search_releases(keyword, refresh)))
                .await
                .map_err(|error| error.to_string())
                .and_then(|inner| inner.map_err(|error| error.to_string()));
            let _ = store.update(cx, |store, cx| {
                store.searching = false;
                match result {
                    Ok(results) => {
                        store.raw_results = results;
                        store.rerank(ctx.target);
                    }
                    Err(error) => {
                        store.error = Some(error);
                    }
                }
                cx.notify();
            });
        }));
    }

    /// A search is in flight.
    #[must_use]
    pub fn searching(&self) -> bool {
        self.searching
    }

    /// The last search error.
    #[must_use]
    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// The active one-shot profile.
    #[must_use]
    pub fn active_profile(&self) -> AutoSelectProfile {
        if let Some(profile) = &self.profile_override {
            return profile.clone();
        }
        self.context
            .as_ref()
            .map_or_else(AutoSelectProfile::default, |c| c.profile.clone())
    }

    /// Saved profiles for **Profile ▾**, if the caller supplies them.
    /// The one-shot override names a profile; re-ranks current results.
    pub fn set_profile_override(
        &mut self,
        profile: Option<AutoSelectProfile>,
        cx: &mut Context<'_, Self>,
    ) {
        self.profile_override = profile;
        self.rerank_current(cx);
    }

    /// The current one-shot override name.
    #[must_use]
    pub fn profile_override_name(&self) -> Option<&str> {
        self.profile_override.as_ref().map(|p| p.name.as_str())
    }

    /// The current sort.
    #[must_use]
    pub fn sort(&self) -> ReleaseSort {
        self.sort
    }

    /// Header click on a column.
    pub fn click_sort(&mut self, column: SortColumn, cx: &mut Context<'_, Self>) {
        self.sort = self.sort.clicked(column);
        cx.notify();
    }

    /// The current filters.
    #[must_use]
    pub fn filters(&self) -> &ReleaseFilters {
        &self.filters
    }

    /// Mutate filters (client-side; never triggers a search).
    pub fn update_filters(
        &mut self,
        update: impl FnOnce(&mut ReleaseFilters),
        cx: &mut Context<'_, Self>,
    ) {
        update(&mut self.filters);
        cx.notify();
    }

    /// The rows shown, filtered + sorted.
    #[must_use]
    pub fn visible_rows(&self) -> Vec<&ReleaseRow> {
        visible_rows(&self.rows, &self.filters, self.sort)
    }

    /// All ranked rows (unfiltered).
    #[must_use]
    pub fn rows(&self) -> &[ReleaseRow] {
        &self.rows
    }

    /// Resolutions present, for the chip row.
    #[must_use]
    pub fn available_resolutions(&self) -> Vec<u32> {
        available_resolutions(&self.rows)
    }

    /// Indexers present, for the chip row.
    #[must_use]
    pub fn available_indexers(&self) -> Vec<String> {
        available_indexers(&self.rows)
    }

    /// ★ Best index into [`Self::visible_rows`].
    #[must_use]
    pub fn best_index(&self) -> Option<usize> {
        best_index(&self.visible_rows())
    }

    /// The expanded batch infohash.
    #[must_use]
    pub fn expanded(&self) -> Option<&str> {
        self.expanded.as_deref()
    }

    /// The batch file-list state for `infohash`.
    #[must_use]
    pub fn batch_state(&self, infohash: &str) -> &BatchState {
        self.batches.get(infohash).unwrap_or(&BatchState::Loading)
    }

    /// Row-level amber message, if any.
    #[must_use]
    pub fn amber(&self, infohash: &str) -> Option<&str> {
        self.amber.get(infohash).map(String::as_str)
    }

    /// Session `no peers` set (lowercase infohashes).
    #[must_use]
    pub fn no_peers(&self) -> &BTreeSet<String> {
        &self.no_peers
    }

    /// Marks a release `no peers` for the rest of the app session (#182 Q9)
    /// and re-ranks.
    pub fn mark_no_peers(&mut self, infohash: &str, cx: &mut Context<'_, Self>) {
        let key = infohash.to_ascii_lowercase();
        self.no_peers.insert(key);
        self.rerank_current(cx);
    }

    /// Clicking a batch row: expand inline, fetching its file list on
    /// demand; clicking the expanded batch collapses. One at a time.
    pub fn toggle_expanded(&mut self, infohash: &str, cx: &mut Context<'_, Self>) {
        if self.expanded.as_deref() == Some(infohash) {
            self.expanded = None;
            cx.notify();
            return;
        }
        self.expanded = Some(infohash.to_owned());
        cx.notify();
        // Fetch the file list on demand if not already loaded.
        if matches!(self.batches.get(infohash), Some(BatchState::Loaded(_))) {
            return;
        }
        let Some(torrent) = self
            .rows
            .iter()
            .find(|row| row.infohash == infohash)
            .map(|r| r.torrent.clone())
        else {
            return;
        };
        let target = self.context.as_ref().map(|c| c.target);
        let services = Arc::clone(&self.services);
        let infohash = infohash.to_owned();
        self.batches.insert(infohash.clone(), BatchState::Loading);
        // AnimeTosho file list when present, else the indexer's torrent file.
        let pre = indexer_files(&torrent);
        self.batch_task = Some(cx.spawn(async move |store, cx| {
            let rows = if pre.is_empty() {
                cx.update(|app| crate::bridge::run(app, services.torrent_files(torrent)))
                    .await
                    .map_err(|error| error.to_string())
                    .and_then(|inner| inner.map_err(|error| error.to_string()))
            } else {
                Ok(pre)
            };
            let _ = store.update(cx, |store, cx| {
                let key = infohash.clone();
                match rows {
                    Ok(rows) => {
                        let files = match target {
                            Some(target) => batch_files(rows, &target),
                            None => Vec::new(),
                        };
                        store.batches.insert(key, BatchState::Loaded(files));
                    }
                    Err(_) => {
                        store.batches.insert(key, BatchState::Failed);
                    }
                }
                cx.notify();
            });
        }));
    }

    /// Clicking a single release: file check then stream. A missing
    /// episode-N file turns the row amber and keeps the sheet open.
    pub fn stream_single(&mut self, infohash: &str, cx: &mut Context<'_, Self>) {
        let Some(row) = self
            .rows
            .iter()
            .find(|row| row.infohash == infohash)
            .cloned()
        else {
            return;
        };
        let Some(ctx) = self.context.clone() else {
            return;
        };
        let services = Arc::clone(&self.services);
        let key = row.infohash.clone();
        self.amber.remove(&key);
        cx.notify();
        self.stream_task = Some(cx.spawn(async move |store, cx| {
            let files = cx
                .update(|app| crate::bridge::run(app, services.torrent_files(row.torrent.clone())))
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default();
            let entries: Vec<(usize, &str)> =
                files.iter().map(|f| (f.index, f.path.as_str())).collect();
            let picked = search::pick_file(entries.iter().copied(), &ctx.target);
            match picked {
                Some(file_index) => {
                    let _ = store.update(cx, |store, cx| {
                        store.play(&ctx, &row.torrent, ctx.episode, file_index, cx);
                        cx.notify();
                    });
                }
                None => {
                    let _ = store.update(cx, |store, cx| {
                        store.amber.insert(key, amber_no_episode(ctx.episode));
                        cx.notify();
                    });
                }
            }
        }));
    }

    /// Playing a file inside an expanded batch; other episode files play as
    /// their own episode.
    pub fn stream_batch_file(
        &mut self,
        infohash: &str,
        file_index: usize,
        path: &str,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(ctx) = self.context.clone() else {
            return;
        };
        let Some(torrent) = self
            .rows
            .iter()
            .find(|row| row.infohash == infohash)
            .map(|r| r.torrent.clone())
        else {
            return;
        };
        let episode = file_episode(path, ctx.episode);
        self.play(&ctx, &torrent, episode, file_index, cx);
    }

    fn play(
        &mut self,
        ctx: &ReleaseListContext,
        torrent: &Torrent,
        episode: u32,
        file_index: usize,
        cx: &mut Context<'_, Self>,
    ) {
        let Ok(info_hash) = torrent.infohash.parse() else {
            return;
        };
        let trackers = torrent
            .magnet_url
            .as_deref()
            .map_or_else(Vec::new, search::trackers_from_magnet);
        let source = StreamSource::torrent_with_source(
            info_hash,
            file_index,
            torrent.magnet_url.clone(),
            trackers,
        );
        let request = engine::PlayRequest::new(ctx.media, episode, source).with_details(
            PlaybackDetails::new(ctx.media_title.clone(), None, ctx.poster.clone()),
        );
        self.playback.update(cx, |playback, playback_cx| {
            playback.play(request, playback_cx);
        });
    }

    fn rerank_current(&mut self, cx: &mut Context<'_, Self>) {
        if let Some(ctx) = self.context.clone() {
            self.rerank(ctx.target);
            cx.notify();
        }
    }

    fn rerank(&mut self, target: EpisodeTarget) {
        let profile = self.active_profile();
        self.rows = re_rank(&self.raw_results, &target, &profile, &self.no_peers);
    }
}

/// `No episode N file in this torrent`.
#[must_use]
pub fn amber_no_episode(episode: u32) -> String {
    format!("No episode {episode} file in this torrent")
}

#[cfg(test)]
mod tests {
    use super::*;
    use search::Indexer;

    fn torrent(name: &str, seeders: u32, size: u64, published: u64) -> Torrent {
        let mut t = Torrent::new(Indexer::Nyaa, "a".repeat(40), name);
        t.seeders = Some(seeders);
        t.size = Some(size);
        t.published = Some(published);
        t
    }

    fn target() -> EpisodeTarget {
        EpisodeTarget::new(1, 5)
    }

    fn profile() -> AutoSelectProfile {
        AutoSelectProfile::default()
    }

    fn build(items: Vec<Torrent>) -> Vec<ReleaseRow> {
        build_rows(items, &target(), &profile(), &BTreeSet::new())
    }

    #[test]
    fn rank_sort_puts_passing_first_and_failing_dimmed_below() {
        let rows = build(vec![
            torrent("[Grp] Show - 06 [1080p]", 99, 1_000, 1), // wrong episode
            torrent("[Grp] Show - 05 [1080p]", 10, 1_000, 1),
            torrent("[Grp] Show - 05 [720p]", 50, 1_000, 1),
        ]);
        let order = visible_rows(&rows, &ReleaseFilters::new(), ReleaseSort::default());
        assert!(
            order.first().is_some_and(|r| r.passes),
            "best passing first"
        );
        assert!(order.get(1).is_some_and(|r| r.passes));
        assert!(!order.last().is_some_and(|r| r.passes), "failing last");
        assert_eq!(
            order.first().map_or_else(String::new, |r| r.name.clone()),
            "[Grp] Show - 05 [1080p]".to_owned()
        );
        // Resolution outranks seeders.
        assert_eq!(order.first().and_then(|r| r.seeders), Some(10));
    }

    #[test]
    fn header_click_sorts_then_reverses() {
        let sort = ReleaseSort::default();
        let sort = sort.clicked(SortColumn::Seeders);
        assert_eq!(sort.column, SortColumn::Seeders);
        assert_eq!(sort.direction, SortDirection::Ascending);
        let sort = sort.clicked(SortColumn::Seeders);
        assert_eq!(sort.direction, SortDirection::Descending);
        // Rank click resets.
        let sort = sort.clicked(SortColumn::Rank);
        assert_eq!(sort.column, SortColumn::Rank);
        assert_eq!(sort.direction, SortDirection::Ascending);
    }

    #[test]
    fn seeders_sort_orders_within_passing_and_failing() {
        let rows = build(vec![
            torrent("[Grp] Show - 05 [1080p]", 10, 1, 1),
            torrent("[Grp] Show - 05 [1080p]", 90, 1, 1),
            torrent("[Grp] Show - 06 [1080p]", 5, 1, 1),
            torrent("[Grp] Show - 06 [1080p]", 500, 1, 1),
        ]);
        let sort = ReleaseSort {
            column: SortColumn::Seeders,
            direction: SortDirection::Descending,
        };
        let order = visible_rows(&rows, &ReleaseFilters::new(), sort);
        assert_eq!(order.first().and_then(|r| r.seeders), Some(90));
        assert_eq!(order.get(1).and_then(|r| r.seeders), Some(10));
        assert!(order.get(1).is_some_and(|r| r.passes));
        assert!(
            !order.get(2).is_some_and(|r| r.passes),
            "failing stays below passing"
        );
        assert_eq!(order.get(2).and_then(|r| r.seeders), Some(500));
    }

    #[test]
    fn kind_resolution_and_indexer_chips_filter_client_side() {
        let mut batch = torrent("[Grp] Show (01-12) [1080p]", 10, 1, 1);
        batch.is_batch = true;
        let rows = build(vec![
            torrent("[Grp] Show - 05 [1080p]", 10, 1, 1),
            torrent("[Grp] Show - 05 [720p]", 10, 1, 1),
            batch,
        ]);
        let mut filters = ReleaseFilters::new();
        filters.kind = KindFilter::Batch;
        let order = visible_rows(&rows, &filters, ReleaseSort::default());
        assert_eq!(order.len(), 1, "only batches");
        assert!(order.first().is_some_and(|r| r.batch));

        let mut filters = ReleaseFilters::new();
        filters.kind = KindFilter::Single;
        let order = visible_rows(&rows, &filters, ReleaseSort::default());
        assert!(order.iter().all(|r| !r.batch));

        let mut filters = ReleaseFilters::new();
        filters.resolutions.insert(1080);
        let order = visible_rows(&rows, &filters, ReleaseSort::default());
        assert!(order.iter().all(|r| r.resolution == Some(1080)));

        let mut filters = ReleaseFilters::new();
        filters.indexers.insert("AnimeTosho".to_owned());
        let order = visible_rows(&rows, &filters, ReleaseSort::default());
        assert!(order.is_empty(), "no AnimeTosho results");

        assert_eq!(available_resolutions(&rows), vec![720, 1080]);
        assert_eq!(available_indexers(&rows), vec!["Nyaa".to_owned()]);
    }

    #[test]
    fn show_filtered_out_keeps_failing_rows() {
        let rows = build(vec![
            torrent("[Grp] Show - 05 [1080p]", 10, 1, 1),
            torrent("[Grp] Show - 06 [1080p]", 10, 1, 1),
        ]);
        let mut filters = ReleaseFilters::new();
        filters.show_filtered_out = false;
        let order = visible_rows(&rows, &filters, ReleaseSort::default());
        assert_eq!(order.len(), 1, "failing hidden");
        filters.show_filtered_out = true;
        let order = visible_rows(&rows, &filters, ReleaseSort::default());
        assert_eq!(order.len(), 2, "failing shown dimmed");
    }

    #[test]
    fn no_peers_marks_the_dead_release() {
        let mut no_peers = BTreeSet::new();
        no_peers.insert("a".repeat(40));
        let rows = build_rows(
            vec![torrent("[Grp] Show - 05 [1080p]", 10, 1, 1)],
            &target(),
            &profile(),
            &no_peers,
        );
        assert_eq!(
            rows.first().and_then(|r| r.status.clone()).as_deref(),
            Some(NO_PEERS)
        );
        assert!(rows.first().is_some_and(|r| r.no_peers));
    }

    #[test]
    fn hard_filter_reasons_are_reported() {
        let mut profile = profile();
        profile.min_seeders = 100;
        let rows = build_rows(
            vec![torrent("[Grp] Show - 05 [1080p]", 1, 1, 1)],
            &target(),
            &profile,
            &BTreeSet::new(),
        );
        assert_eq!(
            rows.first().and_then(|r| r.status.clone()).as_deref(),
            Some("min seeders")
        );
        assert!(!rows.first().is_some_and(|r| r.passes));
    }

    #[test]
    fn excluded_term_and_size_reasons() {
        let mut profile = profile();
        profile.excluded_terms = vec!["Grp".to_owned()];
        let rows = build_rows(
            vec![torrent("[Grp] Show - 05 [1080p]", 10, 1, 1)],
            &target(),
            &profile,
            &BTreeSet::new(),
        );
        assert_eq!(
            rows.first().and_then(|r| r.status.clone()).as_deref(),
            Some("excluded term")
        );
    }

    #[test]
    fn batch_expansion_lists_files_with_requested_preselected_and_nc_dimmed() {
        let rows = vec![
            TorrentFileRow::new(0, "Show - 04.mkv", 100, false),
            TorrentFileRow::new(1, "Show - 05.mkv", 100, false),
            TorrentFileRow::new(2, "NCOP.mkv", 50, false),
            TorrentFileRow::new(3, "Show - 06.mkv", 100, false),
            TorrentFileRow::new(4, "sample.mkv", 10, false),
        ];
        let files = batch_files(rows, &target());
        assert_eq!(files.len(), 5, "all files listed");
        assert_eq!(files.first().and_then(|f| f.episode), Some(4));
        assert!(
            files.get(1).is_some_and(|f| f.is_requested_episode),
            "episode 5 preselected"
        );
        assert!(
            files.get(1).is_some_and(|f| f.preselected),
            "requested episode preselected"
        );
        assert!(files.get(1).is_some_and(|f| f.playable));
        assert!(
            !files.get(2).is_some_and(|f| f.playable),
            "NCOP dimmed, can't play"
        );
        assert_eq!(
            files.get(2).and_then(|f| f.episode),
            None,
            "NC has no episode"
        );
        assert!(
            !files.get(4).is_some_and(|f| f.playable),
            "sample dimmed, can't play"
        );
        // Other episode files are pickable and play as their own episode.
        assert!(files.get(3).is_some_and(|f| f.playable));
        assert_eq!(files.get(3).and_then(|f| f.episode), Some(6));
    }

    #[test]
    fn file_episode_falls_back_to_requested() {
        assert_eq!(file_episode("Show - 06.mkv", 5), 6);
        assert_eq!(file_episode("NCOP.mkv", 5), 5);
        assert_eq!(file_episode("Show - 05.mkv", 5), 5);
    }

    #[test]
    fn batch_range_label_for_batch_one_eight() {
        let rows = build(vec![torrent("[Grp] Show (01-08) [1080p]", 10, 1, 1)]);
        assert!(rows.first().is_some_and(|r| r.batch));
        assert_eq!(
            rows.first().and_then(|r| r.batch_range.clone()).as_deref(),
            Some("Batch 1–8")
        );
    }
}
