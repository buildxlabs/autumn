//! Login against a loopback AniList mock and a fake keychain. No test reaches
//! the live API or the real OS keychain.

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};

use auth::{AniListAuth, AuthError, Keychain, KeychainError};
use axum::{
    Json, Router,
    http::{HeaderMap, StatusCode},
    routing::{get, post},
};
use base64::Engine;
use domain::AniListAccountState;
use serde_json::{Value, json};
use tokio::net::TcpListener;

/// A keychain backed by memory.
#[derive(Debug, Default)]
struct MemoryKeychain {
    token: Mutex<Option<String>>,
}

#[expect(
    clippy::unwrap_used,
    reason = "a poisoned in-memory mutex in a test fake cannot be recovered"
)]
impl Keychain for MemoryKeychain {
    fn get(&self) -> Result<Option<String>, KeychainError> {
        Ok(self.token.lock().unwrap().clone())
    }

    fn set(&self, token: &str) -> Result<(), KeychainError> {
        *self.token.lock().unwrap() = Some(token.to_owned());
        Ok(())
    }

    fn delete(&self) -> Result<(), KeychainError> {
        self.token.lock().unwrap().take();
        Ok(())
    }
}

/// A machine with no usable keychain: writing always fails.
#[derive(Debug, Default)]
struct NoKeychain;

impl Keychain for NoKeychain {
    fn get(&self) -> Result<Option<String>, KeychainError> {
        Err(KeychainError::Unavailable {
            message: "no keychain daemon".to_owned(),
        })
    }

    fn set(&self, _token: &str) -> Result<(), KeychainError> {
        Err(KeychainError::Unavailable {
            message: "no keychain daemon".to_owned(),
        })
    }

    fn delete(&self) -> Result<(), KeychainError> {
        Err(KeychainError::Unavailable {
            message: "no keychain daemon".to_owned(),
        })
    }
}

/// A corrupt stored entry.
#[derive(Debug, Default)]
struct CorruptKeychain;

impl Keychain for CorruptKeychain {
    fn get(&self) -> Result<Option<String>, KeychainError> {
        Err(KeychainError::Corrupt {
            message: "unreadable entry".to_owned(),
        })
    }

    fn set(&self, _token: &str) -> Result<(), KeychainError> {
        Ok(())
    }

    fn delete(&self) -> Result<(), KeychainError> {
        Ok(())
    }
}

/// The mock's answer and the requests it saw.
struct Mock {
    address: String,
    calls: Arc<AtomicUsize>,
    auth: Arc<Mutex<Vec<String>>>,
}

#[expect(
    clippy::unwrap_used,
    reason = "test harness: a missing fixture or failed bind cannot be recovered"
)]
async fn serve(body: Value) -> Mock {
    let calls = Arc::new(AtomicUsize::new(0));
    let auth = Arc::new(Mutex::new(Vec::new()));
    let seen_calls = Arc::clone(&calls);
    let seen_auth = Arc::clone(&auth);
    let app = Router::new()
        .route(
            "/",
            post(
                move |headers: HeaderMap, Json(request): Json<Value>| async move {
                    seen_calls.fetch_add(1, Ordering::SeqCst);
                    if let Some(value) = headers
                        .get("authorization")
                        .and_then(|value| value.to_str().ok())
                    {
                        seen_auth.lock().unwrap().push(value.to_owned());
                    }
                    let _ = request;
                    (StatusCode::OK, Json(body))
                },
            ),
        )
        .route("/gif", get(|| async { (StatusCode::OK, "GIF89a") }))
        .with_state(());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    Mock {
        address,
        calls,
        auth,
    }
}

#[expect(
    clippy::unwrap_used,
    reason = "test harness: a missing fixture or failed bind cannot be recovered"
)]
fn fixture() -> Value {
    let path = format!("{}/tests/fixtures/viewer.json", env!("CARGO_MANIFEST_DIR"));
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

fn token(exp: u64) -> String {
    let payload = format!("{{\"exp\":{exp},\"sub\":123}}");
    format!(
        "header.{}.signature",
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(payload)
    )
}

fn far_future() -> u64 {
    2_100_000_000
}

#[expect(
    clippy::unwrap_used,
    reason = "test harness: a failed bind cannot be recovered"
)]
fn service(mock: &Mock, keychain: Arc<dyn Keychain>) -> AniListAuth {
    let http = net::Client::builder()
        .host("127.0.0.1", 6000, 100, net::HostKind::AniList)
        .max_retries(0)
        .build()
        .unwrap();
    let endpoint = format!("http://{}/", mock.address);
    AniListAuth::with_endpoint(http, endpoint, keychain)
}

#[tokio::test]
async fn login_refuses_without_a_keychain_without_calling_anilist() {
    let mock = serve(fixture()).await;
    let auth = service(&mock, Arc::new(NoKeychain));

    let error = auth.login(&token(far_future()), now()).await.unwrap_err();

    assert!(
        matches!(
            error,
            AuthError::Keychain(KeychainError::Unavailable { .. })
        ),
        "{error:?}"
    );
    assert_eq!(
        mock.calls.load(Ordering::SeqCst),
        0,
        "nothing was sent to AniList"
    );
}

#[tokio::test]
async fn login_stores_the_token_and_shows_the_account() {
    let mock = serve(fixture()).await;
    let keychain = Arc::new(MemoryKeychain::default());
    let shared: Arc<dyn Keychain> = Arc::<MemoryKeychain>::clone(&keychain);
    let auth = service(&mock, shared);
    let pasted = token(far_future());

    let session = auth.login(&pasted, now()).await.unwrap();

    assert_eq!(
        session,
        AniListAccountState::Active {
            name: "TestUser".to_owned(),
            avatar: None,
            expires_at: std::time::UNIX_EPOCH + std::time::Duration::from_secs(far_future()),
            expiring: false,
        }
    );
    assert_eq!(
        keychain.token.lock().unwrap().as_deref(),
        Some(pasted.as_str())
    );
    assert_eq!(
        mock.auth.lock().unwrap().as_slice(),
        [format!("Bearer {pasted}")]
    );
}

#[tokio::test]
async fn a_new_session_restores_from_the_keychain_after_a_restart() {
    let mock = serve(fixture()).await;
    let keychain: Arc<dyn Keychain> = Arc::new(MemoryKeychain::default());
    let first = service(&mock, Arc::clone(&keychain));
    let pasted = token(far_future());
    let expected = first.login(&pasted, now()).await.unwrap();

    let restarted = service(&mock, keychain);
    let restored = restarted.restore(now()).await;

    assert_eq!(restored, expected);
}

#[tokio::test]
async fn sign_out_deletes_the_token_and_leaves_the_app_account_free() {
    let mock = serve(fixture()).await;
    let keychain = Arc::new(MemoryKeychain::default());
    let shared: Arc<dyn Keychain> = Arc::<MemoryKeychain>::clone(&keychain);
    let auth = service(&mock, shared);
    auth.login(&token(far_future()), now()).await.unwrap();

    auth.sign_out().await.unwrap();

    assert!(keychain.token.lock().unwrap().is_none());
    assert_eq!(auth.restore(now()).await, AniListAccountState::LoggedOut);
}

#[tokio::test]
async fn a_corrupt_entry_counts_as_logged_out() {
    let mock = serve(fixture()).await;
    let auth = service(&mock, Arc::new(CorruptKeychain));

    assert_eq!(auth.restore(now()).await, AniListAccountState::LoggedOut);
    assert_eq!(mock.calls.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn an_expired_token_keeps_anilist_read_only() {
    let mock = serve(fixture()).await;
    let keychain = Arc::new(MemoryKeychain::default());
    keychain.set(&token(1_600_000_000)).unwrap();
    let auth = service(&mock, keychain);

    assert_eq!(
        auth.restore(now()).await,
        AniListAccountState::Expired {
            name: None,
            avatar: None,
        }
    );
    assert_eq!(
        mock.calls.load(Ordering::SeqCst),
        0,
        "an expired token needs no Viewer call, even offline"
    );
}

#[tokio::test]
async fn a_rejected_token_keeps_anilist_read_only() {
    let body =
        json!({"errors": [{"message": "Unauthorized.", "status": 401}], "data": {"Viewer": null}});
    let mock = serve(body).await;
    let keychain = Arc::new(MemoryKeychain::default());
    keychain.set(&token(far_future())).unwrap();
    let auth = service(&mock, keychain);

    assert_eq!(
        auth.restore(now()).await,
        AniListAccountState::Expired {
            name: None,
            avatar: None,
        }
    );
}

#[tokio::test]
async fn the_avatar_probe_detects_a_live_gif() {
    let mock = serve(fixture()).await;
    let http = net::Client::builder()
        .host("127.0.0.1", 6000, 100, net::HostKind::Generic)
        .max_retries(0)
        .build()
        .unwrap();

    assert!(auth::animated_at(&http, &format!("http://{}/gif", mock.address)).await);
    assert!(!auth::animated_at(&http, &format!("http://{}/missing", mock.address)).await);
}

fn now() -> std::time::SystemTime {
    std::time::UNIX_EPOCH + std::time::Duration::from_hours(500_000)
}
