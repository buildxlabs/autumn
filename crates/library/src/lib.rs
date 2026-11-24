//! Local library scanning and threshold matching.
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
mod matcher;
mod normalize;
mod scanner;

use std::{collections::HashMap, path::PathBuf};

use db::{Db, LocalFile};
use domain::{MediaId, MediaKind};

pub use error::LibraryError;
pub use matcher::{Candidate, DEFAULT_THRESHOLD, Match, Matcher};
pub use normalize::{NormalizedTitle, normalize};
pub use scanner::{ScannedFile, walk};

/// Files walked and matched so far in one scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScanProgress {
    /// Files found by the walk so far.
    pub files_walked: usize,
    /// Files matched so far.
    pub files_matched: usize,
}

/// The result of one scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub struct ScanSummary {
    /// Files found by the walk.
    pub files_walked: usize,
    /// Files matched to a candidate.
    pub files_matched: usize,
    /// Files stored without a media.
    pub files_unmatched: usize,
}

/// Scans library folders and persists what it finds.
#[derive(Debug, Clone)]
pub struct Library {
    db: Db,
    matcher: Matcher,
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
        }
    }

    /// Walks `folders`, matches their titles against `candidates`, and stores
    /// one row per file. New rows are added, changed rows updated in place and
    /// rows for files that are gone are deleted.
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
        let mut scanned = Vec::new();
        let mut matched = 0;
        for folder in folders {
            for file in walk(folder)? {
                progress(ScanProgress {
                    files_walked: scanned.len() + 1,
                    files_matched: matched,
                });
                scanned.push(file);
            }
        }

        let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
        for (index, file) in scanned.iter().enumerate() {
            groups
                .entry(file.group_key.clone())
                .or_default()
                .push(index);
        }

        let mut wins: HashMap<usize, Match> = HashMap::new();
        let mut media: HashMap<usize, MediaId> = HashMap::new();
        for indexes in groups.values() {
            let Some(first) = indexes.first().copied() else {
                continue;
            };
            let Some(file) = scanned.get(first) else {
                continue;
            };
            // A file with no title in its name or any parent folder must stay
            // unmatched, not bind to the first candidate by coincidence.
            if file.group_key.is_empty() {
                continue;
            }
            let title = normalize(file.release.title.as_deref().unwrap_or(&file.group_key));
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

        let rows: Vec<LocalFile> = scanned
            .iter()
            .enumerate()
            .map(|(index, file)| {
                let found = wins.get(&index).copied();
                LocalFile {
                    path: file.path.clone(),
                    size: file.size,
                    modified: file.modified,
                    title: file.release.title.clone(),
                    group: file.release.group.clone(),
                    season: file.release.season,
                    episode: file.release.episode,
                    kind: scanner::kind_name(file.release.kind).to_owned(),
                    media_id: media.get(&index).copied(),
                    confidence: found.map(|matched| matched.confidence),
                    locked: false,
                    ignored: false,
                }
            })
            .collect();
        self.db.local_file().sync(&rows).await?;

        Ok(ScanSummary {
            files_walked: scanned.len(),
            files_matched: matched,
            files_unmatched: scanned.len() - matched,
        })
    }
}
