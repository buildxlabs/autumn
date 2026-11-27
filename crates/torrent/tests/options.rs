//! Construction-time network options and live rate limits (#210 / ADR-0010),
//! against a real librqbit session on loopback: no internet.

#![expect(clippy::unwrap_used, reason = "test harness panics on setup failure")]

use std::num::NonZeroU32;

use tempfile::TempDir;
use torrent::{PeerDiscovery, RateLimits, TorrentSession, TorrentSessionOptions};

fn cache() -> TempDir {
    tempfile::tempdir().unwrap()
}

#[tokio::test]
async fn rate_limits_apply_at_construction_and_live() {
    let cache = cache();
    let session = TorrentSession::start(
        cache.path().to_path_buf(),
        PeerDiscovery::Only(Vec::new()),
        TorrentSessionOptions {
            rate_limits: RateLimits::from_bytes_per_sec(Some(1_000_000), Some(500_000)),
            ..TorrentSessionOptions::default()
        },
    )
    .await
    .unwrap();

    let applied = session.rate_limits();
    assert_eq!(applied.download_bytes_per_sec, NonZeroU32::new(1_000_000));
    assert_eq!(applied.upload_bytes_per_sec, NonZeroU32::new(500_000));

    // A live change applies without restarting the session.
    session.set_rate_limits(RateLimits::from_bytes_per_sec(Some(2_000_000), None));
    let live = session.rate_limits();
    assert_eq!(live.download_bytes_per_sec, NonZeroU32::new(2_000_000));
    assert_eq!(live.upload_bytes_per_sec, None, "unlimited");

    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_fixed_listen_port_is_bound() {
    let cache = cache();
    // Find a free port, then release it for librqbit to claim.
    let free = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = free.local_addr().unwrap().port();
    drop(free);

    let session = TorrentSession::start(
        cache.path().to_path_buf(),
        PeerDiscovery::Public,
        TorrentSessionOptions {
            listen_port: port,
            dht: false,
            ..TorrentSessionOptions::default()
        },
    )
    .await
    .unwrap();

    let bound = session.listen_addr().unwrap().port();
    assert_eq!(bound, port, "fixed port is used");

    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_random_listen_port_is_the_default() {
    let cache = cache();
    let session = TorrentSession::start(
        cache.path().to_path_buf(),
        PeerDiscovery::Public,
        TorrentSessionOptions {
            dht: false,
            ..TorrentSessionOptions::default()
        },
    )
    .await
    .unwrap();

    // Port 0 in, some nonzero port out.
    assert_ne!(session.listen_addr().unwrap().port(), 0);

    session.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_missing_bound_interface_fails_closed() {
    let cache = cache();
    let result = TorrentSession::start(
        cache.path().to_path_buf(),
        PeerDiscovery::Public,
        TorrentSessionOptions {
            bind_interface: Some("definitely-not-an-interface-xyz".to_owned()),
            dht: false,
            ..TorrentSessionOptions::default()
        },
    )
    .await;

    match result {
        Err(torrent::Error::BindInterface { name, .. }) => {
            assert_eq!(name, "definitely-not-an-interface-xyz");
        }
        other => {
            assert!(
                matches!(other, Err(torrent::Error::BindInterface { .. })),
                "got {other:?}"
            );
        }
    }
}
