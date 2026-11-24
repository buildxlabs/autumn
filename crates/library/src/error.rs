use std::path::PathBuf;

use db::DbError;

/// Why a library scan failed.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LibraryError {
    /// A folder or file could not be read.
    #[error("cannot read {}", .path.display())]
    Walk {
        /// The path that failed.
        path: PathBuf,
        /// The I/O error.
        #[source]
        source: std::io::Error,
    },
    /// A candidate matched but had no ids to cross-reference.
    #[error("matching candidate {title:?} has no external ids")]
    CandidateWithoutExternalIds {
        /// The candidate's title.
        title: String,
    },
    /// Storage failed.
    #[error("library storage failed")]
    Db(#[from] DbError),
}
