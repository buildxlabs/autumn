# What librqbit 9.1.0 gives an embedded streaming client

Wayfinder ticket [#177](https://github.com/buildxlabs/autumn/issues/177)
(`wayfinder:research`, child of [#176](https://github.com/buildxlabs/autumn/issues/176)).
Question: what does `librqbit` (9.x) give an embedded torrent streaming client,
and where does Autumn have to fill gaps? ROADMAP §2 pins `librqbit 9.1`
("Embedded engine and stream reader"); Phase 3's exit criterion is playback in
mpv while downloading, with seeking near the end before completion
([ROADMAP.md §2–3](../ROADMAP.md)). ARCHITECTURE §6 assigns the session,
head+tail priority, file selection, stream reader, disk handoff and cleanup to
the `torrent` crate, and the token-gated byte-range endpoint to `relay`
([ARCHITECTURE.md](../ARCHITECTURE.md)).

## Sources and setup

- Version checked: **9.1.0 is the latest 9.x** (crates.io API:
  <https://crates.io/api/v1/crates/librqbit>, `max_stable_version: 9.1.0`,
  published 2026-10-29). It is Apache-2.0. Docs:
  <https://docs.rs/librqbit/9.1.0/librqbit/>.
- Source read at the exact git tag `v9.1.0`
  (<https://github.com/ikatson/rqbit/tree/v9.1.0>, commit `4309d387`), not from
  the moving `main` branch. All `crates/librqbit/src/...` links below pin that
  tag.
- Build/cargo-deny facts were measured in a throwaway crate **outside the
  repository** (`librqbit-spike`), with a copy of Autumn's `deny.toml`. Nothing
  in the worktree was changed for the experiment. Environment: rustc 1.100.0
  (`rust-toolchain.toml`), cargo 1.102.0, macOS/darwin, 2026-11-25.
- Behavior reference: Seanime's Go/anacrolix implementation in
  [`reference/seanime/internal/torrentstream`](../../../reference/seanime/internal/torrentstream)
  (GPL-3.0 — behavior only, no code ported). This worktree omits `reference/`,
  so the files were read from the main checkout at
  `/Users/gaurav/personal/projects/autumn/reference/seanime/internal/torrentstream/`.

## 1. Streaming reader: `AsyncRead + AsyncSeek`, and what a seek does

**What librqbit gives you.** `ManagedTorrent::stream(file_id)` returns a
`FileStream` after waiting for torrent initialization and metadata resolution;
it registers the stream with the torrent and takes a blocking-work permit
([streaming.rs:337–380](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/torrent_state/streaming.rs#L337-L380)).
`FileStream` implements both `tokio::io::AsyncRead` and `AsyncSeek`
([streaming.rs:167–276](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/torrent_state/streaming.rs#L167-L276)):

- Reads are served from the on-disk storage with `pread_exact` at the stream's
  current position. If the containing piece is not complete, the stream
  registers a waker and returns `Poll::Pending`; piece completion wakes it
  ([streaming.rs:190–243](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/torrent_state/streaming.rs#L190-L243),
  [streaming.rs:104–121](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/torrent_state/streaming.rs#L104-L121)).
  This is the "blocks until pieces are available" behavior rqbit's README
  describes ([README, "Streaming support"](https://github.com/ikatson/rqbit/blob/v9.1.0/README.md#streaming-support)).
- `start_seek` accepts `Start`/`Current`/`End`, rejects negative or past-EOF
  offsets, and writes the new position into the shared `StreamState`
  ([streaming.rs:246–268](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/torrent_state/streaming.rs#L246-L268),
  [streaming.rs:391–399](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/torrent_state/streaming.rs#L391-L399)).
- **How a seek re-prioritizes pieces:** `StreamState::queue` computes the
  window `[file_offset + position, + 32 MiB (default)]` from the *current*
  position, and `TorrentStreams::iter_next_pieces` interleaves and shuffles the
  windows of all active streams
  ([streaming.rs:28–52](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/torrent_state/streaming.rs#L28-L52),
  [streaming.rs:72–102](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/torrent_state/streaming.rs#L72-L102)).
  That iterator is passed as `AcquireRequest.priority_pieces` on every piece
  acquisition, and priority pieces are tried before naturally ordered ones
  ([live/mod.rs:1449–1455](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/torrent_state/live/mod.rs#L1449-L1455),
  [piece_tracker.rs:123–156](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/piece_tracker.rs#L123-L156)).
  So a seek changes the priority window on the next piece request; it is a
  32 MiB lookahead from the new position, not a re-programming of a fixed
  per-piece priority table.
- Dropping the `FileStream` unregisters it, so its priority window disappears
  ([streaming.rs:278–282](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/torrent_state/streaming.rs#L278-L282)).
  `Api::api_stream(idx, file_id)` is the public facade
  ([api.rs:514–517](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/api.rs#L514-L517));
  `pub mod api` is unconditional, so the `Api` facade does **not** require the
  `http-api` feature ([lib.rs:44](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/lib.rs#L44)).

**Gap.** There is no "ready to start playback yet?" signal: reads simply block
on the next piece. Autumn needs the Seanime-style contiguous-window readiness
gate before pointing mpv at the relay URL (see §6).

## 2. Piece and file priority control

**Head+tail is built in, but only one piece at each end.** `FileInfo` iterates
each file's pieces as *first, last, then the rest from start to end*
([file_info.rs:14–35](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/file_info.rs#L14-L35));
`iter_queued_pieces` uses that order for queued pieces
([chunk_tracker.rs:235–248](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/chunk_tracker.rs#L235-L248)).
Combined with the stream lookahead taking precedence, the effective order is:
active-stream windows → file's first piece → file's last piece → remaining
pieces sequentially. **There is no configurable 8/24/4 MiB head+tail window.**

**Sequential mode.** The 9.1.0 README says "Sequential downloading (the
default and only option)"
([README](https://github.com/ikatson/rqbit/blob/v9.1.0/README.md#some-supported-features)).
There is no sequential/rarest-first switch in `SessionOptions` or
`AddTorrentOptions`; the default file order is sorted by relative filename with
a `// TODO: make it configurable`
([live/mod.rs:245–256](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/torrent_state/live/mod.rs#L245-L256)).

**Selecting one file in a batch.** `AddTorrentOptions` has
`only_files: Option<Vec<usize>>` and `only_files_regex` (mutually exclusive),
plus `list_only` to enumerate file indices
([session.rs:200–233](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/session.rs#L200-L233),
[session.rs:251–310](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/session.rs#L251-L310)).
At runtime, `Session::update_only_files(handle, &HashSet<usize>)` /
`Api::api_torrent_action_update_only_files` changes the selection and reconnects
peers as needed
([session.rs:1631–1637](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/session.rs#L1631-L1637),
[api.rs:337–347](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/api.rs#L337-L347));
`ManagedTorrentHandle::only_files()` reads it back
([torrent_state/mod.rs:253](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/torrent_state/mod.rs#L253)).
That is sufficient for "download this episode, skip the rest of the batch", but
the episode→file-index mapping is Autumn's job (Seanime uses an analyzer;
see §6). **No public per-piece priority setter was found** in 9.1.0 — only
stream position and `only_files` drive piece selection. (Evidence: public APIs
in `torrent_state/mod.rs`, `session.rs`, `api.rs`; `file_priorities` itself is a
private `Vec<usize>` in `type_aliases.rs:17`.)

**Gap.** If one-piece head/tail is not enough for fast seeks to the end, the
options are to open a second `FileStream` seeked to the tail (stream windows are
interleaved by design, so this should add tail pieces — inferred from source,
not tested) or to patch/upgrade librqbit. Neither is an API promise.

## 3. Session: persistence, network, limits, stats

**Configuration surface.** `Session::new(output_folder)` and
`new_with_opts(output_folder, SessionOptions)`
([session.rs:556–579](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/session.rs#L556-L579)).
`SessionOptions` (with defaults at
[session.rs:491–519](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/session.rs#L491-L519))
covers:

| Need | Field / API |
| --- | --- |
| DHT | `dht: Option<DhtSessionConfig>` (`None` disables); default enabled with persisted port; bootstrap addrs, port, `DhtPersistenceConfig` ([session.rs:400–421](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/session.rs#L400-L421)) |
| Trackers | `disable_trackers`, `trackers: HashSet<Url>` (always added), per-torrent `trackers`, `force_tracker_interval`, `initial_peers` ([session.rs:434–435,471–472,278–300](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/session.rs#L434-L472)) |
| Peers | `peer_limit`, `connect: Option<ConnectionOptions>` (`proxy_url` socks5, `enable_tcp`, `peer_opts`) ([stream_connect.rs:35–42](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/stream_connect.rs#L35-L42)), block/allow lists, `disable_local_service_discovery`, `ipv4_only`, `bind_device_name` |
| Listen + UPnP | `listen: Option<ListenerOptions>`: `ListenerMode::{TcpOnly,TcpAndUtp,UtpOnly}`, listen addr, `enable_upnp_port_forwarding` (default false), `announce_port` ([listen.rs:22–63](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/listen.rs#L22-L63)); the forwarder is spawned from `session.rs:855–863,1023–1027` |
| Rate limits | `ratelimits: LimitsConfig { upload_bps, download_bps }` at session and per-torrent level; mutable through the public `Session.ratelimits: Limits` (`set_upload_bps` / `set_download_bps`) ([limits.rs:9–83](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/limits.rs#L9-L83), [session.rs:139](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/session.rs#L139)) |
| Resume | `fastresume: bool` (default false) restores state without rehashing ([session.rs:437–438](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/session.rs#L437-L438)) |

**Persistence and resume.** `SessionPersistenceConfig::Json { folder }`
(default: OS data dir + `session`) writes a `session.json` holding each
torrent's info hash, trackers, output folder, `only_files`, paused flag and
`.torrent` bytes; on restore it re-adds with `overwrite: true`
([session.rs:386–397](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/session.rs#L386-L397),
[session_persistence/mod.rs:22–66](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/session_persistence/mod.rs#L22-L66),
[session_persistence/json.rs:35–65](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/session_persistence/json.rs#L35-L65)).
Persistence is **off by default** (`persistence: None`). A `postgres` variant
exists behind the optional `postgres` feature.

**Stats for the overlay.** `Api::api_session_stats()` returns a
`SessionStatsSnapshot` with counters (fetched/uploaded bytes, blocked
connections), up/down speeds, `AggregatePeerStats` (queued, connecting, live TCP
/ uTP / SOCKS, live seeders, seen, dead, not-needed), uptime and connection
stats
([api.rs:263–265](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/api.rs#L263-L265),
[session_stats/snapshot.rs:9–18](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/session_stats/snapshot.rs#L9-L18),
[live/peers/stats/mod.rs:9–24](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/torrent_state/live/peers/stats/mod.rs#L9-L24)).
Per torrent, `ManagedTorrentHandle::stats()` gives `TorrentStats` with
`file_progress: Vec<u64>` (bytes per file), progress/total bytes, speeds,
ETA, state and `finished`
([torrent_state/mod.rs:504](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/torrent_state/mod.rs#L504),
[torrent_state/stats.rs:8–85](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/torrent_state/stats.rs#L8-L85)).
Piece readiness is available as a bitfield plus total piece count via
`Api::api_dump_haves(idx) -> (BF, u32)`
([api.rs:501–512](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/api.rs#L501-L512)).

**Gap.** There is no push stream or 4 Hz sampler. Autumn must poll these
snapshots and coalesce them to ≤4 Hz before the flume channel, as
ARCHITECTURE §4 requires. `file_progress` is bytes, not a percentage.

## 4. Storage: landing, keeping, deleting

- Files land under the session's default output folder, or a per-torrent
  `output_folder` / `sub_folder`; multi-file torrents default to a sub-folder
  ([session.rs:270–274](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/session.rs#L270-L274)).
  The filesystem backend writes `output_folder.join(relative_filename)`
  ([storage/filesystem/fs.rs:125–168](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/storage/filesystem/fs.rs#L125-L168)).
- `AddTorrentOptions.overwrite` (default false) is required to resume/seed on
  top of existing files
  ([session.rs:260–265](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/session.rs#L260-L265)).
- **Keeping vs deleting:** `Api::api_torrent_action_forget` calls
  `Session::delete(idx, false)` (keeps files);
  `api_torrent_action_delete` calls `delete(idx, true)`, which removes the
  persisted torrent and then the files and now-empty directories
  ([api.rs:315–335](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/api.rs#L315-L335),
  [session.rs:1490–1508](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/session.rs#L1490-L1508)).
- **No move/hardlink API was found.** The filesystem storage exposes open/read/
  write, `remove_file` and `remove_directory_if_empty` only
  ([storage/filesystem/fs.rs:90–122](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/storage/filesystem/fs.rs#L90-L122)).

**Gap.** Autumn's "disk handoff" (ARCHITECTURE §6: switch playback to the file
on disk once complete) and any hardlink/copy into the library must be implemented
in the `torrent` crate; librqbit will not move a finished file for you.

## 5. Build: feature flags, system deps, cargo-deny, compile cost

**Features** ([librqbit `Cargo.toml:14–37`](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/Cargo.toml#L14-L37),
[crates.io 9.1.0](https://crates.io/crates/librqbit/9.1.0)):

- `default = ["default-tls", "http-api-client"]`.
- `http-api = ["axum", "tower-http"]` enables librqbit's **own** HTTP server;
  `webui = []` adds its web UI. **Neither is on by default**, so Autumn does
  not need to "turn them off" — just don't enable them. `http-api-client` is a
  *client* for another rqbit's HTTP API
  ([http_api_client.rs:1–13](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/http_api_client.rs#L1-L13))
  and can also be dropped. Module gating confirms `http_api` needs `http-api`
  and `http_api_client` needs `http-api-client`
  ([lib.rs:54–57](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/lib.rs#L54-L57)).
- `rust-tls = ["reqwest/rustls", "sha1w/sha1-ring", "librqbit-core/sha1-ring"]`
  is the feature to pair with ROADMAP's rustls choice. The recommended line is
  `librqbit = { version = "9.1", default-features = false, features = ["rust-tls"] }`.
- Other optional features: `postgres`, `prometheus`, `upnp-serve-adapter`,
  `watch`, `storage_middleware`, `disable-upload`, `tokio-console`,
  `tracing-subscriber-utils`, `async-bt`, `miri`. `disable-upload` gates
  `SessionOptions::disable_upload` — useful if Autumn purely consumes.

**System dependencies.** The rustls-only configuration above built cleanly on
macOS with just the pinned Rust toolchain — no Homebrew package, C/C++
toolchain or npm. With default features, `default-tls` pulls
`reqwest/default-tls` → `native-tls`, which uses SChannel on Windows, Secure
Transport on macOS and **OpenSSL on Linux** ([native-tls README](https://github.com/sfackler/rust-native-tls/blob/master/README.md));
a Linux build with default features therefore needs OpenSSL dev headers and
`pkg-config`. The `webui` feature needs npm
([rqbit README, "Build"](https://github.com/ikatson/rqbit/blob/v9.1.0/README.md#build)).
**Unknown:** I did not actually build on Linux or Windows; the above is derived
from the dependency chain, not from those builds.

**`cargo deny check`: FAILS today.** With a copy of Autumn's `deny.toml`
(`[graph] all-features = true`, `multiple-versions = "warn"`), the spike's
result was:

```text
error[vulnerability]: Double free / use-after-free in `Consumer::skip` and `Consumer::clear` ...
  ID: RUSTSEC-2026-0293
  1 dependent has version requirements that preclude updating:
  librqbit-utp = 0.7.0
    - ringbuf = '^0.4.7'
advisories FAILED, bans ok, licenses ok, sources ok
```

Exit status 1; one advisory (`ringbuf 0.4.8`, fixed in 0.5.2 but
`librqbit-utp 0.7.0` pins `^0.4.7`); 13 non-blocking duplicate-version
warnings; no license, source or git-dependency failures. `librqbit` itself is
Apache-2.0. `cargo tree -i ringbuf` confirms the sole path is
`librqbit → librqbit-utp 0.7.0 → ringbuf 0.4.8`; it is a non-optional
dependency, so no feature flag avoids it. The latest `librqbit-utp` on
crates.io is 0.7.0 and still pins `^0.4.7`, so there is no compatible upgrade
today. Autumn's current `Cargo.lock` has no `ringbuf`. Because `pnpm check`
runs `cargo deny check`, this needs a documented advisory ignore in
`deny.toml` (with reason and review date) or an upstream fix before librqbit
lands.

**Compile-time cost** (cold dev build, throwaway crate, rustc 1.100.0, macOS,
librqbit 9.1.0 with `default-features = false, features = ["rust-tls"]`):

| Config | Wall time |
| --- | ---: |
| Plain `dev` profile | **21.24 s** |
| Autumn-style `dev` (`[profile.dev.package."*"] opt-level = 2`) | **55.47 s** |

349 packages were resolved in the lockfile; 13 duplicate versions. This is a
tiny binary with no source using librqbit, so it is the incremental floor for
adding the dependency, not the full `torrent` + `relay` build.

## 6. Seanime's stream, for behavior (Go / anacrolix)

All paths are under `reference/seanime/internal/` in the main checkout.

- **Client setup:** one torrent at a time, seeding on, listen port 43213 with a
  random-port fallback, `storage.NewFileByInfoHash(DownloadDir)` (files at
  `<DownloadDir>/<infohash>/<path>`), optional IPv6 disable and supplemental
  trackers for public torrents
  ([torrentstream/client.go:83–160,334–374](../../../reference/seanime/internal/torrentstream/client.go)).
- **Head+tail:** `PrioritizeDownloadPieces` sets the first 8 MiB to
  `PiecePriorityNow`, the next 24 MiB to `Readahead`, the last 4 MiB to `High`,
  and everything else to `Normal` (piece-aligned)
  ([util/torrentutil/torrentutil.go:466–519](../../../reference/seanime/internal/util/torrentutil/torrentutil.go)).
  It runs after every auto/manual file selection
  ([torrentstream/finder.go:31–35,86,195](../../../reference/seanime/internal/torrentstream/finder.go)).
- **Seek re-prioritization:** the HTTP handler calls
  `PrioritizeRangeRequestPieces` on a `Range` header — it pads 512 KiB before
  the offset, prioritizes ~10 pieces from there, and lowers the priority of
  pieces far away ([torrentstream/handler.go:72–83](../../../reference/seanime/internal/torrentstream/handler.go),
  [util/torrentutil/torrentutil.go:521–574](../../../reference/seanime/internal/util/torrentutil/torrentutil.go)).
- **Batch pick:** the analyzer maps AniDB episode → file index, every other
  file gets `PiecePriorityNone`, and only the chosen file is downloaded
  ([torrentstream/finder.go:127–200](../../../reference/seanime/internal/torrentstream/finder.go)).
- **Readiness:** streaming starts only when the first 1–2 contiguous pieces of
  the file are complete (2 pieces below 2 MiB, 1 at or above), falling back to
  1 MiB if metadata is absent; the wait loop subscribes to piece-state changes
  with a 250 ms polling fallback
  ([torrentstream/client.go:687–749](../../../reference/seanime/internal/torrentstream/client.go),
  [torrentstream/stream.go:804–848](../../../reference/seanime/internal/torrentstream/stream.go)).
- **Disk handoff:** the handler stats
  `<DownloadDir>/<infohash>/<path>`; if its size matches, it serves the file
  directly instead of the torrent reader
  ([torrentstream/handler.go:57–84](../../../reference/seanime/internal/torrentstream/handler.go)).
- **Cleanup:** non-current and non-preloaded files are deprioritized every 5 s;
  on stop, a torrent under 70 % is dropped together with its files, while a
  preloaded next-episode torrent is kept
  ([torrentstream/client.go:555–603,237–240](../../../reference/seanime/internal/torrentstream/client.go),
  [torrentstream/stream.go:537–555](../../../reference/seanime/internal/torrentstream/stream.go)).
- **Overlay:** a 1 s loop publishes per-file bytes, up/down speeds derived from
  `BytesReadUsefulData` / `BytesWrittenData`, connected seeders/peers and a
  completion percentage as state events
  ([torrentstream/client.go:155–255](../../../reference/seanime/internal/torrentstream/client.go)).
  Seanime emits at ~1 Hz; Autumn's bridge coalesces to 4 Hz.

## Gaps Autumn must fill

| Gap | Why librqbit doesn't cover it |
| --- | --- |
| Playback readiness gate | Reads block per piece, but there is no "first window ready" event (§1, §6) |
| 8/24/4 MiB head+tail windows | Built-in head/tail is one piece each end; no configurable window (§2) |
| Configurable priority/sequential mode | Sequential is the only mode; file order is sorted by name, marked TODO upstream (§2) |
| Episode → file-index mapping | `only_files` takes indices; no analyzer (§2) |
| Token-gated loopback `Range` relay | librqbit's HTTP server is optional and binds its own port; Autumn's `relay` crate serves `FileStream` itself. rqbit's own handler supports only a single `bytes=start-end` range ([streaming.rs handler:66–121](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/http_api/handlers/streaming.rs#L66-L121)), so the relay should implement the full contract it needs |
| Disk handoff + library move/hardlink | No move API; only forget/delete (§4) |
| 4 Hz stats coalescing | Snapshots are pull-based (§3) |
| Cleanup lifecycle on drop | Only whole-torrent `delete(true)`; per-file cleanup is Autumn's logic (§6) |
| Session persistence placement | librqbit writes its own `session.json`; choose whether to enable it and where, alongside Autumn's SQLite settings (§3) |
| cargo-deny advisory | `RUSTSEC-2026-0293` blocks `pnpm check` until ignored with a reason or fixed upstream (§5) |

## Unknowns / not verified

- Linux and Windows builds were not run; only macOS was compiled, and only
  with `rust-tls`. System-dependency claims for those OSes come from the
  dependency chain, not from builds.
- Using two `FileStream`s to emulate a wide tail window is inferred from
  `iter_next_pieces` interleaving; it was not tested and is not a documented
  API guarantee.
- Seek re-prioritization latency under load was not benchmarked; the conclusions
  are from source reading plus the e2e stream tests at
  [`tests/e2e_stream.rs`](https://github.com/ikatson/rqbit/blob/v9.1.0/crates/librqbit/src/tests/e2e_stream.rs).
- No public per-piece priority setter was found; a private/internal path may
  still exist, but using it would be a fork-level dependency.
