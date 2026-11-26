# Roadmap

The master plan for Autumn: decisions, stack, phases with exit criteria, and a coverage matrix that accounts for every feature in the research. [ARCHITECTURE.md](ARCHITECTURE.md) describes the shape of each crate. This file says when it gets built.

Scope: **native desktop only** (macOS, Linux, Windows). No web app, no server mode, no mobile.

Last reviewed: 2026-11-26.

**Manga is not planned for now.** The former Phase 7 sits in the [Backlog](#backlog-ideas-from-the-other-references); the app is anime-only. Phase numbers are unchanged.

**Tracking.** Each phase has a GitHub milestone, a tracking issue whose sub-issues are its build issues, and a column on the project board. Decisions a phase needs before it is ticketed are worked on a `wayfinder:map` issue. See [docs/agents/issue-tracker.md](docs/agents/issue-tracker.md).

## 1. Decisions

### Made

| #   | Decision                                                                                                                                  | Why                                                                                                                                                                                                                                     |
| --- | ----------------------------------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| D1  | Rust + GPUI through **`gpui-kit` 0.10** as the single UI dependency                                                                       | It pins matching `gpui-pre`, `gpui-base`, `gpui-component` and assets, so there are no duplicate-type mismatches.                                                                                                                       |
| D2  | Multi-crate workspace (about 25 crates, created phase by phase)                                                                           | Editing UI never recompiles librqbit or Luau, and back-end crates never see GPUI. See ARCHITECTURE §2–3.                                                                                                                                |
| D3  | Own Tokio bridge, not `gpui-tokio-bridge`                                                                                                 | `gpui-tokio-bridge 0.1.0` depends on `gpui 0.2`, while `gpui-kit` uses `gpui-pre 0.3.12`. The types are incompatible. `gpui-kit 0.10` ships no Tokio integration; switch if a later release does.                                       |
| D4  | Playback through an **external mpv/IINA/VLC/MPC-HC** first, embedded libmpv later (Phase 9)                                               | Native players already handle 10-bit HEVC, AV1 and ASS subtitles. Embedding is the riskiest piece, so it comes after the product works.                                                                                                 |
| D5  | No transcoding pipeline and no ffmpeg dependency                                                                                          | Every source reaches the player directly or through the loopback relay, and mpv/IINA/VLC play 10-bit HEVC, AV1 and ASS natively. Transcoding only matters for browsers and casting, both out of scope.                                  |
| D6  | SQLite via `sqlx` (WAL, embedded migrations). Secrets in the OS keychain only                                                             | Async fits the Tokio engine. Tokens never land in the DB or logs. Without a keychain, AniList login is refused ([ADR 0002](docs/adr/0002-refuse-anilist-login-without-keychain.md)).                                                    |
| D7  | Account-free use is first-class (local tracking platform). AniList is bring-your-own-account                                              | The app must be fully usable without AniList/MAL login. The public AniList catalog needs no account either way. Whether AniList's terms allow an account-free tracker is open in [#21](https://github.com/buildxlabs/autumn/issues/21). |
| D8  | Content sources ship as **extensions**: Luau via `mlua`, plus declarative sources; WASM later; no JavaScript. The app bundles no scrapers | Keeps the core small and legally clean. Sources break and get fixed without an app release. Luau's built-in sandbox avoids maintaining a JS barrier ([ADR 0003](docs/adr/0003-extensions-run-luau-with-our-own-api.md)).                |
| D9  | License: **GPL-3.0-only** (`LICENSE`). Private repo, non-commercial, shared with friends                                                  | Lets us port code from the GPL-3.0 references. Ported files keep their original notice. AGPL references stay behavior-only.                                                                                                             |
| D10 | Extensions target **our own manifest and host API** (was O2)                                                                              | No compatibility layer for an existing community format. The extensions we need are ported by hand ([ADR 0003](docs/adr/0003-extensions-run-luau-with-our-own-api.md)).                                                                 |
| D11 | MSRV is **Rust 1.100**, the toolchain channel (was O3)                                                                                    | `rust-toolchain.toml`, `rust-version` in `Cargo.toml` and `flake.nix` all pin 1.100. One toolchain, no older-compiler support.                                                                                                          |
| D12 | Media are identified by Autumn's own `MediaId`, minted on the first user fact                                                             | Keeps AniList, MAL, AniDB, Kitsu and custom sources equal; external ids are cross-references ([ADR 0001](docs/adr/0001-own-media-id-minted-on-first-user-fact.md)).                                                                     |

### Open (resolve before the phase that needs them)

| #   | Question                                                                                                  | Needed by | Default if nobody decides                                                  |
| --- | --------------------------------------------------------------------------------------------------------- | --------- | -------------------------------------------------------------------------- |
| O4  | Embedded player approach: mpv `--wid` into a child native view, or libmpv render API into a GPUI texture? | Phase 9   | Prototype `--wid` first: it is simpler, and VortX desktop proves it works. |
| O5  | Watch party: own protocol, or Syncplay-compatible?                                                        | Phase 10  | Syncplay-compatible, so it works with existing servers.                    |

## 2. Stack

Versions checked on crates.io on 2026-11-21. Pin them in `[workspace.dependencies]` when the crate that needs them is created.

| Concern             | Crate(s)                                                                                                  | Notes                                                                                                                     |
| ------------------- | --------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------------------------------------------------------------- |
| UI                  | `gpui-kit 0.10` (→ `gpui-pre =0.3.12`, `gpui-component 0.10`)                                             | Use `test-support` in dev-deps. Optional `gpui-fast` backend later.                                                       |
| Async               | `tokio 1.54`, `flume 0.12`                                                                                | Tokio for the engine. flume for engine → UI events.                                                                       |
| HTTP                | `reqwest 0.13` (rustls), `axum 0.8` / `hyper 1`                                                           | axum serves the loopback endpoints.                                                                                       |
| GraphQL             | `cynic 3.14`                                                                                              | AniList schema-typed queries.                                                                                             |
| Storage             | `sqlx 0.9` (sqlite), `rmp-serde 1.3`, `keyring 4`                                                         | MessagePack for cached metadata blobs.                                                                                    |
| Torrent             | `librqbit 9.1`                                                                                            | Embedded engine and stream reader.                                                                                        |
| Extensions          | `mlua 0.12` (features `luau`, `async`, `send`; not `vendored`)                                            | One Luau VM per extension, with memory and interrupt limits. `luau` already builds from source, so `vendored` is dropped. |
| Player (embedded)   | `libmpv2 7`                                                                                               | Phase 9 only, behind a cargo feature.                                                                                     |
| Files               | `notify 9`, `directories 6`                                                                               | Watcher and OS paths.                                                                                                     |
| Parsing             | `scraper 0.29`, `quick-xml 0.42`, `strsim 0.11`, `nucleo-matcher 0.3`                                     | HTML indexers, RSS, title similarity, palette fuzzy search.                                                               |
| Desktop integration | `discord-rich-presence 1.1`, `notify-rust 4`                                                              | RPC and OS notifications.                                                                                                 |
| Observability       | `tracing 0.1`, `tracing-subscriber 0.3`, `tracing-appender 0.2`                                           | Rolling file logs and the in-app viewer.                                                                                  |
| Errors              | `thiserror 2`                                                                                             | `anyhow` only in the binary.                                                                                              |
| Tooling (exists)    | clippy (strict workspace lints), rustfmt, taplo, typos, cargo-deny, cargo-nextest, Vite+ hooks, Nix flake | Already set up in the repo.                                                                                               |

## 3. Phases

Each phase ends on an **exit criterion**: something you can run or observe. A phase is done when every box is checked and `pnpm check` plus `pnpm test:rust` pass.

Phases 0 and 1 overlap. Workspace setup and the Tokio bridge come first. Then the Phase 0 shell and the Phase 1 back-end crates (`net`, `db`, `metadata`, `tracker`) proceed in parallel. Phase 1 screens wait for the shell.

Phases 2 and 3 also overlap: the remaining Phase 2 screens are built while the Phase 3 decisions are locked on the map [Autumn: Phase 3 ready to build](https://github.com/buildxlabs/autumn/issues/176), and Phase 3 build issues start as soon as it closes.

### Phase 0: Foundation and window shell

- [x] Resolve O3 (MSRV); record it in this file (D11: Rust 1.100).
- [x] Root `Cargo.toml`: `[workspace.dependencies]`, `[profile.dev.package."*"] opt-level = 2`, release profile (`lto`, `codegen-units = 1`, `strip`).
- [x] Turn `crates/autumn` into the binary. Create `domain`, `settings`, `ui-theme`, `ui-components`, `ui-state`, `ui-screens`.
- [x] Add `gpui-kit`; get `cargo deny check` green with the GPUI tree. With gpui-kit 0.10 it already exits 0, with 68 non-blocking duplicate-version warnings ([research](docs/research/cargo-deny-gpui-kit.md)); which exceptions to accept is open in [#22](https://github.com/buildxlabs/autumn/issues/22).
- [x] Tokio bridge + event pump (ARCHITECTURE §4) with a test that round-trips a command and an event.
- [x] Window: transparent 40px titlebar with back/forward and a search pill, our own sidebar rail with hover expand (click-pin setting) and "More" overflow, router with back/forward and scroll restore, theme tokens applied (ARCHITECTURE §5).
- [x] Logging to file. App paths created on first run (ARCHITECTURE §7–8).
- [x] CI workflow on pull requests only, running `pnpm check` and `cargo nextest`. Linux runs on every PR through the Nix dev shell. macOS and Windows run when the PR carries the `ci:macos` / `ci:windows` label, or on manual dispatch.
- [x] Pre-push hook refuses pushes to `main`. The org's Free plan has no server-side rulesets for a private repo, so the guard is local.
- **Exit:** `cargo run -p autumn` opens the themed shell on all three OSes. Navigating between placeholder routes and pressing Back restores scroll position.

### Phase 1: Catalog, tracking, and discovery

- [x] `net` (shared client, up to 3 retries, one rate limiter shared by every AniList request at its current degraded 30 req/min plus an unpublished burst limit, poster disk cache), `db` (migrations, settings rows imported once from the Phase 0 `settings.json`, media ids with cross-references to the AniList id and `idMal` only), `metadata` (AniList catalog/search/schedule/user collection; at most 50 items per page, and `MediaListCollection` stops at 11k entries and needs its custom lists read too), `tracker` (AniList + Local platforms).
- [x] AniList login (token paste + PIN URL), avatar (animated-GIF upgrade), sign out, rate-limit indicator (countdown). Token in keychain; login refused without one (ADR 0002). Re-login prompt before the one-year token expires.
- [x] Components: media card (badges, hover popup, context menu, skeleton), carousel (drag, arrows, Alt+wheel), hero banner, virtualized poster grid.
- [x] Screens: Home (fixed default widgets: continue-watching header, discover header, my lists, missed sequels, aired recently, anime carousel, centered title), Discover, Advanced Search, My Lists (status tabs, quick editor), basic Anime Entry (header, synopsis, characters, relations, recommendations, list editor).
- [x] Command palette: open/close, `/search`, `/anime`, `/manga` (`/manga` removed again with the manga UI in Phase 2).
- [x] Top indefinite loader during background fetches.
- [x] Settings screen with a Storage tab holding a "Clear poster cache" button.
- **Exit:** with no account, a user can browse, search, add a show to a local list and see it on Home after restart. With AniList, the same edits round-trip to AniList.

### Phase 2: Local library and external playback

- [x] `release-parser` with a corpus of 500+ real release names. `mkv` (chapters, tracks, fonts).
- [x] `library`: multi-folder scan, matcher with threshold, lock/unlock, specials/NCOP/NCED split, filler marking, watcher + auto-scan, shelving, scan summaries.
- [x] `player`: detection on all OSes, supervised detached spawn, fresh IPC path per launch, mpv + IINA IPC, VLC, MPC-HC.
- [x] `engine` v1: playback manager, continuity (resume after media-ready), watched at threshold/EOF, progress sync, auto-next with countdown, OP/ED chapter skip. An end-to-end engine test covers scan → play → resume → watched → Local progress, and shelving.
- [x] Screens: Entry Library tab (episode cards, range pagination, missing banner, bulk actions), Scan Summaries, Library Explorer (tree), Playback HUD. Home widgets that depend on the library.
- [x] First-run onboarding: library path, account choice, player choice.
- [x] Hide manga from the UI (rail item, `/manga`, Trending Manga carousel); keep the manga data model so AniList manga list entries survive sync and transfer.
- **Exit:** point at a folder of fansub releases. Matched shows appear, play in mpv, resume where they stopped, and progress updates on the active platform at the threshold. (Waits on the real-hardware run in #139.)

### Phase 3: Torrent streaming and search

- [ ] `torrent` (librqbit session, head+tail priority, batch file pick, disk handoff, cleanup on drop) and `relay` torrent endpoint (token-gated `Range`).
- [ ] `search`: Nyaa (+ mirror base URL, proxy) and AnimeTosho, analyzer, batch detection, auto-select profiles, availability badges.
- [ ] Screens: Entry Torrent tab (auto-select, manual search sheet with sortable table), buffer overlay (peers, speeds, readiness), Torrent List with live badges.
- [ ] Magnet paste interception + `/magnet` and `/droptorrent` palette commands. Next-episode preload.
- **Exit:** pick an airing episode, press auto-select, and playback starts in mpv while downloading. Seeking near the end works before the download finishes.

### Phase 4: Extensions

- [ ] `extensions`: Luau sandbox (one VM per extension, ADR 0003), declarative sources, limits, outbound guard, manifest, repository URLs, install/update/remove, settings per extension, error surfacing.
- [ ] Provider adapters: torrent provider (feeds `search`), online stream, custom source, tracker.
- [ ] Screens: Extensions (installed, marketplace, update diff review), extension playground (editor + run).
- [ ] Sidebar badge for updates/errors.
- **Exit:** install a torrent-provider extension from a repository URL; its results show in the manual search alongside Nyaa. A crashing or looping extension is killed and reported without freezing the app.

### Phase 5: Debrid, online streaming, relay

- [ ] `debrid`: Real-Debrid, TorBox, AllDebrid, Premiumize (cache check, resolve, download to library).
- [ ] `onlinestream`: provider select, sub/dub, servers, stale-URL refresh → fallback, manual mapping.
- [ ] `relay` proxy: header/cookie injection, HLS/DASH manifest rewrite, segment prefetch, ranged sub-requests.
- [ ] Screens: Entry Debrid tab (cached badges), Entry Online tab, Debrid dashboard. Optional merge of online availability into library view.
- **Exit:** a cached debrid torrent starts playing within a few seconds. An online stream whose URL has expired refreshes transparently before falling back to another provider.

### Phase 6: Automation and schedule

- [ ] `autodownloader`: rules, profiles, delay timers, multi-indexer comparison, approve/reject queue, history.
- [ ] External torrent clients: qBittorrent (re-auth), Transmission, rqbit. Custom shell-command dispatch. (Moved from Phase 3.)
- [ ] Engine: cron jobs (collection refresh, auto-downloader, availability), OS notifications, Discord Rich Presence, playlists.
- [ ] Screens: Auto-Downloader (rules, profiles, queue/history), Schedule (calendar + missing episodes with one-click actions), Playlists sheet. Badges for missing/queue.
- **Exit:** a rule for an airing show downloads the new episode after its delay window, preferring the higher-ranked group, and a notification fires.

### Phase 7: (moved to Backlog)

Manga was Phase 7. It is not planned for now; see the Backlog.

### Phase 8: Customization, offline, and polish

- [ ] Full appearance studio (all UI settings in §4 row "Theme & UI"), theme presets, custom background image, banner modes/sizes/info-box layouts, card options, default sorts.
- [ ] Spoiler protection (master + thumbnails, titles, descriptions, skip-next).
- [ ] Modular Home with all widget types + Home settings dialog (add, remove, reorder, per-widget filters and layout).
- [ ] Settings: every tab live-reloads affected modules. Unsaved-changes bar.
- [ ] Offline mode: snapshot selected entries, offline platform, queued changes synced on reconnect, offline sidebar.
- [ ] Custom sources screen. MyAnimeList platform. DNS-over-HTTPS setting.
- [ ] Diagnostics: log viewer, error explainer, anonymized issue report bundle, crash log, file-cache screen (extends the Phase 1 Settings Storage tab).
- [ ] Updater (GitHub releases, signature check, stable/beta channels) + announcements banner. Packaging for all three OSes.
- **Exit:** a new user installs from a release artifact, customizes theme and home, goes offline and keeps watching local files, then comes back online and the queued progress syncs.

### Phase 9: Embedded player

- [ ] Resolve O4. `ui-video`: libmpv surface inside the window, GPUI-drawn controls (seekbar with chapter ticks and hover preview, volume boost, track/subtitle menus, speed, stats overlay, screenshots, playlist drawer, info panel, skip buttons).
- [ ] Mini-player that keeps playing while browsing. Anime4K shader presets. Intro/outro skip from an online skip-time database in addition to chapters.
- **Exit:** a 10-bit HEVC MKV with ASS subtitles plays in-window with the same continuity behavior as the external player, and the mini-player survives route changes.

### Phase 10: Social (optional)

- [ ] Resolve O5. Watch party (host/peer, synced play/pause/seek across local, torrent, debrid and online sources) with chat. Library sharing between peers.
- **Exit:** two machines watch the same episode in sync, and a seek on the host propagates within one second.

## 4. Feature coverage matrix

Every feature from the research documents and the reference code is listed here with a phase, or marked out of scope with a reason. When a feature is added or dropped, update this table.

| Area                | Feature                                                                                                                                               | Phase                                              |
| ------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------- |
| Library             | Multi-directory library, fuzzy scanner without naming rules                                                                                           | 2                                                  |
|                     | Release filename parser; one matcher with a configurable threshold (default 0.5), no legacy fallback (OD-4)                                           | 2                                                  |
|                     | File watcher + auto-scan; shelving on drive disconnect                                                                                                | 2                                                  |
|                     | Library explorer (tree, lock/unlock, re-match, rename, delete); bulk file actions                                                                     | 2                                                  |
|                     | Scan summaries with confidence + unmatched list                                                                                                       | 2                                                  |
|                     | Specials / OVA / NCOP / NCED separation; filler marking                                                                                               | 2                                                  |
|                     | Local subtitle file discovery next to videos                                                                                                          | 2                                                  |
| Playback sources    | Local direct play                                                                                                                                     | 2                                                  |
|                     | Torrent streaming: head+tail priority, batch file pick, preload next, disk handoff, cleanup, live overlay                                             | 3                                                  |
|                     | Debrid streaming: 4 providers, cached detection, download to library, dashboard                                                                       | 5                                                  |
|                     | Online streaming via extensions: sub/dub, servers, stale-URL refresh, manual mapping, merge into library                                              | 5                                                  |
|                     | Loopback relay: header injection, HLS/DASH rewrite, prefetch, ranged sub-requests                                                                     | 5                                                  |
| Players             | External mpv, IINA, VLC, MPC-HC; detection per OS; fresh IPC per launch; detached + supervised                                                        | 2                                                  |
|                     | Continuity (resume after media-ready), watched at threshold/EOF, progress sync, auto-next countdown                                                   | 2                                                  |
|                     | Chapter OP/ED skip with custom patterns                                                                                                               | 2                                                  |
|                     | Embedded libmpv player + full control chrome, mini-player, stats, screenshots, Anime4K                                                                | 9                                                  |
|                     | Online skip-time database (intro/outro)                                                                                                               | 9                                                  |
|                     | Subtitle translation of embedded tracks                                                                                                               | Backlog                                            |
| Torrents            | Search drawer: indexers, resolution/codec/group/seeders/size/date, batch badge, availability                                                          | 3                                                  |
|                     | Auto-select scoring profiles                                                                                                                          | 3                                                  |
|                     | Built-in client, torrent list with live badges                                                                                                        | 3                                                  |
|                     | qBittorrent (re-auth), Transmission, rqbit, custom shell command                                                                                      | 6                                                  |
|                     | Mirror base URL + proxy per indexer; infohash-only magnet copy                                                                                        | 3                                                  |
|                     | Auto-downloader: rules, profiles, delay timers, multi-indexer, queue approve/reject, history                                                          | 6                                                  |
| Metadata & tracking | AniList: login, avatar, collection sync, full list editor, manual refresh                                                                             | 1                                                  |
|                     | Local account-free tracking                                                                                                                           | 1                                                  |
|                     | AniZip / animap episode metadata; anime-offline-database for matching                                                                                 | 1–2                                                |
|                     | MyAnimeList platform                                                                                                                                  | 8                                                  |
|                     | Tracker extensions (sync to other services)                                                                                                           | 4                                                  |
|                     | Custom sources (non-AniList entries)                                                                                                                  | 8                                                  |
| Discovery           | Discover carousels (trending, popular, upcoming, movies, schedule, missed sequels, by country); the manga carousel is removed                         | 1                                                  |
|                     | Advanced search with all filters + scroll restore                                                                                                     | 1                                                  |
|                     | My Lists with status tabs, custom lists, quick edit                                                                                                   | 1                                                  |
|                     | Schedule calendar + missing episodes                                                                                                                  | 6                                                  |
|                     | Playlists (cross-series queues)                                                                                                                       | 6                                                  |
| Manga               | Hub, entry, provider compare, refresh, chapter downloads, local manga, reader modes, progress sync, MangaUpdates, manga Home widgets                  | Backlog                                            |
| Extensions          | Sandbox runtime, repository, marketplace, update diffs, playground, health badge                                                                      | 4                                                  |
|                     | Plugin type: hooks, injected UI actions, tray/sidebar items                                                                                           | Backlog (after 4; needs a stable UI-injection API) |
| Shell & UX          | Custom titlebar, hover-expand sidebar, badges, overflow "More", unpinning, back/forward                                                               | 0                                                  |
|                     | Command palette + slash commands + magnet paste interception                                                                                          | 1, 3                                               |
|                     | Media cards (badges, hover popup, context menu, skeleton); episode cards; carousels (drag, Alt+wheel)                                                 | 1–2                                                |
|                     | Entry page: default banner mode (Large, Fluid) in Phase 1; 6 other banner modes, Small, Boxed, blurred background in 8; source tabs with their phases | 1, 2, 3, 5, 8                                      |
|                     | Home: fixed set of 7 widgets in Phase 1; library/schedule/manga widgets with their phases; modular home (15 widget types) + settings dialog in 8      | 1, 8                                               |
|                     | Spoiler protection suite (5 toggles)                                                                                                                  | 8                                                  |
|                     | Theme & UI studio (colors, presets, sidebar, backgrounds, banners, cards, sorting)                                                                    | 8                                                  |
|                     | Route cross-fade; reduced-motion respect                                                                                                              | 8                                                  |
|                     | Onboarding wizard                                                                                                                                     | 2                                                  |
|                     | Feature tour; announcements banner                                                                                                                    | 8                                                  |
| System              | Settings with live reload (all tabs)                                                                                                                  | 8                                                  |
|                     | Offline mode + snapshot sync                                                                                                                          | 8                                                  |
|                     | Discord Rich Presence; OS notifications; cron jobs                                                                                                    | 6                                                  |
|                     | DNS-over-HTTPS; outbound request guard; user-agent pool                                                                                               | 1, 4, 8                                            |
|                     | Log viewer, error explainer, anonymized issue report, crash log, file-cache screen                                                                    | 8                                                  |
|                     | Auto-updater with channels + signature verification                                                                                                   | 8                                                  |
| Social              | Watch party with chat; library sharing                                                                                                                | 10                                                 |

### Out of scope (desktop-only product)

| Feature                                                          | Why it is out                                                  |
| ---------------------------------------------------------------- | -------------------------------------------------------------- |
| Web UI, server port/host/password/CORS settings                  | No browser client. The UI is native.                           |
| FFmpeg HLS transcoding and hardware transcode settings           | mpv plays the formats natively (D5).                           |
| Browser subtitle renderers (ASS-in-JS, PGS-in-canvas)            | mpv/libass renders subtitles.                                  |
| Session recorder for bug reports                                 | DOM-specific. Replaced by the anonymized log bundle.           |
| Custom CSS                                                       | No CSS in GPUI. Replaced by theme presets and token overrides. |
| Mobile companion, mobile server, touch gestures, mobile intents  | Desktop only.                                                  |
| Cast / Chromecast, picture-in-picture                            | Revisit after Phase 9. The mini-player covers the in-app need. |
| Webview pages from plugins; headless-browser APIs for extensions | Would need an embedded browser. Revisit with the plugin type.  |

### Backlog: ideas from the other references

Not committed. Pull an item into a phase only when the core phases ship.

- Stremio-protocol add-ons as an extra source type (VortX, MovieBox-TUI).
- Trakt / SIMKL / Kitsu tracking (Mangayomi, VortX).
- Multiple local profiles with PIN (VortX, Zangetsu, dango).
- Backup/restore and library import/export (VortX, Mangayomi).
- Serverless device-to-device sync (Mangayomi `sync_host`).
- Multi-segment HTTP downloader with `.part` resume for direct links (MovieBox-TUI).
- Hardlink completed downloads into the library (Reiverr).
- Library insights / stats dashboard (dango, BookOrbit).
- IPTV / M3U playlists (VortX, MovieBox-TUI).
- Light novels (Mangayomi, Zangetsu).
- Manga, the former Phase 7: `manga` crate (provider chapters, compare, refresh, chapter downloads with AVIF, local CBZ/folders/PDF), MangaUpdates metadata, Manga hub and entry screens, the Reader (long strip, single, double, RTL/LTR, fit modes, zoom, keyboard, auto-hide HUD, progress sync), manga Discover carousel and Home widgets, `/manga` palette mode. The manga data model (`MediaKind::Manga`, AniList manga lists) stays in the code.

## 5. Research documents

The source research lives in [`docs/research/`](docs/research/):

- `STREAMING_PLATFORMS_RESEARCH.md`: the 14 reference apps compared feature by feature.
- `GPUI_STREAMING_ARCHITECTURE.md`: the original GPUI blueprint. Superseded by ARCHITECTURE.md where they differ (bridge crate, crate names, phase order).
- `SEANIME_UI_SPEC.md`, `seanime-complete-master-feature-list-backend-ui-ux.md`: screen-by-screen UI spec and feature list for the primary reference.
- `anilist-api.md`: AniList rate limits, auth, schema limits and ToS, checked for Phase 1.
- `gpui-kit-shell.md`: what gpui-kit 0.10 gives the Phase 0 shell, and its gaps.
- `mlua-luau-sandbox.md`: the Luau sandbox spike behind D8.
- `cargo-deny-gpui-kit.md`: what `cargo deny check` reports with gpui-kit 0.10 in the tree.

## 6. Reference map

Autumn is GPL-3.0-only (D9). Code from GPL-3.0, Apache-2.0 and MIT references may be ported, keeping the original copyright and license notice in the ported file. AGPL-3.0 references are for behavior only.

| Need                                                                                                | Look at                                                            | License    |
| --------------------------------------------------------------------------------------------------- | ------------------------------------------------------------------ | ---------- |
| Overall feature behavior, scanner, matcher, torrent streaming, auto-select, extensions, continuity  | `reference/seanime/internal/*`, UI in `seanime-web/src/app/(main)` | GPL-3.0    |
| mpv spawn/IPC, detached sessions, Lua tracker, relay proxy, multi-segment downloads, updater verify | `reference/MovieBox-Tui/src/{player,proxy,download,updater}`       | Apache-2.0 |
| Nyaa scraping, mirrors, torrent client dispatch                                                     | `reference/nyaa/src/{source,client}`                               | GPL-3.0    |
| mpv `--wid` embedding, child supervisor, loopback token gate                                        | `reference/VortX/desktop/src-tauri/src/player.rs`                  | GPL-3.0    |
| Reader modes, local tracker, device sync, serialized DB writes                                      | `reference/mangayomi/{lib,rust}`                                   | Apache-2.0 |
| QuickJS provider manifests                                                                          | `reference/Zangetsu/js_harness`, `providers`                       | GPL-3.0    |
| Watch-along rooms, bidirectional tracker merge                                                      | `reference/dango/server`                                           | MIT        |
| Hardlinks, on-demand episodes                                                                       | `reference/reiverr/backend`                                        | AGPL-3.0   |
| Library dock, metadata provider plug-ins                                                            | `reference/bookorbit/server`                                       | AGPL-3.0   |
