use domain::{ExternalId, MediaId, MediaKind};

/// Why a database operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum DbError {
    /// SQLite or the connection pool failed.
    #[error("database error")]
    Sqlx(#[from] sqlx::Error),
    /// A migration could not be applied.
    #[error("database migration failed")]
    Migrate(#[from] sqlx::migrate::MigrateError),
    /// No media has this id.
    #[error("no media with id {}", .0.get())]
    UnknownMedia(MediaId),
    /// Minting needs at least one external id to link.
    #[error("a media can only be minted from at least one external id")]
    NoExternalIds,
    /// The external id already names another media.
    #[error("{id:?} already names media {}", other.get())]
    ExternalIdTaken {
        /// The external id asked for.
        id: ExternalId,
        /// The media it names.
        other: MediaId,
    },
    /// This build does not store that media kind.
    #[error("unsupported media kind {0:?}")]
    UnsupportedKind(MediaKind),
    /// This build does not store that kind of external id. Phase 1 stores
    /// only AniList ids and `idMal`.
    #[error("unsupported external id {0:?}")]
    UnsupportedExternalId(ExternalId),
}
