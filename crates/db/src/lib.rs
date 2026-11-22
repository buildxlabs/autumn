//! SQLite storage: the pool, embedded migrations and repositories.
//!
//! See ARCHITECTURE §2 and §6, and ADR 0001 for media identity.

mod error;
mod media;

use std::{path::Path, time::Duration};

use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqliteSynchronous},
};

pub use error::DbError;
pub use media::{LinkConflict, MediaRepo, Minted};

/// How long a write waits for another connection's lock before failing.
const BUSY_TIMEOUT: Duration = Duration::from_secs(5);

/// The app database (`<data>/autumn.db`), in WAL mode, migrated on open.
#[derive(Debug, Clone)]
pub struct Db {
    pool: SqlitePool,
}

impl Db {
    /// Opens or creates the database at `path` and applies any pending
    /// migrations.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when the file can't be opened, or
    /// [`DbError::Migrate`] when a migration fails.
    pub async fn open(path: &Path) -> Result<Self, DbError> {
        let options = SqliteConnectOptions::new()
            .filename(path)
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            // NORMAL is durable across app crashes in WAL mode; only an OS
            // crash can lose the last commits.
            .synchronous(SqliteSynchronous::Normal)
            .foreign_keys(true)
            .busy_timeout(BUSY_TIMEOUT);
        let pool = SqlitePool::connect_with(options).await?;
        sqlx::migrate!().run(&pool).await?;
        Ok(Self { pool })
    }

    /// Media identity and cross-references.
    pub fn media(&self) -> MediaRepo<'_> {
        MediaRepo::new(&self.pool)
    }

    /// SQLite's journal mode for this database, such as `"wal"`.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when the query fails.
    pub async fn journal_mode(&self) -> Result<String, DbError> {
        Ok(sqlx::query_scalar("PRAGMA journal_mode")
            .fetch_one(&self.pool)
            .await?)
    }

    /// Waits for open connections to finish and closes them.
    pub async fn close(self) {
        self.pool.close().await;
    }
}
