//! Next-episode preload (#224, #208).
//!
//! ```text
//! 50 % into a torrent stream of regular episode N, with auto-next and
//! preload on, N+1 aired and not in the library:
//!   resolve: N+1's file in the same batch torrent, else auto-select with
//!            the media's effective profile
//!   hold the torrent (origin preload) and select N+1's file
//!   readiness gate first, then the rest of the file
//!   yield: paused while the current stream buffers, resumed once it has
//!          gone 10 s without buffering
//! consumed: auto-next, or a play of (media, N+1)
//! kept for 10 min: the player closed, or auto-next was cancelled
//! dropped now: Stop, a play of anything else, the toggle turned off
//! ```
//!
//! At most one preload exists; a new one replaces the old. A drop releases
//! the preload's hold, so a torrent nothing else uses is paused, then
//! deleted when partial or left seeding when complete (#180).

use std::{
    future::Future,
    sync::{Arc, Mutex, RwLock},
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use domain::{InfoHash, MediaId, StreamSource, TorrentLink, TorrentOrigin};
use metadata::{Aging, AniZip};
use search::EpisodeTarget;
use settings::AutoSelectSettings;
use tokio::{sync::watch, task::JoinHandle};
use torrent::{TorrentSession, TorrentSource};
use tracing::{debug, info, warn};

use super::{PlayRequest, PlaybackManager, lock};
use crate::{AutoSelectRequest, AutoSelector, BoxFuture, StreamOrigin, TorrentStreams};

/// The preload starts at this fraction of the current episode (#208 Q3).
pub(super) const PRELOAD_AT: f64 = 0.5;
/// A yielded preload resumes once the current stream has gone this long
/// without buffering (#208 Q6).
pub(super) const RESUME_AFTER: Duration = Duration::from_secs(10);
/// How long a preload is kept after the player closes or auto-next is
/// cancelled (#208 Q7).
pub(super) const GRACE: Duration = Duration::from_mins(10);

/// Whether an episode has aired, from metadata.
pub trait EpisodeAiring: Send + Sync {
    /// Whether regular episode `episode` of AniList media `anilist_id` is
    /// listed and has aired.
    fn has_aired(&self, anilist_id: u32, episode: u32) -> BoxFuture<bool>;
}

/// Air dates from `AniZip`'s episode map.
#[derive(Debug, Clone)]
pub struct AniZipAiring {
    anizip: AniZip,
}

impl AniZipAiring {
    /// Reads air dates through `anizip`.
    #[must_use]
    pub const fn new(anizip: AniZip) -> Self {
        Self { anizip }
    }
}

impl EpisodeAiring for AniZipAiring {
    fn has_aired(&self, anilist_id: u32, episode: u32) -> BoxFuture<bool> {
        let anizip = self.anizip.clone();
        Box::pin(async move {
            let Ok(Some(media)) = anizip.media(anilist_id, Aging::Airing).await else {
                return false;
            };
            media
                .episodes
                .get(&episode.to_string())
                .is_some_and(|entry| entry.air_date.as_deref().is_none_or(aired_by_today))
        })
    }
}

/// Whether a `YYYY-MM-DD` air date is today or earlier. An unreadable date
/// counts as not aired.
fn aired_by_today(date: &str) -> bool {
    let mut parts = date.get(..10).unwrap_or(date).split('-');
    let (Some(year), Some(month), Some(day)) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    let (Ok(year), Ok(month), Ok(day)) = (year.parse(), month.parse(), day.parse()) else {
        return false;
    };
    let today = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_secs() / 86_400);
    days_from_civil(year, month, day) <= i64::try_from(today).unwrap_or(i64::MAX)
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(year: i64, month: i64, day: i64) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// What a preload downloads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Fetch {
    /// N+1 is another file of the batch torrent the current stream plays.
    SameBatch {
        info_hash: InfoHash,
        current: usize,
        next: usize,
    },
    /// N+1 is in a torrent of its own, found by auto-select.
    Torrent {
        info_hash: InfoHash,
        file_index: usize,
    },
}

impl Fetch {
    pub(super) const fn info_hash(self) -> InfoHash {
        match self {
            Self::SameBatch { info_hash, .. } | Self::Torrent { info_hash, .. } => info_hash,
        }
    }

    const fn file_index(self) -> usize {
        match self {
            Self::SameBatch { next, .. } => next,
            Self::Torrent { file_index, .. } => file_index,
        }
    }

    /// The stream auto-next consumes.
    pub(super) const fn source(self) -> StreamSource {
        StreamSource::torrent(self.info_hash(), self.file_index())
    }

    /// Yields or resumes: a separate torrent is paused or resumed, a
    /// same-batch preload has N+1 removed from or added to the selected
    /// files (#208 Q6).
    async fn transfer(self, session: &TorrentSession, transfer: Transfer) {
        let files = match (self, transfer) {
            (Self::Torrent { info_hash, .. }, Transfer::Yielded) => {
                log_transfer(session.pause(info_hash).await, transfer);
                return;
            }
            (Self::Torrent { info_hash, .. }, Transfer::Running) => {
                log_transfer(session.resume(info_hash).await, transfer);
                return;
            }
            (Self::SameBatch { current, .. }, Transfer::Yielded) => vec![current],
            (Self::SameBatch { current, next, .. }, Transfer::Running) => vec![current, next],
        };
        log_transfer(
            session.update_only_files(self.info_hash(), &files).await,
            transfer,
        );
    }
}

fn log_transfer(result: Result<(), torrent::Error>, transfer: Transfer) {
    if let Err(error) = result {
        debug!(%error, ?transfer, "could not move the preload");
    }
}

/// Whether a preload downloads or has yielded to the current stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Transfer {
    /// Paused for the current stream's buffering.
    Yielded,
    /// Downloading.
    Running,
}

/// How far the preload got.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Outcome {
    /// Still resolving N+1's release.
    Pending,
    /// N+1 has a stream; the torrent is held with origin preload.
    Ready(Fetch),
    /// Auto-select found nothing; shown only at episode end (#208 Q4).
    Failed(String),
    /// N+1 is not preloadable: in the library, not aired, or nothing to
    /// search with.
    Skipped,
}

/// The one preload (#208 Q7).
#[derive(Debug)]
pub(super) struct Preload {
    id: u64,
    media: MediaId,
    episode: u32,
    outcome: Arc<Mutex<Outcome>>,
    task: JoinHandle<()>,
    grace: Option<JoinHandle<()>>,
}

impl Drop for Preload {
    fn drop(&mut self) {
        self.task.abort();
        if let Some(grace) = &self.grace {
            grace.abort();
        }
    }
}

impl Preload {
    fn outcome(&self) -> Outcome {
        lock(&self.outcome).clone()
    }
}

/// A preload whose stream is known, as the Torrent List shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct Preloaded {
    /// The media of the preloaded episode.
    pub media: MediaId,
    /// The preloaded episode.
    pub episode: u32,
    /// The stream auto-next will play.
    pub source: StreamSource,
}

/// What preloading needs beyond the playback manager.
pub(super) struct PreloadDeps {
    pub(super) selector: Arc<RwLock<AutoSelector>>,
    pub(super) airing: Arc<dyn EpisodeAiring>,
    pub(super) profiles: Mutex<AutoSelectSettings>,
}

impl std::fmt::Debug for PreloadDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PreloadDeps").finish_non_exhaustive()
    }
}

/// The search for the episode after `request`'s: one episode on, and the
/// eid looked up again.
pub(super) fn next_search(request: &PlayRequest) -> Option<AutoSelectRequest> {
    let mut next = request.auto_select.clone()?;
    next.search.episode = next.search.episode.checked_add(1)?;
    next.search.absolute_episode = next
        .search
        .absolute_episode
        .and_then(|absolute| absolute.checked_add(1));
    next.search.anidb_eid = None;
    Some(next)
}

/// N+1 as a file-match target.
fn next_target(request: &PlayRequest) -> Option<EpisodeTarget> {
    if let Some(next) = next_search(request) {
        let mut target = EpisodeTarget::new(next.search.season, next.search.episode);
        if let Some(absolute) = next.search.absolute_episode {
            target = target.with_absolute_episode(absolute);
        }
        return Some(target);
    }
    Some(EpisodeTarget::new(1, request.episode.checked_add(1)?))
}

/// N+1's file in the batch torrent `info_hash`, other than `current`.
pub(super) async fn batch_file(
    session: &TorrentSession,
    info_hash: InfoHash,
    current: usize,
    request: &PlayRequest,
) -> Option<usize> {
    let target = next_target(request)?;
    let magnet = TorrentSource::Magnet(format!("magnet:?xt=urn:btih:{info_hash}"));
    let files = session.list_files(magnet).await.ok()?;
    search::pick_file(
        files.iter().map(|file| (file.index, file.path.as_str())),
        &target,
    )
    .filter(|index| *index != current)
}

/// Calls `transfer(Yielded)` while `buffering` holds, and
/// `transfer(Running)` once it has been clear for `resume_after`.
/// A stall during the wait starts it again. Runs until dropped.
pub(super) async fn yield_while_buffering<F, Fut>(
    mut buffering: watch::Receiver<bool>,
    resume_after: Duration,
    mut transfer: F,
) where
    F: FnMut(Transfer) -> Fut,
    Fut: Future<Output = ()>,
{
    let mut yielded = false;
    loop {
        let stalled = *buffering.borrow_and_update();
        if stalled && !yielded {
            transfer(Transfer::Yielded).await;
            yielded = true;
        }
        if yielded && !stalled {
            tokio::select! {
                () = tokio::time::sleep(resume_after) => {
                    transfer(Transfer::Running).await;
                    yielded = false;
                    continue;
                }
                changed = buffering.changed() => {
                    if changed.is_err() {
                        return std::future::pending().await;
                    }
                    continue;
                }
            }
        }
        if buffering.changed().await.is_err() {
            return std::future::pending().await;
        }
    }
}

impl PlaybackManager {
    /// Turns the `Preload next episode` setting on or off. Off drops the
    /// preload now (#208 Q7).
    #[expect(
        clippy::fn_params_excessive_bools,
        reason = "the one flag is the setting itself"
    )]
    pub async fn set_preload(&self, enabled: bool) {
        self.preload_enabled
            .store(enabled, std::sync::atomic::Ordering::SeqCst);
        if !enabled {
            self.drop_preload().await;
        }
    }

    /// Replaces the auto-select profiles a preload searches with.
    pub fn set_auto_select_settings(&self, settings: AutoSelectSettings) {
        if let Some(deps) = &self.preload_deps {
            *lock(&deps.profiles) = settings;
        }
    }

    /// The preload, once its stream is known.
    #[must_use]
    pub fn preloaded(&self) -> Option<Preloaded> {
        let slot = lock(&self.preload);
        let preload = slot.as_ref()?;
        match preload.outcome() {
            Outcome::Ready(fetch) => Some(Preloaded {
                media: preload.media,
                episode: preload.episode,
                source: fetch.source(),
            }),
            _ => None,
        }
    }

    /// Tells the preload whether the current stream is buffering.
    #[expect(
        clippy::fn_params_excessive_bools,
        reason = "the one flag is the relay's Buffering flag"
    )]
    pub(super) fn set_stream_buffering(&self, buffering: bool) {
        self.buffering.send_replace(buffering);
    }

    /// The preload's outcome for `(media, episode)`, if one exists.
    pub(super) fn preload_outcome(&self, media: MediaId, episode: u32) -> Option<Outcome> {
        lock(&self.preload)
            .as_ref()
            .filter(|preload| preload.media == media && preload.episode == episode)
            .map(Preload::outcome)
    }

    /// Starts preloading the episode after `request` (#208 Q1), unless the
    /// setting, auto-next or the request rule it out, or a preload of that
    /// episode already exists. A preload of anything else is replaced.
    pub(super) async fn start_preload(self: &Arc<Self>, request: &PlayRequest) {
        let enabled = self
            .preload_enabled
            .load(std::sync::atomic::Ordering::SeqCst);
        if !enabled
            || self.auto_next.is_none()
            || self.preload_deps.is_none()
            || !request.kind.is_regular()
            || !matches!(request.source, StreamSource::Torrent { .. })
        {
            return;
        }
        let Some(episode) = request.episode.checked_add(1) else {
            return;
        };
        if self.preload_outcome(request.media, episode).is_some() {
            return;
        }
        self.drop_preload().await;
        let Some(streams) = self.torrent.clone() else {
            return;
        };
        let outcome = Arc::new(Mutex::new(Outcome::Pending));
        let id = self
            .next_id
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let manager = Arc::clone(self);
        let current = request.clone();
        let shared = Arc::clone(&outcome);
        let task = tokio::spawn(async move {
            manager.run_preload(streams, current, shared).await;
        });
        *lock(&self.preload) = Some(Preload {
            id,
            media: request.media,
            episode,
            outcome,
            task,
            grace: None,
        });
    }

    /// Resolves N+1, holds its torrent, then fetches it, yielding to the
    /// current stream.
    async fn run_preload(
        &self,
        streams: Arc<TorrentStreams>,
        current: PlayRequest,
        outcome: Arc<Mutex<Outcome>>,
    ) {
        let session = match streams.session().await {
            Ok(session) => session,
            Err(error) => {
                warn!(%error, "no torrent session for the preload");
                *lock(&outcome) = Outcome::Failed(error.to_string());
                return;
            }
        };
        let fetch = match self.resolve_preload(&session, &current).await {
            Ok(Some(fetch)) => fetch,
            Ok(None) => {
                *lock(&outcome) = Outcome::Skipped;
                return;
            }
            Err(reason) => {
                info!(%reason, "no release found for the next episode");
                *lock(&outcome) = Outcome::Failed(reason);
                return;
            }
        };
        let next = current.episode.checked_add(1);
        let anilist_id = self.anilist_id(current.media).await;
        {
            // Held and Ready together, so an abort never leaks a hold.
            let mut outcome = lock(&outcome);
            streams.track(fetch.info_hash(), StreamOrigin::Preload);
            streams.register(
                fetch.info_hash(),
                TorrentOrigin::Preload,
                next.map(|episode| TorrentLink::new(current.media, episode, anilist_id)),
            );
            *outcome = Outcome::Ready(fetch);
        }
        if let Err(error) = select_preload(&session, fetch).await {
            warn!(%error, "the preload could not start");
            return;
        }
        let buffering = self.buffering.subscribe();
        let resume_after = self.preload_resume_after;
        tokio::join!(
            async {
                if let Err(error) = session
                    .wait_ready(fetch.info_hash(), fetch.file_index())
                    .await
                {
                    debug!(%error, "the preload's readiness gate was not met");
                }
            },
            yield_while_buffering(buffering, resume_after, |transfer| {
                let session = Arc::clone(&session);
                async move { fetch.transfer(&session, transfer).await }
            }),
        );
    }

    /// N+1's stream: the same batch first, else auto-select. `Ok(None)` when
    /// N+1 is not preloadable (#208 Q1–Q2, Q4).
    async fn resolve_preload(
        &self,
        session: &TorrentSession,
        current: &PlayRequest,
    ) -> Result<Option<Fetch>, String> {
        let StreamSource::Torrent {
            info_hash,
            file_index,
        } = current.source
        else {
            return Ok(None);
        };
        let Some(deps) = &self.preload_deps else {
            return Ok(None);
        };
        if self.next_episode(current).await.is_some() {
            return Ok(None);
        }
        let Some(next) = current.episode.checked_add(1) else {
            return Ok(None);
        };
        let Some(anilist_id) = self.anilist_id(current.media).await else {
            return Ok(None);
        };
        if !deps.airing.has_aired(anilist_id, next).await {
            return Ok(None);
        }
        if let Some(next) = batch_file(session, info_hash, file_index, current).await {
            return Ok(Some(Fetch::SameBatch {
                info_hash,
                current: file_index,
                next,
            }));
        }
        let Some(search) = next_search(current) else {
            return Ok(None);
        };
        let selector = deps
            .selector
            .read()
            .map_err(|_| "the auto-select pipeline is unavailable".to_owned())?
            .clone();
        let profile = lock(&deps.profiles).effective(Some(current.media.get()));
        let selection = selector
            .select(search, &profile)
            .await
            .map_err(|error| error.to_string())?;
        match selection.source {
            StreamSource::Torrent {
                info_hash,
                file_index,
            } => Ok(Some(Fetch::Torrent {
                info_hash,
                file_index,
            })),
            _ => Ok(None),
        }
    }

    /// Takes the preload of `(media, episode)` when its stream is known. The
    /// caller holds the torrent for playback, then calls
    /// [`Self::release_preload_hold`].
    pub(super) fn take_preload(&self, media: MediaId, episode: u32) -> Option<Fetch> {
        let mut slot = lock(&self.preload);
        let matches = slot
            .as_ref()
            .is_some_and(|preload| preload.media == media && preload.episode == episode);
        if !matches {
            return None;
        }
        match slot.take().map(|preload| preload.outcome()) {
            Some(Outcome::Ready(fetch)) => Some(fetch),
            // Still resolving or failed: playback goes its own way.
            _ => None,
        }
    }

    /// Releases a consumed preload's hold once playback holds the torrent.
    pub(super) async fn release_preload_hold(&self, fetch: Fetch) {
        if let Some(streams) = &self.torrent {
            streams
                .end_stream(fetch.info_hash(), StreamOrigin::Preload)
                .await;
        }
    }

    /// Drops the preload now (#208 Q7).
    pub(super) async fn drop_preload(&self) {
        let preload = lock(&self.preload).take();
        if let Some(preload) = preload {
            self.release(&preload).await;
        }
    }

    /// Keeps the preload for the grace period, then drops it (#208 Q7).
    pub(super) fn start_grace(self: &Arc<Self>) {
        let mut slot = lock(&self.preload);
        let Some(preload) = slot.as_mut() else {
            return;
        };
        if preload.grace.is_some() {
            return;
        }
        let manager = Arc::clone(self);
        let (id, grace) = (preload.id, self.preload_grace);
        preload.grace = Some(tokio::spawn(async move {
            tokio::time::sleep(grace).await;
            let expired = {
                let mut slot = lock(&manager.preload);
                if slot.as_ref().is_some_and(|preload| preload.id == id) {
                    slot.take()
                } else {
                    None
                }
            };
            if let Some(mut expired) = expired {
                debug!("the preload's grace period ended");
                // This task is the grace; don't abort it from Drop.
                expired.grace = None;
                manager.release(&expired).await;
            }
        }));
    }

    /// A dropped preload gives N+1's file back, then its hold: a torrent
    /// nothing else holds is paused and deleted when partial (#180).
    async fn release(&self, preload: &Preload) {
        preload.task.abort();
        let Outcome::Ready(fetch) = preload.outcome() else {
            return;
        };
        let Some(streams) = &self.torrent else {
            return;
        };
        if let Fetch::SameBatch {
            info_hash, current, ..
        } = fetch
            && streams.origin(info_hash) == Some(StreamOrigin::Playback)
            && let Ok(session) = streams.session().await
            && let Err(error) = session.update_only_files(info_hash, &[current]).await
        {
            debug!(%error, "could not deselect a dropped preload");
        }
        streams
            .end_stream(fetch.info_hash(), StreamOrigin::Preload)
            .await;
    }

    async fn anilist_id(&self, media: MediaId) -> Option<u32> {
        let ids = self.db.media().external_ids(media).await.ok()?;
        ids.into_iter().find_map(|id| match id {
            domain::ExternalId::AniList(id) => Some(id),
            _ => None,
        })
    }
}

/// Adds N+1's file to the session: the batch selects it beside the current
/// file, a separate torrent is added with only that file.
async fn select_preload(session: &TorrentSession, fetch: Fetch) -> Result<(), torrent::Error> {
    match fetch {
        Fetch::SameBatch {
            info_hash,
            current,
            next,
        } => session.update_only_files(info_hash, &[current, next]).await,
        Fetch::Torrent {
            info_hash,
            file_index,
        } => {
            let existing = session.stats(info_hash).is_ok();
            let magnet = TorrentSource::Magnet(format!("magnet:?xt=urn:btih:{info_hash}"));
            session.add(magnet, file_index).await?;
            if existing {
                session.update_only_files(info_hash, &[file_index]).await?;
            }
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    type Log = Arc<Mutex<Vec<(Transfer, Duration)>>>;

    /// Records each yield and resume at its paused-clock time since the
    /// start.
    fn recorder() -> (Log, impl FnMut(Transfer) -> std::future::Ready<()>) {
        let seen = Arc::new(Mutex::new(Vec::new()));
        let start = tokio::time::Instant::now();
        let log = Arc::clone(&seen);
        (seen, move |transfer| {
            log.lock().unwrap().push((transfer, start.elapsed()));
            std::future::ready(())
        })
    }

    async fn settle() {
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn a_preload_yields_while_buffering_and_resumes_after_ten_clean_seconds() {
        let (tx, rx) = watch::channel(false);
        let (seen, set_running) = recorder();
        let task = tokio::spawn(yield_while_buffering(rx, RESUME_AFTER, set_running));

        settle().await;
        assert!(
            seen.lock().unwrap().is_empty(),
            "nothing yields while clear"
        );
        tx.send_replace(true);
        settle().await;
        tokio::time::sleep(Duration::from_secs(3)).await;
        tx.send_replace(false);
        tokio::time::sleep(Duration::from_secs(20)).await;

        assert_eq!(
            *seen.lock().unwrap(),
            [
                (Transfer::Yielded, Duration::ZERO),
                (Transfer::Running, Duration::from_secs(13)),
            ]
        );
        task.abort();
    }

    #[tokio::test(start_paused = true)]
    async fn a_stall_during_the_clean_wait_starts_it_again() {
        let (tx, rx) = watch::channel(true);
        let (seen, set_running) = recorder();
        let task = tokio::spawn(yield_while_buffering(rx, RESUME_AFTER, set_running));

        settle().await;
        tx.send_replace(false);
        tokio::time::sleep(Duration::from_secs(6)).await;
        tx.send_replace(true);
        settle().await;
        tokio::time::sleep(Duration::from_secs(1)).await;
        tx.send_replace(false);
        tokio::time::sleep(Duration::from_secs(9)).await;
        assert_eq!(
            *seen.lock().unwrap(),
            [(Transfer::Yielded, Duration::ZERO)],
            "not yet"
        );
        tokio::time::sleep(Duration::from_secs(2)).await;

        assert_eq!(
            *seen.lock().unwrap(),
            [
                (Transfer::Yielded, Duration::ZERO),
                (Transfer::Running, Duration::from_secs(17)),
            ],
            "resumed 10 s after the last stall cleared, never paused twice"
        );
        task.abort();
    }

    #[test]
    fn air_dates_compare_against_today() {
        assert!(aired_by_today("2023-09-29"));
        assert!(!aired_by_today("2999-01-01"));
        assert!(!aired_by_today("soon"));
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
    }
}
