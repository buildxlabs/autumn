//! The stale-first catalog cache through its public API, against a
//! recorded response served from a loopback mock.

use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

use axum::{Json, Router, extract::State, routing::post};
use metadata::{AniList, CatalogCache};
use serde_json::Value;
use tokio::net::TcpListener;

/// A cache over a mock AniList that answers every POST with the Frieren
/// entry and counts the requests.
#[expect(
    clippy::unwrap_used,
    reason = "test harness: a missing fixture or failed bind cannot be recovered"
)]
async fn serve(cache_dir: &std::path::Path) -> (CatalogCache, Arc<AtomicUsize>) {
    let path = format!(
        "{}/tests/fixtures/media_detail_154587.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let body: Value = serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route(
            "/",
            post(move |State(hits): State<Arc<AtomicUsize>>| async move {
                hits.fetch_add(1, Ordering::SeqCst);
                Json(body)
            }),
        )
        .with_state(Arc::clone(&hits));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let http = net::Client::builder().max_retries(0).build().unwrap();
    let anilist = AniList::with_endpoint(http, format!("http://{address}/"));
    let disk = net::PosterCache::new(cache_dir).unwrap();
    (CatalogCache::new(anilist, disk), hits)
}

#[tokio::test]
async fn a_viewed_entry_is_served_from_disk_on_the_next_visit() {
    let dir = tempfile::tempdir().unwrap();
    let (cache, hits) = serve(dir.path()).await;

    let first = cache.media(154_587).await.unwrap();
    let second = cache.media(154_587).await.unwrap();

    assert_eq!(hits.load(Ordering::SeqCst), 1, "the second visit is a hit");
    assert!(second.refresh.is_none(), "a just-fetched entry is fresh");
    assert_eq!(second.cached.value, first.cached.value);
    assert_eq!(
        std::fs::read_dir(dir.path().join("metadata"))
            .unwrap()
            .count(),
        1,
        "one blob under metadata/"
    );
}

#[tokio::test]
async fn the_cache_survives_a_restart() {
    let dir = tempfile::tempdir().unwrap();
    let (cache, _) = serve(dir.path()).await;
    let first = cache.media(154_587).await.unwrap();

    let (reopened, hits) = serve(dir.path()).await;
    let again = reopened.media(154_587).await.unwrap();

    assert_eq!(hits.load(Ordering::SeqCst), 0);
    assert_eq!(again.cached.value, first.cached.value);
}

#[tokio::test]
async fn each_entry_has_its_own_blob() {
    let dir = tempfile::tempdir().unwrap();
    let (cache, hits) = serve(dir.path()).await;

    cache.media(1).await.unwrap();
    cache.media(2).await.unwrap();

    assert_eq!(hits.load(Ordering::SeqCst), 2);
}
