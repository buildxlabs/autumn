use sqlx::{SqlitePool, query, query_scalar};

use crate::DbError;

/// Resume points for one-shot list transfers. The JSON is owned by the
/// tracker crate; this repository only stores it under a direction key.
#[derive(Debug, Clone, Copy)]
pub struct TransferRepo<'a> {
    pool: &'a SqlitePool,
}

impl<'a> TransferRepo<'a> {
    pub(crate) fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// The stored JSON for `direction`, if a transfer was left unfinished.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when the query fails.
    pub async fn load(&self, direction: &str) -> Result<Option<String>, DbError> {
        Ok(
            query_scalar("SELECT value FROM list_transfer WHERE direction = ?")
                .bind(direction)
                .fetch_optional(self.pool)
                .await?,
        )
    }

    /// Stores `json` as the resume point for `direction`, replacing any older
    /// one in one statement.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when the value is not valid JSON or the write fails.
    pub async fn save(&self, direction: &str, json: &str) -> Result<(), DbError> {
        query("INSERT OR REPLACE INTO list_transfer (direction, value) VALUES (?, ?)")
            .bind(direction)
            .bind(json)
            .execute(self.pool)
            .await?;
        Ok(())
    }

    /// Drops the resume point, so the next transfer starts fresh.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when the delete fails.
    pub async fn clear(&self, direction: &str) -> Result<(), DbError> {
        query("DELETE FROM list_transfer WHERE direction = ?")
            .bind(direction)
            .execute(self.pool)
            .await?;
        Ok(())
    }
}
