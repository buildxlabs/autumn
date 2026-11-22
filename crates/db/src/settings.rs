use sqlx::{SqliteConnection, SqlitePool};

use crate::DbError;

/// Settings rows: one JSON value per settings tab. The `settings` crate owns
/// what the JSON means; this only stores it.
#[derive(Debug, Clone, Copy)]
pub struct SettingsRepo<'a> {
    pool: &'a SqlitePool,
}

impl<'a> SettingsRepo<'a> {
    pub(crate) fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Every stored `(tab, JSON)` row, sorted by tab. Empty on a fresh
    /// install, which means every tab takes its defaults.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when the query fails.
    pub async fn rows(&self) -> Result<Vec<(String, String)>, DbError> {
        Ok(
            sqlx::query_as("SELECT tab, value FROM settings ORDER BY tab")
                .fetch_all(self.pool)
                .await?,
        )
    }

    /// The one-time import of the Phase 0 `settings.json`: stores `rows` and
    /// marks the import done, in one transaction. Returns `false`, storing
    /// nothing, when the import already ran or settings rows already exist;
    /// the import is marked done then too.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when a value is not valid JSON or the write fails;
    /// nothing is saved then.
    pub async fn import_once(&self, rows: &[(String, String)]) -> Result<bool, DbError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let marked = sqlx::query("INSERT OR IGNORE INTO settings_import (id) VALUES (1)")
            .execute(&mut *tx)
            .await?;
        if marked.rows_affected() == 0 {
            return Ok(false);
        }
        // Rows saved since (say, after the file couldn't be read) are newer
        // than any file: keep them, and still mark the import done.
        let has_rows: bool = sqlx::query_scalar("SELECT EXISTS (SELECT 1 FROM settings)")
            .fetch_one(&mut *tx)
            .await?;
        if !has_rows {
            save_in(&mut tx, rows).await?;
        }
        tx.commit().await?;
        Ok(!has_rows)
    }

    /// Whether the one-time import already ran.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when the query fails.
    pub async fn imported(&self) -> Result<bool, DbError> {
        let done: Option<i64> = sqlx::query_scalar("SELECT id FROM settings_import")
            .fetch_optional(self.pool)
            .await?;
        Ok(done.is_some())
    }

    /// Stores `(tab, JSON)` rows in one transaction, replacing those tabs and
    /// leaving the others alone.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when a value is not valid JSON or the write fails;
    /// nothing is saved then.
    pub async fn save(&self, rows: &[(String, String)]) -> Result<(), DbError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        save_in(&mut tx, rows).await?;
        tx.commit().await?;
        Ok(())
    }
}

async fn save_in(conn: &mut SqliteConnection, rows: &[(String, String)]) -> Result<(), DbError> {
    for (tab, json) in rows {
        sqlx::query("INSERT OR REPLACE INTO settings (tab, value) VALUES (?, ?)")
            .bind(tab)
            .bind(json)
            .execute(&mut *conn)
            .await?;
    }
    Ok(())
}
