//! Listing a torrent's files without downloading it, for auto-select's
//! file-level verify (#218). Loopback only: no internet.

#![expect(
    clippy::unwrap_used,
    reason = "test harness: setup helpers panic on failure"
)]

use std::{net::Ipv4Addr, path::Path, sync::Arc, time::Duration};

use librqbit::{CreateTorrentOptions, ListenerOptions, Session, SessionOptions};
use tempfile::TempDir;
use tokio::time::timeout;
use torrent::{PeerDiscovery, TorrentFile, TorrentSession, TorrentSessionOptions, TorrentSource};

const LIMIT: Duration = Duration::from_secs(60);

/// A batch folder with two episodes and a creditless opening.
fn batch(dir: &Path) -> std::path::PathBuf {
    let root = dir.join("[Grp] Show [1080p]");
    std::fs::create_dir_all(root.join("NC")).unwrap();
    std::fs::write(root.join("[Grp] Show - 01.mkv"), vec![1; 40_000]).unwrap();
    std::fs::write(root.join("[Grp] Show - 02.mkv"), vec![2; 50_000]).unwrap();
    std::fs::write(
        root.join("NC").join("[Grp] Show - NCOP.mkv"),
        vec![3; 30_000],
    )
    .unwrap();
    root
}

fn paths(files: &[TorrentFile]) -> Vec<(usize, &str)> {
    let mut paths: Vec<_> = files
        .iter()
        .map(|file| (file.index, file.path.as_str()))
        .collect();
    paths.sort_unstable();
    paths
}

/// A plain librqbit session seeding `folder` on loopback.
async fn seed(dir: &Path, folder: &Path) -> (Arc<Session>, Vec<u8>, String) {
    let seeder = Session::new_with_opts(
        dir.into(),
        SessionOptions {
            dht: None,
            disable_local_service_discovery: true,
            disable_trackers: true,
            listen: Some(ListenerOptions {
                listen_addr: (Ipv4Addr::LOCALHOST, 0).into(),
                ..Default::default()
            }),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let (created, handle) = seeder
        .create_and_serve_torrent(folder, CreateTorrentOptions::default())
        .await
        .unwrap();
    handle.wait_until_completed().await.unwrap();
    let magnet = format!("magnet:?xt=urn:btih:{}", created.info_hash().as_string());
    (seeder, created.as_bytes().unwrap().to_vec(), magnet)
}

async fn leecher(root: &Path, discovery: PeerDiscovery) -> TorrentSession {
    TorrentSession::start(
        root.to_path_buf(),
        discovery,
        TorrentSessionOptions::default(),
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn a_torrent_file_lists_its_files_without_adding_the_torrent() {
    let content = TempDir::new().unwrap();
    let (_seeder, bytes, _) = seed(content.path(), &batch(content.path())).await;
    let cache = TempDir::new().unwrap();
    let session = leecher(cache.path(), PeerDiscovery::Only(Vec::new())).await;

    let files = session
        .list_files(TorrentSource::File(bytes.clone()))
        .await
        .unwrap();

    let listed = paths(&files);
    assert_eq!(listed.len(), 3);
    assert!(
        listed
            .iter()
            .any(|(_, path)| *path == "NC/[Grp] Show - NCOP.mkv")
    );
    let second = files
        .iter()
        .find(|file| file.path == "[Grp] Show - 02.mkv")
        .unwrap();
    assert_eq!(second.len, 50_000);
    // Listing leaves nothing behind: the same index can then be added.
    session
        .add(TorrentSource::File(bytes), second.index)
        .await
        .unwrap();
}

#[tokio::test]
async fn a_magnet_lists_its_files_from_peer_metadata() {
    let content = TempDir::new().unwrap();
    let (seeder, _, magnet) = seed(content.path(), &batch(content.path())).await;
    let peer = seeder.listen_addr().unwrap();
    let cache = TempDir::new().unwrap();
    let session = leecher(cache.path(), PeerDiscovery::Only(vec![peer])).await;

    let files = timeout(LIMIT, session.list_files(TorrentSource::Magnet(magnet)))
        .await
        .unwrap()
        .unwrap();

    let mut names: Vec<_> = files.iter().map(|file| file.path.as_str()).collect();
    names.sort_unstable();
    assert_eq!(
        names,
        [
            "NC/[Grp] Show - NCOP.mkv",
            "[Grp] Show - 01.mkv",
            "[Grp] Show - 02.mkv"
        ]
    );
}
