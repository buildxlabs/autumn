//! The stale-first catalog cache through its public API, against a
//! recorded response served from a loopback mock.

use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use axum::{Json, Router, extract::State, routing::post};
use metadata::{AniList, CATALOG_TTL, CatalogCache};
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

/// Writes a blob at `age`, as the real cache would have stored it then.
#[expect(
    clippy::unwrap_used,
    reason = "test harness: a missing fixture or failed write cannot be recovered"
)]
async fn seed_section(dir: &std::path::Path, key: &str, age: Duration, value: &str) {
    let seconds = (SystemTime::now() - age)
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let bytes = rmp_serde::to_vec_named(&(seconds, value)).unwrap();
    net::PosterCache::new(dir)
        .unwrap()
        .write_metadata(key, bytes)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_discover_section_refreshes_only_after_six_hours() {
    let dir = tempfile::tempdir().unwrap();
    let (cache, _) = serve(dir.path()).await;
    let key = "discover-section:trending";
    let disk_key = format!("v1:{key}");

    seed_section(dir.path(), &disk_key, Duration::from_hours(5), "cached").await;
    let fresh = cache
        .section(key.to_owned(), CATALOG_TTL, async { Ok("new".to_owned()) })
        .await
        .unwrap();
    assert_eq!(fresh.cached.value, "cached");
    assert!(fresh.refresh.is_none(), "five hours is still fresh");

    seed_section(dir.path(), &disk_key, Duration::from_hours(7), "cached").await;
    let stale = cache
        .section(key.to_owned(), CATALOG_TTL, async { Ok("new".to_owned()) })
        .await
        .unwrap();
    assert_eq!(
        stale.cached.value, "cached",
        "the stale value is served at once"
    );
    let refreshed = stale.refresh.unwrap().await.unwrap();
    assert_eq!(refreshed.value, "new", "the refresh lands afterwards");
}

#[tokio::test]
async fn each_entry_has_its_own_blob() {
    let dir = tempfile::tempdir().unwrap();
    let (cache, hits) = serve(dir.path()).await;

    cache.media(1).await.unwrap();
    cache.media(2).await.unwrap();

    assert_eq!(hits.load(Ordering::SeqCst), 2);
}
