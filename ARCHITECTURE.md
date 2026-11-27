# Architecture

How Autumn is put together: the crates, the rules between them, and how the GPUI front end talks to the Tokio back end. [ROADMAP.md](ROADMAP.md) says _when_ each piece is built. This file says _what shape_ it takes.

Status: the design target. Most of these crates don't exist yet. A crate is created in the roadmap phase that first needs it, not earlier.

## 1. Shape of the system

Autumn is one native desktop process. No browser UI and no HTTP API for a front end. The only sockets it opens are loopback-only (`127.0.0.1`) streaming endpoints that feed the video player.

```text
┌──────────────────────────── main thread (GPUI) ────────────────────────────┐
│ gpui-kit window → AppShell                                                  │
│   Titlebar (40px) · Sidebar rail (64px → 260px on hover) · Route view       │
│   PlaybackHud · overlays (dialogs, sheets, toasts, command palette)         │
│                                                                             │
│ Stores: Entity<LibraryStore> Entity<PlaybackStore> Entity<TorrentStore> …   │
│ Router: Entity<RouterState>   Theme: Entity<ThemeStore>                     │
└───────────────▲────────────────────────────────────────┬───────────────────┘
                │ AppEvent (flume, throttled)            │ bridge::run(cx, fut)
                │                                        ▼
┌─────────────────────────── Tokio runtime (engine) ─────────────────────────┐
│ Engine (composition root): modules started/stopped from Settings            │
│  metadata · tracker · library · torrent · search · debrid · extensions      │
│  player · continuity · autodownloader · onlinestream · cron · rpc           │
│                                                                             │
│ Loopback HTTP (127.0.0.1:<random>, per-session token in every path)         │
│   /t/<token>/<infohash>/<file>   torrent byte-range stream                  │
│   /r/<token>/<id>                relay: header injection, HLS/DASH rewrite  │
│                                                                             │
│ SQLite (WAL) · disk cache (posters, metadata) · OS keychain (secrets)       │
└──────────────────────────────────────┬─────────────────────────────────────┘
                                       ▼
            External player (mpv / IINA / VLC / MPC-HC) over JSON IPC
            later: libmpv embedded in the window (Phase 9)
```

## 2. Crate map

Every crate lives at `crates/<crate-name>/` and is picked up by the `crates/*` member glob. All crates are `publish = false` and use `[lints] workspace = true`. Shared dependency versions live in `[workspace.dependencies]` in the root `Cargo.toml`. Crates use `dep.workspace = true` and never pin their own versions.

**Naming:** crates are named for what they do, with no project prefix (`library`, not `autumn-library`). UI crates share a `ui-` prefix so they sort together. The one exception is the binary, `autumn`, because it is the app. A crate name must not shadow a standard crate (`core`, `std`, `alloc`, `test`, `proc_macro`) or a crates.io dependency we use. That rule is why the domain crate is `domain`, not `core`.

| Layer        | Crate            | Owns                                                                                                                                                                                                                                        |
| ------------ | ---------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| Domain       | `domain`         | IDs (`MediaId`, `EpisodeNumber`, `InfoHash`), external ids (`AniList`, `Mal`, `AniDb`, `Kitsu`, `Custom { source, key }`; see ADR 0001), media/episode/chapter models, `StreamSource`, list status, `AppEvent`, shared error kinds. No I/O. |
| Foundation   | `settings`       | Typed settings (one struct per settings tab), defaults, validation, app paths (config/data/cache dirs).                                                                                                                                     |
|              | `db`             | SQLite pool, embedded migrations, repositories (media ids and cross-references per ADR 0001, local files, continuity, history, playlists, rules, settings rows, offline snapshots).                                                         |
|              | `net`            | One shared `reqwest` client: retries/backoff, rate limiter, DNS-over-HTTPS, HTTP proxy, user-agent pool, outbound guard, image/poster download + disk cache (keyed by image URL).                                                           |
| Pure logic   | `release-parser` | Anime release filename parser (title, group, season, episode, range, resolution, codec, audio, batch tags). Pure functions, large test corpus.                                                                                              |
|              | `mkv`            | Matroska/EBML reader: duration, chapters, tracks, attachments (fonts). Used for skip markers and track pickers.                                                                                                                             |
| Integrations | `metadata`       | AniList GraphQL (catalog, search, schedule, user collection), AniZip/animap episode metadata, anime-offline-database, filler lists.                                                                                                         |
|              | `auth`           | AniList token lifecycle: keychain-only storage, PIN login URL, JWT expiry, Viewer account state and sign out. Tokens never reach settings, the DB or logs (ADR 0002).                                                                       |
|              | `tracker`        | `Platform` trait + AniList, MyAnimeList, and **local** (account-free) platforms. Offline platform wraps cached snapshots.                                                                                                                   |
|              | `library`        | Folder scanner, matcher (title normalization + similarity), file watcher, auto-scan, shelving of offline drives, scan summaries, library explorer ops.                                                                                      |
|              | `torrent`        | Embedded `librqbit` session, head+tail piece priority, file selection in batches, stream reader, disk handoff; qBittorrent/Transmission/rqbit clients.                                                                                      |
|              | `search`         | Torrent indexers (Nyaa, AnimeTosho, extension-provided), result analyzer, batch detection, auto-select scoring profiles, availability checks.                                                                                               |
|              | `debrid`         | Real-Debrid, TorBox, AllDebrid, Premiumize behind one `DebridProvider` trait: cache check, add, resolve, download-to-library.                                                                                                               |
|              | `relay`          | Loopback server: torrent range endpoint, relay proxy (headers, cookies, HLS/DASH manifest rewrite, segment prefetch, ranged sub-requests).                                                                                                  |
|              | `player`         | Player detection per OS, detached + supervised spawn, fresh IPC endpoint per launch, mpv/IINA JSON IPC, VLC HTTP, MPC-HC web API, playback events.                                                                                          |
|              | `extensions`     | Luau sandbox through `mlua` (one VM per extension), declarative sources, extension manifest + repository + marketplace, provider adapters (torrent, online stream, custom source, tracker, plugin).                                         |
|              | `onlinestream`   | Online episode/server resolution through extensions, sub/dub, stale-URL refresh then provider fallback, manual provider mapping.                                                                                                            |
|              | `autodownloader` | Rules, profiles, delay timers, multi-indexer comparison, approve/reject queue, history.                                                                                                                                                     |
| Composition  | `engine`         | Owns the Tokio runtime and every module. Live reload on settings save, event bus, cron jobs, continuity + playback manager, playlists, OS notifications, Discord RPC, offline sync, updater, logs/crash reports.                            |
| UI           | `ui-theme`       | Design tokens, theme presets, mapping onto `gpui-component` `Theme`, the full appearance-settings model.                                                                                                                                    |
|              | `ui-components`  | Presentational widgets that take view models: media card, episode card, carousel, hero banner, poster grid with a bounded image cache, sidebar rail (our own, not `gpui-component`'s `Sidebar`), playback HUD, skeletons.                   |
|              | `ui-state`       | Tokio bridge, stores (`Entity<…Store>`), router, actions + keybindings, event pump.                                                                                                                                                         |
|              | `ui-screens`     | One module per route (home, entry, discover, search, lists, schedule, torrents, debrid, autodownloader, extensions, settings, …).                                                                                                           |
|              | `ui-video`       | Phase 9 only: embedded libmpv surface. Behind a cargo feature so builds without libmpv still work.                                                                                                                                          |
| Binary       | `autumn`         | `main.rs`: logging, engine start, `gpui_kit::application()`, the GPUI `HttpClient` adapter over `net`'s client, window open, asset bundle. App identity and packaging metadata (§8).                                                        |

The existing `crates/autumn` crate becomes the binary in Phase 0.

The thin `engine` owns the Tokio-side services: settings rows, the database,
the shared client, the poster and metadata caches, and the Local platform.
The `autumn` binary still builds the Tokio runtime and resolves the app paths;
that startup work moves into the full engine in Phase 2.

## 3. Dependency rules

These are what keep compile times and coupling down. Each rule is checkable.

1. **Only `ui-*` and `autumn` depend on `gpui-kit`.** Back-end crates never import GPUI. Check with `cargo tree -i gpui-pre`: no back-end crate appears.
2. **Only `engine` knows every module.** Integration crates depend on `domain` plus the foundation crates. They depend on another integration crate only along the edges below.
3. **`ui-components` does not depend on `engine`.** Widgets take plain view-model structs, so a widget change never recompiles the back end and widgets can be tested in isolation.
4. **No cycles, no `pub use` re-export chains across layers.** A crate's public API is its own types plus `domain` types. One exception: `ui_state::events` (`channel()`, `start_pump`) exposes `flume`'s `Sender` and `Receiver`, because that channel is the engine → UI boundary (§4).

Allowed integration edges:

```text
library        → release-parser, mkv, metadata
search         → release-parser, extensions
torrent        → release-parser
debrid         → search (for the shared result type)
onlinestream   → extensions
autodownloader → search, torrent, debrid, tracker
tracker        → metadata
relay          → torrent
auth           → metadata (the authenticated Viewer account lookup)
engine         → mkv (chapter skip markers for local files)
engine         → search (auto-select pipeline #218)
engine         → torrent (auto-select file lists #218)
```

Foundation, UI and binary edges in use (rules 1–3 already allow them; they're listed so a new one shows up in review):

```text
db            → domain
auth           → domain, net
engine         → domain, net, db, metadata, tracker, settings, library, player, mkv (library watcher #123, scan module + playback manager #127/#128, chapter skip #131)
library        → domain, db
metadata      → domain, net
player        → domain
search        → net (indexer HTTP through the shared client, §6)
search        → settings (ranks with the persisted AutoSelectProfile, #218)
tracker       → domain, db
ui-state      → domain, engine, metadata, tracker, db (the one active platform is exposed to screens through ui-state via opaque handles; db and metadata let it own gateway construction without screens naming tracker)
ui-components → (gpui-kit only)
ui-screens    → ui-components, ui-state, db, metadata, domain, settings, futures, time (ui-theme as a dev-dependency only)
autumn        → engine, settings, db, domain, metadata, net, player, auth, tracker, ui-state, ui-screens, ui-theme (the composition root builds the active platform)
```

Screens never depend on `engine`: they reach its services only through the
ui-state stores. Engine fakes for screen tests live in `ui_state::test_support`
(behind the `test-util` feature), not in `ui-screens`.

Adding an edge means changing these lists in the same commit, with the reason in the commit body.

## 4. Runtime model: GPUI ↔ Tokio

GPUI runs its own executor on the main thread. `reqwest`, `librqbit`, `sqlx` and the IPC code need Tokio. The bridge is ours, in `ui_state::bridge`:

- **Why not `gpui-tokio-bridge`:** version 0.1.0 depends on `gpui 0.2.x`, while `gpui-kit 0.10` pins `gpui-pre =0.3.12`. Those are different crates with incompatible types. `gpui-kit 0.10` has no Tokio feature or bridge of its own ([research](docs/research/gpui-kit-shell.md)). The bridge is about 50 lines, so we own it. If a later gpui-kit release ships a Tokio integration, we switch to it.
- **Runtime:** `autumn` builds a multi-thread Tokio runtime before `gpui_kit::application()` and stores its `Handle` in a GPUI `Global`.
- **Commands (UI → engine):** `bridge::run(cx, future) -> Task<Result<T>>` calls `handle.spawn(future)` and awaits the `JoinHandle` on GPUI's background executor. A `JoinHandle` can be polled from any executor. The returned `Task` goes into the owning entity (`_search: Option<Task<()>>`). Replacing it drops and cancels the previous request.
- **Events (engine → UI):** the engine publishes `AppEvent` on a bounded `flume` channel. One foreground task started at boot runs `recv_async().await` in a loop, routes each event to its store with `entity.update(cx, …)`, and the store calls `cx.notify()`. `flume` works under any executor.
- **Throttling:** high-frequency sources (torrent stats, player `time-pos`, scan progress) are coalesced in the engine to at most 4 Hz before they reach the channel.
- **Rule:** `render()` never blocks or performs I/O. Views use `cx.observe` / `cx.subscribe` and never poll.

## 5. UI architecture

- **Bootstrap order:** `gpui_kit::application().with_assets(…).with_http_client(adapter)` (the adapter needs only the Tokio `Handle`, built earlier) → `gpui_kit::init(cx)` → bridge init → `ui_theme::init(cx, &settings.ui)` → stores + event pump → keybindings → `gpui_kit::open_window(…)` with a transparent titlebar (macOS traffic lights at `(14, 13)` in the 40px bar). Without the adapter, GPUI's default client fails every `img()` URL.
- **Shell:** `AppShell` = titlebar + sidebar rail + route view + optional `PlaybackHud`. Overlays (dialogs, sheets, notifications, popovers) are hosted by `gpui-component`'s `Root`, which `gpui_kit::open_window` installs.
- **Titlebar:** 40px and transparent, built on `gpui-component`'s `TitleBar` with `.h(px(40.))`. It holds back/forward buttons and a search pill that opens the command palette. No route title.
- **Sidebar:** our own 64px icon rail that expands to 260px, not `gpui-component`'s `Sidebar` (that one collapses to a fixed 48px and has no hover expand). By default it expands on hover after a 150ms delay, overlaying the content so the layout slot stays 64px and poster grids don't reflow, and it closes as soon as the pointer leaves. A click-pin setting makes it push the content instead. Items appear only when their feature is enabled. Each item has a live badge (missing episodes, active torrents, queue size, extension updates): a count pill on the icon when collapsed, a trailing pill when expanded. Items that don't fit the window height, and items the user unpins (right-click → Unpin), share one "More" popup menu, which highlights when the current route is inside it. The bottom system group (Extensions, Sync, Settings, account) never folds.
- **Router:** `enum Route` (Home, Entry { id, tab }, Discover, Search(filters), Lists { status }, Schedule, Torrents, Debrid, AutoDownloader, ScanSummaries, Extensions, Sync, CustomSources, Settings { tab }). `RouterState` holds back/forward stacks and a scroll offset per route key, so Back restores position. `cmd-[` / `cmd-]` and the mouse back/forward buttons map to it. Scroll restore saves `ScrollHandle::offset()` per route key and calls `set_offset` on return.
- **Command palette:** `gpui-component` command/list inside a dialog. Opens with `cmd-j`/`ctrl-j` or `q`; `s` opens it in `/search` mode. Slash commands swap the item set: `gpui-component`'s `Command` has no delegate, so items are passed on each render. Pasting a `magnet:` link while no text input has focus opens it in `/magnet` mode, skipping straight to episode pick when already on an entry.
- **Widgets map onto `gpui-component` first:** `command`, `carousel`, `virtual_list`, `table`, `tab`, `dialog`, `sheet`, `notification`, `hover_card`, `menu`, `skeleton`, `setting`, `tree` (library explorer), `diff` (extension update review), `color_picker` (theme), `time` (calendar). Only one `sheet` can be open at a time, and `carousel` has no autoplay. Drop to raw `div()`/`canvas()` only for media visuals: poster gradients, blurred banners, seekbar chapter ticks.
- **Grids:** poster grids are virtualized as rows of a `v_virtual_list` (`gpui-component` has no virtual grid). The column count comes from container width, so a 2,000-entry list renders only visible rows.
- **Poster images:** `img()` loads go through the `HttpClient` adapter in `autumn`, which runs `net`'s shared client on the bridge's Tokio handle, so `net`'s disk cache serves repeats. GPUI's in-memory image cache never evicts, so each poster grid wraps its images in a bounded LRU from `ui-components` (~256 decoded images) that drops the least recent with `cx.drop_image`. Posters use AniList's `coverImage.extraLarge`, falling back to `large`, then `medium`. Banners use `bannerImage`.
- **Theme:** tokens live in `ui-theme` (background `#070707`, paper `#0B0B0B`, elevated `#0D0D0D`, hairline border `rgba(255,255,255,0.10)`, accent `#6152DF`, radius 8px). Saving appearance settings updates `Theme::global_mut(cx)` and refreshes windows, with no restart.
- **Key contexts:** `Workspace`, `CommandPalette`, `Player`. Actions are declared with `actions!` per context.

## 6. Back-end subsystems

**Settings and live reload.** Settings are typed structs per tab (serde defaults handle new fields). Phase 0 stores them in `settings.json` in the config dir; its absence means first run. In Phase 1, a one-time migration copies it into JSON rows in `autumn.db` and retires the file. On save, the engine diffs old and new and restarts only the affected modules: player, torrent client, watcher, Discord RPC, debrid.

**Persistence.** `sqlx` with SQLite in WAL mode, migrations embedded with `sqlx::migrate!`. Secrets (AniList token, MAL token, debrid keys, qBittorrent password) go in the OS keychain (`keyring`), never in SQLite or logs. With no keychain there is no file fallback: AniList login is refused (ADR 0002). The disk cache (`posters/`, `metadata/`, and `tmp/` for incomplete downloads) lives in the OS cache dir and is always safe to delete. Posters and metadata blobs share a ~500 MB cap, evicting the oldest by mtime first. Settings → Storage has a "Clear poster cache" button. Catalog data is cached only for what the user views: AniList's terms forbid crawling.

**Tracking platforms.** `trait Platform` covers collection fetch, entry update, progress update and delete. Implementations: AniList, MyAnimeList, Local (no account: collections stored in SQLite), Offline (read-only snapshots + queued changes). The engine holds one active platform, and switching is a settings change.

**Library.** Scan = walk folders → parse filenames → group → match against the user's collection and the offline anime database → hydrate episode numbers (AniZip/animap, specials/NC separation) → persist. Locked files are never re-matched. Files on a disconnected drive are shelved, not deleted. Every scan writes a summary.

**Playback.** `PlaybackManager` takes a `StreamSource` (local path, torrent, debrid URL, online URL + headers), turns it into a URL the player can open (directly, or through the loopback relay), launches the player with resume position, follows IPC events, writes continuity, marks watched at the configured threshold or EOF, syncs progress to the platform, and offers the next episode. Players run detached under a supervisor, so closing a window never kills playback and an app crash never leaves an orphaned socket.

**Torrent streaming.** librqbit session with file-based storage. When a stream starts, pieces at the head and tail of the selected file are prioritized so container headers and seek index arrive first. The loopback endpoint serves `Range` requests from the torrent stream reader. Once complete, playback switches to the file on disk. Dropping a stream removes its files.

**Extensions.** Extensions are Luau scripts run through `mlua`, or declarative sources (a manifest of URL templates and selectors, no code); a WASM runtime may follow behind the same provider API (ADR 0003). Each Luau extension gets its own `Lua` VM with a memory limit and an interrupt-based time limit, and awaited host futures get their own Tokio timeout. mlua's default filesystem `require` survives `sandbox(true)`, so it is replaced with ours before sandboxing. Network access goes through `net` with the outbound guard (no private or loopback addresses), and there is no filesystem access. Extensions declare a type (torrent provider, online stream provider, manga provider, custom source, tracker, plugin) and a manifest with version and permissions. Updates show a diff before install.

**Security.** Loopback endpoints bind `127.0.0.1` only and put a random per-session token in every path, so other local processes can't use them. Logs redact tokens and paths on export. The updater verifies signatures before applying.

## 7. Errors, logging, testing

- **Errors:** each library crate has its own `thiserror` enum. `anyhow` is allowed only in the `autumn` binary. Workspace lints deny `unwrap`, `expect`, `panic`, indexing, and printing. Tests may unwrap (`clippy.toml`).
- **Logging:** `tracing` with a rolling file appender in `<data>/logs/`: files named `autumn.YYYY-MM-DD.log` (UTC), rotated daily, the last 7 kept. INFO goes to file and WARN to stderr; `RUST_LOG` overrides both. Secret values are never logged, only their presence. The in-app log viewer reads that directory. Crash and issue bundles replace home and library paths with placeholders and strip known secrets.
- **Tests:** pure crates (`release-parser`, `mkv`, scoring, matching) get table-driven unit tests from real release names. HTTP integrations test against recorded fixtures, never live services in CI. UI uses `gpui-kit`'s `test-support` (`#[gpui_kit::test]`, headless windows). Run with `cargo nextest`.

## 8. Platforms and packaging

macOS (Apple silicon), Linux (x86_64, aarch64) and Windows. The Nix dev shell covers the first three. Packaging targets: macOS `.app` + DMG, Linux AppImage (plus a Nix package), Windows MSI. Player detection searches the standard install locations per OS (Homebrew, MacPorts, `/Applications`, Flatpak/Snap, WinGet/Scoop/Chocolatey, registry).

**App identity.** The bundle id is `dev.buildxlabs.autumn`: the macOS bundle id, Linux App ID and Windows MSI identity. App paths come from `directories::ProjectDirs::from("dev", "buildxlabs", "Autumn")`, and all three are created at startup. Linux honors the XDG variables.

| OS      | Config                                                | Data                                                  | Cache                                    |
| ------- | ----------------------------------------------------- | ----------------------------------------------------- | ---------------------------------------- |
| macOS   | `~/Library/Application Support/dev.buildxlabs.Autumn` | `~/Library/Application Support/dev.buildxlabs.Autumn` | `~/Library/Caches/dev.buildxlabs.Autumn` |
| Linux   | `~/.config/autumn`                                    | `~/.local/share/autumn`                               | `~/.cache/autumn`                        |
| Windows | `<Roaming>\buildxlabs\Autumn\config`                  | `<Roaming>\buildxlabs\Autumn\data`                    | `<Local>\buildxlabs\Autumn\cache`        |

Settings are rows in `autumn.db`. The Phase 0 `settings.json` in Config is imported once at startup and renamed to `settings.json.imported` (or `settings.json.corrupt` if it can't be parsed); nothing writes it afterwards. Data holds `autumn.db` (with its WAL sidecars) and `logs/`.
