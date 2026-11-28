//! Incoming-torrent match review store (#223, #209, ADR-0009).
//!
//! One incoming torrent pipeline; nothing auto-commits. The store drives
//! validation → add-for-metadata → match review → Stream or Add to Torrent
//! List through the bridge. Rendering stays pure.

use std::sync::Arc;

use domain::{InfoHash, MediaKind, TorrentLink};
use engine::{EngineServices, IncomingSource, MatchCandidate, MatchReview};

pub use engine::IncomingFile;
use gpui_kit::{Context, Task};

use crate::entry_store::call;

/// Where an incoming torrent's match review stands.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum ReviewState {
    /// Not reviewing.
    #[default]
    Idle,
    /// A magnet is being added for metadata ("Finding files…"); Cancel drops.
    FindingFiles,
    /// A validation error shown inline; no session was created.
    Rejected(String),
    /// A duplicate infohash jumped to its existing row.
    Duplicate(InfoHash),
    /// Match review: ranked suggestions, nothing committed yet.
    Reviewing,
}

/// The match-review sheet's data.
#[derive(Debug, Clone)]
pub struct IncomingReview {
    /// The torrent in the session.
    pub info_hash: InfoHash,
    /// Display name, when known.
    pub name: Option<String>,
    /// The torrent's files once metadata is known.
    pub files: Vec<IncomingFile>,
    /// Parsed `release-parser` titles to match.
    pub titles: Vec<String>,
    /// Ranked suggestions (decisive + ambiguous); empty when failed.
    pub suggestions: Vec<MatchCandidate>,
    /// Whether nothing matched: manual media + episode pick.
    pub failed: bool,
    /// The picked media, if any.
    pub chosen: Option<MatchCandidate>,
    /// The episode number.
    pub episode: u32,
    /// Whether a duplicate infohash jumped here.
    pub duplicate: bool,
    /// Manually picked `file_index`, overriding the #181 auto-pick.
    pub chosen_file: Option<usize>,
    /// The media's season for file matching (#181 Q7); 1 when unknown.
    pub season: u32,
}

impl IncomingReview {
    /// The `file_index` for the chosen episode (#181 reused mapping), when
    /// a video file holds it.
    #[must_use]
    pub fn file_index(&self, season: u32) -> Option<usize> {
        engine::pick_episode_file(&self.files, season, self.episode)
    }

    /// The `file_index` for the chosen episode at this review's season.
    #[must_use]
    pub fn picked_file_index(&self) -> Option<usize> {
        self.chosen_file.or_else(|| self.file_index(self.season))
    }
}

/// Drives the incoming-torrent pipeline for screens.
pub struct IncomingStore {
    services: Option<Arc<dyn EngineServices>>,
    /// Review state.
    pub state: ReviewState,
    /// The active review, when [`ReviewState::Reviewing`].
    pub review: Option<IncomingReview>,
    /// Infohash being added for metadata during "Finding files…".
    finding_hash: Option<InfoHash>,
    task: Option<Task<()>>,
}

impl std::fmt::Debug for IncomingStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IncomingStore")
            .field("state", &self.state)
            .field("reviewing", &self.review.is_some())
            .finish_non_exhaustive()
    }
}

impl IncomingStore {
    /// A store calling engine services through the bridge.
    #[must_use]
    pub fn new(services: Arc<dyn EngineServices>) -> Self {
        Self {
            services: Some(services),
            state: ReviewState::Idle,
            review: None,
            finding_hash: None,
            task: None,
        }
    }

    /// A store with no engine; ignores actions.
    #[must_use]
    pub fn inert() -> Self {
        Self {
            services: None,
            state: ReviewState::Idle,
            review: None,
            finding_hash: None,
            task: None,
        }
    }

    /// Closes the review; cancels any incoming metadata fetch in progress
    /// (Cancel = drop + delete partials, #209 Q4).
    pub fn cancel(&mut self, cx: &mut Context<'_, Self>) {
        let was_finding = self.state == ReviewState::FindingFiles;
        let info_hash = self
            .finding_hash
            .or_else(|| self.review.as_ref().map(|r| r.info_hash));
        self.task = None;
        self.state = ReviewState::Idle;
        self.review = None;
        self.finding_hash = None;
        // Cancel during "Finding files…" drops the incoming torrent.
        if let (true, Some(info_hash)) = (was_finding, info_hash)
            && let Some(services) = self.services.clone()
        {
            self.task = Some(cx.spawn(async move |_, cx| {
                let _ = call(cx, services.incoming_cancel(info_hash)).await;
            }));
        }
        cx.notify();
    }

    /// Cancels an incoming torrent in "Finding files…" by infohash, then
    /// closes. Used when cancelling before review opens.
    pub fn cancel_incoming(&mut self, info_hash: InfoHash, cx: &mut Context<'_, Self>) {
        self.task = None;
        self.state = ReviewState::Idle;
        self.review = None;
        if let Some(services) = self.services.clone() {
            self.task = Some(cx.spawn(async move |_, cx| {
                let _ = call(cx, services.incoming_cancel(info_hash)).await;
            }));
        }
        cx.notify();
    }

    /// Opens match review on a torrent already in the session (#222
    /// Match & play): no add, no auto-commit.
    pub fn review_existing(
        &mut self,
        info_hash: InfoHash,
        name: Option<String>,
        files: Vec<IncomingFile>,
        cx: &mut Context<'_, Self>,
    ) {
        let Some(services) = self.services.clone() else {
            return;
        };
        self.state = ReviewState::Reviewing;
        self.begin_review(&services, info_hash, name, files, false, cx);
    }

    /// Manual media pick (#209 Q3 `failed`): active-platform + AniList
    /// candidates for `query`, through the bridge.
    pub fn search_media(&mut self, query: String, cx: &mut Context<'_, Self>) {
        let Some(services) = self.services.clone() else {
            return;
        };
        self.task = Some(cx.spawn(async move |store, cx| {
            let candidates = call(cx, services.incoming_search_media(query))
                .await
                .unwrap_or_default();
            let _ = store.update(cx, |store, cx| {
                if let Some(review) = store.review.as_mut() {
                    review.suggestions = candidates;
                    review.failed = review.suggestions.is_empty();
                }
                cx.notify();
            });
        }));
    }

    /// Picks a media for a failed/decisive review.
    pub fn choose(&mut self, candidate: MatchCandidate, cx: &mut Context<'_, Self>) {
        if let Some(review) = self.review.as_mut() {
            review.chosen = Some(candidate);
            review.failed = false;
        }
        cx.notify();
    }

    /// Sets the media season for file matching (#181 Q7).
    pub fn set_season(&mut self, season: u32, cx: &mut Context<'_, Self>) {
        if let Some(review) = self.review.as_mut() {
            review.season = season;
        }
        cx.notify();
    }

    /// Manually picks a `file_index` (fallback when no file matches the
    /// episode, #209 Q4); `None` restores the #181 auto-pick.
    pub fn set_file(&mut self, file_index: Option<usize>, cx: &mut Context<'_, Self>) {
        if let Some(review) = self.review.as_mut() {
            review.chosen_file = file_index;
        }
        cx.notify();
    }

    /// Sets the episode number.
    pub fn set_episode(&mut self, episode: u32, cx: &mut Context<'_, Self>) {
        if let Some(review) = self.review.as_mut() {
            review.episode = episode;
        }
        cx.notify();
    }

    /// Validates a pasted magnet (#209 Q6) then starts the pipeline;
    /// malformed links are rejected inline with no session.
    pub fn open_magnet(&mut self, text: &str, cx: &mut Context<'_, Self>) {
        match engine::parse_magnet(text) {
            Ok(link) => self.open(IncomingSource::Magnet(link), cx),
            Err(error) => {
                self.state = ReviewState::Rejected(error.to_string());
                self.review = None;
                cx.notify();
            }
        }
    }

    /// Validates `.torrent` bytes (#209 Q6) then starts the pipeline;
    /// unreadable or non-video torrents are rejected inline.
    pub fn open_torrent_file(&mut self, bytes: &[u8], cx: &mut Context<'_, Self>) {
        match engine::parse_torrent_file(bytes) {
            Ok(source) => self.open(IncomingSource::TorrentFile(source), cx),
            Err(error) => {
                self.state = ReviewState::Rejected(error.to_string());
                self.review = None;
                cx.notify();
            }
        }
    }

    /// Starts the incoming pipeline for a validated `source`.
    ///
    /// "Finding files…" while a magnet's metadata arrives; then match
    /// review. A duplicate infohash reports [`ReviewState::Duplicate`].
    pub fn open(&mut self, source: IncomingSource, cx: &mut Context<'_, Self>) {
        let Some(services) = self.services.clone() else {
            return;
        };
        self.state = ReviewState::FindingFiles;
        self.review = None;
        self.finding_hash = Some(source.info_hash());
        self.task = Some(cx.spawn(async move |store, cx| {
            let added = call(cx, services.incoming_add(source)).await;
            match added {
                Ok(added) => {
                    let _ = store.update(cx, |store, cx| {
                        store.begin_review(
                            &services,
                            added.info_hash,
                            added.name.clone(),
                            added.files.clone(),
                            added.duplicate,
                            cx,
                        );
                    });
                }
                Err(message) => {
                    let _ = store.update(cx, |store, cx| {
                        store.state = ReviewState::Rejected(message);
                        store.review = None;
                        store.finding_hash = None;
                        cx.notify();
                    });
                }
            }
        }));
        cx.notify();
    }

    /// Moves to match review once metadata is known; computes suggestions.
    #[expect(clippy::fn_params_excessive_bools, reason = "single duplicate flag")]
    fn begin_review(
        &mut self,
        services: &Arc<dyn EngineServices>,
        info_hash: InfoHash,
        name: Option<String>,
        files: Vec<IncomingFile>,
        duplicate: bool,
        cx: &mut Context<'_, Self>,
    ) {
        // Parse the torrent name + every file name (#209 Q2-Q3).
        let names = titles_and_names(name.as_deref(), &files);
        let titles = search::match_titles(&names);
        // Seed episode suggestion from the parsed release.
        let episode = search::episode_suggestion(&names).unwrap_or(1);
        let review = IncomingReview {
            info_hash,
            name,
            files,
            titles: titles.clone(),
            suggestions: Vec::new(),
            failed: false,
            chosen: None,
            episode,
            duplicate,
            chosen_file: None,
            season: 1,
        };
        self.state = if duplicate {
            ReviewState::Duplicate(info_hash)
        } else {
            ReviewState::Reviewing
        };
        self.finding_hash = None;
        self.review = Some(review);
        cx.notify();

        // Rank suggestions through `library::matcher`.
        let services = Arc::clone(services);
        self.task = Some(cx.spawn(async move |store, cx| {
            let review = call(cx, services.incoming_match(titles)).await;
            let _ = store.update(cx, |store, cx| {
                if let Some(review_state) = store.review.as_mut() {
                    match review {
                        Ok(MatchReview::Decisive(s)) => {
                            review_state.suggestions = vec![s.candidate.clone()];
                            review_state.chosen = Some(s.candidate);
                        }
                        Ok(MatchReview::Ambiguous(ranked)) => {
                            review_state.suggestions =
                                ranked.into_iter().map(|s| s.candidate).collect();
                            // Never auto-commit; preselect the top suggestion.
                            review_state.chosen = review_state.suggestions.first().cloned();
                        }
                        Ok(MatchReview::Failed) | Err(_) => {
                            review_state.failed = true;
                        }
                        // `MatchReview` is `#[non_exhaustive]`.
                        _ => {}
                    }
                }
                cx.notify();
            });
        }));
    }

    /// Mints (or finds) the media, commits the match, selects the file and
    /// (for Stream) reports the source. `add_only` = Add to Torrent List.
    ///
    /// Returns `(media, episode, info_hash, file_index)` for Stream on
    /// success; `None` when no file holds the episode.
    #[expect(clippy::fn_params_excessive_bools, reason = "single add-only flag")]
    pub fn confirm(
        &mut self,
        add_only: bool,
        anilist_id: Option<u32>,
        cx: &mut Context<'_, Self>,
        on_ready: impl FnOnce(domain::MediaId, u32, InfoHash, usize, &mut Context<'_, Self>)
        + Send
        + 'static,
    ) {
        let Some(services) = self.services.clone() else {
            return;
        };
        let Some(review) = self.review.clone() else {
            return;
        };
        let Some(chosen) = review.chosen.clone() else {
            return;
        };
        let episode = review.episode;
        let info_hash = review.info_hash;
        let Some(file_index) = review.picked_file_index() else {
            // No video file holds the episode: fall back to manual file
            // pick; never invent a match.
            self.state =
                ReviewState::Rejected("no video file holds that episode; pick a file".to_owned());
            cx.notify();
            return;
        };
        let _ = anilist_id;
        let kind = MediaKind::Anime;
        let ids = chosen.external_ids.clone();
        let preset_media = chosen.media;
        self.task = Some(cx.spawn(async move |store, cx| {
            // Media identity: reuse the active-platform media, else mint on
            // this first user fact (ADR-0001); never auto-commit a match.
            let media = match preset_media {
                Some(media) => Ok(media),
                None => call(cx, services.mint(kind, &ids)).await,
            };
            let Ok(media) = media else {
                let _ = store.update(cx, |store, cx| {
                    store.state = ReviewState::Rejected("could not resolve media".to_owned());
                    cx.notify();
                });
                return;
            };
            let link = TorrentLink::new(media, episode, anilist_of(&ids));
            let _ = call(cx, services.incoming_commit(info_hash, Some(link))).await;
            let _ = call(cx, services.incoming_select_file(info_hash, file_index)).await;
            let _ = store.update(cx, |store, cx| {
                store.state = ReviewState::Idle;
                store.review = None;
                cx.notify();
                if !add_only {
                    on_ready(media, episode, info_hash, file_index, cx);
                }
            });
        }));
    }
}

fn anilist_of(ids: &[domain::ExternalId]) -> Option<u32> {
    ids.iter().find_map(|id| match id {
        domain::ExternalId::AniList(id) => Some(*id),
        _ => None,
    })
}

fn titles_and_names(name: Option<&str>, files: &[IncomingFile]) -> Vec<String> {
    let mut names = Vec::new();
    if let Some(name) = name {
        names.push(name.to_owned());
    }
    names.extend(files.iter().map(|f| {
        f.path
            .rsplit(['/', '\\'])
            .next()
            .unwrap_or(&f.path)
            .to_owned()
    }));
    names
}
