//! Integration tests for the disk poster cache, using a loopback server only.

use std::{
    fs::{self, FileTimes, OpenOptions},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, UNIX_EPOCH},
};

use axum::{Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use net::{Client, PosterCache};
use sha2::{Digest, Sha256};
use tokio::{io::AsyncWriteExt, net::TcpListener, task::JoinHandle};

async fn image(State(hits): State<Arc<AtomicUsize>>) -> impl IntoResponse {
    hits.fetch_add(1, Ordering::SeqCst);
    (StatusCode::OK, "poster")
}

#[expect(
    clippy::unwrap_used,
    reason = "test harness: a failed bind or serve cannot be recovered"
)]
async fn serve() -> (String, Arc<AtomicUsize>, JoinHandle<()>) {
    let hits = Arc::new(AtomicUsize::new(0));
    let app = Router::new()
        .route("/{*path}", get(image))
        .with_state(Arc::clone(&hits));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (format!("http://{address}"), hits, server)
}

fn poster_path(cache_dir: &Path, url: &str) -> PathBuf {
    use std::fmt::Write;
    let name =
        Sha256::digest(url.as_bytes())
            .iter()
            .fold(String::with_capacity(64), |mut name, byte| {
                let _ = write!(&mut name, "{byte:02x}");
                name
            });
    cache_dir.join("posters").join(name)
}

#[expect(clippy::unwrap_used, reason = "test fixtures own these files")]
fn set_mtime(path: &Path, seconds: u64) {
    let file = OpenOptions::new().write(true).open(path).unwrap();
    let times = FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(seconds));
    file.set_times(times).unwrap();
}

#[tokio::test]
async fn a_second_fetch_of_the_same_url_makes_no_request() {
    let dir = tempfile::tempdir().unwrap();
    let cache = PosterCache::new(dir.path()).unwrap();
    let (base, hits, server) = serve().await;
    let client = Client::builder().build().unwrap();
    let url = format!("{base}/poster");

    let first = cache.get(&client, &url).await.unwrap();
    let second = cache.get(&client, &url).await.unwrap();

    assert_eq!(first, second);
    assert_eq!(first, b"poster".to_vec());
    assert_eq!(
        hits.load(Ordering::SeqCst),
        1,
        "the second fetch was cached"
    );
    assert!(poster_path(dir.path(), &url).exists());
    server.abort();
}

#[tokio::test]
async fn eviction_removes_the_oldest_file_across_posters_and_metadata() {
    let dir = tempfile::tempdir().unwrap();

    // Seed files before opening the cache so its initial size scan sees them.
    let first = "http://example.invalid/one";
    let second = "http://example.invalid/two";
    let oldest_poster = poster_path(dir.path(), first);
    let newer_poster = poster_path(dir.path(), second);
    fs::create_dir_all(dir.path().join("posters")).unwrap();
    fs::write(&oldest_poster, b"poster").unwrap();
    fs::write(&newer_poster, b"poster").unwrap();
    set_mtime(&oldest_poster, 100);
    set_mtime(&newer_poster, 200);

    let blob = dir.path().join("metadata").join("blob");
    fs::create_dir_all(dir.path().join("metadata")).unwrap();
    fs::write(&blob, b"metadata-blob").unwrap();
    set_mtime(&blob, 50);

    let cache = PosterCache::with_cap(dir.path(), 12).unwrap();
    let (base, _hits, server) = serve().await;
    let client = Client::builder().build().unwrap();

    let third = format!("{base}/three");
    cache.get(&client, &third).await.unwrap();

    assert!(
        !blob.exists(),
        "the metadata blob was oldest and went first"
    );
    assert!(!oldest_poster.exists(), "then the oldest poster went");
    assert!(newer_poster.exists());
    assert!(poster_path(dir.path(), &third).exists());
    server.abort();
}

#[tokio::test]
async fn clearing_empties_posters_but_keeps_metadata() {
    let dir = tempfile::tempdir().unwrap();
    let cache = PosterCache::new(dir.path()).unwrap();
    let (base, _hits, server) = serve().await;
    let client = Client::builder().build().unwrap();
    let url = format!("{base}/poster");

    cache.get(&client, &url).await.unwrap();
    let poster = poster_path(dir.path(), &url);
    assert!(poster.exists());
    let blob = dir.path().join("metadata").join("blob");
    fs::write(&blob, b"meta").unwrap();

    cache.clear().await.unwrap();

    assert!(!poster.exists());
    assert_eq!(
        fs::read_dir(dir.path().join("posters")).unwrap().count(),
        0,
        "posters/ is empty"
    );
    assert!(blob.exists());
    server.abort();
}

#[tokio::test]
async fn an_interrupted_write_leaves_no_partial_file() {
    let dir = tempfile::tempdir().unwrap();
    let cache = PosterCache::new(dir.path()).unwrap();

    // Announce more bytes than are sent, then close the connection.
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        let (mut socket, _) = listener.accept().await.unwrap();
        socket
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nshort")
            .await
            .unwrap();
        drop(socket);
    });

    let client = Client::builder().build().unwrap();
    let result = cache
        .get(&client, &format!("http://{address}/poster"))
        .await;

    assert!(result.is_err(), "a truncated body must fail the fetch");
    assert_eq!(
        fs::read_dir(dir.path().join("posters")).unwrap().count(),
        0,
        "no partial poster was committed"
    );
    assert_eq!(
        fs::read_dir(dir.path().join("tmp")).unwrap().count(),
        0,
        "no temporary file was left behind"
    );
    server.abort();
}

#[cfg(unix)]
#[tokio::test]
async fn a_failed_write_leaves_no_partial_file() {
    use std::os::unix::fs::PermissionsExt;

    let dir = tempfile::tempdir().unwrap();
    let cache = PosterCache::new(dir.path()).unwrap();
    let (base, _hits, server) = serve().await;
    let client = Client::builder().build().unwrap();
    let url = format!("{base}/poster");

    let tmp = dir.path().join("tmp");
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o500)).unwrap();
    let result = cache.get(&client, &url).await;
    fs::set_permissions(&tmp, fs::Permissions::from_mode(0o700)).unwrap();

    assert!(result.is_err(), "an unwritable tmp dir must fail the write");
    assert_eq!(
        fs::read_dir(dir.path().join("posters")).unwrap().count(),
        0,
        "no partial poster was committed"
    );
    assert_eq!(
        fs::read_dir(&tmp).unwrap().count(),
        0,
        "no temp file survived"
    );
    server.abort();
}

#[tokio::test]
async fn concurrent_gets_of_the_same_url_are_safe() {
    let dir = tempfile::tempdir().unwrap();
    let cache = PosterCache::new(dir.path()).unwrap();
    let (base, _hits, server) = serve().await;
    let client = Client::builder().build().unwrap();
    let url = format!("{base}/poster");

    let first = {
        let (cache, client, url) = (cache.clone(), client.clone(), url.clone());
        tokio::spawn(async move { cache.get(&client, &url).await })
    };
    let second = {
        let (cache, client, url) = (cache.clone(), client.clone(), url.clone());
        tokio::spawn(async move { cache.get(&client, &url).await })
    };

    let a = first.await.unwrap().unwrap();
    let b = second.await.unwrap().unwrap();

    assert_eq!(a, b);
    assert_eq!(a, b"poster".to_vec());
    assert_eq!(fs::read_dir(dir.path().join("posters")).unwrap().count(), 1);
    assert_eq!(fs::read_dir(dir.path().join("tmp")).unwrap().count(), 0);
    server.abort();
}
