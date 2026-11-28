//! One in-process librqbit seeder shared by the engine tests (Phase 3, #257).
//!
//! A loopback session seeds a single file: no DHT, trackers or live network.

use std::{
    net::{Ipv4Addr, SocketAddr},
    num::NonZeroU32,
    sync::Arc,
};

use domain::InfoHash;
use librqbit::{CreateTorrentOptions, ListenerOptions, Session, SessionOptions};
use tempfile::TempDir;

/// Deterministic pseudo-random bytes, so every piece hashes differently.
pub(crate) fn contents(len: u64) -> Vec<u8> {
    let len = usize::try_from(len).unwrap();
    let mut x: u64 = 0x9E37_79B9_7F4A_7C15;
    let mut buf = Vec::with_capacity(len + 8);
    while buf.len() < len {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        buf.extend_from_slice(&x.to_le_bytes());
    }
    buf.truncate(len);
    buf
}

/// A plain librqbit session seeding one file over loopback.
pub(crate) struct Seeder {
    session: Arc<Session>,
    _dir: TempDir,
    pub(crate) addr: SocketAddr,
    pub(crate) info_hash: InfoHash,
    pub(crate) content: Vec<u8>,
    /// Bencoded metainfo, for `TorrentSource::File`.
    pub(crate) metainfo: Vec<u8>,
}

impl Seeder {
    /// Seeds `len` bytes named `name`, uploading at `upload_bps` (`None` for
    /// unlimited).
    pub(crate) async fn new(name: &str, len: u64, upload_bps: Option<u32>) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        let content = contents(len);
        std::fs::write(&path, &content).unwrap();
        let session = Session::new_with_opts(
            dir.path().into(),
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
        let (created, handle) = session
            .create_and_serve_torrent(
                &path,
                CreateTorrentOptions {
                    piece_length: Some(256 * 1024),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        handle.wait_until_completed().await.unwrap();
        let seeder = Self {
            addr: session.listen_addr().unwrap(),
            info_hash: InfoHash::from_bytes(created.info_hash().0),
            session,
            _dir: dir,
            content,
            metainfo: created.as_bytes().unwrap().to_vec(),
        };
        seeder.throttle(upload_bps);
        seeder
    }

    /// Limits how fast the seeder uploads, `None` for unlimited.
    pub(crate) fn throttle(&self, bytes_per_sec: Option<u32>) {
        self.session
            .ratelimits
            .set_upload_bps(bytes_per_sec.and_then(NonZeroU32::new));
    }
}
