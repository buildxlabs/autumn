//! The Torrent List's in-memory account of the session (#207).
//!
//! librqbit owns the torrents; this module only remembers why each one was
//! added and, when known, its (media, episode) link. The list itself is a
//! snapshot built from live session stats at most 4 Hz, never stored here.

use std::{collections::HashSet, sync::Mutex};

use domain::{InfoHash, TorrentLink, TorrentListRow, TorrentOrigin, derive_torrent_state};
use torrent::TorrentEntry;

/// What the session does not know about a torrent: why it is there and what
/// episode it belongs to.
#[derive(Debug, Clone)]
struct Meta {
    origin: TorrentOrigin,
    link: Option<TorrentLink>,
}

/// The registrations of torrents added this run. Insertion order is the
/// "newest added first" order of the screen (#207 Q2).
#[derive(Debug, Default)]
pub(crate) struct TorrentList {
    /// Oldest registration first. A torrent registered twice keeps its first
    /// position, so a replay does not move it up the screen.
    meta: Mutex<Vec<(InfoHash, Meta)>>,
}

impl TorrentList {
    /// Remembers why `info_hash` was added and its link, when there is one.
    pub(crate) fn register(
        &self,
        info_hash: InfoHash,
        origin: TorrentOrigin,
        link: Option<TorrentLink>,
    ) {
        let mut meta = lock(&self.meta);
        if let Some(existing) = meta.iter_mut().find(|(hash, _)| *hash == info_hash) {
            // A preload of the next episode in a batch shares the current
            // stream's torrent; it must not take over the row while that
            // episode plays (#208).
            if existing.1.origin == TorrentOrigin::Stream && origin == TorrentOrigin::Preload {
                return;
            }
            existing.1 = Meta { origin, link };
        } else {
            meta.push((info_hash, Meta { origin, link }));
        }
    }

    /// Forgets `info_hash` when it leaves the session.
    pub(crate) fn unregister(&self, info_hash: InfoHash) {
        let mut meta = lock(&self.meta);
        if let Some(at) = meta.iter().position(|(hash, _)| *hash == info_hash) {
            meta.remove(at);
        }
    }

    /// Joins a session snapshot with the registrations and the streams that
    /// are currently attached to playback. Rows are newest first; a torrent
    /// added outside those registrations is an incoming row at the end.
    pub(crate) fn rows(
        &self,
        entries: Vec<TorrentEntry>,
        active: &HashSet<InfoHash>,
    ) -> Vec<TorrentListRow> {
        let meta = lock(&self.meta);
        // Off-by-one so an unregistered row (0) sorts after every real
        // position when the rows are ordered newest first.
        let order = |hash: InfoHash| {
            meta.iter()
                .position(|(registered, _)| *registered == hash)
                .map_or(0, |position| position.saturating_add(1))
        };
        let mut entries = entries;
        entries.sort_by_key(|entry| std::cmp::Reverse(order(entry.info_hash)));
        entries
            .into_iter()
            .map(|entry| {
                let info_hash = entry.info_hash;
                let known = meta.iter().find(|(hash, _)| *hash == info_hash);
                let origin = known.map_or(TorrentOrigin::Incoming, |(_, meta)| meta.origin);
                let link = known.and_then(|(_, meta)| meta.link.clone());
                let stats = entry.stats;
                let row_state = derive_torrent_state(
                    stats.paused,
                    stats.finished,
                    active.contains(&info_hash),
                    stats.selected_bytes,
                    stats.selected_done_bytes,
                );
                let ratio_permille = if stats.downloaded_bytes == 0 {
                    0
                } else {
                    stats
                        .uploaded_bytes
                        .saturating_mul(1000)
                        .checked_div(stats.downloaded_bytes)
                        .unwrap_or(0)
                        .try_into()
                        .unwrap_or(u32::MAX)
                };
                TorrentListRow {
                    info_hash,
                    name: entry.name,
                    origin,
                    state: row_state,
                    selected_done_bytes: stats.selected_done_bytes,
                    selected_bytes: stats.selected_bytes,
                    download_bps: stats.download_bps,
                    upload_bps: stats.upload_bps,
                    peers: stats.peers,
                    ratio_permille,
                    link,
                    files: entry.files,
                    magnet: entry.magnet,
                }
            })
            .collect()
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

#[cfg(test)]
mod tests {
    #![expect(
        clippy::indexing_slicing,
        reason = "test fixtures are read at known row positions"
    )]

    use std::collections::HashSet;

    use domain::{InfoHash, TorrentFileRow, TorrentLink, TorrentOrigin, TorrentState};
    use torrent::{TorrentEntry, TorrentStats};

    use super::TorrentList;

    fn hash(byte: u8) -> InfoHash {
        InfoHash::from_bytes([byte; 20])
    }

    fn entry(info_hash: InfoHash) -> TorrentEntry {
        TorrentEntry {
            info_hash,
            name: format!("torrent-{info_hash}"),
            magnet: format!("magnet:?xt=urn:btih:{info_hash}"),
            stats: TorrentStats {
                selected_done_bytes: 10,
                selected_bytes: 100,
                finished: false,
                paused: false,
                download_bps: 512,
                upload_bps: 128,
                peers: 3,
                downloaded_bytes: 1000,
                uploaded_bytes: 250,
            },
            files: vec![TorrentFileRow::new(0, "ep.mkv", 100, false)],
        }
    }

    #[test]
    fn registrations_supply_origin_and_link_and_newest_comes_first() {
        let list = TorrentList::default();
        let (first, second, third) = (hash(1), hash(2), hash(3));
        list.register(
            first,
            TorrentOrigin::Preload,
            Some(TorrentLink::new(domain::MediaId::new(7), 4, Some(42))),
        );
        list.register(second, TorrentOrigin::Stream, None);

        let rows = list.rows(
            vec![entry(third), entry(first), entry(second)],
            &HashSet::new(),
        );

        assert_eq!(
            rows.iter().map(|row| row.info_hash).collect::<Vec<_>>(),
            [second, first, third],
            "registered rows newest first, an unregistered one last"
        );
        assert_eq!(rows[0].origin, TorrentOrigin::Stream);
        assert_eq!(rows[1].origin, TorrentOrigin::Preload);
        assert_eq!(rows[1].state, TorrentState::Downloading);
        assert_eq!(
            rows[1].link.as_ref().and_then(|link| link.anilist_id),
            Some(42)
        );
        assert_eq!(rows[2].origin, TorrentOrigin::Incoming);
        assert_eq!(rows[0].ratio_permille, 250, "informational ratio");
    }

    #[test]
    fn a_preload_does_not_replace_a_stream_registration() {
        let list = TorrentList::default();
        let info_hash = hash(1);
        let stream_link = TorrentLink::new(domain::MediaId::new(7), 4, Some(42));
        list.register(info_hash, TorrentOrigin::Stream, Some(stream_link.clone()));
        list.register(
            info_hash,
            TorrentOrigin::Preload,
            Some(TorrentLink::new(domain::MediaId::new(7), 5, Some(42))),
        );

        let rows = list.rows(vec![entry(info_hash)], &HashSet::new());
        assert_eq!(rows[0].origin, TorrentOrigin::Stream);
        assert_eq!(rows[0].link, Some(stream_link));
    }

    #[test]
    fn an_attached_stream_is_streaming_and_a_negative_count_cannot_happen() {
        let list = TorrentList::default();
        let info_hash = hash(1);
        list.register(info_hash, TorrentOrigin::Stream, None);
        let active = HashSet::from([info_hash]);

        let rows = list.rows(vec![entry(info_hash)], &active);
        assert_eq!(rows[0].state, TorrentState::Streaming);

        list.unregister(info_hash);
        let rows = list.rows(vec![entry(info_hash)], &HashSet::new());
        assert_eq!(
            rows[0].origin,
            TorrentOrigin::Incoming,
            "registration is gone"
        );
    }
}
