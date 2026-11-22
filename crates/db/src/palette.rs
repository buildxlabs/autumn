//! The command palette's local side (#63): cached catalog titles and the
//! recent searches and views the palette shows with no query.

use domain::MediaKind;
use sqlx::{FromRow, SqlitePool};

use crate::{DbError, media::kind_column};

/// How many recent searches and views are kept.
const RECENT_LIMIT: i64 = 20;

/// A cached display title, keyed by AniList id and media kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CatalogTitle {
    /// The AniList id this title belongs to.
    pub anilist_id: u32,
    /// Anime or manga.
    pub kind: MediaKind,
    /// The title to display.
    pub title: String,
    /// Alternate titles, matched as well as `title`.
    pub aliases: Vec<String>,
}

/// The palette's local data, on the app database.
#[derive(Debug, Clone, Copy)]
pub struct PaletteRepo<'a> {
    pool: &'a SqlitePool,
}

impl<'a> PaletteRepo<'a> {
    pub(crate) fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// Caches the titles of catalog results the user has seen, replacing any
    /// earlier title for the same media.
    ///
    /// # Errors
    ///
    /// [`DbError::UnsupportedKind`], [`DbError::Sqlx`] when a query fails, or
    /// [`DbError::InvalidRow`] for an id AniList could not have sent.
    pub async fn remember(&self, titles: &[CatalogTitle]) -> Result<(), DbError> {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        for title in titles {
            let kind = kind_column(title.kind)?;
            let id = i64::from(title.anilist_id);
            sqlx::query(
                "INSERT INTO catalog_title (anilist_id, kind, title, aliases)
                 VALUES (?, ?, ?, ?)
                 ON CONFLICT (anilist_id, kind) DO UPDATE SET
                     title = excluded.title,
                     aliases = excluded.aliases",
            )
            .bind(id)
            .bind(kind)
            .bind(&title.title)
            .bind(title.aliases.join("\n"))
            .execute(&mut *tx)
            .await?;
        }
        tx.commit().await?;
        Ok(())
    }

    /// Titles of `kind` media in the user's local list, most recently updated
    /// first. Media whose title was never cached are left out.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when a query fails, [`DbError::UnsupportedKind`], or
    /// [`DbError::InvalidRow`] for a stored AniList id.
    pub async fn local(&self, kind: MediaKind) -> Result<Vec<CatalogTitle>, DbError> {
        let kind = kind_column(kind)?;
        let mut conn = self.pool.acquire().await?;
        let rows: Vec<TitleRow> = sqlx::query_as(
            "SELECT CAST(e.external_id AS INTEGER) AS anilist_id, t.title, t.aliases
             FROM local_list_entry l
             JOIN media m ON m.id = l.media_id
             JOIN media_external_id e ON e.media_id = m.id AND e.provider = 'anilist'
             JOIN catalog_title t ON t.anilist_id = CAST(e.external_id AS INTEGER) AND t.kind = ?
             WHERE m.kind = ?
             ORDER BY l.updated_at DESC, e.external_id",
        )
        .bind(kind)
        .bind(kind)
        .fetch_all(&mut *conn)
        .await?;
        rows.into_iter().map(|row| row.into_title(kind)).collect()
    }

    /// The last queries, most recent first.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when a query fails.
    pub async fn recent_searches(&self) -> Result<Vec<String>, DbError> {
        Ok(
            sqlx::query_scalar("SELECT query FROM recent_search ORDER BY created_at DESC LIMIT ?")
                .bind(RECENT_LIMIT)
                .fetch_all(self.pool)
                .await?,
        )
    }

    /// Titles of the most recently viewed `kind` media, most recent first.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when a query fails, [`DbError::UnsupportedKind`], or
    /// [`DbError::InvalidRow`] for a stored id or kind.
    pub async fn recent_views(&self, kind: MediaKind) -> Result<Vec<CatalogTitle>, DbError> {
        let kind = kind_column(kind)?;
        let rows: Vec<TitleRow> = sqlx::query_as(
            "SELECT v.anilist_id, t.title, t.aliases
             FROM recent_view v
             JOIN catalog_title t ON t.anilist_id = v.anilist_id AND t.kind = v.kind
             WHERE v.kind = ?
             ORDER BY v.viewed_at DESC
             LIMIT ?",
        )
        .bind(kind)
        .bind(RECENT_LIMIT)
        .fetch_all(self.pool)
        .await?;
        rows.into_iter().map(|row| row.into_title(kind)).collect()
    }

    /// Records a search, moving it to the top and pruning the oldest entries.
    /// Whitespace-only queries are ignored.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when a query fails.
    pub async fn record_search(&self, query: &str) -> Result<(), DbError> {
        let query = query.trim();
        if query.is_empty() {
            return Ok(());
        }
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query(
            "INSERT INTO recent_search (query, created_at) VALUES (?, unixepoch('subsec'))
             ON CONFLICT (query) DO UPDATE SET created_at = unixepoch('subsec')",
        )
        .bind(query)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "DELETE FROM recent_search WHERE query NOT IN (
                 SELECT query FROM recent_search ORDER BY created_at DESC LIMIT ?
             )",
        )
        .bind(RECENT_LIMIT)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }

    /// Records a view of `anilist_id`, moving it to the top and pruning the
    /// oldest entries. The title must already be cached by [`Self::remember`].
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when a query fails, [`DbError::UnsupportedKind`], or
    /// [`DbError::InvalidRow`] for an id AniList could not have sent.
    pub async fn record_view(&self, anilist_id: u32, kind: MediaKind) -> Result<(), DbError> {
        let kind = kind_column(kind)?;
        let id = i64::from(anilist_id);
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await?;
        sqlx::query(
            "INSERT INTO recent_view (anilist_id, kind, viewed_at) VALUES (?, ?, unixepoch('subsec'))
             ON CONFLICT (anilist_id, kind) DO UPDATE SET viewed_at = unixepoch('subsec')",
        )
        .bind(id)
        .bind(kind)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            "DELETE FROM recent_view WHERE (anilist_id, kind) NOT IN (
                         SELECT anilist_id, kind FROM recent_view ORDER BY viewed_at DESC LIMIT ?
                     )",
        )
        .bind(RECENT_LIMIT)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

#[derive(FromRow)]
struct TitleRow {
    anilist_id: i64,
    title: String,
    aliases: String,
}

impl TitleRow {
    fn into_title(self, kind: &str) -> Result<CatalogTitle, DbError> {
        let anilist_id =
            u32::try_from(self.anilist_id).map_err(|_| DbError::InvalidRow("anilist_id"))?;
        let kind = match kind {
            "anime" => MediaKind::Anime,
            "manga" => MediaKind::Manga,
            _ => return Err(DbError::InvalidRow("kind")),
        };
        Ok(CatalogTitle {
            anilist_id,
            kind,
            title: self.title,
            aliases: if self.aliases.is_empty() {
                Vec::new()
            } else {
                self.aliases.split('\n').map(str::to_owned).collect()
            },
        })
    }
}
