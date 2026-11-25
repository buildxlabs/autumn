use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

use domain::MediaId;
use sqlx::{FromRow, SqlitePool};

use crate::DbError;

/// One local file as a library scan found it.
///
/// `kind` is one of `episode`, `special`, `movie`, `ncop`, `nced` or `extra`.
/// `locked` and `ignored` are user overrides; a scan keeps them. `filler` comes
/// from the metadata lookup and is refreshed on each scan.
#[derive(Debug, Clone, PartialEq)]
#[expect(
    clippy::struct_excessive_bools,
    reason = "a local file's lock, ignore, filler and shelved flags are independent columns"
)]
pub struct LocalFile {
    /// The file's path, which identifies the row across scans.
    pub path: PathBuf,
    /// Size in bytes.
    pub size: i64,
    /// Modification time in seconds since the Unix epoch.
    pub modified: i64,
    /// The parsed title, if the name gave one.
    pub title: Option<String>,
    /// The parsed release group, if the name gave one.
    pub group: Option<String>,
    /// The parsed season number.
    pub season: Option<u32>,
    /// The parsed episode number within its kind.
    pub episode: Option<u32>,
    /// The parsed episode kind.
    pub kind: String,
    /// The media this file was matched to, absent when unmatched.
    pub media_id: Option<MediaId>,
    /// The match confidence, absent when unmatched.
    pub confidence: Option<f64>,
    /// Whether the file's match is fixed.
    pub locked: bool,
    /// Whether the file is deliberately excluded from the library.
    pub ignored: bool,
    /// Whether this numbered episode is listed as filler.
    pub filler: bool,
    /// Whether the file's drive is disconnected.
    pub shelved: bool,
}

/// Whether a library file flag is being set or cleared.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[non_exhaustive]
pub enum FileFlag {
    /// Set the flag.
    Set,
    /// Clear the flag.
    Clear,
}

impl FileFlag {
    fn bit(self) -> i64 {
        match self {
            Self::Set => 1,
            Self::Clear => 0,
        }
    }
}

/// Storage for the files found by library scans.
#[derive(Debug, Clone, Copy)]
pub struct LocalFileRepo<'a> {
    pool: &'a SqlitePool,
}

impl<'a> LocalFileRepo<'a> {
    pub(crate) fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Every stored local file, ordered by path.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when a query fails, or [`DbError::InvalidRow`] when a
    /// stored value does not fit its type.
    pub async fn all(&self) -> Result<Vec<LocalFile>, DbError> {
        let rows: Vec<LocalFileRow> = sqlx::query_as(
            "SELECT path, size, modified, title, group_name, season, episode,
                    kind, media_id, confidence, locked, ignored, filler, shelved
             FROM local_file ORDER BY path",
        )
        .fetch_all(self.pool)
        .await?;
        rows.into_iter().map(LocalFileRow::into_file).collect()
    }

    /// Adds new files, updates existing rows in place and deletes rows for
    /// files no longer present. Flags are preserved for rows that exist.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when a query fails.
    pub async fn sync(&self, files: &[LocalFile]) -> Result<(), DbError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        let existing: Vec<String> = sqlx::query_scalar("SELECT path FROM local_file")
            .fetch_all(&mut *tx)
            .await?;
        // Keep the strings alive while `wanted` borrows them.
        let paths: Vec<String> = files
            .iter()
            .map(|file| file.path.to_string_lossy().into_owned())
            .collect();
        let wanted: HashSet<&str> = paths.iter().map(String::as_str).collect();
        for path in existing {
            // An override stays listed even when a scan does not see the file,
            // so a scan of a different folder set cannot discard the user's
            // lock or ignore decision.
            if !wanted.contains(path.as_str()) {
                let overridden: i64 =
                    sqlx::query_scalar("SELECT locked OR ignored FROM local_file WHERE path = ?")
                        .bind(&path)
                        .fetch_one(&mut *tx)
                        .await?;
                if overridden != 0 {
                    continue;
                }
                sqlx::query("DELETE FROM local_file WHERE path = ?")
                    .bind(path)
                    .execute(&mut *tx)
                    .await?;
            }
        }
        for (file, path) in files.iter().zip(paths) {
            sqlx::query(
                "INSERT INTO local_file (
                    path, size, modified, title, group_name, season, episode,
                    kind, media_id, confidence, locked, ignored, filler, shelved
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT (path) DO UPDATE SET
                    size = excluded.size,
                    modified = excluded.modified,
                    title = excluded.title,
                    group_name = excluded.group_name,
                    season = excluded.season,
                    episode = excluded.episode,
                    kind = excluded.kind,
                    media_id = excluded.media_id,
                    confidence = excluded.confidence,
                    filler = excluded.filler,
                    shelved = excluded.shelved",
            )
            .bind(path)
            .bind(file.size)
            .bind(file.modified)
            .bind(&file.title)
            .bind(&file.group)
            .bind(file.season.map(i64::from))
            .bind(file.episode.map(i64::from))
            .bind(&file.kind)
            .bind(file.media_id.map(MediaId::get))
            .bind(file.confidence)
            .bind(i64::from(file.locked))
            .bind(i64::from(file.ignored))
            .bind(i64::from(file.filler))
            .bind(i64::from(file.shelved))
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Sets one file's lock flag, returning the number of rows changed.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when the query fails.
    pub async fn set_locked(&self, path: &Path, flag: FileFlag) -> Result<u64, DbError> {
        Ok(
            sqlx::query("UPDATE local_file SET locked = ? WHERE path = ?")
                .bind(flag.bit())
                .bind(path.to_string_lossy())
                .execute(self.pool)
                .await?
                .rows_affected(),
        )
    }

    /// Sets every file of `media` to `locked`, returning the rows changed.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when the query fails.
    pub async fn set_media_locked(&self, media: MediaId, flag: FileFlag) -> Result<u64, DbError> {
        Ok(
            sqlx::query("UPDATE local_file SET locked = ? WHERE media_id = ?")
                .bind(flag.bit())
                .bind(media.get())
                .execute(self.pool)
                .await?
                .rows_affected(),
        )
    }

    /// Sets one file's ignored flag, returning the number of rows changed.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when the query fails.
    pub async fn set_ignored(&self, path: &Path, flag: FileFlag) -> Result<u64, DbError> {
        Ok(
            sqlx::query("UPDATE local_file SET ignored = ? WHERE path = ?")
                .bind(flag.bit())
                .bind(path.to_string_lossy())
                .execute(self.pool)
                .await?
                .rows_affected(),
        )
    }

    /// Assigns one file to `media`, optionally numbering it, and locks it.
    /// Returns the number of rows changed.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when the query fails.
    pub async fn manual_match(
        &self,
        path: &Path,
        media: MediaId,
        episode: Option<u32>,
    ) -> Result<u64, DbError> {
        Ok(sqlx::query(
            "UPDATE local_file SET media_id = ?, episode = COALESCE(?, episode),
                    confidence = 1.0, locked = 1, ignored = 0 WHERE path = ?",
        )
        .bind(media.get())
        .bind(episode.map(i64::from))
        .bind(path.to_string_lossy())
        .execute(self.pool)
        .await?
        .rows_affected())
    }
}

#[derive(FromRow)]
struct LocalFileRow {
    path: String,
    size: i64,
    modified: i64,
    title: Option<String>,
    group_name: Option<String>,
    season: Option<i64>,
    episode: Option<i64>,
    kind: String,
    media_id: Option<i64>,
    confidence: Option<f64>,
    locked: i64,
    ignored: i64,
    filler: i64,
    shelved: i64,
}

impl LocalFileRow {
    fn into_file(self) -> Result<LocalFile, DbError> {
        let number = |value: Option<i64>, column: &'static str| {
            value
                .map(|value| u32::try_from(value).map_err(|_| DbError::InvalidRow(column)))
                .transpose()
        };
        Ok(LocalFile {
            path: PathBuf::from(self.path),
            size: self.size,
            modified: self.modified,
            title: self.title,
            group: self.group_name,
            season: number(self.season, "season")?,
            episode: number(self.episode, "episode")?,
            kind: self.kind,
            media_id: self.media_id.map(MediaId::new),
            confidence: self.confidence,
            locked: self.locked != 0,
            ignored: self.ignored != 0,
            filler: self.filler != 0,
            shelved: self.shelved != 0,
        })
    }
}
