//! The disposable torrent cache can be cleared without touching live streams.

#![expect(
    clippy::unwrap_used,
    reason = "test harness: setup helpers panic on failure"
)]

use std::{path::Path, str::FromStr};

use domain::InfoHash;
use tempfile::TempDir;
use torrent::TorrentCache;

const LIVE: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const IDLE: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn infohash(hex: &str) -> InfoHash {
    InfoHash::from_str(hex).unwrap()
}

fn write_file(root: &Path, dir: &str, bytes: usize) {
    let folder = root.join(dir);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("ep.mkv"), vec![0; bytes]).unwrap();
}

#[tokio::test]
async fn clearing_skips_live_infohashes_and_reports_the_remaining_size() {
    let dir = tempfile::tempdir().unwrap();
    write_file(dir.path(), LIVE, 30);
    write_file(dir.path(), IDLE, 12);
    let cache = TorrentCache::new(dir.path().to_path_buf());

    let remaining = cache.clear(&[infohash(LIVE)]).await.unwrap();

    assert!(
        dir.path().join(LIVE).join("ep.mkv").exists(),
        "live file stays"
    );
    assert!(!dir.path().join(IDLE).exists(), "idle torrent is deleted");
    assert_eq!(remaining, 30, "the size shown after the clear");
}

#[tokio::test]
async fn a_clear_with_no_active_infohashes_empties_the_cache() {
    let dir = tempfile::tempdir().unwrap();
    write_file(dir.path(), LIVE, 30);
    write_file(dir.path(), IDLE, 12);
    let cache = TorrentCache::new(dir.path().to_path_buf());

    let remaining = cache.clear(&[]).await.unwrap();

    assert_eq!(remaining, 0);
    assert!(!dir.path().join(LIVE).exists());
    assert!(!dir.path().join(IDLE).exists());
}

#[tokio::test]
async fn size_counts_only_infohash_directories() {
    let dir = tempfile::tempdir().unwrap();
    write_file(dir.path(), LIVE, 30);
    write_file(dir.path(), "not-an-infohash", 100);
    let cache = TorrentCache::new(dir.path().to_path_buf());

    assert_eq!(cache.size_bytes().await.unwrap(), 30);
}

#[tokio::test]
async fn a_missing_cache_root_has_no_size_and_clears_cleanly() {
    let dir = TempDir::new().unwrap();
    let cache = TorrentCache::new(dir.path().join("torrents"));

    assert_eq!(cache.size_bytes().await.unwrap(), 0);
    assert_eq!(cache.clear(&[]).await.unwrap(), 0);
}
