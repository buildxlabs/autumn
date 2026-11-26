use domain::MediaId;
use sqlx::{FromRow, SqlitePool};

use crate::DbError;

/// The last saved position of one episode.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Continuity {
    /// Seconds from the start of the file.
    pub position: f64,
    /// The file's duration, when the player reported one.
    pub duration: Option<f64>,
}

/// Resume positions, keyed by media and episode.
#[derive(Debug, Clone, Copy)]
pub struct ContinuityRepo<'a> {
    pool: &'a SqlitePool,
}

impl<'a> ContinuityRepo<'a> {
    pub(crate) fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// The saved position for `episode`, if it has one.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when the query fails, or [`DbError::InvalidRow`] when
    /// a stored value is not a valid position.
    pub async fn get(&self, media: MediaId, episode: u32) -> Result<Option<Continuity>, DbError> {
        let row: Option<ContinuityRow> = sqlx::query_as(
            "SELECT position, duration FROM continuity WHERE media_id = ? AND episode = ?",
        )
        .bind(media.get())
        .bind(i64::from(episode))
        .fetch_optional(self.pool)
        .await?;
        row.map(ContinuityRow::into_position).transpose()
    }

    /// Every saved position of `media`, by episode number.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when the query fails, or [`DbError::InvalidRow`] when
    /// a stored value is not a valid position or episode.
    pub async fn for_media(&self, media: MediaId) -> Result<Vec<(u32, Continuity)>, DbError> {
        let rows: Vec<EpisodeRow> = sqlx::query_as(
            "SELECT episode, position, duration FROM continuity
             WHERE media_id = ? ORDER BY episode",
        )
        .bind(media.get())
        .fetch_all(self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                let episode = u32::try_from(row.episode)
                    .map_err(|_| DbError::InvalidRow("continuity.episode"))?;
                let position = ContinuityRow {
                    position: row.position,
                    duration: row.duration,
                }
                .into_position()?;
                Ok((episode, position))
            })
            .collect()
    }

    /// Upserts the position of `episode`.
    ///
    /// # Errors
    ///
    /// [`DbError::InvalidRow`] when `position` or `duration` is negative or
    /// not finite, or [`DbError::Sqlx`] when the write fails.
    pub async fn save(
        &self,
        media: MediaId,
        episode: u32,
        position: f64,
        duration: Option<f64>,
    ) -> Result<(), DbError> {
        check_seconds(position, "continuity.position")?;
        duration
            .map(|value| check_seconds(value, "continuity.duration"))
            .transpose()?;
        sqlx::query(
            "INSERT INTO continuity (media_id, episode, position, duration, updated_at)
             VALUES (?, ?, ?, ?, unixepoch())
             ON CONFLICT (media_id, episode) DO UPDATE SET
                position = excluded.position,
                duration = excluded.duration,
                updated_at = excluded.updated_at",
        )
        .bind(media.get())
        .bind(i64::from(episode))
        .bind(position)
        .bind(duration)
        .execute(self.pool)
        .await?;
        Ok(())
    }

    /// Forgets `episode`'s position, so a watched episode never resumes.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when the write fails.
    pub async fn clear(&self, media: MediaId, episode: u32) -> Result<(), DbError> {
        sqlx::query("DELETE FROM continuity WHERE media_id = ? AND episode = ?")
            .bind(media.get())
            .bind(i64::from(episode))
            .execute(self.pool)
            .await?;
        Ok(())
    }
}

fn check_seconds(value: f64, column: &'static str) -> Result<(), DbError> {
    if value.is_finite() && value >= 0.0 {
        Ok(())
    } else {
        Err(DbError::InvalidRow(column))
    }
}

#[derive(FromRow)]
struct ContinuityRow {
    position: f64,
    duration: Option<f64>,
}

#[derive(FromRow)]
struct EpisodeRow {
    episode: i64,
    position: f64,
    duration: Option<f64>,
}

impl ContinuityRow {
    fn into_position(self) -> Result<Continuity, DbError> {
        check_seconds(self.position, "continuity.position")?;
        if let Some(duration) = self.duration {
            check_seconds(duration, "continuity.duration")?;
        }
        Ok(Continuity {
            position: self.position,
            duration: self.duration,
        })
    }
}
