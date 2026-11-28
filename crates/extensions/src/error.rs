//! Why an extension call failed.

use domain::ExtensionId;

/// The longest [`ExtensionError::Runtime`] message kept, in bytes (#267 §3).
pub(crate) const MAX_MESSAGE_BYTES: usize = 1024;
/// The longest [`ExtensionError::Runtime`] traceback kept, in bytes.
pub(crate) const MAX_TRACEBACK_BYTES: usize = 8 * 1024;

/// Why loading or calling an extension failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum ExtensionError {
    /// No runnable extension has this id.
    #[error("no extension with id {0}")]
    UnknownExtension(ExtensionId),
    /// The chunk did not return a provider table, for example
    /// `search: expected function, got nil`.
    #[error("invalid provider: {0}")]
    InvalidProvider(String),
    /// `search` returned something other than an array, for example
    /// `results: expected array, got string`.
    #[error("invalid result: {0}")]
    InvalidResult(String),
    /// The script raised an error or failed to compile.
    #[error("{message}")]
    Runtime {
        /// The error value as a string, at most 1 KiB.
        message: String,
        /// `debug.traceback` at the error, at most 8 KiB; empty for a
        /// compile error.
        traceback: String,
    },
    /// The call used more than its active Luau CPU time (1 s).
    #[error("the extension used too much CPU time")]
    CpuLimit,
    /// The call ran longer than its wall-clock limit (30 s), host waits
    /// included.
    #[error("the extension took too long")]
    WallTimeout,
    /// The VM ran out of its memory cap (32 MiB), even if the script caught
    /// the error.
    #[error("the extension ran out of memory")]
    MemoryLimit,
    /// The call made more HTTP requests than allowed (8).
    #[error("the extension made too many requests")]
    RequestLimit,
    /// The outbound guard refused a request (#305).
    #[error("blocked: {0}")]
    NetworkBlocked(String),
    /// The caller left mid-call, or the extension was stopped.
    #[error("the extension call was cancelled")]
    Cancelled,
    /// The VM failed outside the script, for example while building a table.
    #[error("extension VM failed: {0}")]
    Vm(String),
    /// The extension's actor has stopped, so the call never ran.
    #[error("extension {0} has stopped")]
    Stopped(ExtensionId),
}

impl ExtensionError {
    /// A [`Self::Runtime`] with both parts truncated to their caps.
    pub(crate) fn runtime(mut message: String, mut traceback: String) -> Self {
        truncate(&mut message, MAX_MESSAGE_BYTES);
        truncate(&mut traceback, MAX_TRACEBACK_BYTES);
        Self::Runtime { message, traceback }
    }
}

impl From<mlua::Error> for ExtensionError {
    fn from(error: mlua::Error) -> Self {
        match error {
            mlua::Error::MemoryError(_) => Self::MemoryLimit,
            error => Self::Vm(error.to_string()),
        }
    }
}

/// Cuts `text` to at most `max` bytes on a character boundary.
fn truncate(text: &mut String, max: usize) {
    let end = text.floor_char_boundary(max);
    text.truncate(end);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runtime_errors_are_cut_to_their_caps_on_char_boundaries() {
        let error = ExtensionError::runtime("é".repeat(1000), "x".repeat(10_000));
        assert!(
            matches!(&error, ExtensionError::Runtime { message, traceback }
                if message.len() == MAX_MESSAGE_BYTES && traceback.len() == MAX_TRACEBACK_BYTES),
            "2-byte chars fill 1 KiB exactly and the traceback is capped: {error:?}"
        );

        let error = ExtensionError::runtime(format!("a{}", "é".repeat(600)), String::new());
        assert!(
            matches!(&error, ExtensionError::Runtime { message, .. } if message.len() == 1023),
            "a split char is dropped whole: {error:?}"
        );
    }
}
