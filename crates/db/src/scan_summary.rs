//! Persisted outcomes of library scans.
//!
//! Only the newest [`KEEP`] summaries are kept. Each one records its folders,
//! timing, per-media counts and every file left without a media, with the
//! reason it was left there.

use std::{
    path::PathBuf,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use domain::MediaId;
use sqlx::{FromRow, SqlitePool};

use crate::DbError;

/// How many scan summaries are kept.
const KEEP: i64 = 20;

/// Why a file was left without a media by a scan.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum UnmatchedReason {
    /// No title could be parsed from the name or its parent folders.
    NoTitle,
    /// No candidate was supplied, or none scored above zero.
    NoCandidate,
    /// The best candidate scored at or below the threshold.
    BelowThreshold {
        /// The best candidate's score.
        best_score: f64,
        /// The best candidate's title.
        best_candidate: String,
    },
    /// The user marked the file ignored.
    Ignored,
    /// The drive holding the file is disconnected.
    Shelved,
}

impl UnmatchedReason {
    /// The stored form of this reason.
    fn code(&self) -> &'static str {
        match self {
            Self::NoTitle => "no_title",
            Self::NoCandidate => "no_candidate",
            Self::BelowThreshold { .. } => "below_threshold",
            Self::Ignored => "ignored",
            Self::Shelved => "shelved",
        }
    }

    fn best_score(&self) -> Option<f64> {
        match self {
            Self::BelowThreshold { best_score, .. } => Some(*best_score),
            _ => None,
        }
    }

    fn best_candidate(&self) -> Option<&str> {
        match self {
            Self::BelowThreshold { best_candidate, .. } => Some(best_candidate),
            _ => None,
        }
    }
}

/// One file a scan left without a media.
#[derive(Debug, Clone, PartialEq)]
pub struct UnmatchedFile {
    /// Where the file lives.
    pub path: PathBuf,
    /// Why it has no media.
    pub reason: UnmatchedReason,
}

/// A media's matched files in one scan.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MediaMatchSummary {
    /// The media the files matched.
    pub media: MediaId,
    /// How many files matched it.
    pub files: usize,
    /// The lowest confidence among those files.
    pub lowest_confidence: f64,
}

/// The outcome of one library scan.
#[derive(Debug, Clone, PartialEq)]
pub struct ScanSummary {
    /// When the scan started.
    pub started_at: SystemTime,
    /// How long the scan took.
    pub duration: Duration,
    /// The folders the scan walked.
    pub folders: Vec<PathBuf>,
    /// Files found by the walk.
    pub files_walked: usize,
    /// Files matched to a media.
    pub files_matched: usize,
    /// Files left without a media.
    pub files_unmatched: usize,
    /// Locked files seen by the scan.
    pub locked: usize,
    /// Ignored files seen by the scan.
    pub ignored: usize,
    /// Shelved files seen by the scan.
    pub shelved: usize,
    /// Matched files grouped by media.
    pub by_media: Vec<MediaMatchSummary>,
    /// Files the scan left without a media, and why.
    pub unmatched: Vec<UnmatchedFile>,
}

/// Storage for scan summaries.
#[derive(Debug, Clone, Copy)]
pub struct ScanSummaryRepo<'a> {
    pool: &'a SqlitePool,
}

impl<'a> ScanSummaryRepo<'a> {
    pub(crate) fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Stores `summary` and evicts everything but the newest [`KEEP`].
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when a query fails, or [`DbError::InvalidRow`] when a
    /// value cannot be stored.
    pub async fn record(&self, summary: &ScanSummary) -> Result<(), DbError> {
        let started = summary
            .started_at
            .duration_since(UNIX_EPOCH)
            .map_or(0, |since| since.as_secs());
        let duration = i64::try_from(summary.duration.as_millis()).unwrap_or(i64::MAX);
        let folders = serde_json::to_string(&summary.folders)
            .map_err(|_| DbError::InvalidRow("scan_summary.folders"))?;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let id = sqlx::query(
            "INSERT INTO scan_summary (
                started_at, duration_ms, folders, files_walked, files_matched,
                files_unmatched, locked_count, ignored_count, shelved_count
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?)",
        )
        .bind(i64::try_from(started).unwrap_or(i64::MAX))
        .bind(duration)
        .bind(folders)
        .bind(number(summary.files_walked))
        .bind(number(summary.files_matched))
        .bind(number(summary.files_unmatched))
        .bind(number(summary.locked))
        .bind(number(summary.ignored))
        .bind(number(summary.shelved))
        .execute(&mut *tx)
        .await?
        .last_insert_rowid();
        for entry in &summary.by_media {
            sqlx::query(
                "INSERT INTO scan_summary_media
                    (summary_id, media_id, matched_files, lowest_confidence)
                 VALUES (?, ?, ?, ?)",
            )
            .bind(id)
            .bind(entry.media.get())
            .bind(number(entry.files))
            .bind(entry.lowest_confidence)
            .execute(&mut *tx)
            .await?;
        }
        for file in &summary.unmatched {
            sqlx::query(
                "INSERT INTO scan_summary_unmatched
                    (summary_id, path, reason, best_score, best_candidate)
                 VALUES (?, ?, ?, ?, ?)",
            )
            .bind(id)
            .bind(file.path.to_string_lossy())
            .bind(file.reason.code())
            .bind(file.reason.best_score())
            .bind(file.reason.best_candidate())
            .execute(&mut *tx)
            .await?;
        }
        sqlx::query("DELETE FROM scan_summary WHERE id NOT IN (SELECT id FROM scan_summary ORDER BY id DESC LIMIT ?)")
            .bind(KEEP)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    /// The newest stored summary, if any.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when a query fails, or [`DbError::InvalidRow`] when a
    /// stored value does not fit its type.
    pub async fn latest(&self) -> Result<Option<ScanSummary>, DbError> {
        let Some(row): Option<SummaryRow> =
            sqlx::query_as("SELECT * FROM scan_summary ORDER BY id DESC LIMIT 1")
                .fetch_optional(self.pool)
                .await?
        else {
            return Ok(None);
        };
        self.decode(row).await.map(Some)
    }

    /// Every stored summary, newest first. The Scan Summaries screen reads
    /// these in order without having to know the storage ids.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when a query fails, or [`DbError::InvalidRow`] when a
    /// stored value does not fit its type.
    pub async fn all(&self) -> Result<Vec<ScanSummary>, DbError> {
        let rows: Vec<SummaryRow> = sqlx::query_as("SELECT * FROM scan_summary ORDER BY id DESC")
            .fetch_all(self.pool)
            .await?;
        let mut summaries = Vec::with_capacity(rows.len());
        for row in rows {
            summaries.push(self.decode(row).await?);
        }
        Ok(summaries)
    }

    async fn decode(&self, row: SummaryRow) -> Result<ScanSummary, DbError> {
        let media: Vec<MediaRow> = sqlx::query_as(
            "SELECT media_id, matched_files, lowest_confidence
             FROM scan_summary_media WHERE summary_id = ? ORDER BY media_id",
        )
        .bind(row.id)
        .fetch_all(self.pool)
        .await?;
        let unmatched: Vec<UnmatchedRow> = sqlx::query_as(
            "SELECT path, reason, best_score, best_candidate
             FROM scan_summary_unmatched WHERE summary_id = ? ORDER BY path",
        )
        .bind(row.id)
        .fetch_all(self.pool)
        .await?;
        Ok(ScanSummary {
            started_at: UNIX_EPOCH
                .checked_add(Duration::from_secs(
                    u64::try_from(row.started_at).map_err(|_| DbError::InvalidRow("started_at"))?,
                ))
                .ok_or(DbError::InvalidRow("started_at"))?,
            duration: Duration::from_millis(
                u64::try_from(row.duration_ms).map_err(|_| DbError::InvalidRow("duration_ms"))?,
            ),
            folders: serde_json::from_str(&row.folders)
                .map_err(|_| DbError::InvalidRow("scan_summary.folders"))?,
            files_walked: count(row.files_walked, "files_walked")?,
            files_matched: count(row.files_matched, "files_matched")?,
            files_unmatched: count(row.files_unmatched, "files_unmatched")?,
            locked: count(row.locked_count, "locked_count")?,
            ignored: count(row.ignored_count, "ignored_count")?,
            shelved: count(row.shelved_count, "shelved_count")?,
            by_media: media
                .into_iter()
                .map(|entry| {
                    Ok(MediaMatchSummary {
                        media: MediaId::new(entry.media_id),
                        files: count(entry.matched_files, "matched_files")?,
                        lowest_confidence: entry.lowest_confidence,
                    })
                })
                .collect::<Result<_, DbError>>()?,
            unmatched: unmatched
                .into_iter()
                .map(|file| {
                    let reason = reason(&file)?;
                    Ok(UnmatchedFile {
                        path: PathBuf::from(file.path),
                        reason,
                    })
                })
                .collect::<Result<_, DbError>>()?,
        })
    }

    /// How many summaries are stored.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when the query fails.
    pub async fn count(&self) -> Result<usize, DbError> {
        let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM scan_summary")
            .fetch_one(self.pool)
            .await?;
        Ok(usize::try_from(count).unwrap_or(usize::MAX))
    }
}

fn number(value: usize) -> i64 {
    i64::try_from(value).unwrap_or(i64::MAX)
}

fn count(value: i64, column: &'static str) -> Result<usize, DbError> {
    usize::try_from(value).map_err(|_| DbError::InvalidRow(column))
}

fn reason(row: &UnmatchedRow) -> Result<UnmatchedReason, DbError> {
    match row.reason.as_str() {
        "no_title" => Ok(UnmatchedReason::NoTitle),
        "no_candidate" => Ok(UnmatchedReason::NoCandidate),
        "below_threshold" => Ok(UnmatchedReason::BelowThreshold {
            best_score: row.best_score.ok_or(DbError::InvalidRow("best_score"))?,
            best_candidate: row
                .best_candidate
                .as_ref()
                .ok_or(DbError::InvalidRow("best_candidate"))?
                .clone(),
        }),
        "ignored" => Ok(UnmatchedReason::Ignored),
        "shelved" => Ok(UnmatchedReason::Shelved),
        _ => Err(DbError::InvalidRow("reason")),
    }
}

#[derive(FromRow)]
struct SummaryRow {
    id: i64,
    started_at: i64,
    duration_ms: i64,
    folders: String,
    files_walked: i64,
    files_matched: i64,
    files_unmatched: i64,
    locked_count: i64,
    ignored_count: i64,
    shelved_count: i64,
}

#[derive(FromRow)]
struct MediaRow {
    media_id: i64,
    matched_files: i64,
    lowest_confidence: f64,
}

#[derive(FromRow)]
struct UnmatchedRow {
    path: String,
    reason: String,
    best_score: Option<f64>,
    best_candidate: Option<String>,
}
