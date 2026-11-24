use std::{collections::HashSet, path::PathBuf};

use domain::MediaId;
use sqlx::{FromRow, SqlitePool};

use crate::DbError;

/// One local file as a library scan found it.
///
/// `kind` is one of `episode`, `special`, `movie`, `ncop`, `nced` or `extra`.
/// The flags are reserved for later library work; a scan leaves them alone.
#[derive(Debug, Clone, PartialEq)]
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
                    kind, media_id, confidence, locked, ignored
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
            if !wanted.contains(path.as_str()) {
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
                    kind, media_id, confidence, locked, ignored
                 ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
                 ON CONFLICT (path) DO UPDATE SET
                    size = excluded.size,
                    modified = excluded.modified,
                    title = excluded.title,
                    group_name = excluded.group_name,
                    season = excluded.season,
                    episode = excluded.episode,
                    kind = excluded.kind,
                    media_id = excluded.media_id,
                    confidence = excluded.confidence",
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
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
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
        })
    }
}
