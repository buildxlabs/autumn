//! The Entry page's Torrents tab (#220, #182 Q1–Q3, Q11, #181).
//!
//! One row per regular episode. **Stream** runs auto-select through
//! [`EngineServices::auto_select`] (`Searching › Verifying files n/3 ›
//! Starting stream`, with Cancel) and plays the verified torrent file through
//! [`PlaybackStore::play`]. **Choose…** opens the release list once #221
//! lands; for now it is a no-op. Unaired episodes are dimmed with their air
//! time; `in library` and `✓ watched` pills come from the library and list
//! progress. The per-media **Profile ▾** (#182 Q11) picks the effective
//! auto-select profile and writes `media_profiles`. Recently aired rows also
//! carry an availability badge (#226): a profile-independent indexer check,
//! retried every five minutes while `waiting`.

use std::{collections::BTreeMap, sync::Arc, time::SystemTime};

use domain::{ExternalId, MediaId, MediaKind, PlaybackDetails, StreamSource};
use engine::{AutoSelectProgress, AutoSelectRequest, EngineServices, PlayRequest};
use gpui_kit::{Context, Entity, Task};
use metadata::EntryKind;
use search::{
    Availability, AvailabilityTracker, EpisodeTarget, Indexer, SystemWallClock, parse_air_date,
};
use settings::{AutoSelectProfile, AutoSelectSettings};

use crate::{bridge, entry_store::call, playback_store::PlaybackStore};

/// One regular episode row on the Torrents tab.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct TorrentEpisode {
    /// The episode number within this media.
    pub number: u32,
    /// The `AniZip` title, else `Episode N`.
    pub title: String,
    /// The air date (`YYYY-MM-DD`, possibly with a time), when known.
    pub air_date: Option<String>,
    /// Whether it has aired.
    pub aired: bool,
    /// Whether a local file in the library covers it (`in library`).
    pub in_library: bool,
    /// Whether list progress covers it (`✓ watched`).
    pub watched: bool,
}

/// The inline stage shown while a row is running (#182 Q2, #220).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(clippy::exhaustive_enums, reason = "the row renders every stage")]
pub enum TorrentStage {
    /// `Searching`.
    Searching,
    /// `Verifying files n/3`: `done` of `total` candidates checked.
    VerifyingFiles {
        /// Candidates checked so far, 1-based current.
        done: u32,
        /// Total candidates being verified, up to 3.
        total: u32,
    },
    /// `Starting stream`.
    StartingStream,
}

impl TorrentStage {
    /// The inline label, `Verifying files n/3` for `VerifyingFiles`.
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Searching => "Searching".to_owned(),
            Self::VerifyingFiles { done, total } => format!("Verifying files {done}/{total}"),
            Self::StartingStream => "Starting stream".to_owned(),
        }
    }
}

/// A row's runtime state (#220). Row state transitions are tested here.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[expect(clippy::exhaustive_enums, reason = "the row renders every state")]
pub enum TorrentRowState {
    /// Not running.
    #[default]
    Idle,
    /// Auto-select is running.
    Running {
        /// The current inline stage.
        stage: TorrentStage,
    },
    /// The run failed; `reason` is shown. Choose… is offered.
    Failed {
        /// Why it failed.
        reason: String,
    },
}

/// The `in library` pill target and label.
pub const IN_LIBRARY: &str = "in library";
/// The `✓ watched` pill label.
pub const WATCHED: &str = "✓ watched";

/// The Torrents tab's store.
pub struct EntryTorrentStore {
    services: Arc<dyn EngineServices>,
    playback: Entity<PlaybackStore>,
    anilist_id: Option<u32>,
    media: Option<MediaId>,
    episodes: Vec<TorrentEpisode>,
    states: BTreeMap<u32, TorrentRowState>,
    tasks: BTreeMap<u32, Task<()>>,
    /// Availability state and in-flight budget for recently aired episodes.
    tracker: AvailabilityTracker<SystemWallClock>,
    /// One cancellable check-and-recheck loop per aired episode.
    availability_tasks: BTreeMap<u32, Task<()>>,
    /// Known titles, romaji first, for the indexer query (#181 Q1).
    titles: Vec<String>,
    /// The media's title, for playback labels.
    media_title: String,
    /// The media's poster URL.
    poster: Option<String>,
    /// `AniZip` episode data.
    ep_meta: Option<metadata::Media>,
    /// This media's season `M`.
    season: u32,
    /// `AniDB` anime id, when known.
    anidb_aid: Option<u32>,
    /// The auto-select profiles, default and per-media overrides.
    auto_select: AutoSelectSettings,
    /// Whether torrent streaming is enabled (always in Phase 3).
    torrents_enabled: bool,
    /// A failed auto-select to auto-open the release list for (#182 Q2).
    pending_open: Option<(u32, String)>,
    load_task: Option<Task<()>>,
    /// Whether this media is finished releasing (#227).
    finished: bool,
    /// Total regular episodes (#227).
    total_episodes: u32,
    /// Whether the Torrents tab is currently visible (#227 Q4).
    torrents_tab_visible: bool,
    /// Batch suggestion strip state (#227, new module `batch_suggestions`).
    batch: crate::batch_suggestions::BatchSuggestions,
    /// The one batch-suggestion search task (#227).
    batch_task: Option<Task<()>>,
    /// The on-demand batch file-list task (#227).
    batch_file_task: Option<Task<()>>,
}

impl std::fmt::Debug for EntryTorrentStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EntryTorrentStore")
            .field("anilist_id", &self.anilist_id)
            .field("media", &self.media)
            .field("episodes", &self.episodes.len())
            .finish_non_exhaustive()
    }
}

impl EntryTorrentStore {
    /// A store over `services`, playing through `playback`.
    #[must_use]
    pub fn new(services: Arc<dyn EngineServices>, playback: Entity<PlaybackStore>) -> Self {
        Self {
            services,
            playback,
            anilist_id: None,
            media: None,
            episodes: Vec::new(),
            states: BTreeMap::new(),
            tasks: BTreeMap::new(),
            tracker: AvailabilityTracker::new(),
            availability_tasks: BTreeMap::new(),
            titles: Vec::new(),
            media_title: String::new(),
            poster: None,
            ep_meta: None,
            season: 1,
            anidb_aid: None,
            auto_select: AutoSelectSettings::default(),
            torrents_enabled: true,
            pending_open: None,
            load_task: None,
            finished: false,
            total_episodes: 0,
            torrents_tab_visible: false,
            batch: crate::batch_suggestions::BatchSuggestions::default(),
            batch_task: None,
            batch_file_task: None,
        }
    }

    /// Opens AniList `anilist_id`. `today` is `YYYY-MM-DD`. Reopening the
    /// same media reloads.
    pub fn open(&mut self, anilist_id: u32, today: &str, cx: &mut Context<'_, Self>) {
        self.anilist_id = Some(anilist_id);
        self.episodes.clear();
        self.states.clear();
        self.tasks.clear();
        self.tracker = AvailabilityTracker::new();
        self.availability_tasks.clear();
        self.finished = false;
        self.total_episodes = 0;
        self.torrents_tab_visible = false;
        self.batch = crate::batch_suggestions::BatchSuggestions::default();
        self.batch_task = None;
        self.batch_file_task = None;
        let services = Arc::clone(&self.services);
        let today = today.to_owned();
        self.load_task = Some(cx.spawn(async move |store, cx| {
            let ids = [ExternalId::AniList(anilist_id)];
            let media = call(cx, services.local_media(MediaKind::Anime, &ids))
                .await
                .ok()
                .flatten();
            let availability = match media {
                Some(media) => call(cx, services.availability(media.get()))
                    .await
                    .unwrap_or_default(),
                None => Vec::new(),
            };
            let detail = call(cx, services.detail(anilist_id))
                .await
                .ok()
                .map(|load| load.cached.value);
            let ep_meta = call(cx, services.episode_metadata(anilist_id))
                .await
                .ok()
                .flatten();
            let progress = match media {
                Some(media) => call(cx, services.local_entry(media))
                    .await
                    .ok()
                    .flatten()
                    .map_or(0, |entry| entry.progress),
                None => 0,
            };
            let files = match media {
                Some(media) => call(cx, services.library_files(media))
                    .await
                    .unwrap_or_default(),
                None => Vec::new(),
            };
            let auto_select = call(cx, services.auto_select_settings())
                .await
                .unwrap_or_default();

            let summary = detail.as_ref().map(|d| &d.summary);
            let titles = collect_titles(summary, ep_meta.as_ref());
            let media_title = summary.map_or_else(String::new, display_title);
            let poster = summary.and_then(|s| s.poster.clone());
            let anidb_aid = ep_meta
                .as_ref()
                .and_then(|m| m.mappings.as_ref())
                .and_then(|m| m.anidb_id);
            let season = search::episode_season(&titles, ep_meta.as_ref().and_then(media_season));
            let total = summary
                .and_then(|s| s.episodes)
                .or_else(|| ep_meta.as_ref().and_then(|m| m.episode_count))
                .unwrap_or(0);
            let finished =
                summary.and_then(|s| s.status.clone()) == Some(metadata::MediaStatus::Finished);
            let owned = owned_episodes(&files);
            let episodes = build_episodes(ep_meta.as_ref(), total, &owned, progress, &today);

            let _ = store.update(cx, |store, cx| {
                if store.anilist_id != Some(anilist_id) {
                    return;
                }
                store.media = media;
                store.total_episodes = u32::try_from(episodes.len()).unwrap_or(u32::MAX);
                store.episodes = episodes;
                store.titles = titles;
                store.media_title = media_title;
                store.poster = poster;
                store.ep_meta = ep_meta;
                store.season = season;
                store.anidb_aid = anidb_aid;
                store.auto_select = auto_select;
                store.restore_availability(&availability);
                store.finished = finished;
                store.start_availability(cx);
                if store.torrents_tab_visible {
                    store.start_batch_suggestions(cx);
                }
                cx.notify();
            });
        }));
    }

    /// Whether torrent streaming is enabled (drives tab visibility, #182 Q1).
    #[must_use]
    pub fn torrents_enabled(&self) -> bool {
        self.torrents_enabled
    }

    /// The episode rows, in number order.
    #[must_use]
    pub fn episodes(&self) -> &[TorrentEpisode] {
        &self.episodes
    }

    /// A row's runtime state.
    #[must_use]
    pub fn state(&self, episode: u32) -> &TorrentRowState {
        self.states.get(&episode).unwrap_or(&TorrentRowState::Idle)
    }

    /// The episode's availability badge (#181 Q12). A plain value the release
    /// list can reuse when it lands (#221).
    #[must_use]
    pub fn availability(&self, episode: u32) -> Availability {
        self.media.map_or(Availability::None, |media| {
            self.tracker.state(media.get(), episode)
        })
    }

    /// The next unwatched aired episode with no library file (#182 Q2), for
    /// the header's primary **Stream episode N**.
    #[must_use]
    pub fn next_streamable(&self) -> Option<&TorrentEpisode> {
        self.episodes
            .iter()
            .find(|episode| episode.aired && !episode.watched && !episode.in_library)
    }

    /// The effective auto-select profile for this media (#182 Q11).
    #[must_use]
    pub fn effective_profile(&self) -> AutoSelectProfile {
        self.auto_select.effective(self.media.map(MediaId::get))
    }

    /// Every saved profile, for **Profile ▾**.
    #[must_use]
    pub fn profiles(&self) -> &[AutoSelectProfile] {
        &self.auto_select.profiles
    }

    /// The name of the global default profile.
    #[must_use]
    pub fn default_profile_name(&self) -> &str {
        &self.auto_select.default_profile
    }

    /// The per-media override name, when set.
    #[must_use]
    pub fn media_profile(&self) -> Option<&str> {
        self.media
            .and_then(|media| self.auto_select.media_profiles.get(&media.get()))
            .map(String::as_str)
    }

    /// Sets or clears this media's profile override (#182 Q11). `profile`
    /// names a saved profile; `None` resets to the global default.
    pub fn set_media_profile(&mut self, profile: Option<String>, cx: &mut Context<'_, Self>) {
        let Some(media) = self.media else { return };
        let name = profile.filter(|name| !name.is_empty());
        // Optimistic local update.
        match &name {
            Some(name) => {
                self.auto_select
                    .media_profiles
                    .insert(media.get(), name.clone());
            }
            None => {
                self.auto_select.media_profiles.remove(&media.get());
            }
        }
        cx.notify();
        self.re_rank_batch_suggestions();
        let services = Arc::clone(&self.services);
        let key = media.get();
        cx.spawn(async move |_, cx| {
            let _ = cx
                .update(|app| bridge::run(app, services.set_media_profile(key, name)))
                .await;
        })
        .detach();
    }

    /// Starts auto-select for `episode` then streams (#220). Cancelling
    /// ([`Self::cancel`]) drops the run.
    pub fn stream(&mut self, episode: u32, cx: &mut Context<'_, Self>) {
        let Some(media) = self.media else { return };
        let Some(anilist_id) = self.anilist_id else {
            return;
        };
        // A fresh attempt clears a prior failure.
        self.tasks.remove(&episode);
        self.set_state(
            episode,
            TorrentRowState::Running {
                stage: TorrentStage::Searching,
            },
            cx,
        );

        let request = self.request(anilist_id, episode);
        let profile = self.effective_profile();
        let services = Arc::clone(&self.services);
        let playback = self.playback.clone();
        let title = self.media_title.clone();
        let poster = self.poster.clone();

        let (progress_tx, progress_rx) = flume::unbounded::<AutoSelectProgress>();

        // Pump progress events back onto this entity.
        let pump_store = cx.entity().downgrade();
        let pump = cx.spawn(async move |_, cx| {
            while let Ok(progress) = progress_rx.recv_async().await {
                let _ = pump_store.update(cx, |store, cx| {
                    store.apply_progress(episode, &progress, cx);
                });
            }
        });

        let task = cx.spawn(async move |store, cx| {
            let run = services.auto_select(
                request,
                profile,
                Box::new(move |progress| {
                    let _ = progress_tx.send(progress);
                }),
            );
            let result = match cx.update(|app| bridge::run(app, run)).await {
                Ok(result) => result,
                Err(error) => {
                    fail(&store, cx, episode, error.to_string());
                    drop(pump);
                    return;
                }
            };
            drop(pump);
            match result {
                Ok(selection) => {
                    let source: StreamSource = selection.source;
                    let request = PlayRequest::new(media, episode, source)
                        .with_details(PlaybackDetails::new(title, None, poster));
                    let _ = store.update(cx, |store, cx| {
                        store.set_state(
                            episode,
                            TorrentRowState::Running {
                                stage: TorrentStage::StartingStream,
                            },
                            cx,
                        );
                        store.tasks.remove(&episode);
                        cx.notify();
                    });
                    cx.update(|app| {
                        playback.update(app, |playback, playback_cx| {
                            playback.play(request, playback_cx);
                        });
                    });
                }
                Err(error) => {
                    fail(&store, cx, episode, error.to_string());
                }
            }
        });
        self.tasks.insert(episode, task);
    }

    /// Cancels a running row, resetting it to `Idle` (#220). If it was
    /// starting a stream, stops it.
    pub fn cancel(&mut self, episode: u32, cx: &mut Context<'_, Self>) {
        let starting = matches!(
            self.state(episode),
            TorrentRowState::Running {
                stage: TorrentStage::StartingStream
            }
        );
        // Dropping the task aborts the run.
        self.tasks.remove(&episode);
        self.set_state(episode, TorrentRowState::Idle, cx);
        if starting {
            self.playback.update(cx, PlaybackStore::stop);
        }
    }

    /// Resets a `Starting stream` row to `Idle`.
    pub fn reset(&mut self, episode: u32, cx: &mut Context<'_, Self>) {
        self.tasks.remove(&episode);
        self.set_state(episode, TorrentRowState::Idle, cx);
    }

    /// Clears every `Starting stream` row (a playback stop, #182 Q8).
    pub fn reset_started(&mut self, cx: &mut Context<'_, Self>) {
        let started: Vec<u32> = self
            .states
            .iter()
            .filter(|(_, state)| {
                matches!(
                    state,
                    TorrentRowState::Running {
                        stage: TorrentStage::StartingStream
                    }
                )
            })
            .map(|(number, _)| *number)
            .collect();
        let changed = !started.is_empty();
        for number in started {
            self.states.insert(number, TorrentRowState::Idle);
        }
        if changed {
            cx.notify();
        }
    }

    fn apply_progress(
        &mut self,
        episode: u32,
        progress: &AutoSelectProgress,
        cx: &mut Context<'_, Self>,
    ) {
        // Only a running row tracks progress.
        if !matches!(self.state(episode), TorrentRowState::Running { .. }) {
            return;
        }
        let stage = match progress {
            AutoSelectProgress::Searching => TorrentStage::Searching,
            AutoSelectProgress::Verifying { checking, total } => TorrentStage::VerifyingFiles {
                done: *checking,
                total: *total,
            },
            _ => return,
        };
        self.set_state(episode, TorrentRowState::Running { stage }, cx);
    }

    fn set_state(&mut self, episode: u32, state: TorrentRowState, cx: &mut Context<'_, Self>) {
        self.states.insert(episode, state);
        cx.notify();
    }

    /// Builds the release-list context for `episode` (#221), with the
    /// auto-select failure reason when opening after a failed run.
    #[must_use]
    pub fn release_list_context(
        &self,
        episode: u32,
        failure_reason: Option<String>,
    ) -> Option<crate::release_list::ReleaseListContext> {
        let media = self.media?;
        let anilist_id = self.anilist_id?;
        let auto = self.request(anilist_id, episode);
        let search_request = auto.search;
        let mut target = search::EpisodeTarget::new(search_request.season, episode);
        if let Some(absolute) = search_request.absolute_episode {
            target = target.with_absolute_episode(absolute);
        }
        let prefill_query = prefill_query(&search_request);
        let episode_title = self.ep_meta.as_ref().and_then(|meta| {
            meta.entries(EntryKind::Regular)
                .find(|entry| entry.number() == Some(episode))
                .and_then(|entry| {
                    ["en", "x-jat"]
                        .iter()
                        .find_map(|lang| entry.titles.get(*lang))
                        .or_else(|| entry.titles.values().next())
                        .cloned()
                })
        });
        let absolute_episode = search_request.absolute_episode;
        let season = search_request.season;
        Some(crate::release_list::ReleaseListContext {
            media,
            anilist_id,
            episode,
            episode_title,
            season,
            absolute_episode,
            search: search_request,
            target,
            availability: self.availability(episode),
            media_title: self.media_title.clone(),
            poster: self.poster.clone(),
            profile: self.effective_profile(),
            profiles: self.auto_select.profiles.clone(),
            prefill_query,
            failure_reason,
        })
    }

    fn search_request(&self, episode: u32) -> search::SearchRequest {
        let mut request = search::SearchRequest::new(self.titles.clone(), self.season, episode);
        if let Some(anidb_aid) = self.anidb_aid {
            request = request.with_anidb_ids(anidb_aid, None);
        }
        if let Some(meta) = &self.ep_meta {
            let entry = meta
                .entries(EntryKind::Regular)
                .find(|entry| entry.number() == Some(episode));
            if let Some(entry) = entry {
                if let Some(absolute) = entry.absolute_number
                    && absolute != episode
                {
                    request = request.with_absolute_episode(absolute);
                }
                if let Some(eid) = entry.anidb_eid
                    && request.anidb_eid.is_none()
                {
                    request.anidb_eid = Some(eid);
                }
            }
        }
        request
    }

    fn request(&self, anilist_id: u32, episode: u32) -> AutoSelectRequest {
        AutoSelectRequest::new(self.search_request(episode), Some(anilist_id))
    }

    /// Loads saved badge results into the tracker before checks start (#251).
    fn restore_availability(&mut self, records: &[db::AvailabilityRecord]) {
        let Some(media) = self.media else { return };
        let mut by_episode: BTreeMap<u32, Vec<(Indexer, bool, SystemTime)>> = BTreeMap::new();
        for record in records {
            if record.media == media.get() {
                by_episode.entry(record.episode).or_default().push((
                    search_indexer(record.indexer),
                    record.found,
                    record.checked_at,
                ));
            }
        }
        for (episode, checks) in by_episode {
            self.tracker.restore(media.get(), episode, checks);
        }
    }

    /// Whether the batch suggestion strip shows (#227, #182 Q4): finished
    /// media with more than one episode.
    #[must_use]
    pub fn should_show_batch_suggestions(&self) -> bool {
        crate::batch_suggestions::should_show(self.finished, self.total_episodes)
    }

    /// Top batch suggestions by the effective profile (#227).
    #[must_use]
    pub fn batch_suggestions(&self) -> &[crate::batch_suggestions::BatchSuggestion] {
        &self.batch.suggestions
    }

    /// The one batch search is in flight (#227).
    #[must_use]
    pub fn batch_suggestions_loading(&self) -> bool {
        self.batch.loading
    }

    /// The last batch search error (#227).
    #[must_use]
    pub fn batch_suggestions_error(&self) -> Option<&str> {
        self.batch.error.as_deref()
    }

    /// Expanded suggestion infohash (#227 **Episodes ▾**).
    #[must_use]
    pub fn batch_expanded(&self) -> Option<&str> {
        self.batch.expanded.as_deref()
    }

    /// A suggestion's on-demand file-list state (#227); reuses the release
    /// list's batch pick.
    #[must_use]
    pub fn batch_state(&self, infohash: &str) -> &crate::batch_suggestions::BatchState {
        self.batch
            .batches
            .get(infohash)
            .unwrap_or(&crate::batch_suggestions::BatchState::Loading)
    }

    /// Called when the Torrents tab is first shown (#227, #182 Q4): runs
    /// one batch search if not already started.
    pub fn open_torrents_tab(&mut self, cx: &mut Context<'_, Self>) {
        self.torrents_tab_visible = true;
        self.start_batch_suggestions(cx);
    }

    /// Runs one batch search on first visit (within #181's budget and 60 s
    /// cache); no-op unless finished media with more than one episode.
    fn start_batch_suggestions(&mut self, cx: &mut Context<'_, Self>) {
        if self.batch.started || !self.should_show_batch_suggestions() {
            return;
        }
        self.batch.started = true;
        self.batch.loading = true;
        self.batch.error = None;
        let request = self.batch_search_request();
        let services = Arc::clone(&self.services);
        let task = cx.spawn(async move |store, cx| {
            let run = services.search_torrents(request, search::LaneFilter::All);
            let result = match cx.update(|app| bridge::run(app, run)).await {
                Ok(result) => result,
                Err(error) => {
                    let _ = store.update(cx, |store, cx| {
                        store.batch.loading = false;
                        store.batch.error = Some(error.to_string());
                        cx.notify();
                    });
                    return;
                }
            };
            let _ = store.update(cx, |store, cx| {
                store.batch.loading = false;
                match result {
                    Ok(outcome) => {
                        store.batch.raw = outcome.torrents;
                        store.re_rank_batch_suggestions();
                    }
                    Err(error) => {
                        store.batch.error = Some(error.to_string());
                    }
                }
                cx.notify();
            });
        });
        self.batch_task = Some(task);
    }

    /// Recomputes top-3 suggestions from raw results with the effective
    /// profile (#227).
    fn re_rank_batch_suggestions(&mut self) {
        let profile = self.effective_profile();
        self.batch.suggestions = crate::batch_suggestions::batch_suggestions(
            self.batch.raw.clone(),
            self.season,
            self.total_episodes,
            &profile,
            crate::batch_suggestions::BATCH_SUGGESTION_COUNT,
        );
    }

    /// One batch search request: titles + season, no per-episode ids, so
    /// batches are not filtered to one episode (#227). `plan` still adds
    /// `romaji batch` queries within #181's budget.
    fn batch_search_request(&self) -> search::SearchRequest {
        let mut request = search::SearchRequest::new(self.titles.clone(), self.season, 1);
        if let Some(anidb_aid) = self.anidb_aid {
            request = request.with_anidb_ids(anidb_aid, None);
        }
        request
    }

    /// **Episodes ▾**: expand inline, fetching the file list on demand;
    /// reuses the release list's batch file pick (#227, #182 Q7).
    pub fn toggle_batch_episodes(&mut self, infohash: &str, cx: &mut Context<'_, Self>) {
        if self.batch.expanded.as_deref() == Some(infohash) {
            self.batch.expanded = None;
            cx.notify();
            return;
        }
        self.batch.expanded = Some(infohash.to_owned());
        cx.notify();
        if matches!(
            self.batch.batches.get(infohash),
            Some(crate::batch_suggestions::BatchState::Loaded(_))
        ) {
            return;
        }
        let Some(torrent) = self
            .batch
            .suggestions
            .iter()
            .find(|s| s.infohash == infohash)
            .map(|s| s.torrent.clone())
        else {
            return;
        };
        let services = Arc::clone(&self.services);
        let infohash = infohash.to_owned();
        let season = self.season;
        self.batch.batches.insert(
            infohash.clone(),
            crate::batch_suggestions::BatchState::Loading,
        );
        // AnimeTosho file list when present, else the indexer's torrent file.
        let pre = crate::batch_suggestions::indexer_files(&torrent);
        self.batch_file_task = Some(cx.spawn(async move |store, cx| {
            let rows = if pre.is_empty() {
                cx.update(|app| bridge::run(app, services.torrent_files(torrent)))
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
                        let files = crate::batch_suggestions::suggestion_files(rows, season);
                        store
                            .batch
                            .batches
                            .insert(key, crate::batch_suggestions::BatchState::Loaded(files));
                    }
                    Err(_) => {
                        store
                            .batch
                            .batches
                            .insert(key, crate::batch_suggestions::BatchState::Failed);
                    }
                }
                cx.notify();
            });
        }));
    }

    /// Picking a file in **Episodes ▾** streams it (#227); reuses the
    /// release list's batch file pick. Fills `StreamSource::Torrent`
    /// `magnet` + `trackers` (#250). Other episode files play as their own
    /// episode.
    pub fn stream_batch_suggestion_file(
        &mut self,
        hash: &str,
        file_index: usize,
        path: &str,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(media) = self.media else { return };
        let Some((info_hash, magnet_url)) = self
            .batch
            .suggestions
            .iter()
            .find(|s| s.infohash == hash)
            .and_then(|s| {
                s.torrent
                    .infohash
                    .parse::<domain::InfoHash>()
                    .ok()
                    .map(|hash| (hash, s.torrent.magnet_url.clone()))
            })
        else {
            return;
        };
        let trackers = magnet_url
            .as_deref()
            .map_or_else(Vec::new, search::trackers_from_magnet);
        let source = StreamSource::torrent_with_source(info_hash, file_index, magnet_url, trackers);
        let episode = crate::batch_suggestions::file_episode(path, 1);
        let request = PlayRequest::new(media, episode, source).with_details(PlaybackDetails::new(
            self.media_title.clone(),
            None,
            self.poster.clone(),
        ));
        self.playback.update(cx, |playback, playback_cx| {
            playback.play(request, playback_cx);
        });
    }

    /// Register the aired episodes and fill the check budget (#226).
    fn start_availability(&mut self, cx: &mut Context<'_, Self>) {
        self.availability_tasks.clear();
        let Some(media) = self.media else { return };
        let key = media.get();
        for episode in &self.episodes {
            if let Some(aired_at) = episode.air_date.as_deref().and_then(parse_air_date) {
                self.tracker.track(key, episode.number, aired_at);
            }
        }
        self.pump_availability(cx);
    }

    /// Grant queued checks up to the tracker's in-flight cap.
    fn pump_availability(&mut self, cx: &mut Context<'_, Self>) {
        let media = self.media.map(MediaId::get);
        for (_, episode) in self.tracker.pump() {
            self.spawn_availability_check(episode, media, cx);
        }
    }

    /// One check and its rechecks, cancelled by `open` when the media changes.
    fn spawn_availability_check(
        &mut self,
        episode: u32,
        media: Option<i64>,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(media) = media else { return };
        let request = self.search_request(episode);
        let mut target = EpisodeTarget::new(request.season, request.episode);
        if let Some(absolute) = request.absolute_episode {
            target = target.with_absolute_episode(absolute);
        }
        let services = Arc::clone(&self.services);
        let save_services = Arc::clone(&services);
        let weak = cx.entity().downgrade();
        let task = cx.spawn(async move |_, cx| {
            loop {
                let outcome = match cx
                    .update(|app| {
                        bridge::run(
                            app,
                            services.search_torrents(request.clone(), search::LaneFilter::BuiltIn),
                        )
                    })
                    .await
                {
                    Ok(Ok(outcome)) => Ok(outcome.torrents),
                    _ => Err(()),
                };
                let result = weak.update(cx, |store, cx| {
                    match outcome {
                        Ok(torrents) => {
                            let found = torrents
                                .iter()
                                .filter(|torrent| search::matches_torrent(torrent, &target))
                                .map(|torrent| torrent.indexer.clone())
                                .collect::<Vec<_>>();
                            store.tracker.complete(media, episode, &found);
                        }
                        Err(()) => store.tracker.fail(media, episode),
                    }
                    cx.notify();
                    store.pump_availability(cx);
                    let records: Vec<db::AvailabilityRecord> = store
                        .tracker
                        .cached(media, episode)
                        .into_iter()
                        .filter_map(|(indexer, found, checked_at)| {
                            record_indexer(&indexer).map(|indexer| db::AvailabilityRecord {
                                media,
                                episode,
                                indexer,
                                found,
                                checked_at,
                            })
                        })
                        .collect();
                    (store.tracker.recheck_delay(media, episode), records)
                });
                let Ok((delay, records)) = result else { return };
                if !records.is_empty() {
                    let _ = cx
                        .update(|app| bridge::run(app, save_services.save_availability(records)))
                        .await;
                }
                let Some(delay) = delay else { return };
                cx.background_executor().timer(delay).await;
                let granted = weak.update(cx, |store, cx| {
                    let granted = store.tracker.begin(media, episode);
                    store.pump_availability(cx);
                    granted
                });
                if !granted.unwrap_or(false) {
                    let _ = weak.update(cx, |store, cx| {
                        store.tracker.enqueue(media, episode);
                        store.pump_availability(cx);
                    });
                    return;
                }
            }
        });
        self.availability_tasks.insert(episode, task);
    }
}

fn fail(
    store: &gpui_kit::WeakEntity<EntryTorrentStore>,
    cx: &mut gpui_kit::AsyncApp,
    episode: u32,
    reason: String,
) {
    let _ = store.update(cx, |store, cx| {
        store.tasks.remove(&episode);
        store.pending_open = Some((episode, reason.clone()));
        store.set_state(episode, TorrentRowState::Failed { reason }, cx);
    });
}

/// The indexer stored in the availability table, as `search` names it.
#[must_use]
fn search_indexer(indexer: db::AvailabilityIndexer) -> Indexer {
    match indexer {
        db::AvailabilityIndexer::Nyaa => Indexer::Nyaa,
        db::AvailabilityIndexer::AnimeTosho => Indexer::AnimeTosho,
    }
}

/// The row code for an indexer the tracker has just checked, or `None` for an
/// indexer this build cannot store.
#[must_use]
fn record_indexer(indexer: &Indexer) -> Option<db::AvailabilityIndexer> {
    match indexer {
        Indexer::Nyaa => Some(db::AvailabilityIndexer::Nyaa),
        Indexer::AnimeTosho => Some(db::AvailabilityIndexer::AnimeTosho),
        _ => None,
    }
}

/// Romaji, English, native then `AniZip` titles, ordered + deduped (#181 Q1).
fn collect_titles(
    summary: Option<&metadata::MediaSummary>,
    ep_meta: Option<&metadata::Media>,
) -> Vec<String> {
    let mut titles: Vec<String> = Vec::new();
    let mut push = |title: &str| {
        let trimmed = title.trim();
        if !trimmed.is_empty() && !titles.iter().any(|t| t.eq_ignore_ascii_case(trimmed)) {
            titles.push(trimmed.to_owned());
        }
    };
    if let Some(summary) = summary {
        if let Some(romaji) = &summary.title.romaji {
            push(romaji);
        }
        if let Some(english) = &summary.title.english {
            push(english);
        }
        if let Some(native) = &summary.title.native {
            push(native);
        }
    }
    if let Some(meta) = ep_meta {
        for value in meta.titles.values() {
            push(value);
        }
    }
    titles
}

fn display_title(summary: &metadata::MediaSummary) -> String {
    summary
        .title
        .english
        .clone()
        .or_else(|| summary.title.romaji.clone())
        .or_else(|| summary.title.native.clone())
        .unwrap_or_else(|| "Untitled".to_owned())
}

/// The requested media's season `M`: `AniZip` `season_number`, else 1.
fn media_season(meta: &metadata::Media) -> Option<u32> {
    meta.entries(EntryKind::Regular)
        .filter(|entry| entry.season_number.is_some_and(|season| season > 0))
        .find_map(|entry| entry.season_number)
}

fn owned_episodes(files: &[db::LocalFile]) -> std::collections::BTreeSet<u32> {
    files
        .iter()
        .filter(|file| file.kind == "episode")
        .filter_map(|file| file.episode)
        .collect()
}

fn build_episodes(
    meta: Option<&metadata::Media>,
    total: u32,
    owned: &std::collections::BTreeSet<u32>,
    progress: u32,
    today: &str,
) -> Vec<TorrentEpisode> {
    let mut by_number: BTreeMap<u32, TorrentEpisode> = BTreeMap::new();
    // Unaired/finished media with no total still lists what `AniZip` knows.
    let mut count = total;
    if let Some(meta) = meta {
        for entry in meta.entries(EntryKind::Regular) {
            if let Some(number) = entry.number() {
                count = count.max(number);
                insert_episode(
                    &mut by_number,
                    number,
                    episode_title(entry),
                    entry.aired().map(str::to_owned),
                    owned,
                    progress,
                    today,
                );
            }
        }
    }
    for number in 1..=count {
        if !by_number.contains_key(&number) {
            insert_episode(&mut by_number, number, None, None, owned, progress, today);
        }
    }
    by_number.into_values().collect()
}

/// The English title, else romanized, else any (mirrors `entry_library`).
fn episode_title(entry: &metadata::Episode) -> Option<String> {
    ["en", "x-jat"]
        .iter()
        .find_map(|language| entry.titles.get(*language))
        .or_else(|| entry.titles.values().next())
        .filter(|title| !title.trim().is_empty())
        .cloned()
}

fn insert_episode(
    by_number: &mut BTreeMap<u32, TorrentEpisode>,
    number: u32,
    title: Option<String>,
    air_date: Option<String>,
    owned: &std::collections::BTreeSet<u32>,
    progress: u32,
    today: &str,
) {
    let aired = air_date
        .as_deref()
        .is_some_and(|date| !date.is_empty() && date.get(..10).unwrap_or(date) <= today);
    by_number.insert(
        number,
        TorrentEpisode {
            number,
            title: title.unwrap_or_else(|| format!("Episode {number}")),
            air_date,
            aired,
            in_library: owned.contains(&number),
            watched: progress >= number,
        },
    );
}

/// Takes a pending auto-select-failure auto-open (#182 Q2).
pub fn take_pending_open(store: &mut EntryTorrentStore) -> Option<(u32, String)> {
    store.pending_open.take()
}

/// `#181`'s first query keyword, for prefilling the editable query (#182 Q6).
fn prefill_query(request: &search::SearchRequest) -> String {
    let plan = search::plan(request);
    plan.numbered
        .first()
        .map(|query| match query {
            search::Query::Nyaa { keyword } => keyword.clone(),
            search::Query::AnimeTosho { keyword, .. } => keyword.clone().unwrap_or_default(),
            _ => String::new(),
        })
        .filter(|keyword| !keyword.is_empty())
        .or_else(|| request.titles.first().cloned())
        .unwrap_or_default()
}
#[cfg(test)]
mod tests {
    use std::time::{Duration, SystemTime};

    use gpui_kit::{AppContext, TestAppContext};
    use tokio::runtime::Builder;

    use crate::{bridge, playback_store::PlaybackStore, test_support::pending_services};

    use super::*;

    fn store(cx: &mut TestAppContext) -> gpui_kit::Entity<EntryTorrentStore> {
        let services = pending_services();
        let playback = cx.new(|_| PlaybackStore::new(pending_services()));
        cx.new(|_| EntryTorrentStore::new(services, playback))
    }

    fn episode(number: u32) -> TorrentEpisode {
        TorrentEpisode {
            number,
            title: format!("Episode {number}"),
            air_date: None,
            aired: true,
            in_library: false,
            watched: false,
        }
    }

    fn stage(
        store: &gpui_kit::Entity<EntryTorrentStore>,
        cx: &TestAppContext,
        ep: u32,
    ) -> TorrentRowState {
        store.read_with(cx, |store, _| store.state(ep).clone())
    }

    #[gpui_kit::test]
    fn row_state_transitions_searching_verifying_starting_then_idle(cx: &mut TestAppContext) {
        let store = store(cx);
        let ep = 1;
        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.set_state(
                    ep,
                    TorrentRowState::Running {
                        stage: TorrentStage::Searching,
                    },
                    cx,
                );
            });
        });
        assert_eq!(
            stage(&store, cx, ep),
            TorrentRowState::Running {
                stage: TorrentStage::Searching
            }
        );

        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.apply_progress(
                    ep,
                    &AutoSelectProgress::Verifying {
                        checking: 1,
                        total: 3,
                    },
                    cx,
                );
            });
        });
        assert_eq!(
            stage(&store, cx, ep),
            TorrentRowState::Running {
                stage: TorrentStage::VerifyingFiles { done: 1, total: 3 }
            }
        );

        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.apply_progress(
                    ep,
                    &AutoSelectProgress::Verifying {
                        checking: 2,
                        total: 3,
                    },
                    cx,
                );
            });
        });
        assert_eq!(
            stage(&store, cx, ep),
            TorrentRowState::Running {
                stage: TorrentStage::VerifyingFiles { done: 2, total: 3 }
            }
        );
        assert_eq!(
            TorrentStage::VerifyingFiles { done: 2, total: 3 }.label(),
            "Verifying files 2/3"
        );

        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.set_state(
                    ep,
                    TorrentRowState::Running {
                        stage: TorrentStage::StartingStream,
                    },
                    cx,
                );
            });
        });
        assert_eq!(
            stage(&store, cx, ep),
            TorrentRowState::Running {
                stage: TorrentStage::StartingStream
            }
        );
        assert_eq!(TorrentStage::StartingStream.label(), "Starting stream");

        // `Starting stream` clears.
        cx.update(|cx| store.update(cx, EntryTorrentStore::reset_started));
        assert_eq!(stage(&store, cx, ep), TorrentRowState::Idle);
    }

    #[gpui_kit::test]
    fn cancel_resets_a_running_row_to_idle(cx: &mut TestAppContext) {
        let store = store(cx);
        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.set_state(
                    1,
                    TorrentRowState::Running {
                        stage: TorrentStage::Searching,
                    },
                    cx,
                );
                store.cancel(1, cx);
            });
        });
        assert_eq!(stage(&store, cx, 1), TorrentRowState::Idle);
    }

    #[gpui_kit::test]
    fn a_failed_row_shows_the_reason(cx: &mut TestAppContext) {
        let store = store(cx);
        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.set_state(
                    3,
                    TorrentRowState::Failed {
                        reason: "no torrent matches".to_owned(),
                    },
                    cx,
                );
            });
        });
        assert_eq!(
            stage(&store, cx, 3),
            TorrentRowState::Failed {
                reason: "no torrent matches".to_owned()
            }
        );
        assert_eq!(TorrentStage::Searching.label(), "Searching");
    }

    #[gpui_kit::test]
    fn streaming_with_no_match_ends_in_a_failed_row(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .build()
            .unwrap();
        cx.update(|cx| bridge::init(cx, runtime.handle().clone()));
        let store = store(cx);
        cx.update(|cx| {
            store.update(cx, |store, _cx| {
                store.media = Some(MediaId::new(1));
                store.anilist_id = Some(100);
                store.titles = vec!["Frieren".to_owned()];
                store.episodes = vec![episode(1)];
            });
        });

        cx.update(|cx| store.update(cx, |store, cx| store.stream(1, cx)));
        // Immediately searching...
        assert_eq!(
            stage(&store, cx, 1),
            TorrentRowState::Running {
                stage: TorrentStage::Searching
            }
        );
        // The bridge completes on Tokio, then posts back to GPUI: a second
        // park lets the posted update land before the assertion.
        cx.run_until_parked();
        std::thread::sleep(Duration::from_millis(50));
        cx.run_until_parked();
        // ...then failed (no fake auto-select in `PendingServices`).
        assert!(
            matches!(stage(&store, cx, 1), TorrentRowState::Failed { .. }),
            "a no-match run ends Failed: {:?}",
            stage(&store, cx, 1)
        );
    }

    #[gpui_kit::test]
    fn availability_getter_exposes_the_tracker_state(cx: &mut TestAppContext) {
        let store = store(cx);
        cx.update(|cx| {
            store.update(cx, |store, _| {
                store.media = Some(MediaId::new(1));
                store
                    .tracker
                    .track(1, 1, SystemTime::now() - Duration::from_mins(1));
            });
        });
        let badge = |cx: &TestAppContext| store.read_with(cx, |store, _| store.availability(1));
        assert_eq!(badge(cx), Availability::Waiting);
        cx.update(|cx| {
            store.update(cx, |store, _| {
                assert!(store.tracker.begin(1, 1));
            });
        });
        assert_eq!(badge(cx), Availability::Checking);
    }

    #[gpui_kit::test]
    fn a_restored_check_becomes_the_release_list_pill_without_a_recheck(cx: &mut TestAppContext) {
        let store = store(cx);
        cx.update(|cx| {
            store.update(cx, |store, _| {
                store.media = Some(MediaId::new(7));
                store.anilist_id = Some(100);
                let now = SystemTime::now();
                store.tracker.track(7, 3, now - Duration::from_mins(2));
                store.restore_availability(&[db::AvailabilityRecord {
                    media: 7,
                    episode: 3,
                    indexer: db::AvailabilityIndexer::Nyaa,
                    found: true,
                    checked_at: now - Duration::from_mins(1),
                }]);
            });
        });
        assert_eq!(
            store.read_with(cx, |store, _| store.availability(3)),
            Availability::Available
        );
        cx.update(|cx| {
            store.update(cx, |store, _| {
                assert!(
                    store.tracker.pump().is_empty(),
                    "an available episode is not checked again after a restart"
                );
            });
        });

        let pill = |cx: &TestAppContext, episode| {
            store.read_with(cx, |store, _| {
                store
                    .release_list_context(episode, None)
                    .and_then(|context| context.availability.label().map(str::to_owned))
            })
        };
        assert_eq!(pill(cx, 3), Some("available".to_owned()));
        assert_eq!(pill(cx, 4), None, "unknown and none render nothing");
    }

    #[gpui_kit::test]
    fn next_streamable_is_the_next_unwatched_aired_episode_with_no_library_file(
        cx: &mut TestAppContext,
    ) {
        let store = store(cx);
        cx.update(|cx| {
            store.update(cx, |store, _cx| {
                store.episodes = vec![
                    TorrentEpisode {
                        watched: true,
                        ..episode(1)
                    },
                    TorrentEpisode {
                        in_library: true,
                        ..episode(2)
                    },
                    TorrentEpisode {
                        aired: false,
                        ..episode(3)
                    },
                    episode(4),
                ];
            });
        });
        let number = store.read_with(cx, |store, _| store.next_streamable().map(|e| e.number));
        assert_eq!(number, Some(4));
    }

    #[gpui_kit::test]
    fn setting_a_media_profile_writes_media_profiles(cx: &mut TestAppContext) {
        cx.executor().allow_parking();
        let runtime = Builder::new_multi_thread()
            .worker_threads(1)
            .build()
            .unwrap();
        cx.update(|cx| bridge::init(cx, runtime.handle().clone()));
        let store = store(cx);
        cx.update(|cx| {
            store.update(cx, |store, _cx| {
                store.media = Some(MediaId::new(7));
                store.auto_select = AutoSelectSettings::default();
            });
        });

        cx.update(|cx| {
            store.update(cx, |store, cx| {
                store.set_media_profile(Some("SubsPlease".to_owned()), cx);
            });
        });
        let chosen = store.read_with(cx, |store, _| store.media_profile().map(str::to_owned));
        assert_eq!(chosen, Some("SubsPlease".to_owned()));

        cx.update(|cx| store.update(cx, |store, cx| store.set_media_profile(None, cx)));
        assert_eq!(
            store.read_with(cx, |store, _| store.media_profile().map(str::to_owned)),
            None
        );
    }
}
