//! The AniList account state shared with the UI. No I/O lives here.

use std::time::{Duration, SystemTime};

/// Seven days before expiry: when the re-login prompt appears.
#[expect(
    clippy::duration_suboptimal_units,
    reason = "seven days reads clearer than hours; Duration::from_days is not a stable const fn"
)]
pub const ANILIST_RELOGIN_WINDOW: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The state of the AniList account right now.
///
/// This is a view model: the UI reads it from an event-pump store and never
/// sees a token. See ADR 0002.
#[derive(Debug, Clone, PartialEq, Eq)]
#[non_exhaustive]
pub enum AniListAccountState {
    /// No account connected; the account-free platform is active.
    LoggedOut,
    /// Connected with a token AniList can still use.
    Active {
        /// The AniList account's display name.
        name: String,
        /// The avatar URL to display, if AniList has one.
        avatar: Option<String>,
        /// When the token expires.
        expires_at: SystemTime,
        /// Whether the re-login prompt should be shown yet.
        expiring: bool,
    },
    /// The token expired or was revoked: AniList stays active but read-only.
    Expired {
        /// The account name, if AniList supplied one before the failure.
        name: Option<String>,
        /// The avatar URL, if AniList supplied one before the failure.
        avatar: Option<String>,
    },
}
