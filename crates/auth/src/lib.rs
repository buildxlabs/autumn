//! AniList account lifecycle: keychain-backed login, `Viewer`, expiry and
//! sign out. Tokens never reach settings, SQLite or logs (ADR 0002).

mod avatar;
mod keychain;
mod token;

use std::{sync::Arc, time::SystemTime};

use domain::AniListAccountState;
use metadata::{ANILIST_ENDPOINT, AniList};
use thiserror::Error;

pub use avatar::{animated, animated_at};
pub use keychain::{ACCOUNT, Keychain, KeychainError, SERVICE, SystemKeychain};
pub use token::{TokenError, TokenExpiry, decode_expiry};

/// The public client id for the PIN/implicit flow. Decision #13: the id may
/// be compiled in; the client secret is never used or shipped.
pub const ANILIST_CLIENT_ID: &str = "57719";

/// A failure the account UI can explain to the user.
#[derive(Debug, Error)]
#[non_exhaustive]
pub enum AuthError {
    /// The pasted token is malformed or already expired.
    #[error("that is not a usable AniList token; paste a fresh one")]
    InvalidToken,
    /// The OS keychain refused the operation.
    #[error(transparent)]
    Keychain(#[from] KeychainError),
    /// AniList rejected the account lookup.
    #[error("AniList could not confirm the account: {0}")]
    Api(#[from] metadata::Error),
}

/// The AniList authorization URL for the PIN flow (#13).
#[must_use]
pub fn login_url() -> String {
    format!(
        "https://anilist.co/api/v2/oauth/authorize?client_id={ANILIST_CLIENT_ID}&response_type=token"
    )
}

/// Owns the AniList account and token. Cloning shares the client and keychain seam.
#[derive(Clone)]
pub struct AniListAuth {
    anilist: AniList,
    http: net::Client,
    keychain: Arc<dyn Keychain>,
}

impl std::fmt::Debug for AniListAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AniListAuth").finish_non_exhaustive()
    }
}

impl AniListAuth {
    /// A service using AniList's endpoint and the platform keychain.
    #[must_use]
    pub fn new(http: net::Client, keychain: Arc<dyn Keychain>) -> Self {
        Self::with_endpoint(http, ANILIST_ENDPOINT, keychain)
    }

    /// A service for another endpoint, such as a loopback mock in tests.
    #[must_use]
    pub fn with_endpoint(
        http: net::Client,
        endpoint: impl Into<String>,
        keychain: Arc<dyn Keychain>,
    ) -> Self {
        let anilist = metadata::AniList::with_endpoint(http.clone(), endpoint);
        Self {
            anilist,
            http,
            keychain,
        }
    }

    /// Restores the token from the keychain at startup.
    ///
    /// A missing, corrupt or unreadable entry leaves the app account-free;
    /// the underlying keychain error is only traced, never shown as a token.
    /// An expired token restores as read-only without a network call.
    /// Returns the state for the UI to publish.
    pub async fn restore(&self, now: SystemTime) -> AniListAccountState {
        let token = match get_token(&self.keychain).await {
            Ok(Some(token)) => token,
            Ok(None) => return AniListAccountState::LoggedOut,
            Err(KeychainError::Corrupt { message }) => {
                tracing::warn!(%message, "the stored AniList token is corrupt; logged out");
                return AniListAccountState::LoggedOut;
            }
            Err(error) => {
                tracing::warn!(error = %error, "the AniList keychain is unavailable; staying account-free");
                return AniListAccountState::LoggedOut;
            }
        };
        let Ok(expiry) = decode_expiry(&token) else {
            tracing::warn!("the stored AniList token is unusable; logged out");
            let _ = self.sign_out().await;
            return AniListAccountState::LoggedOut;
        };
        if expiry.expired(now) {
            // Do not depend on the network to know a token is expired: an
            // offline restart must still show the re-login banner.
            return AniListAccountState::Expired {
                name: None,
                avatar: None,
            };
        }
        match self.account_from_token(&token, expiry, now).await {
            Ok(account) => account,
            Err(AuthError::InvalidToken) => {
                tracing::warn!("the stored AniList token is unusable; logged out");
                let _ = self.sign_out().await;
                AniListAccountState::LoggedOut
            }
            Err(error) => {
                tracing::warn!(error = %error, "could not restore the AniList account");
                AniListAccountState::LoggedOut
            }
        }
    }

    /// Stores a pasted token in the keychain, then reads the account.
    ///
    /// A keychain that cannot store the token refuses the login: nothing is
    /// sent to AniList and the app stays account-free (ADR 0002). A failed
    /// account lookup removes the token again.
    ///
    /// # Errors
    ///
    /// [`AuthError::InvalidToken`], [`AuthError::Keychain`] or
    /// [`AuthError::Api`], each with a message safe to show in the UI.
    pub async fn login(
        &self,
        token: &str,
        now: SystemTime,
    ) -> Result<AniListAccountState, AuthError> {
        let expiry = decode_expiry(token).map_err(|_| AuthError::InvalidToken)?;
        if expiry.expired(now) {
            return Err(AuthError::InvalidToken);
        }
        set_token(&self.keychain, token).await?;
        match self.account_from_token(token, expiry, now).await {
            Ok(account) => Ok(account),
            Err(error) => {
                let _ = self.sign_out().await;
                Err(error)
            }
        }
    }

    /// Deletes the token from the keychain.
    ///
    /// # Errors
    ///
    /// [`AuthError::Keychain`] when the store refuses the delete.
    pub async fn sign_out(&self) -> Result<(), AuthError> {
        delete_token(&self.keychain)
            .await
            .map_err(AuthError::Keychain)
    }

    async fn account_from_token(
        &self,
        token: &str,
        expiry: TokenExpiry,
        now: SystemTime,
    ) -> Result<AniListAccountState, AuthError> {
        let viewer = match self.anilist.viewer(token).await {
            Ok(viewer) => viewer,
            Err(
                metadata::Error::Unauthorized
                | metadata::Error::Api {
                    status: Some(401), ..
                }
                | metadata::Error::Net(net::Error::Http { status: 401, .. }),
            ) => {
                // Expired or revoked: AniList stays active but read-only.
                return Ok(AniListAccountState::Expired {
                    name: None,
                    avatar: None,
                });
            }
            Err(error) => return Err(AuthError::Api(error)),
        };
        let still = viewer.avatar.large.or(viewer.avatar.medium);
        let avatar = match animated(&self.http, still.as_deref()).await {
            Some(animated) => Some(animated),
            None => still,
        };
        if expiry.expired(now) {
            return Ok(AniListAccountState::Expired {
                name: Some(viewer.name),
                avatar,
            });
        }
        Ok(AniListAccountState::Active {
            name: viewer.name,
            avatar,
            expires_at: expiry.expires_at,
            expiring: expiry.expiring(now),
        })
    }
}

async fn get_token(keychain: &Arc<dyn Keychain>) -> Result<Option<String>, KeychainError> {
    let keychain = Arc::clone(keychain);
    tokio::task::spawn_blocking(move || keychain.get())
        .await
        .map_err(|error| KeychainError::Failed {
            message: error.to_string(),
        })?
}

async fn set_token(keychain: &Arc<dyn Keychain>, token: &str) -> Result<(), KeychainError> {
    let keychain = Arc::clone(keychain);
    let token = token.to_owned();
    tokio::task::spawn_blocking(move || keychain.set(&token))
        .await
        .map_err(|error| KeychainError::Failed {
            message: error.to_string(),
        })?
}

async fn delete_token(keychain: &Arc<dyn Keychain>) -> Result<(), KeychainError> {
    let keychain = Arc::clone(keychain);
    tokio::task::spawn_blocking(move || keychain.delete())
        .await
        .map_err(|error| KeychainError::Failed {
            message: error.to_string(),
        })?
}
