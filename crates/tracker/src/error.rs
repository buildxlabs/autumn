use db::DbError;
use domain::MediaId;

/// Why a platform operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum PlatformError {
    /// No media has this id.
    #[error("no media with id {}", .0.get())]
    UnknownMedia(MediaId),
    /// The Local platform's storage failed.
    #[error("local list storage failed")]
    Storage(#[source] DbError),
    /// AniList could not answer. Mutations are sent once and report this
    /// error, so the UI can offer a Retry action instead of silently
    /// replaying an edit.
    #[error("AniList request failed")]
    Remote(#[source] metadata::Error),
    /// The media has no AniList cross-reference to write.
    #[error("media {} has no AniList id", .0.get())]
    NoAniListId(MediaId),
    /// An operation needs AniList, but the account is signed out.
    #[error("AniList is not connected")]
    NotConnected,
}

impl From<metadata::Error> for PlatformError {
    fn from(error: metadata::Error) -> Self {
        Self::Remote(error)
    }
}

impl From<DbError> for PlatformError {
    fn from(error: DbError) -> Self {
        match error {
            DbError::UnknownMedia(media) => Self::UnknownMedia(media),
            error => Self::Storage(error),
        }
    }
}
