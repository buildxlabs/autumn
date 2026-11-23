//! The OS keychain seam: the AniList token lives nowhere else (ADR 0002).

use keyring::v1::{Entry, Error as KeyringError};
use thiserror::Error;

/// The keychain service Autumn uses.
pub const SERVICE: &str = "dev.buildxlabs.autumn";
/// The keychain account holding the AniList token.
pub const ACCOUNT: &str = "anilist-token";

/// A place to keep the AniList token.
pub trait Keychain: Send + Sync {
    /// Reads the stored token, or `None` when there is no entry.
    ///
    /// # Errors
    ///
    /// [`KeychainError`] when the store cannot be reached or the entry cannot
    /// be read.
    fn get(&self) -> Result<Option<String>, KeychainError>;

    /// Stores or replaces the token.
    ///
    /// # Errors
    ///
    /// [`KeychainError`] when the store refuses the write.
    fn set(&self, token: &str) -> Result<(), KeychainError>;

    /// Deletes the stored token. Deleting an absent entry succeeds.
    ///
    /// # Errors
    ///
    /// [`KeychainError`] when the store refuses the delete.
    fn delete(&self) -> Result<(), KeychainError>;
}

/// Why the keychain could not serve a request.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum KeychainError {
    /// There is no usable keychain on this machine.
    #[error("no OS keychain is available: {message}")]
    Unavailable {
        /// The platform's explanation, without any token.
        message: String,
    },
    /// The keychain exists but refused access.
    #[error("access to the OS keychain was denied: {message}")]
    Denied {
        /// The platform's explanation, without any token.
        message: String,
    },
    /// The stored entry is corrupt and must be treated as logged out.
    #[error("the stored AniList token is corrupt: {message}")]
    Corrupt {
        /// The platform's explanation, without any token.
        message: String,
    },
    /// Another keychain failure.
    #[error("the OS keychain failed: {message}")]
    Failed {
        /// The platform's explanation, without any token.
        message: String,
    },
}

/// The platform's keychain: Keychain Services, Credential Manager or Secret
/// Service, chosen by `keyring`.
#[derive(Debug, Default, Clone, Copy)]
pub struct SystemKeychain;

impl Keychain for SystemKeychain {
    fn get(&self) -> Result<Option<String>, KeychainError> {
        let entry = Entry::new(SERVICE, ACCOUNT).map_err(|error| classify(&error))?;
        match entry.get_password() {
            Ok(token) => Ok(Some(token)),
            Err(KeyringError::NoEntry) => Ok(None),
            Err(error) => {
                trace_keychain("read", &error);
                Err(classify(&error))
            }
        }
    }

    fn set(&self, token: &str) -> Result<(), KeychainError> {
        let entry = Entry::new(SERVICE, ACCOUNT).map_err(|error| classify(&error))?;
        entry.set_password(token).map_err(|error| {
            trace_keychain("write", &error);
            classify(&error)
        })
    }

    fn delete(&self) -> Result<(), KeychainError> {
        let entry = Entry::new(SERVICE, ACCOUNT).map_err(|error| classify(&error))?;
        match entry.delete_credential() {
            Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
            Err(error) => {
                trace_keychain("delete", &error);
                Err(classify(&error))
            }
        }
    }
}

fn classify(error: &KeyringError) -> KeychainError {
    let message = error.to_string();
    match error {
        KeyringError::NoDefaultStore
        | KeyringError::NotSupportedByStore(_)
        | KeyringError::Invalid(..) => KeychainError::Unavailable { message },
        KeyringError::NoStorageAccess(_) | KeyringError::PlatformFailure(_) => {
            KeychainError::Denied { message }
        }
        KeyringError::BadEncoding(_) | KeyringError::BadDataFormat(..) => {
            KeychainError::Corrupt { message }
        }
        _ => KeychainError::Failed { message },
    }
}

fn trace_keychain(action: &str, error: &KeyringError) {
    // A keychain error names the platform problem, never the stored token.
    tracing::trace!(%action, error = %error, "keychain {action} failed");
}
