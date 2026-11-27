//! Local library scanning, threshold matching and user overrides.
//!
//! Folders go in, matched local files come out and are persisted through
//! `db`. The engine passes in the candidates (ADR 0001); this crate never
//! fetches them. See ARCHITECTURE §6 for the scan pipeline.
//!
//! The title normalizer follows behavior from seanime's
//! `internal/library/scanner/title_normalization.go` (GPL-3.0). Original
//! copyright stays with the seanime authors; see `reference/seanime/LICENSE`.
//! The notice is kept with the ported behavior in [`normalize`].

mod error;
mod hydrate;
mod matcher;
mod normalize;
mod scanner;
mod watcher;

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    time::{Duration, Instant, SystemTime},
};

use db::{Db, FileFlag, LocalFile};
use domain::{ExternalId, MediaId, MediaKind};
use hydrate::{HydratedFile, Hydration};
use metadata::{AniZip, AnimeFillerList};

pub use db::{MediaMatchSummary, ScanSummary, UnmatchedFile, UnmatchedReason};
pub use error::LibraryError;
pub use matcher::{Candidate, DEFAULT_THRESHOLD, Match, Matcher};
pub use normalize::{NormalizedTitle, normalize};
pub use scanner::{ScannedFile, walk};
pub use watcher::AutoScan;

/// Files walked and matched so far in one scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanProgress {
    /// Files found by the walk so far.
    pub files_walked: usize,
    /// Files matched so far.
    pub files_matched: usize,
}

/// Scans library folders and persists what it finds.
#[derive(Debug, Clone)]
pub struct Library {
    db: Db,
    matcher: Matcher,
    hydration: Option<Hydration>,
}

impl Library {
    /// A library using the default matching threshold.
    pub fn new(db: Db) -> Self {
        Self::with_threshold(db, DEFAULT_THRESHOLD)
    }

    /// A library using a caller-supplied matching threshold.
    pub fn with_threshold(db: Db, threshold: f64) -> Self {
        Self {
            db,
            matcher: Matcher::new(threshold),
            hydration: None,
        }
    }

    /// A library that hydrates its matches with `AniZip` and filler metadata.
    /// The clients are fetched once per matched media on each scan.
    pub fn with_metadata(db: Db, anizip: AniZip, filler: AnimeFillerList) -> Self {
        Self::new(db).hydrating(anizip, filler)
    }

    /// Hydrates matches while keeping this library's matching threshold.
    #[must_use]
    pub fn hydrating(mut self, anizip: AniZip, filler: AnimeFillerList) -> Self {
        self.hydration = Some(Hydration { anizip, filler });
        self
    }

    /// Walks `folders`, matches their titles against `candidates`, and stores
    /// one row per file. Locked files keep their media and episode, ignored
    /// files stay unmatched, and every scan writes a summary.
    ///
    /// A folder that is missing or yields no files is treated as absent: its
    /// rows are shelved until a scan finds files in it again.
    ///
    /// `progress` is called as files are walked and as groups are matched; the
    /// engine throttles it before reporting to the UI.
    ///
    /// # Errors
    ///
    /// [`LibraryError::Walk`] when a folder cannot be read,
    /// [`LibraryError::CandidateWithoutExternalIds`] when a matching candidate
    /// has nothing to cross-reference, or [`LibraryError::Db`] for storage
    /// errors.
    pub async fn scan(
        &self,
        folders: &[PathBuf],
        candidates: &[Candidate],
        mut progress: impl FnMut(ScanProgress) + Send,
    ) -> Result<ScanSummary, LibraryError> {
        let started_at = SystemTime::now();
        let elapsed = Instant::now();
        let existing: HashMap<String, LocalFile> = self
            .db
            .local_file()
            .all()
            .await?
            .into_iter()
            .map(|file| (file.path.to_string_lossy().into_owned(), file))
            .collect();
        let mut scanned = Vec::new();
        let matched = 0;
        let mut empty_folders = Vec::new();
        for folder in folders.iter().filter(|folder| folder.is_dir()) {
            let mut found = false;
            for file in walk(folder)? {
                progress(ScanProgress {
                    files_walked: scanned.len() + 1,
                    files_matched: matched,
                });
                scanned.push(file);
                found = true;
            }
            if !found {
                // An unmounted mount point stays behind as an empty folder.
                empty_folders.push(folder);
            }
        }
        let walked: HashSet<String> = scanned
            .iter()
            .map(|file| file.path.to_string_lossy().into_owned())
            .collect();
        let mut preserved: HashMap<String, LocalFile> = HashMap::new();
        for (key, row) in &existing {
            let under_absent = folders
                .iter()
                .any(|folder| !folder.is_dir() && Path::new(key).starts_with(folder))
                || empty_folders
                    .iter()
                    .any(|folder| Path::new(key).starts_with(folder));
            if under_absent {
                // A missing drive or empty mount shelves its files instead
                // of losing them.
                let mut shelved = row.clone();
                shelved.shelved = true;
                preserved.insert(key.clone(), shelved);
            } else if row.shelved && walked.contains(key) {
                // The drive returned: keep the match without re-matching.
                let mut returned = row.clone();
                returned.shelved = false;
                preserved.insert(key.clone(), returned);
            }
            // A shelved file that is still absent from a present folder is
            // deliberately not preserved: `sync` deletes its row.
        }
        let overrides: HashMap<String, LocalFile> = existing
            .iter()
            .filter(|(_, row)| row.locked || row.ignored || row.shelved)
            .map(|(key, row)| (key.clone(), row.clone()))
            .chain(
                preserved
                    .iter()
                    .map(|(key, row)| (key.clone(), row.clone())),
            )
            .collect();
        let eligible = eligible_files(&scanned, &overrides);
        let (wins, mut media) = self
            .match_groups(&scanned, &eligible, candidates, &mut progress, matched)
            .await?;
        let hydrated = if let Some(clients) = &self.hydration {
            let result =
                hydrate::hydrate_all(&self.db, clients, &scanned, &wins, &media, candidates)
                    .await?;
            media.extend(result.media);
            result.files
        } else {
            HashMap::new()
        };
        let mut rows = build_rows(&scanned, &existing, &preserved, &wins, &media, &hydrated);
        for (key, row) in &preserved {
            if !walked.contains(key) {
                rows.push(row.clone());
            }
        }
        self.db.local_file().sync(&rows).await?;

        let summary = summarize(
            self.matcher,
            &scanned,
            &rows,
            folders,
            started_at,
            elapsed.elapsed(),
            candidates,
        );
        self.db.scan_summary().record(&summary).await?;
        Ok(summary)
    }

    /// The newest stored scan summary, if a scan has run.
    ///
    /// # Errors
    ///
    /// [`LibraryError::Db`] when the summary cannot be read.
    pub async fn latest_summary(&self) -> Result<Option<ScanSummary>, LibraryError> {
        self.db.scan_summary().latest().await.map_err(Into::into)
    }

    /// All stored scan summaries, newest first.
    ///
    /// # Errors
    ///
    /// [`LibraryError::Db`] when the summaries cannot be read.
    pub async fn summaries(&self) -> Result<Vec<ScanSummary>, LibraryError> {
        self.db.scan_summary().all().await.map_err(Into::into)
    }

    /// Fixes one file's media so re-scans leave it alone. Returns the number
    /// of rows changed, so a path that is not stored can be detected.
    ///
    /// # Errors
    ///
    /// [`LibraryError::Db`] when the row cannot be updated.
    pub async fn lock_file(&self, path: &Path) -> Result<usize, LibraryError> {
        Ok(changed(
            self.db.local_file().set_locked(path, FileFlag::Set).await?,
        ))
    }

    /// Lets the matcher handle one file again. Returns the rows changed.
    ///
    /// # Errors
    ///
    /// [`LibraryError::Db`] when the row cannot be updated.
    pub async fn unlock_file(&self, path: &Path) -> Result<usize, LibraryError> {
        Ok(changed(
            self.db
                .local_file()
                .set_locked(path, FileFlag::Clear)
                .await?,
        ))
    }

    /// Locks every file currently matched to `media`. Returns the rows changed.
    ///
    /// # Errors
    ///
    /// [`LibraryError::Db`] when the rows cannot be updated.
    pub async fn lock_media(&self, media: MediaId) -> Result<usize, LibraryError> {
        Ok(changed(
            self.db
                .local_file()
                .set_media_locked(media, FileFlag::Set)
                .await?,
        ))
    }

    /// Unlocks every file currently matched to `media`. Returns the rows changed.
    ///
    /// # Errors
    ///
    /// [`LibraryError::Db`] when the rows cannot be updated.
    pub async fn unlock_media(&self, media: MediaId) -> Result<usize, LibraryError> {
        Ok(changed(
            self.db
                .local_file()
                .set_media_locked(media, FileFlag::Clear)
                .await?,
        ))
    }

    /// Leaves `path` listed but out of matching and episode lists. Returns the
    /// rows changed.
    ///
    /// # Errors
    ///
    /// [`LibraryError::Db`] when the row cannot be updated.
    pub async fn ignore_file(&self, path: &Path) -> Result<usize, LibraryError> {
        Ok(changed(
            self.db
                .local_file()
                .set_ignored(path, FileFlag::Set)
                .await?,
        ))
    }

    /// Lets a scan match `path` again. Returns the rows changed.
    ///
    /// # Errors
    ///
    /// [`LibraryError::Db`] when the row cannot be updated.
    pub async fn unignore_file(&self, path: &Path) -> Result<usize, LibraryError> {
        Ok(changed(
            self.db
                .local_file()
                .set_ignored(path, FileFlag::Clear)
                .await?,
        ))
    }

    /// Assigns files to the AniList media `anilist_id`, minting it if new, and
    /// locks them. An episode number is set only when one is supplied. Returns
    /// the number of stored files that were matched.
    ///
    /// # Errors
    ///
    /// [`LibraryError::Db`] when the media or rows cannot be updated.
    pub async fn manual_match(
        &self,
        paths: &[PathBuf],
        anilist_id: u32,
        episode: Option<u32>,
    ) -> Result<usize, LibraryError> {
        let minted = self
            .db
            .media()
            .find_or_mint(MediaKind::Anime, &[ExternalId::AniList(anilist_id)])
            .await?;
        let mut changed: usize = 0;
        for path in paths {
            let rows = self
                .db
                .local_file()
                .manual_match(path, minted.media, episode)
                .await?;
            changed = changed.saturating_add(usize::try_from(rows).unwrap_or(usize::MAX));
        }
        Ok(changed)
    }

    /// Matches the eligible groups, minting a media for each winner.
    async fn match_groups<P: FnMut(ScanProgress) + Send>(
        &self,
        scanned: &[ScannedFile],
        eligible: &[bool],
        candidates: &[Candidate],
        progress: &mut P,
        mut matched: usize,
    ) -> Result<(HashMap<usize, Match>, HashMap<usize, MediaId>), LibraryError> {
        let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, file) in scanned.iter().enumerate() {
            if eligible.get(index).copied().unwrap_or(false) {
                groups
                    .entry(file.group_key.clone())
                    .or_default()
                    .push(index);
            }
        }
        let mut wins = HashMap::new();
        let mut media = HashMap::new();
        for indexes in groups.values() {
            let Some(first) = indexes.first().copied() else {
                continue;
            };
            let Some(file) = scanned.get(first) else {
                continue;
            };
            let title = file_title(file);
            let Some(found) = self.matcher.best(&title, candidates) else {
                continue;
            };
            let Some(candidate) = candidates.get(found.candidate) else {
                continue;
            };
            if candidate.external_ids.is_empty() {
                return Err(LibraryError::CandidateWithoutExternalIds {
                    title: candidate.title.clone(),
                });
            }
            let minted = self
                .db
                .media()
                .find_or_mint(MediaKind::Anime, &candidate.external_ids)
                .await?;
            for index in indexes {
                wins.insert(*index, found);
                media.insert(*index, minted.media);
                matched += 1;
                progress(ScanProgress {
                    files_walked: scanned.len(),
                    files_matched: matched,
                });
            }
        }
        Ok((wins, media))
    }
}

/// A row count from SQLite, saturated at the platform's maximum.
fn changed(rows: u64) -> usize {
    usize::try_from(rows).unwrap_or(usize::MAX)
}

/// Files the matcher may touch: neither overridden nor untitled.
fn eligible_files(scanned: &[ScannedFile], existing: &HashMap<String, LocalFile>) -> Vec<bool> {
    scanned
        .iter()
        .map(|file| {
            let key = file.path.to_string_lossy();
            !existing.contains_key(key.as_ref()) && !file.group_key.is_empty()
        })
        .collect()
}

/// Builds the rows a scan stores, keeping a locked file's match and clearing
/// an ignored file's.
fn build_rows(
    scanned: &[ScannedFile],
    existing: &HashMap<String, LocalFile>,
    preserved: &HashMap<String, LocalFile>,
    wins: &HashMap<usize, Match>,
    media: &HashMap<usize, MediaId>,
    hydrated: &HashMap<usize, HydratedFile>,
) -> Vec<LocalFile> {
    scanned
        .iter()
        .enumerate()
        .map(|(index, file)| {
            let key = file.path.to_string_lossy().into_owned();
            let prior = preserved.get(&key).or_else(|| existing.get(&key));
            let kept = preserved.contains_key(&key);
            let locked = prior.is_some_and(|row| row.locked);
            let ignored = prior.is_some_and(|row| row.ignored);
            let found = wins.get(&index).copied();
            let fresh = hydrated.get(&index);
            let parsed_kind = || scanner::kind_name(file.release.kind).to_owned();
            let unmatched = fresh.is_some_and(|hydrated| hydrated.unmatched);
            let media_id = if ignored || unmatched {
                None
            } else {
                media.get(&index).copied().or_else(|| {
                    if locked || kept {
                        prior.and_then(|row| row.media_id)
                    } else {
                        None
                    }
                })
            };
            let (season, episode, kind, filler) = if locked || kept {
                (
                    prior.and_then(|row| row.season),
                    prior.and_then(|row| row.episode),
                    prior.map_or_else(parsed_kind, |row| row.kind.clone()),
                    prior.is_some_and(|row| row.filler),
                )
            } else if ignored {
                (None, None, parsed_kind(), false)
            } else if let Some(hydrated) = fresh {
                (
                    hydrated.season,
                    hydrated.episode,
                    hydrated.kind.clone(),
                    hydrated.filler,
                )
            } else {
                (
                    file.release.season,
                    file.release.episode,
                    parsed_kind(),
                    false,
                )
            };
            LocalFile {
                path: file.path.clone(),
                size: file.size,
                modified: file.modified,
                title: file.release.title.clone(),
                group: file.release.group.clone(),
                season,
                episode,
                kind,
                media_id,
                confidence: if ignored || unmatched {
                    None
                } else if locked || kept {
                    prior.and_then(|row| row.confidence)
                } else {
                    found.map(|matched| matched.confidence)
                },
                locked,
                ignored,
                filler,
                shelved: prior.is_some_and(|row| row.shelved),
            }
        })
        .collect()
}

/// The summary recorded for one scan.
fn summarize(
    matcher: Matcher,
    scanned: &[ScannedFile],
    rows: &[LocalFile],
    folders: &[PathBuf],
    started_at: SystemTime,
    duration: Duration,
    candidates: &[Candidate],
) -> ScanSummary {
    let mut by_media: HashMap<MediaId, MediaMatchSummary> = HashMap::new();
    let mut locked = 0;
    let mut ignored = 0;
    let mut unmatched = Vec::new();
    let parsed: HashMap<String, &ScannedFile> = scanned
        .iter()
        .map(|file| (file.path.to_string_lossy().into_owned(), file))
        .collect();
    for row in rows {
        if row.locked {
            locked += 1;
        }
        if row.ignored {
            ignored += 1;
        }
        if let (Some(media), Some(confidence)) = (row.media_id, row.confidence) {
            let entry = by_media.entry(media).or_insert(MediaMatchSummary {
                media,
                files: 0,
                lowest_confidence: confidence,
            });
            entry.files += 1;
            entry.lowest_confidence = entry.lowest_confidence.min(confidence);
            continue;
        }
        let reason = if row.ignored {
            UnmatchedReason::Ignored
        } else {
            let key = row.path.to_string_lossy();
            parsed
                .get(key.as_ref())
                .map_or(UnmatchedReason::NoTitle, |file| {
                    unmatched_reason(matcher, file, candidates)
                })
        };
        unmatched.push(UnmatchedFile {
            path: row.path.clone(),
            reason,
        });
    }
    let mut by_media: Vec<MediaMatchSummary> = by_media.into_values().collect();
    by_media.sort_by_key(|entry| entry.media.get());
    ScanSummary {
        started_at,
        duration,
        folders: folders.to_vec(),
        files_walked: scanned.len(),
        files_matched: rows.iter().filter(|row| row.media_id.is_some()).count(),
        files_unmatched: unmatched.len(),
        locked,
        ignored,
        shelved: rows.iter().filter(|row| row.shelved).count(),
        by_media,
        unmatched,
    }
}

/// The title a file is matched with. The parser moves a season marker
/// (`S2`, `Season 2`) out of the title; it is put back so the matcher can
/// prefer the candidate of that season.
fn file_title(file: &ScannedFile) -> NormalizedTitle {
    let mut title = normalize(file.release.title.as_deref().unwrap_or(&file.group_key));
    title.season = title.season.or(file.release.season);
    title
}

/// Why a scan left `file` without a media.
fn unmatched_reason(
    matcher: Matcher,
    file: &ScannedFile,
    candidates: &[Candidate],
) -> UnmatchedReason {
    if file.group_key.is_empty() {
        return UnmatchedReason::NoTitle;
    }
    let title = file_title(file);
    match matcher.best_candidate(&title, candidates) {
        Some(best) => UnmatchedReason::BelowThreshold {
            best_score: best.confidence,
            best_candidate: candidates
                .get(best.candidate)
                .map_or_else(String::new, |candidate| candidate.title.clone()),
        },
        None => UnmatchedReason::NoCandidate,
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use domain::ExternalId;
    use release_parser::Release;

    use super::{Candidate, Matcher, ScannedFile, UnmatchedReason, unmatched_reason};

    #[test]
    fn a_file_without_a_title_reports_that_reason() {
        let file = ScannedFile {
            path: PathBuf::from("- 01.mkv"),
            size: 0,
            modified: 0,
            release: Release::default(),
            group_key: String::new(),
        };
        let candidates = [Candidate::new("Series Alpha", vec![ExternalId::AniList(1)])];

        assert_eq!(
            unmatched_reason(Matcher::default(), &file, &candidates),
            UnmatchedReason::NoTitle
        );
    }
}
