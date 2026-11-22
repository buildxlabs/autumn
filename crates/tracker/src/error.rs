use db::DbError;
use domain::MediaId;

/// Why a platform operation failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum TrackerError {
    /// No media has this id.
    #[error("no media with id {}", .0.get())]
    UnknownMedia(MediaId),
    /// The Local platform's storage failed.
    #[error("local list storage failed")]
    Storage(#[source] DbError),
}

impl From<DbError> for TrackerError {
    fn from(error: DbError) -> Self {
        match error {
            DbError::UnknownMedia(media) => Self::UnknownMedia(media),
            error => Self::Storage(error),
        }
    }
}
