//! Auto-select: search, filter, rank, then verify the top candidates at file
//! level (#181 Q11, ADR 0007).
//!
//! ```text
//! eid hint (AniZip) → search both indexers → rank with the profile
//!   → for each of the top 3, in order: file list → pick the episode file
//!   → StreamSource::Torrent, or IndexerError / NoTorrentsMatch / NoFileFound
//! ```
//!
//! Nothing is relaxed on failure: the caller falls back to the release list
//! with the results carried by the error. The torrent is only listed here,
//! never added; playback adds it once a file is chosen.

use std::{sync::Arc, time::Duration};

use domain::{ExtensionId, InfoHash, StreamSource};
use metadata::{Aging, AniZip};
use search::{
    EpisodeTarget, LaneFilter, SearchOutcome, SearchRequest, Searcher, Torrent, Transport,
};
use settings::AutoSelectProfile;
use torrent::{TorrentFile, TorrentSource};

use crate::{BoxFuture, TorrentStreams};

/// How many ranked candidates are verified before giving up (Seanime's
/// `MaxTorrentCandidatesToCheck`, #181 Q11).
pub const VERIFY_CANDIDATES: usize = 3;

/// How long a magnet may take to deliver its metadata for the file list.
pub const FILE_LIST_TIMEOUT: Duration = Duration::from_secs(30);

/// Searches the torrent indexers.
pub trait TorrentSearch: Send + Sync {
    /// The outcome for `request` on the lanes in `filter`, deduped by infohash.
    fn search(
        &self,
        request: SearchRequest,
        lanes: LaneFilter,
    ) -> BoxFuture<Result<SearchOutcome, search::Error>>;

    /// One keyword-only search on both indexers; `refresh` bypasses the
    /// 60 s cache. The default is empty, for fakes.
    #[expect(
        clippy::fn_params_excessive_bools,
        reason = "refresh bypasses the cache"
    )]
    fn search_keyword(
        &self,
        _keyword: String,
        _refresh: bool,
    ) -> BoxFuture<Result<SearchOutcome, search::Error>> {
        Box::pin(async { Ok(SearchOutcome::default()) })
    }

    /// Drops every cached result of extension `id`, after it is disabled,
    /// uninstalled or updated. The default does nothing, for fakes.
    fn purge_provider(&self, _id: &ExtensionId) {}
}

/// Lists a torrent's files without downloading it.
pub trait FileLister: Send + Sync {
    /// The files of `torrent`, each with the index playback selects it by.
    fn files(&self, torrent: Torrent) -> BoxFuture<Result<Vec<TorrentFile>, FileListError>>;
}

/// Looks up `AniDB` episode ids, the optional `eid=` search hint (#206).
pub trait EpisodeIds: Send + Sync {
    /// The eid of `AniZip` episode `episode_key` of AniList media `anilist_id`.
    fn anidb_eid(&self, anilist_id: u32, episode_key: String) -> BoxFuture<Option<u32>>;
}

/// Why a torrent's file list could not be fetched.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum FileListError {
    /// The torrent session could not read the metadata.
    #[error("the torrent metadata could not be read: {0}")]
    Torrent(#[from] torrent::Error),
    /// A magnet's metadata did not arrive in time.
    #[error("the torrent metadata did not arrive within {FILE_LIST_TIMEOUT:?}")]
    TimedOut,
}

/// Why auto-select chose nothing. Each failure falls back to the release list.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum AutoSelectError {
    /// An indexer failed after its retry.
    #[error("the torrent search failed: {0}")]
    IndexerError(#[source] search::Error),
    /// No result is the episode and passes the profile's filters.
    #[error("no torrent matches the episode and the profile")]
    NoTorrentsMatch {
        /// Every search result, unfiltered, for the release list.
        results: Vec<Torrent>,
    },
    /// None of the top candidates holds a file of the episode.
    #[error("no candidate torrent holds the episode's file")]
    NoFileFound {
        /// The ranked candidates that passed the filters.
        candidates: Vec<Torrent>,
    },
}

/// What auto-select is asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct AutoSelectRequest {
    /// The indexer search; its season and episode are also the match target.
    pub search: SearchRequest,
    /// The media's AniList id, for the `AniZip` eid lookup when the search has
    /// no eid yet.
    pub anilist_id: Option<u32>,
}

impl AutoSelectRequest {
    /// Auto-select for `search`, with the AniList id when known.
    #[must_use]
    pub const fn new(search: SearchRequest, anilist_id: Option<u32>) -> Self {
        Self { search, anilist_id }
    }
}

/// The verified choice: a torrent and the episode's file inside it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub struct AutoSelection {
    /// What playback opens.
    pub source: StreamSource,
    /// The chosen indexer result, with its magnet and `.torrent` links.
    pub torrent: Torrent,
    /// The chosen file's path inside the torrent.
    pub file_path: String,
}

/// Progress of an auto-select run, for the row's inline stage display.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AutoSelectProgress {
    /// The indexer search is running.
    Searching,
    /// Some lanes failed but others answered; the search continues without
    /// relaxing the match criteria (ADR 0011).
    LaneFailures {
        /// Display names of the lanes that failed.
        names: Vec<String>,
    },
    /// Verifying a candidate's file list; `checking` of `total`, both 1-based
    /// `checking` up to [`VERIFY_CANDIDATES`].
    Verifying {
        /// The candidate being checked, 1-based.
        checking: u32,
        /// How many candidates will be checked, up to [`VERIFY_CANDIDATES`].
        total: u32,
    },
}

/// Runs the auto-select pipeline.
#[derive(Clone)]
pub struct AutoSelector {
    search: Arc<dyn TorrentSearch>,
    files: Arc<dyn FileLister>,
    ids: Arc<dyn EpisodeIds>,
}

impl std::fmt::Debug for AutoSelector {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AutoSelector").finish_non_exhaustive()
    }
}

impl AutoSelector {
    /// A selector over the given search, file lister and eid lookup.
    #[must_use]
    pub fn new(
        search: Arc<dyn TorrentSearch>,
        files: Arc<dyn FileLister>,
        ids: Arc<dyn EpisodeIds>,
    ) -> Self {
        Self { search, files, ids }
    }

    /// The raw indexer search on `lanes`, without ranking or file checks
    /// (#181 Q12). Availability badges pass [`LaneFilter::BuiltIn`].
    pub fn search(
        &self,
        request: SearchRequest,
        lanes: LaneFilter,
    ) -> BoxFuture<Result<SearchOutcome, search::Error>> {
        let search = Arc::clone(&self.search);
        Box::pin(async move { search.search(request, lanes).await })
    }

    /// Choose a torrent and file for the request's episode with `profile`.
    ///
    /// # Errors
    ///
    /// [`AutoSelectError::IndexerError`] when every indexer lane failed,
    /// [`AutoSelectError::NoTorrentsMatch`] when no result passes the
    /// filters, [`AutoSelectError::NoFileFound`] when none of the top
    /// [`VERIFY_CANDIDATES`] holds the episode's file.
    pub async fn select(
        &self,
        request: AutoSelectRequest,
        profile: &AutoSelectProfile,
    ) -> Result<AutoSelection, AutoSelectError> {
        self.select_with_progress(request, profile, |_| {}).await
    }

    /// Like [`Self::select`], reporting [`AutoSelectProgress`] as it runs.
    ///
    /// `progress` is called with [`AutoSelectProgress::Searching`] before the
    /// indexer search and [`AutoSelectProgress::Verifying`] before each
    /// candidate's file-level check, up to [`VERIFY_CANDIDATES`].
    ///
    /// # Errors
    ///
    /// As [`Self::select`].
    pub async fn select_with_progress(
        &self,
        request: AutoSelectRequest,
        profile: &AutoSelectProfile,
        mut progress: impl FnMut(AutoSelectProgress) + Send,
    ) -> Result<AutoSelection, AutoSelectError> {
        let mut search = request.search;
        if search.anidb_eid.is_none()
            && let Some(anilist_id) = request.anilist_id
        {
            search.anidb_eid = self
                .ids
                .anidb_eid(anilist_id, search.episode.to_string())
                .await;
        }
        let mut target = EpisodeTarget::new(search.season, search.episode);
        if let Some(absolute) = search.absolute_episode {
            target = target.with_absolute_episode(absolute);
        }
        progress(AutoSelectProgress::Searching);
        let outcome = self
            .search
            .search(search, LaneFilter::All)
            .await
            .map_err(AutoSelectError::IndexerError)?;
        if !outcome.failures.is_empty() {
            progress(AutoSelectProgress::LaneFailures {
                names: outcome
                    .failures
                    .iter()
                    .map(|failure| failure.indexer.label().to_owned())
                    .collect(),
            });
        }
        let results = outcome.torrents;
        let candidates = search::rank(results.clone(), &target, profile);
        if candidates.is_empty() {
            return Err(AutoSelectError::NoTorrentsMatch { results });
        }
        let total = u32::try_from(candidates.len().min(VERIFY_CANDIDATES)).unwrap_or(u32::MAX);
        for (index, candidate) in candidates.iter().take(VERIFY_CANDIDATES).enumerate() {
            progress(AutoSelectProgress::Verifying {
                checking: u32::try_from(index).unwrap_or(u32::MAX) + 1,
                total,
            });
            if let Some(selection) = self.verify(candidate, &target).await {
                return Ok(selection);
            }
        }
        Err(AutoSelectError::NoFileFound { candidates })
    }

    /// One keyword-only search on both indexers; `refresh` bypasses the
    /// 60 s cache.
    ///
    /// # Errors
    ///
    /// [`search::Error::AllLanesFailed`] when every lane failed.
    #[expect(
        clippy::fn_params_excessive_bools,
        reason = "refresh bypasses the cache"
    )]
    pub async fn search_keyword(
        &self,
        keyword: &str,
        refresh: bool,
    ) -> Result<SearchOutcome, search::Error> {
        self.search
            .search_keyword(keyword.to_owned(), refresh)
            .await
    }

    /// Drops every cached search result of extension `id` (ADR 0011).
    pub fn purge_provider(&self, id: &ExtensionId) {
        self.search.purge_provider(id);
    }

    /// A torrent's file list, for the release list's on-demand batch pick.
    ///
    /// # Errors
    ///
    /// [`FileListError`] when the metadata cannot be read in time.
    pub async fn list_files(&self, torrent: &Torrent) -> Result<Vec<TorrentFile>, FileListError> {
        if !torrent.files.is_empty() {
            return Ok(torrent
                .files
                .iter()
                .enumerate()
                .map(|(index, path)| TorrentFile::new(index, path.clone(), 0))
                .collect());
        }
        self.files.files(torrent.clone()).await
    }

    /// The episode's file in `torrent`, or `None` to try the next candidate.
    async fn verify(&self, torrent: &Torrent, target: &EpisodeTarget) -> Option<AutoSelection> {
        let Ok(info_hash) = torrent.infohash.parse::<InfoHash>() else {
            tracing::info!(name = %torrent.name, "auto-select skipped a result without a v1 infohash");
            return None;
        };
        let files: Vec<(usize, String)> = if torrent.files.is_empty() {
            match self.files.files(torrent.clone()).await {
                Ok(files) => files
                    .into_iter()
                    .map(|file| (file.index, file.path))
                    .collect(),
                Err(error) => {
                    tracing::warn!(name = %torrent.name, %error, "auto-select could not list a torrent's files");
                    return None;
                }
            }
        } else {
            torrent.files.iter().cloned().enumerate().collect()
        };
        let index = search::pick_file(
            files.iter().map(|(index, path)| (*index, path.as_str())),
            target,
        );
        let Some((file_index, file_path)) =
            index.and_then(|index| files.into_iter().find(|(i, _)| *i == index))
        else {
            tracing::info!(name = %torrent.name, "auto-select found no episode file in a candidate");
            return None;
        };
        let magnet = torrent.magnet_url.clone();
        let trackers = magnet
            .as_deref()
            .map_or_else(Vec::new, search::trackers_from_magnet);
        Some(AutoSelection {
            source: StreamSource::torrent_with_source(info_hash, file_index, magnet, trackers),
            torrent: torrent.clone(),
            file_path,
        })
    }
}

impl<T: Transport + 'static> TorrentSearch for Arc<Searcher<T>> {
    fn search(
        &self,
        request: SearchRequest,
        lanes: LaneFilter,
    ) -> BoxFuture<Result<SearchOutcome, search::Error>> {
        let searcher = Arc::clone(self);
        Box::pin(async move { Searcher::search_with_filter(&searcher, &request, lanes).await })
    }

    fn search_keyword(
        &self,
        keyword: String,
        refresh: bool,
    ) -> BoxFuture<Result<SearchOutcome, search::Error>> {
        let searcher = Arc::clone(self);
        Box::pin(async move { Searcher::search_keyword(&searcher, &keyword, refresh).await })
    }

    fn purge_provider(&self, id: &ExtensionId) {
        Searcher::purge_provider(self, id);
    }
}

/// Lists files through the engine's torrent session, by magnet. The session
/// starts on the first list.
#[derive(Debug, Clone)]
pub struct SessionFiles {
    streams: Arc<TorrentStreams>,
}

impl SessionFiles {
    /// List through the session of `streams`.
    #[must_use]
    pub const fn new(streams: Arc<TorrentStreams>) -> Self {
        Self { streams }
    }
}

impl FileLister for SessionFiles {
    fn files(&self, torrent: Torrent) -> BoxFuture<Result<Vec<TorrentFile>, FileListError>> {
        let streams = Arc::clone(&self.streams);
        Box::pin(async move {
            let session = streams.session().await?;
            let magnet = torrent
                .magnet_url
                .unwrap_or_else(|| format!("magnet:?xt=urn:btih:{}", torrent.infohash));
            tokio::time::timeout(
                FILE_LIST_TIMEOUT,
                session.list_files(TorrentSource::Magnet(magnet)),
            )
            .await
            .map_err(|_| FileListError::TimedOut)?
            .map_err(FileListError::from)
        })
    }
}

/// `AniDB` eids from the cached `AniZip` episode payload.
#[derive(Debug, Clone)]
pub struct AniZipEpisodeIds {
    anizip: AniZip,
}

impl AniZipEpisodeIds {
    /// Look eids up through `anizip`.
    #[must_use]
    pub const fn new(anizip: AniZip) -> Self {
        Self { anizip }
    }
}

impl EpisodeIds for AniZipEpisodeIds {
    fn anidb_eid(&self, anilist_id: u32, episode_key: String) -> BoxFuture<Option<u32>> {
        let anizip = self.anizip.clone();
        Box::pin(async move {
            anizip
                .anidb_eid(anilist_id, &episode_key, Aging::Airing)
                .await
        })
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use search::Indexer;

    use super::*;

    const FRIEREN: u32 = 154_587;

    /// A Nyaa result with `seeders` and a v1 infohash padded from `hash`.
    fn result(hash: char, name: &str, seeders: u32) -> Torrent {
        let mut torrent = Torrent::new(Indexer::Nyaa, hash.to_string().repeat(40), name);
        torrent.seeders = Some(seeders);
        torrent.size = Some(1_000_000_000);
        torrent.magnet_url = Some(format!("magnet:?xt=urn:btih:{}", torrent.infohash));
        torrent
    }

    fn hash(hash: char) -> InfoHash {
        hash.to_string().repeat(40).parse().unwrap()
    }

    #[derive(Default)]
    struct FakeSearch {
        reply: Mutex<Option<Result<SearchOutcome, search::Error>>>,
        requests: Mutex<Vec<SearchRequest>>,
    }

    impl TorrentSearch for FakeSearch {
        fn search(
            &self,
            request: SearchRequest,
            _lanes: LaneFilter,
        ) -> BoxFuture<Result<SearchOutcome, search::Error>> {
            self.requests.lock().unwrap().push(request);
            let reply = self
                .reply
                .lock()
                .unwrap()
                .take()
                .unwrap_or(Ok(SearchOutcome::default()));
            Box::pin(async move { reply })
        }
    }

    /// File lists by infohash; a missing infohash fails like a stalled magnet.
    #[derive(Default)]
    struct FakeFiles {
        lists: Vec<(String, Vec<&'static str>)>,
        asked: Mutex<Vec<String>>,
    }

    impl FileLister for FakeFiles {
        fn files(&self, torrent: Torrent) -> BoxFuture<Result<Vec<TorrentFile>, FileListError>> {
            self.asked.lock().unwrap().push(torrent.name.clone());
            let list = self
                .lists
                .iter()
                .find(|(hash, _)| *hash == torrent.infohash)
                .map(|(_, files)| {
                    files
                        .iter()
                        .enumerate()
                        .map(|(index, path)| TorrentFile::new(index, *path, 1))
                        .collect()
                });
            Box::pin(async move { list.ok_or(FileListError::TimedOut) })
        }
    }

    struct FakeIds(Option<u32>);

    impl EpisodeIds for FakeIds {
        fn anidb_eid(&self, anilist_id: u32, episode_key: String) -> BoxFuture<Option<u32>> {
            let eid = (anilist_id == FRIEREN && episode_key == "5")
                .then_some(self.0)
                .flatten();
            Box::pin(async move { eid })
        }
    }

    struct Rig {
        search: Arc<FakeSearch>,
        files: Arc<FakeFiles>,
        selector: AutoSelector,
    }

    fn rig(
        results: Result<Vec<Torrent>, search::Error>,
        lists: Vec<(char, Vec<&'static str>)>,
    ) -> Rig {
        let reply = results.map(|torrents| SearchOutcome {
            torrents,
            failures: Vec::new(),
        });
        rig_with_outcome(reply, lists)
    }

    fn rig_with_outcome(
        reply: Result<SearchOutcome, search::Error>,
        lists: Vec<(char, Vec<&'static str>)>,
    ) -> Rig {
        let search = Arc::new(FakeSearch {
            reply: Mutex::new(Some(reply)),
            ..FakeSearch::default()
        });
        let files = Arc::new(FakeFiles {
            lists: lists
                .into_iter()
                .map(|(hash, files)| (hash.to_string().repeat(40), files))
                .collect(),
            ..FakeFiles::default()
        });
        let dyn_search: Arc<dyn TorrentSearch> = Arc::<FakeSearch>::clone(&search);
        let dyn_files: Arc<dyn FileLister> = Arc::<FakeFiles>::clone(&files);
        let selector = AutoSelector::new(dyn_search, dyn_files, Arc::new(FakeIds(Some(271_422))));
        Rig {
            search,
            files,
            selector,
        }
    }

    fn episode_5() -> AutoSelectRequest {
        let search = SearchRequest::new(vec!["Sousou no Frieren".to_owned()], 1, 5)
            .with_anidb_ids(17_617, None);
        AutoSelectRequest::new(search, Some(FRIEREN))
    }

    fn assert_torrent(source: &StreamSource, h: char, file_index: usize) {
        match source {
            StreamSource::Torrent {
                info_hash,
                file_index: index,
                ..
            } => {
                assert_eq!(*info_hash, hash(h), "infohash");
                assert_eq!(*index, file_index, "file index");
            }
            other => assert!(
                matches!(other, StreamSource::Torrent { .. }),
                "not a torrent source: {other:?}"
            ),
        }
    }

    async fn select(rig: &Rig) -> Result<AutoSelection, AutoSelectError> {
        rig.selector
            .select(episode_5(), &AutoSelectProfile::default())
            .await
    }

    #[tokio::test]
    async fn the_best_ranked_torrent_with_the_episode_file_is_chosen() {
        let rig = rig(
            Ok(vec![
                result('a', "[Grp] Sousou no Frieren - 05 [720p]", 90),
                result('b', "[Grp] Sousou no Frieren - 05 [1080p]", 10),
                result('c', "[Grp] Sousou no Frieren - 06 [1080p]", 99),
            ]),
            vec![('b', vec!["[Grp] Sousou no Frieren - 05 [1080p].mkv"])],
        );

        let selection = select(&rig).await.unwrap();

        assert_torrent(&selection.source, 'b', 0);
        assert_eq!(
            selection.file_path,
            "[Grp] Sousou no Frieren - 05 [1080p].mkv"
        );
        assert_eq!(
            *rig.files.asked.lock().unwrap(),
            ["[Grp] Sousou no Frieren - 05 [1080p]"]
        );
    }

    #[tokio::test]
    async fn the_indexer_file_list_is_used_without_fetching_metadata() {
        let mut batch = result('a', "[Grp] Sousou no Frieren (01-28) [1080p]", 50);
        batch.files = vec![
            "NCOP.mkv".to_owned(),
            "[Grp] Sousou no Frieren - 04.mkv".to_owned(),
            "[Grp] Sousou no Frieren - 05.mkv".to_owned(),
        ];
        let rig = rig(Ok(vec![batch]), Vec::new());

        let selection = select(&rig).await.unwrap();

        assert_torrent(&selection.source, 'a', 2);
        assert_eq!(*rig.files.asked.lock().unwrap(), Vec::<String>::new());
    }

    #[tokio::test]
    async fn a_candidate_without_the_episode_file_falls_through_to_the_next() {
        let rig = rig(
            Ok(vec![
                result('a', "[Grp] Sousou no Frieren (01-28) [1080p]", 90),
                result('b', "[Grp] Sousou no Frieren - 05 [1080p]", 10),
            ]),
            vec![
                (
                    'a',
                    vec![
                        "[Grp] Sousou no Frieren - 01.mkv",
                        "[Grp] Sousou no Frieren - 02.mkv",
                    ],
                ),
                ('b', vec!["[Grp] Sousou no Frieren - 05 [1080p].mkv"]),
            ],
        );

        let selection = select(&rig).await.unwrap();

        assert_torrent(&selection.source, 'b', 0);
        assert_eq!(
            *rig.files.asked.lock().unwrap(),
            [
                "[Grp] Sousou no Frieren (01-28) [1080p]",
                "[Grp] Sousou no Frieren - 05 [1080p]"
            ]
        );
    }

    #[tokio::test]
    async fn a_candidate_whose_metadata_fails_falls_through_to_the_next() {
        let rig = rig(
            Ok(vec![
                result('a', "[Grp] Sousou no Frieren - 05 [1080p]", 90),
                result('b', "[Grp] Sousou no Frieren - 05 [1080p]", 10),
            ]),
            vec![('b', vec!["[Grp] Sousou no Frieren - 05.mkv"])],
        );

        let selection = select(&rig).await.unwrap();

        assert_torrent(&selection.source, 'b', 0);
    }

    #[tokio::test]
    async fn results_that_all_fail_the_filters_are_no_torrents_match() {
        let found = vec![
            result('a', "[Grp] Sousou no Frieren - 06 [1080p]", 90),
            result('b', "[Grp] Sousou no Frieren - NCOP [1080p]", 10),
        ];
        let rig = rig(Ok(found.clone()), Vec::new());

        let error = select(&rig).await.unwrap_err();

        assert!(
            matches!(&error, AutoSelectError::NoTorrentsMatch { results } if *results == found),
            "{error:?}"
        );
        assert_eq!(*rig.files.asked.lock().unwrap(), Vec::<String>::new());
    }

    #[tokio::test]
    async fn an_empty_search_is_no_torrents_match() {
        let rig = rig(Ok(Vec::new()), Vec::new());

        let error = select(&rig).await.unwrap_err();

        assert!(
            matches!(&error, AutoSelectError::NoTorrentsMatch { results } if results.is_empty()),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn progress_reports_searching_then_verifying_each_candidate() {
        let names = [
            "[A] Sousou no Frieren - 05 [1080p]",
            "[B] Sousou no Frieren - 05 [1080p]",
            "[C] Sousou no Frieren - 05 [1080p]",
            "[D] Sousou no Frieren - 05 [1080p]",
        ];
        let found: Vec<Torrent> = names
            .iter()
            .zip(['a', 'b', 'c', 'd'])
            .zip([40, 30, 20, 10])
            .map(|((name, hash), seeders)| result(hash, name, seeders))
            .collect();
        let rig = rig(Ok(found), Vec::new()); // no file lists -> every candidate fails

        let mut seen = Vec::new();
        let _ = rig
            .selector
            .select_with_progress(episode_5(), &AutoSelectProfile::default(), |p| {
                seen.push(p);
            })
            .await;

        assert_eq!(
            seen,
            vec![
                AutoSelectProgress::Searching,
                AutoSelectProgress::Verifying {
                    checking: 1,
                    total: 3
                },
                AutoSelectProgress::Verifying {
                    checking: 2,
                    total: 3
                },
                AutoSelectProgress::Verifying {
                    checking: 3,
                    total: 3
                },
            ],
            "Searching then Verifying n/3 for the top 3"
        );
    }

    #[tokio::test]
    async fn only_the_top_three_are_verified_before_no_file_found() {
        let names = [
            "[A] Sousou no Frieren (01-28) [1080p]",
            "[B] Sousou no Frieren (01-28) [1080p]",
            "[C] Sousou no Frieren (01-28) [1080p]",
            "[D] Sousou no Frieren - 05 [1080p]",
        ];
        let found: Vec<Torrent> = names
            .iter()
            .zip(['a', 'b', 'c', 'd'])
            .zip([40, 30, 20, 10])
            .map(|((name, hash), seeders)| result(hash, name, seeders))
            .collect();
        let no_episode = vec!["[X] Sousou no Frieren - 01.mkv"];
        let rig = rig(
            Ok(found),
            vec![
                ('a', no_episode.clone()),
                ('b', no_episode.clone()),
                ('c', no_episode),
                ('d', vec!["[D] Sousou no Frieren - 05 [1080p].mkv"]),
            ],
        );

        let error = select(&rig).await.unwrap_err();

        let offered = match &error {
            AutoSelectError::NoFileFound { candidates } => Some(candidates.len()),
            _ => None,
        };
        assert_eq!(
            offered,
            Some(4),
            "every ranked candidate is offered: {error:?}"
        );
        assert_eq!(*rig.files.asked.lock().unwrap(), names[..3]);
    }

    #[tokio::test]
    async fn every_lane_failing_is_an_indexer_error() {
        let rig = rig(Err(search::Error::AllLanesFailed), Vec::new());

        let error = select(&rig).await.unwrap_err();

        assert!(
            matches!(
                error,
                AutoSelectError::IndexerError(search::Error::AllLanesFailed)
            ),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn a_failed_lane_still_selects_and_is_named_in_progress() {
        let rig = rig_with_outcome(
            Ok(SearchOutcome {
                torrents: vec![result('a', "[Grp] Sousou no Frieren - 05 [1080p]", 90)],
                failures: vec![search::LaneFailure {
                    indexer: Indexer::AnimeTosho,
                    error: search::Error::Http {
                        indexer: Indexer::AnimeTosho,
                        status: 500,
                    },
                }],
            }),
            vec![('a', vec!["[Grp] Sousou no Frieren - 05 [1080p].mkv"])],
        );

        let mut seen = Vec::new();
        let selection = rig
            .selector
            .select_with_progress(episode_5(), &AutoSelectProfile::default(), |progress| {
                seen.push(progress);
            })
            .await
            .unwrap();

        assert_torrent(&selection.source, 'a', 0);
        assert_eq!(
            seen.first(),
            Some(&AutoSelectProgress::Searching),
            "the search runs first"
        );
        assert_eq!(
            seen.iter().find_map(|progress| match progress {
                AutoSelectProgress::LaneFailures { names } => Some(names.clone()),
                _ => None,
            }),
            Some(vec!["AnimeTosho".to_owned()]),
            "the failed lane is named without relaxing the match"
        );
    }

    #[tokio::test]
    async fn the_anizip_eid_is_sent_as_the_search_hint() {
        let rig = rig(Ok(Vec::new()), Vec::new());

        let _ = select(&rig).await;

        let requests = rig.search.requests.lock().unwrap();
        assert_eq!(requests.first().and_then(|r| r.anidb_eid), Some(271_422));
    }

    #[tokio::test]
    async fn a_known_eid_is_kept_and_a_missing_one_is_not_required() {
        let rig = rig(Ok(Vec::new()), Vec::new());
        let mut known = episode_5();
        known.search.anidb_eid = Some(1);
        let unknown = AutoSelectRequest::new(episode_5().search, Some(1));

        let _ = rig
            .selector
            .select(known, &AutoSelectProfile::default())
            .await;
        *rig.search.reply.lock().unwrap() = Some(Ok(SearchOutcome::default()));
        let _ = rig
            .selector
            .select(unknown, &AutoSelectProfile::default())
            .await;

        let eids: Vec<_> = rig
            .search
            .requests
            .lock()
            .unwrap()
            .iter()
            .map(|r| r.anidb_eid)
            .collect();
        assert_eq!(eids, [Some(1), None]);
    }
}
