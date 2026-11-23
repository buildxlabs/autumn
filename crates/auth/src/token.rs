//! Decoding the expiry of an AniList JWT without trusting the token's claims
//! beyond `exp`.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use domain::ANILIST_RELOGIN_WINDOW;
use serde::Deserialize;
use thiserror::Error;

/// When an AniList token expires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenExpiry {
    /// The `exp` claim, as a wall-clock time.
    pub expires_at: SystemTime,
}

impl TokenExpiry {
    /// Whether the re-login prompt should be shown at `now`.
    #[must_use]
    pub fn expiring(self, now: SystemTime) -> bool {
        self.expires_at <= now
            || self
                .expires_at
                .duration_since(now)
                .is_ok_and(|left| left <= ANILIST_RELOGIN_WINDOW)
    }

    /// Whether the token has expired at `now`.
    #[must_use]
    pub fn expired(self, now: SystemTime) -> bool {
        self.expires_at <= now
    }
}

/// A token whose `exp` claim could not be read.
#[derive(Debug, Error, PartialEq, Eq)]
#[non_exhaustive]
pub enum TokenError {
    /// The token is not a JWT with a readable expiry.
    #[error("the AniList token has no readable expiry")]
    Malformed,
}

/// Reads the `exp` claim of a JWT's payload.
///
/// # Errors
///
/// [`TokenError::Malformed`] when the token has no payload or its `exp` is
/// missing or out of range.
pub fn decode_expiry(token: &str) -> Result<TokenExpiry, TokenError> {
    let payload = token.split('.').nth(1).ok_or(TokenError::Malformed)?;
    let bytes = URL_SAFE_NO_PAD
        .decode(payload)
        .map_err(|_| TokenError::Malformed)?;
    let claims: Claims = serde_json::from_slice(&bytes).map_err(|_| TokenError::Malformed)?;
    let seconds = u64::try_from(claims.exp).map_err(|_| TokenError::Malformed)?;
    let expires_at = UNIX_EPOCH
        .checked_add(Duration::from_secs(seconds))
        .ok_or(TokenError::Malformed)?;
    Ok(TokenExpiry { expires_at })
}

#[derive(Debug, Deserialize)]
struct Claims {
    exp: i64,
}

#[cfg(test)]
mod tests {
    use super::{TokenError, decode_expiry};
    use base64::Engine;
    use std::time::{Duration, SystemTime, UNIX_EPOCH};

    fn token_at(expires_at: SystemTime) -> String {
        let seconds = expires_at
            .duration_since(UNIX_EPOCH)
            .expect("test dates are after the epoch")
            .as_secs();
        let payload = format!("{{\"exp\":{seconds},\"sub\":123}}");
        format!(
            "header.{}.signature",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload)
        )
    }

    #[test]
    fn a_valid_expiry_is_read_from_the_jwt() {
        let now = UNIX_EPOCH + Duration::from_hours(500_000);
        let expiry = decode_expiry(&token_at(now + Duration::from_hours(8_760))).unwrap();

        assert_eq!(expiry.expires_at, now + Duration::from_hours(8_760));
        assert!(!expiry.expired(now));
        assert!(!expiry.expiring(now));
    }

    #[test]
    fn a_token_near_expiry_asks_for_relogin_but_still_works() {
        let now = UNIX_EPOCH + Duration::from_hours(500_000);
        let expiry = decode_expiry(&token_at(now + Duration::from_hours(72))).unwrap();

        assert!(expiry.expiring(now), "three days is inside the window");
        assert!(!expiry.expired(now));
    }

    #[test]
    fn an_expired_token_is_expired_and_expiring() {
        let now = UNIX_EPOCH + Duration::from_hours(500_000);
        let expiry = decode_expiry(&token_at(now - Duration::from_hours(24))).unwrap();

        assert!(expiry.expired(now));
        assert!(expiry.expiring(now));
    }

    #[test]
    fn malformed_tokens_are_rejected() {
        let no_payload = "header.signature";
        let bad_base64 = "header.!!!.signature";
        let no_exp = format!(
            "header.{}.signature",
            base64::engine::general_purpose::URL_SAFE_NO_PAD.encode("{}")
        );
        for token in ["", no_payload, bad_base64, &no_exp] {
            assert_eq!(
                decode_expiry(token),
                Err(TokenError::Malformed),
                "{token:?}"
            );
        }
    }
}
