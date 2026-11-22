use domain::{ExternalId, MediaId, MediaKind};
use sqlx::{SqliteConnection, SqlitePool};

use crate::DbError;

/// Starts a transaction that reads, then writes. IMMEDIATE takes the write
/// lock up front, so a concurrent writer waits on the busy timeout instead of
/// failing with `SQLITE_BUSY` when a deferred read upgrades to a write.
const BEGIN_WRITE: &str = "BEGIN IMMEDIATE";

/// The `provider` column value for AniList ids.
const ANILIST: &str = "anilist";

/// Media identity and cross-references (ADR 0001).
#[derive(Debug, Clone, Copy)]
pub struct MediaRepo<'a> {
    pool: &'a SqlitePool,
}

/// The result of [`MediaRepo::find_or_mint`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Minted {
    /// The media found or minted.
    pub media: MediaId,
    /// External ids that were not linked because they disagree with what is
    /// stored. Each was also logged at WARN.
    pub conflicts: Vec<LinkConflict>,
}

/// An external id that was not linked, because the first link wins and
/// conflicts are never merged automatically.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum LinkConflict {
    /// The id already names another media.
    NamesOtherMedia {
        /// The id that was not linked.
        id: ExternalId,
        /// The media it already names.
        other: MediaId,
    },
    /// The media already has a different id from the same provider.
    ProviderAlreadyLinked {
        /// The id that was not linked.
        id: ExternalId,
    },
}

impl<'a> MediaRepo<'a> {
    pub(crate) fn new(pool: &'a SqlitePool) -> Self {
        Self { pool }
    }

    /// The `kind` media the first known id in `ids` names, if any. Never mints:
    /// catalog, browse and search results stay unminted.
    ///
    /// # Errors
    ///
    /// [`DbError::Sqlx`] when a query fails, or
    /// [`DbError::UnsupportedExternalId`] for an id this build can't store.
    pub async fn find(
        &self,
        kind: MediaKind,
        ids: &[ExternalId],
    ) -> Result<Option<MediaId>, DbError> {
        let mut conn = self.pool.acquire().await?;
        find_in(&mut conn, kind, ids).await
    }

    /// Finds the media one of `ids` names, or mints a new one, then links the
    /// rest of `ids` to it. Call it when the user first records something
    /// about a media.
    ///
    /// Ids are looked up in order and the first match is reused. An id that
    /// disagrees with a stored link is not linked; it is returned in
    /// [`Minted::conflicts`] and logged.
    ///
    /// # Errors
    ///
    /// [`DbError::NoExternalIds`] for an empty `ids`, [`DbError::Sqlx`] when a
    /// query fails, or [`DbError::UnsupportedExternalId`].
    pub async fn find_or_mint(
        &self,
        kind: MediaKind,
        ids: &[ExternalId],
    ) -> Result<Minted, DbError> {
        if ids.is_empty() {
            return Err(DbError::NoExternalIds);
        }
        let mut tx = self.pool.begin_with(BEGIN_WRITE).await?;
        let media = if let Some(media) = find_in(&mut tx, kind, ids).await? {
            media
        } else {
            let id = sqlx::query_scalar("INSERT INTO media (kind) VALUES (?) RETURNING id")
                .bind(kind_column(kind)?)
                .fetch_one(&mut *tx)
                .await?;
            MediaId::new(id)
        };

        let mut conflicts = Vec::new();
        for id in ids {
            let (provider, key) = columns(kind, id)?;
            sqlx::query(
                "INSERT OR IGNORE INTO media_external_id (provider, external_id, media_id)
                 VALUES (?, ?, ?)",
            )
            .bind(provider)
            .bind(&key)
            .bind(media.get())
            .execute(&mut *tx)
            .await?;
            let conflict = match owner(&mut tx, provider, &key).await? {
                Some(owner) if owner == media => continue,
                Some(other) => LinkConflict::NamesOtherMedia {
                    id: id.clone(),
                    other,
                },
                None => LinkConflict::ProviderAlreadyLinked { id: id.clone() },
            };
            tracing::warn!(
                media = media.get(),
                ?conflict,
                "cross-reference conflict: kept the first link"
            );
            conflicts.push(conflict);
        }
        tx.commit().await?;
        Ok(Minted { media, conflicts })
    }

    /// Flags a media whose AniList record was merged or deleted. It keeps its
    /// id and all user data until [`MediaRepo::relink_anilist`].
    ///
    /// # Errors
    ///
    /// [`DbError::UnknownMedia`], or [`DbError::Sqlx`] when the query fails.
    pub async fn flag_missing_from_anilist(&self, media: MediaId) -> Result<(), DbError> {
        let done = sqlx::query("UPDATE media SET missing_from_anilist = 1 WHERE id = ?")
            .bind(media.get())
            .execute(self.pool)
            .await?;
        if done.rows_affected() == 0 {
            return Err(DbError::UnknownMedia(media));
        }
        Ok(())
    }

    /// Whether the media is flagged as missing from AniList.
    ///
    /// # Errors
    ///
    /// [`DbError::UnknownMedia`], or [`DbError::Sqlx`] when the query fails.
    pub async fn is_missing_from_anilist(&self, media: MediaId) -> Result<bool, DbError> {
        sqlx::query_scalar("SELECT missing_from_anilist FROM media WHERE id = ?")
            .bind(media.get())
            .fetch_optional(self.pool)
            .await?
            .ok_or(DbError::UnknownMedia(media))
    }

    /// Replaces the media's AniList cross-reference with `anilist_id` and
    /// clears the missing flag. Nothing else is rewritten.
    ///
    /// # Errors
    ///
    /// [`DbError::ExternalIdTaken`] when another media owns `anilist_id`,
    /// [`DbError::UnknownMedia`], or [`DbError::Sqlx`] when a query fails.
    pub async fn relink_anilist(&self, media: MediaId, anilist_id: u32) -> Result<(), DbError> {
        let key = anilist_id.to_string();
        let mut tx = self.pool.begin_with(BEGIN_WRITE).await?;
        if let Some(other) = owner(&mut tx, ANILIST, &key).await?
            && other != media
        {
            return Err(DbError::ExternalIdTaken {
                id: ExternalId::AniList(anilist_id),
                other,
            });
        }
        let cleared = sqlx::query("UPDATE media SET missing_from_anilist = 0 WHERE id = ?")
            .bind(media.get())
            .execute(&mut *tx)
            .await?;
        if cleared.rows_affected() == 0 {
            return Err(DbError::UnknownMedia(media));
        }
        sqlx::query("DELETE FROM media_external_id WHERE media_id = ? AND provider = ?")
            .bind(media.get())
            .bind(ANILIST)
            .execute(&mut *tx)
            .await?;
        sqlx::query(
            "INSERT INTO media_external_id (provider, external_id, media_id) VALUES (?, ?, ?)",
        )
        .bind(ANILIST)
        .bind(&key)
        .bind(media.get())
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(())
    }
}

async fn find_in(
    conn: &mut SqliteConnection,
    kind: MediaKind,
    ids: &[ExternalId],
) -> Result<Option<MediaId>, DbError> {
    let wanted = kind_column(kind)?;
    for id in ids {
        let (provider, key) = columns(kind, id)?;
        let found: Option<(i64, String)> = sqlx::query_as(
            "SELECT media.id, media.kind FROM media_external_id
             JOIN media ON media.id = media_external_id.media_id
             WHERE provider = ? AND external_id = ?",
        )
        .bind(provider)
        .bind(&key)
        .fetch_optional(&mut *conn)
        .await?;
        match found {
            Some((media, stored)) if stored == wanted => return Ok(Some(MediaId::new(media))),
            // AniList ids are shared by anime and manga, so a caller can hold
            // the right id with the wrong kind. Never reuse across kinds.
            Some((media, stored)) => tracing::warn!(
                ?id,
                media,
                stored_kind = %stored,
                wanted_kind = wanted,
                "external id names a media of the other kind; not reused"
            ),
            None => {}
        }
    }
    Ok(None)
}

async fn owner(
    conn: &mut SqliteConnection,
    provider: &str,
    key: &str,
) -> Result<Option<MediaId>, DbError> {
    let id: Option<i64> = sqlx::query_scalar(
        "SELECT media_id FROM media_external_id WHERE provider = ? AND external_id = ?",
    )
    .bind(provider)
    .bind(key)
    .fetch_optional(conn)
    .await?;
    Ok(id.map(MediaId::new))
}

/// The `(provider, external_id)` columns for an external id of a `kind`
/// media. Phase 1 stores only AniList ids and `idMal` (ADR 0001).
///
/// AniList numbers anime and manga in one sequence, but MyAnimeList numbers
/// them separately (anime 1 and manga 1 are different works), so MAL ids are
/// stored under a provider per kind.
fn columns(kind: MediaKind, id: &ExternalId) -> Result<(&'static str, String), DbError> {
    let provider = match (id, kind) {
        (ExternalId::AniList(_), _) => ANILIST,
        (ExternalId::Mal(_), MediaKind::Anime) => "mal_anime",
        (ExternalId::Mal(_), MediaKind::Manga) => "mal_manga",
        _ => return Err(DbError::UnsupportedExternalId(id.clone())),
    };
    let key = match id {
        ExternalId::AniList(key) | ExternalId::Mal(key) => key.to_string(),
        _ => return Err(DbError::UnsupportedExternalId(id.clone())),
    };
    Ok((provider, key))
}

/// The `media.kind` column value.
fn kind_column(kind: MediaKind) -> Result<&'static str, DbError> {
    match kind {
        MediaKind::Anime => Ok("anime"),
        MediaKind::Manga => Ok("manga"),
        _ => Err(DbError::UnsupportedKind(kind)),
    }
}
