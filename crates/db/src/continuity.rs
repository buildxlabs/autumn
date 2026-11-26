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

/// One saved resume position, with the media and episode it belongs to.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResumePoint {
    /// The media the episode belongs to.
    pub media: MediaId,
    /// The episode's number in its main run.
    pub episode: u32,
    /// Seconds from the start of the file.
    pub position: f64,
    /// The file's duration, when the player reported one.
    pub duration: Option<f64>,
    /// When the position was saved, in seconds since the Unix epoch.
    pub updated_at: i64,
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

    /// Every saved position, newest first, so a Home widget can find the
    /// episode a user was watching without one query per media.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when the query fails, or [`DbError::InvalidRow`] when
    /// a stored value is not a valid position.
    pub async fn all(&self) -> Result<Vec<ResumePoint>, DbError> {
        let rows: Vec<AllRow> = sqlx::query_as(
            "SELECT media_id, episode, position, duration, updated_at
                            FROM continuity ORDER BY updated_at DESC",
        )
        .fetch_all(self.pool)
        .await?;
        rows.into_iter()
            .map(|row| {
                Ok(ResumePoint {
                    media: MediaId::new(row.media_id),
                    episode: u32::try_from(row.episode)
                        .map_err(|_| DbError::InvalidRow("continuity.episode"))?,
                    position: check(row.position, "continuity.position")?,
                    duration: row
                        .duration
                        .map(|value| check(value, "continuity.duration"))
                        .transpose()?,
                    updated_at: row.updated_at,
                })
            })
            .collect()
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
    check(value, column).map(|_| ())
}

fn check(value: f64, column: &'static str) -> Result<f64, DbError> {
    if value.is_finite() && value >= 0.0 {
        Ok(value)
    } else {
        Err(DbError::InvalidRow(column))
    }
}

#[derive(FromRow)]
struct AllRow {
    media_id: i64,
    episode: i64,
    position: f64,
    duration: Option<f64>,
    updated_at: i64,
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

#[cfg(test)]
mod tests {
    use domain::{ExternalId, MediaKind};
    use tempfile::tempdir;

    use crate::Db;

    #[tokio::test]
    async fn all_lists_every_saved_position_with_its_media_and_episode() {
        let dir = tempdir().unwrap();
        let db = Db::open(&dir.path().join("continuity.db")).await.unwrap();
        let media = db
            .media()
            .find_or_mint(MediaKind::Anime, &[ExternalId::AniList(42)])
            .await
            .unwrap()
            .media;
        db.continuity()
            .save(media, 3, 120.0, Some(1400.0))
            .await
            .unwrap();
        db.continuity().save(media, 4, 60.0, None).await.unwrap();

        let points = db.continuity().all().await.unwrap();

        assert_eq!(points.len(), 2, "both saved positions are listed");
        assert!(points.iter().all(|point| point.media == media));
        assert!(
            points.iter().any(|point| point.episode == 3
                && (point.position - 120.0).abs() < 0.001
                && point.duration == Some(1400.0)),
            "the resumed episode keeps its position and duration: {points:?}"
        );
        assert!(
            points
                .iter()
                .any(|point| point.episode == 4 && point.duration.is_none()),
            "an episode the player hasn't measured is listed too"
        );
    }
}
