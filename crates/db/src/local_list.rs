use std::{
    collections::HashMap,
    time::{Duration, SystemTime},
};

use domain::{AdvancedScores, FuzzyDate, ListEntry, ListStatus, MediaId, MediaKind, Score};
use sqlx::{FromRow, SqliteConnection, SqlitePool};

use crate::{DbError, media::kind_column};

/// The Local platform's list entries (decision #16, ADR 0004): one per media,
/// with AniList's full field set and custom-list membership.
#[derive(Debug, Clone, Copy)]
pub struct LocalListRepo<'a> {
    pool: &'a SqlitePool,
}

impl<'a> LocalListRepo<'a> {
    pub(crate) fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// The entry for `media`, if it is in the list.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when a query fails, or [`DbError::InvalidRow`].
    pub async fn entry(&self, media: MediaId) -> Result<Option<ListEntry>, DbError> {
        let mut conn = self.pool.acquire().await?;
        entry_in(&mut conn, media).await
    }

    /// Every entry for `kind` media, most recently updated first.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when a query fails, [`DbError::UnsupportedKind`], or
    /// [`DbError::InvalidRow`].
    pub async fn entries(&self, kind: MediaKind) -> Result<Vec<ListEntry>, DbError> {
        let kind = kind_column(kind)?;
        let mut conn = self.pool.acquire().await?;
        let rows: Vec<EntryRow> = sqlx::query_as(
            "SELECT e.* FROM local_list_entry e
             JOIN media m ON m.id = e.media_id WHERE m.kind = ?
             ORDER BY e.updated_at DESC, e.media_id",
        )
        .bind(kind)
        .fetch_all(&mut *conn)
        .await?;
        let memberships: Vec<(i64, String)> = sqlx::query_as(
            "SELECT c.media_id, c.name FROM local_custom_list_entry c
             JOIN media m ON m.id = c.media_id WHERE m.kind = ? ORDER BY c.name",
        )
        .bind(kind)
        .fetch_all(&mut *conn)
        .await?;
        let mut custom_lists: HashMap<i64, Vec<String>> = HashMap::new();
        for (media, name) in memberships {
            custom_lists.entry(media).or_default().push(name);
        }
        rows.into_iter()
            .map(|row| {
                let lists = custom_lists.remove(&row.media_id).unwrap_or_default();
                row.into_entry(lists)
            })
            .collect()
    }

    /// Adds or replaces the entry for `entry.media`, custom lists included,
    /// and returns it as stored.
    ///
    /// The timestamps in `entry` are ignored: `created_at` is set when the
    /// entry is first saved and kept after, and `updated_at` is set on every
    /// save. Both have one-second precision.
    ///
    /// # Errors
    ///
    /// [`DbError::UnknownMedia`], [`DbError::UnsupportedStatus`],
    /// [`DbError::Sqlx`] when a query fails, or [`DbError::InvalidRow`].
    pub async fn save(&self, entry: &ListEntry) -> Result<ListEntry, DbError> {
        let media = entry.media;
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        ensure_media(&mut tx, media).await?;
        let started = date_columns(entry.started_at);
        let completed = date_columns(entry.completed_at);
        let scores = entry.advanced_scores;
        sqlx::query(
            "INSERT INTO local_list_entry (
                media_id, status, score, progress, progress_volumes, repeat,
                started_year, started_month, started_day,
                completed_year, completed_month, completed_day,
                notes, private, priority,
                score_story, score_characters, score_visuals, score_audio, score_enjoyment,
                hidden_from_status_lists, created_at, updated_at
             ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                       unixepoch(), unixepoch())
             ON CONFLICT (media_id) DO UPDATE SET
                status = excluded.status,
                score = excluded.score,
                progress = excluded.progress,
                progress_volumes = excluded.progress_volumes,
                repeat = excluded.repeat,
                started_year = excluded.started_year,
                started_month = excluded.started_month,
                started_day = excluded.started_day,
                completed_year = excluded.completed_year,
                completed_month = excluded.completed_month,
                completed_day = excluded.completed_day,
                notes = excluded.notes,
                private = excluded.private,
                priority = excluded.priority,
                score_story = excluded.score_story,
                score_characters = excluded.score_characters,
                score_visuals = excluded.score_visuals,
                score_audio = excluded.score_audio,
                score_enjoyment = excluded.score_enjoyment,
                hidden_from_status_lists = excluded.hidden_from_status_lists,
                updated_at = excluded.updated_at",
        )
        .bind(media.get())
        .bind(status_column(entry.status)?)
        .bind(entry.score.raw())
        .bind(entry.progress)
        .bind(entry.progress_volumes)
        .bind(entry.repeat)
        .bind(started.0)
        .bind(started.1)
        .bind(started.2)
        .bind(completed.0)
        .bind(completed.1)
        .bind(completed.2)
        .bind(&entry.notes)
        .bind(entry.private)
        .bind(entry.priority)
        .bind(scores.story.raw())
        .bind(scores.characters.raw())
        .bind(scores.visuals.raw())
        .bind(scores.audio.raw())
        .bind(scores.enjoyment.raw())
        .bind(entry.hidden_from_status_lists)
        .execute(&mut *tx)
        .await?;
        sqlx::query("DELETE FROM local_custom_list_entry WHERE media_id = ?")
            .bind(media.get())
            .execute(&mut *tx)
            .await?;
        for name in &entry.custom_lists {
            sqlx::query(
                "INSERT OR IGNORE INTO local_custom_list_entry (media_id, name) VALUES (?, ?)",
            )
            .bind(media.get())
            .bind(name)
            .execute(&mut *tx)
            .await?;
        }
        let saved = entry_in(&mut tx, media)
            .await?
            .ok_or(DbError::UnknownMedia(media))?;
        tx.commit().await?;
        Ok(saved)
    }

    /// Sets the progress of `media`'s entry and returns the entry, in one
    /// transaction. A media without an entry gets a new Current entry with
    /// nothing else set.
    ///
    /// # Errors
    ///
    /// [`DbError::UnknownMedia`], [`DbError::Sqlx`] when a query fails, or
    /// [`DbError::InvalidRow`].
    pub async fn set_progress(&self, media: MediaId, progress: u32) -> Result<ListEntry, DbError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        ensure_media(&mut tx, media).await?;
        sqlx::query(
            "INSERT INTO local_list_entry (
                media_id, status, score, progress, progress_volumes, repeat, notes, private,
                priority, score_story, score_characters, score_visuals, score_audio,
                score_enjoyment, hidden_from_status_lists, created_at, updated_at
             ) VALUES (?, 'current', 0, ?, 0, 0, '', 0, 0, 0, 0, 0, 0, 0, 0,
                       unixepoch(), unixepoch())
             ON CONFLICT (media_id) DO UPDATE SET
                progress = excluded.progress,
                updated_at = excluded.updated_at",
        )
        .bind(media.get())
        .bind(progress)
        .execute(&mut *tx)
        .await?;
        let saved = entry_in(&mut tx, media)
            .await?
            .ok_or(DbError::UnknownMedia(media))?;
        tx.commit().await?;
        Ok(saved)
    }

    /// Removes the entry for `media` and its custom-list membership. Returns
    /// whether there was one.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when the query fails.
    pub async fn delete(&self, media: MediaId) -> Result<bool, DbError> {
        let done = sqlx::query("DELETE FROM local_list_entry WHERE media_id = ?")
            .bind(media.get())
            .execute(self.pool)
            .await?;
        Ok(done.rows_affected() > 0)
    }
}

async fn ensure_media(conn: &mut SqliteConnection, media: MediaId) -> Result<(), DbError> {
    let known: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM media WHERE id = ?)")
        .bind(media.get())
        .fetch_one(conn)
        .await?;
    if known {
        Ok(())
    } else {
        Err(DbError::UnknownMedia(media))
    }
}

async fn entry_in(
    conn: &mut SqliteConnection,
    media: MediaId,
) -> Result<Option<ListEntry>, DbError> {
    let row: Option<EntryRow> = sqlx::query_as("SELECT * FROM local_list_entry WHERE media_id = ?")
        .bind(media.get())
        .fetch_optional(&mut *conn)
        .await?;
    let Some(row) = row else {
        return Ok(None);
    };
    let lists = sqlx::query_scalar(
        "SELECT name FROM local_custom_list_entry WHERE media_id = ? ORDER BY name",
    )
    .bind(media.get())
    .fetch_all(&mut *conn)
    .await?;
    row.into_entry(lists).map(Some)
}

/// A `local_list_entry` row as SQLite returns it, checked by
/// [`EntryRow::into_entry`].
#[derive(FromRow)]
struct EntryRow {
    media_id: i64,
    status: String,
    score: i64,
    progress: i64,
    progress_volumes: i64,
    repeat: i64,
    started_year: Option<i64>,
    started_month: Option<i64>,
    started_day: Option<i64>,
    completed_year: Option<i64>,
    completed_month: Option<i64>,
    completed_day: Option<i64>,
    notes: String,
    private: bool,
    priority: i64,
    score_story: i64,
    score_characters: i64,
    score_visuals: i64,
    score_audio: i64,
    score_enjoyment: i64,
    hidden_from_status_lists: bool,
    created_at: i64,
    updated_at: i64,
}

impl EntryRow {
    fn into_entry(self, custom_lists: Vec<String>) -> Result<ListEntry, DbError> {
        Ok(ListEntry {
            media: MediaId::new(self.media_id),
            status: status_from_column(&self.status)?,
            score: score(self.score, "score")?,
            progress: int(self.progress, "progress")?,
            progress_volumes: int(self.progress_volumes, "progress_volumes")?,
            repeat: int(self.repeat, "repeat")?,
            started_at: date(
                (self.started_year, self.started_month, self.started_day),
                "started_year, started_month, started_day",
            )?,
            completed_at: date(
                (
                    self.completed_year,
                    self.completed_month,
                    self.completed_day,
                ),
                "completed_year, completed_month, completed_day",
            )?,
            notes: self.notes,
            private: self.private,
            priority: int(self.priority, "priority")?,
            advanced_scores: AdvancedScores {
                story: score(self.score_story, "score_story")?,
                characters: score(self.score_characters, "score_characters")?,
                visuals: score(self.score_visuals, "score_visuals")?,
                audio: score(self.score_audio, "score_audio")?,
                enjoyment: score(self.score_enjoyment, "score_enjoyment")?,
            },
            custom_lists,
            hidden_from_status_lists: self.hidden_from_status_lists,
            created_at: time(self.created_at, "created_at")?,
            updated_at: time(self.updated_at, "updated_at")?,
        })
    }
}

fn int<T: TryFrom<i64>>(value: i64, column: &'static str) -> Result<T, DbError> {
    T::try_from(value).map_err(|_| DbError::InvalidRow(column))
}

fn score(value: i64, column: &'static str) -> Result<Score, DbError> {
    Score::new(int(value, column)?).map_err(|_| DbError::InvalidRow(column))
}

fn date(
    (year, month, day): (Option<i64>, Option<i64>, Option<i64>),
    column: &'static str,
) -> Result<FuzzyDate, DbError> {
    let year = year.map(|year| int(year, column)).transpose()?;
    let month = month.map(|month| int(month, column)).transpose()?;
    let day = day.map(|day| int(day, column)).transpose()?;
    FuzzyDate::new(year, month, day).map_err(|_| DbError::InvalidRow(column))
}

fn time(secs: i64, column: &'static str) -> Result<SystemTime, DbError> {
    let secs = int(secs, column)?;
    SystemTime::UNIX_EPOCH
        .checked_add(Duration::from_secs(secs))
        .ok_or(DbError::InvalidRow(column))
}

fn date_columns(date: FuzzyDate) -> (Option<u16>, Option<u8>, Option<u8>) {
    (date.year(), date.month(), date.day())
}

/// The `status` column value.
fn status_column(status: ListStatus) -> Result<&'static str, DbError> {
    match status {
        ListStatus::Current => Ok("current"),
        ListStatus::Planning => Ok("planning"),
        ListStatus::Completed => Ok("completed"),
        ListStatus::Dropped => Ok("dropped"),
        ListStatus::Paused => Ok("paused"),
        ListStatus::Repeating => Ok("repeating"),
        _ => Err(DbError::UnsupportedStatus(status)),
    }
}

fn status_from_column(column: &str) -> Result<ListStatus, DbError> {
    match column {
        "current" => Ok(ListStatus::Current),
        "planning" => Ok(ListStatus::Planning),
        "completed" => Ok(ListStatus::Completed),
        "dropped" => Ok(ListStatus::Dropped),
        "paused" => Ok(ListStatus::Paused),
        "repeating" => Ok(ListStatus::Repeating),
        _ => Err(DbError::InvalidRow("status")),
    }
}
