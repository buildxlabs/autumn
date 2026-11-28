//! Persisted availability badge results (#251, #181 Q13).
//!
//! A row records one indexer's answer for one (media, episode) pair. `search`
//! applies the 24 h freshness and five-minute recheck rules when the rows are
//! restored; this module only stores and loads them.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sqlx::{FromRow, SqlitePool};

use crate::DbError;

/// The indexer a badge result came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[expect(
    clippy::exhaustive_enums,
    reason = "the two indexers are fixed in Phase 3"
)]
pub enum AvailabilityIndexer {
    /// `Nyaa`.
    Nyaa,
    /// `AnimeTosho`.
    AnimeTosho,
}

impl AvailabilityIndexer {
    /// The stored code for this indexer.
    #[must_use]
    pub const fn code(self) -> &'static str {
        match self {
            Self::Nyaa => "nyaa",
            Self::AnimeTosho => "animetosho",
        }
    }

    /// Reads a stored code, ignoring rows written by another version.
    #[must_use]
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "nyaa" => Some(Self::Nyaa),
            "animetosho" => Some(Self::AnimeTosho),
            _ => None,
        }
    }
}

/// One persisted answer: whether `indexer` had a matching release for
/// `episode` when it was checked at `checked_at`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AvailabilityRecord {
    /// The media, as a raw [`domain::MediaId`] value.
    pub media: i64,
    /// The episode number within the media.
    pub episode: u32,
    /// The indexer that was checked.
    pub indexer: AvailabilityIndexer,
    /// Whether a matching release was found.
    pub found: bool,
    /// When the check finished.
    pub checked_at: SystemTime,
}

/// Storage for badge results.
#[derive(Debug, Clone, Copy)]
pub struct AvailabilityRepo<'a> {
    pool: &'a SqlitePool,
}

#[derive(FromRow)]
struct Row {
    episode: i64,
    indexer: String,
    found: i64,
    checked_at: i64,
}

impl<'a> AvailabilityRepo<'a> {
    pub(crate) fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Every stored result for `media`, newest check first per episode.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when a query fails, or [`DbError::InvalidRow`] when
    /// a stored value does not fit its type.
    pub async fn for_media(&self, media: i64) -> Result<Vec<AvailabilityRecord>, DbError> {
        let rows: Vec<Row> = sqlx::query_as(
            "SELECT episode, indexer, found, checked_at FROM availability
             WHERE media_id = ? ORDER BY episode, indexer",
        )
        .bind(media)
        .fetch_all(self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let indexer = AvailabilityIndexer::from_code(&row.indexer)
                    .ok_or(DbError::InvalidRow("availability.indexer"))?;
                let episode = u32::try_from(row.episode)
                    .map_err(|_| DbError::InvalidRow("availability.episode"))?;
                let seconds = u64::try_from(row.checked_at)
                    .map_err(|_| DbError::InvalidRow("availability.checked_at"))?;
                Ok(AvailabilityRecord {
                    media,
                    episode,
                    indexer,
                    found: row.found != 0,
                    checked_at: UNIX_EPOCH + Duration::from_secs(seconds),
                })
            })
            .collect()
    }

    /// Stores `records`, ignoring an answer older than one already stored for
    /// the same (media, episode, indexer).
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when a query fails.
    pub async fn save(&self, records: &[AvailabilityRecord]) -> Result<(), DbError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        for record in records {
            sqlx::query(
                "INSERT INTO availability (media_id, episode, indexer, found, checked_at)
                 VALUES (?, ?, ?, ?, ?)
                 ON CONFLICT (media_id, episode, indexer) DO UPDATE SET
                     found = excluded.found,
                     checked_at = excluded.checked_at
                 WHERE excluded.checked_at >= availability.checked_at",
            )
            .bind(record.media)
            .bind(i64::from(record.episode))
            .bind(record.indexer.code())
            .bind(i64::from(i32::from(record.found)))
            .bind(seconds(record.checked_at))
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

/// Unix seconds for a check time; times before the epoch are stored as zero.
fn seconds(at: SystemTime) -> i64 {
    let elapsed = at.duration_since(UNIX_EPOCH).unwrap_or_default().as_secs();
    i64::try_from(elapsed).unwrap_or(i64::MAX)
}
