# Streaming, Anime & Media Platforms — Complete Feature & Workflow Reference (All 14 Projects)

A comprehensive, feature-by-feature study of all **14 open-source applications** in this workspace: what each app offers, how its UI and workflows are structured, how it connects to external providers and metadata services, how it acquires and plays/streams/downloads media, and all the subtle quality-of-life and edge-case behaviors built into each one.

---

## Quick Directory of All 14 Applications

| #   | Application             | GitHub Link                                                                                                  | Primary Media Types                                  | Platforms                                                                                      |
| --- | ----------------------- | ------------------------------------------------------------------------------------------------------------ | ---------------------------------------------------- | ---------------------------------------------------------------------------------------------- |
| 1   | **Seanime**             | [https://github.com/5rahim/seanime](https://github.com/5rahim/seanime)                                       | Anime, Manga                                         | Windows, macOS, Linux (Web + Desktop), iOS & Android (Companion + Mobile Server)               |
| 2   | **VortX**               | [https://github.com/VortXTV/VortX](https://github.com/VortXTV/VortX)                                         | Movies, TV, Anime, Live TV/IPTV, Audio               | Apple TV (tvOS), iPhone/iPad (iOS), macOS, Android & Android TV (Preview), Windows/Linux (WIP) |
| 3   | **MovieBox-TUI**        | [https://github.com/mesamirh/MovieBox-TUI](https://github.com/mesamirh/MovieBox-TUI)                         | Movies, TV, Anime, Asian Dramas, Live TV/IPTV        | macOS, Linux, Windows, Android (Termux)                                                        |
| 4   | **nyaa**                | [https://github.com/Beastwick18/nyaa](https://github.com/Beastwick18/nyaa)                                   | Anime & General Torrents                             | Linux, Windows, macOS (Terminal)                                                               |
| 5   | **Mangayomi**           | [https://github.com/kodjodevf/mangayomi](https://github.com/kodjodevf/mangayomi)                             | Manga, Webtoons, Comics, Light Novels, Anime, Movies | Android, iOS, macOS, Linux, Windows                                                            |
| 6   | **Zangetsu**            | [https://github.com/Spyou/Zangetsu](https://github.com/Spyou/Zangetsu)                                       | Anime, Movies, Series, Manga, Light Novels           | Android, Android TV, iOS (Beta), Windows (WIP)                                                 |
| 7   | **Zenshin**             | [https://github.com/hitarth-gg/zenshin](https://github.com/hitarth-gg/zenshin)                               | Anime                                                | Windows, Linux, macOS (Desktop / Web)                                                          |
| 8   | **dango**               | [https://github.com/serifpersia/dango](https://github.com/serifpersia/dango)                                 | Anime, Manga, TV/Movies, ASMR                        | Web (Self-Hosted Local), Windows, macOS, Linux, Android (Termux / Standalone APK)              |
| 9   | **LiSA**                | [https://github.com/Cosmicoppai/LiSA](https://github.com/Cosmicoppai/LiSA)                                   | Anime, Manga                                         | Windows, macOS, Linux                                                                          |
| 10  | **AniWorld-Downloader** | [https://github.com/phoenixthrush/AniWorld-Downloader](https://github.com/phoenixthrush/AniWorld-Downloader) | Anime, Series, Movies, Manga                         | Windows, macOS, Linux, Docker (CLI, TUI, Web UI)                                               |
| 11  | **Reiverr**             | [https://github.com/aleksilassila/reiverr](https://github.com/aleksilassila/reiverr)                         | Movies, TV Shows                                     | Self-Hosted Web (Docker), Samsung Smart TV (Tizen), Android TV / Fire TV                       |
| 12  | **bobarr-v2**           | [https://github.com/iam4x/bobarr](https://github.com/iam4x/bobarr)                                           | Movies, TV Shows                                     | Self-Hosted Web (Docker)                                                                       |
| 13  | **WeFlix_v2**           | [https://github.com/kweephyo-pmt/WeFlix_v2](https://github.com/kweephyo-pmt/WeFlix_v2)                       | Movies, TV Shows, Anime, K-Drama, C-Drama, Donghua   | Web Browser                                                                                    |
| 14  | **BookOrbit**           | [https://github.com/bookorbit/bookorbit](https://github.com/bookorbit/bookorbit)                             | Ebooks, PDFs, Comics, Audiobooks                     | Self-Hosted Web (Docker), iPhone, Apple Watch, Kobo, KOReader                                  |

---

## 1. Seanime

**GitHub**: [https://github.com/5rahim/seanime](https://github.com/5rahim/seanime)

### What It Is

Seanime is a self-hosted, local-first anime and manga media server with a full web interface, a dedicated desktop client (**Seanime Denshi**), and mobile companion/server apps (**Seanime Tenji**). It unifies four completely different ways of consuming anime—**local files**, **P2P torrent streaming**, **Debrid streaming**, and **online web streaming**—inside one consistent interface tied to AniList (plus custom non-AniList sources).

### Complete Feature Inventory

#### A. Library Management & Smart Local File Scanning

- **Flexible Folder Scanning**: Scans one primary library folder plus any number of additional library directories without requiring rigid Jellyfin/Plex naming conventions or folder structures.
- **Fuzzy Filename Parsing (`Habari`)**: Parses complex fansub filenames (release group, anime title, season, episode number, resolution, codec, dual-audio, batch tags) and matches them to AniList entries using configurable matching algorithms and similarity thresholds (including legacy matching fallback).
- **Library Explorer & File Locking**: Lets users inspect raw directories, manually re-match or lock files to specific entries, and view library storage stats.
- **External Drive Resilience ("Shelving")**: If an external hard drive containing part of the library is disconnected, locked files are temporarily "shelved" instead of deleted from the database, preserving all metadata and match state until the drive reconnects.
- **Automatic File Watching**: Watches configured library paths for newly added/removed files and triggers auto-scans automatically.

#### B. The 4 Unified Streaming & Playback Sources

1. **Local Media Streaming (`Mediastream`)**:
   - Inspects local file codecs and containers automatically.
   - **Direct Play**: Streams compatible files directly to the browser or client with zero transcoding.
   - **On-the-Fly HLS Transcoding**: Uses FFmpeg (with hardware acceleration such as Low-power Intel QSV) to transcode incompatible codecs (e.g., 10-bit HEVC or unsupported audio) into browser-playable HLS on the fly.
   - **Embedded Subtitle & Font Extraction**: Extracts MKV subtitle tracks and embedded fonts (even for filenames containing spaces) so styled anime subtitles render accurately in web browsers.
2. **Torrent Streaming (`Torrentstream`)**:
   - Streams episodes directly from BitTorrent swarms without waiting for the entire file to finish downloading.
   - **Head + Tail Piece Prioritization (`setPriorityDownloadStrategy`)**: Prioritizes downloading both the beginning pieces (container header/init) and ending pieces (MKV cues/chapters/seek table) first so playback starts quickly and seeking works smoothly.
   - **Batch Detection & File Selection**: Automatically detects season batch torrents and picks the exact episode file inside a multi-file torrent.
   - **Auto-Select Profiles**: Automatically chooses the best torrent based on user-ranked rules (preferred providers/indexers, release groups, resolutions, codecs, sources, languages, and blocked/avoided terms).
   - **Next-Episode Preloading**: Optional preloading of the next episode's torrent stream while watching the current one.
   - **Seamless Disk Handoff**: Once a streamed torrent finishes downloading, replays stream directly from the file on disk; dropping a torrent immediately cleans it from disk.
3. **Debrid Streaming (`Debridstream`)**:
   - Integrates with **Real-Debrid**, **Torbox**, **AllDebrid**, and **Premiumize**.
   - Checks whether a torrent (single or batch) is already cached on the Debrid provider and launches an instant direct HTTPS stream—eliminating P2P buffering and IP exposure.
   - Also supports downloading resolved files from Debrid directly into the local library.
4. **Online Streaming (`Onlinestream`)**:
   - Streams episodes directly from web streaming sites through installable extensions.
   - Supports sub/dub selection, multiple servers/mirrors, and **stale URL refresh** (automatically refreshing expired stream URLs before falling back to another provider).
   - Option to merge online streaming availability right into the main library view.

#### C. Built-In & External Video Players

- **VideoCore (Built-in Web Player)**:
  - Custom HTML5 video player supporting **HLS.js**, **JASSUB** (full ASS/SSA anime subtitle typesetting in the browser), **PGS Blu-ray bitmap subtitles** (`VideoCorePgsRenderer`), mobile touch gestures, mini-player mode (with View Transitions), screenshot directory saving, keyboard shortcuts, and custom chapter/OP/ED auto-skip rules (skip buttons + timeline highlighting).
- **MpvCore (Built-in Desktop Player in Seanime Denshi)**:
  - Embedded `libmpv` player (`mpv-prism`) inside the desktop app with native 10-bit HEVC/AV1 decoding, full `libass` subtitle rendering, GLSL shaders (e.g., Anime4K), dual-GPU support, and log export from settings.
- **External Media Players**:
  - First-class control of **MPV**, **VLC**, **MPC-HC**, and **IINA** (macOS), plus mobile player intents/deep links (Outplayer, VLC) on iOS/Android.
  - **Playback Manager**: Polls/monitors player status, restores watch continuity timestamps (waiting until MPV/IINA media is actually ready before seeking), auto-plays next episodes, updates AniList progress automatically, and pushes Discord Rich Presence.
  - **Fresh IPC Socket per Launch**: Generates a new IPC socket for every MPV launch to prevent stale socket collisions.

#### D. Torrent Search, Clients & Auto-Downloader

- **Built-In Torrent Search**: Searches multiple torrent indexers via extensions with availability badges for recently released episodes.
- **Torrent Client Support**: Integrates with **qBittorrent** (with automatic re-authentication), **Transmission**, and an experimental **built-in torrent client** (using classic file-based downloading rather than memory-mapped files).
- **Advanced Auto-Downloader**:
  - Monitors airing shows and automatically downloads new episodes based on **Auto-Downloader Profiles**.
  - Supports multi-indexer comparison (comparing up to 10x more torrents per run), scoring systems, release-group/codec/resolution preferences, min seeders, min/max file sizes, excluded keywords, and **download delay timers** (e.g., wait N minutes after release to see if a higher-priority fansub group uploads).

#### E. Extensions, Plugins & Marketplace

- **In-App Extension Marketplace**: Browse, install, update, and inspect code diffs for community extensions.
- **4 Extension Types**:
  1. **Torrent Providers** (indexers/trackers).
  2. **Online Streaming Providers** (web anime stream extractors).
  3. **Manga Providers** (chapter & page fetchers).
  4. **App Plugins**: Full behavioral plugins running in a sandboxed JS runtime (**Goja**) that can hook into backend events, add custom UI buttons/dropdowns/tray icons with badge counts, open custom Webview windows, use ChromeDP APIs, parse filenames via `$habari`, read settings, and push custom Discord Rich Presence activities.

#### F. Manga, Social (Nakama), Discovery, Offline & UI

- **Manga Reader**: Read local manga or chapters from extensions in one unified reader with zooming, source comparison across all installed providers, source refresh, and offline chapter downloads (including AVIF workarounds).
- **Nakama (Social Features)**:
  - **Watch Party**: Synchronized playback across friends (works with local files, torrent/debrid streams, online streams, and custom sources via host relay).
  - **Library Sharing**: Share your local anime library with connected peers.
- **Offline Mode (Platform Abstraction Layer)**: Swaps the live AniList API layer for a local Offline platform backed by cached metadata and local progress so browsing, reading, and watching local files work seamlessly without internet.
- **Playlists, Release Schedule & Continuity**: Binge-watching playlists, airing calendar / missed-episode tracker, watch continuity history, and anonymized log export + deduplicated Issue Recorder for bug reports.
- **UI & Customization**: Custom color themes, custom background images, layout/carousel options, horizontal scrolling via `Alt + mouse wheel`, scroll-position restoration on search/lists, automatic sidebar overflow handling (auto-unpinning items), and **live settings reload** (re-initializing players, torrent clients, watchers, and Discord RPC without restarting).

---

## 2. VortX (formerly StremioX)

**GitHub**: [https://github.com/VortXTV/VortX](https://github.com/VortXTV/VortX)

### What It Is

VortX is a fully native, Apple-first streaming client built for **Apple TV, iPhone, iPad, and Apple-silicon Mac**, alongside an **Android phone/tablet/TV** preview (with Windows, Linux, and LG/Samsung TV apps in development). Unlike web-wrapped apps, it uses native OS UI frameworks (SwiftUI on Apple, Jetpack Compose on Android) paired with native video playback engines and a shared core.

### Complete Feature Inventory

#### A. Home, Discovery, Library & Profiles

- **Cinematic Home Screen**: Continue Watching row, featured artwork, trailers, add-on catalog rows, and customizable home layout (hide or reorder any section or catalog row).
- **Rich Detail Pages**: Movies and TV series pages with cast, ratings, trailers, season/episode lists, episode progress indicators, and manual resume/watched controls.
- **Discover, Search & Smart Collections**: Filter catalogs by type, genre, and sort order; per-profile "For You" and interest rows; **Smart Collections** dynamically generated from rules (genre, cast, year, rating).
- **Multi-User Profiles**: Separate viewer profiles with custom names, avatars, appearance settings, isolated watch histories, and optional **PIN locks**.

#### B. Unified Source Connectivity

- **Stremio-Compatible Add-ons**: Install standard Stremio add-ons via manifest URL, reorder them, toggle visibility, and use **Cinemeta** for metadata.
- **Ranked "Watch Now" Aggregation**: Collects streams from all enabled add-ons and presents a ranked picker displaying resolution (4K/1080p/etc.), format (HDR/Dolby Vision/remux), file size, and cached-debrid status when provided by the add-on.
- **Debrid, Torrents & Usenet**:
  - Supports Debrid services, raw P2P torrents/magnets (in Full builds), and **Usenet/NZB** through configured services.
  - **Full vs. Lite / "Direct Links Only" Mode**: Apple TV ships both a Full build (with embedded torrent streaming transport and Apple TV Top Shelf extension) and a Lite build enforcing Direct Links Only (HTTP/S and resolved Debrid links only, hiding raw torrents/magnets).
- **Personal Media Servers & Network Shares**: Connects directly to personal **Plex**, **Jellyfin**, and **Emby** servers, plus local network shares over **SMB** and **WebDAV**.
- **Built-In Live TV / IPTV**: Supports **M3U/M3U8** and **Xtream Codes** playlists along with **XMLTV** EPG guide data, channel search, and category browsing.
- **External Trackers & Imports**: Optional **Stremio** account/library import, plus **Trakt** and **SIMKL** integrations (separate from VortX accounts).

#### C. Multi-Engine Native Playback

- **Selectable Playback Engines (Per-Source Capable)**:
  - **Apple**: Choose between **`libmpv` / MPVKit**, **`AVPlayer`** (used for Dolby Vision / live HLS paths), and **`AetherEngine`** (available on macOS 15+, iOS 18+, tvOS 18+; isolated in its own framework so its FFmpeg libraries never conflict with `libmpv`'s FFmpeg).
  - **Android**: Separate Full MPV and Media3 playback paths.
- **Playback Controls & Polish**:
  - Audio & subtitle track selection, default language preferences, subtitle styling & timing sync, playback speed, aspect ratio controls, seek previews, playback stats/info overlay, source switching mid-stream, next-episode preparation, and external player handoffs.
  - **Intro & Credit Skipping**: Integrates with **SkipDB** and **TheIntroDB** (plus chapter timings) for automatic/prompted intro and credit skipping.
  - **Visual Performance Controls**: Dedicated settings to tune artwork, glass/blur effects, and animations for smoother performance on lower-powered devices.

#### D. Subtle Things VortX Does Properly

- **Bounded Disk Streaming Cache & OOM Protection**: On memory-constrained devices like Apple TV (where 80 GB 4K remuxes can trigger OS "Jetsam" kills), VortX offers an opt-in bounded disk streaming cache that immediately unlinks consumed cache files while enforcing RAM guardrails (keeping local torrents on a separate small in-memory buffer and keeping Dolby Vision Profile 7 FEL packets in RAM).
- **Subtitle Metadata Sanitizer**: Uses a dedicated subtitle harness to strip malformed metadata leaks from subtitle tracks before rendering.
- **End-to-End Encrypted Account Sync & Backups**: Syncs profiles, settings, and watch progress across devices via an E2E-encrypted account document (while keeping device-local credentials local). Supports local backup/restore and library export/import on iOS/macOS, and **QR-code backup/restore** on Apple TV via a phone browser.
- **Offline Playback & Recovery**: Supports offline playback, safe profile recovery, and retrying failed subtitle add-ons while retaining source ownership.

---

## 3. MovieBox-TUI

**GitHub**: [https://github.com/mesamirh/MovieBox-TUI](https://github.com/mesamirh/MovieBox-TUI)

### What It Is

A fast, keyboard-first terminal application for discovering, streaming, and batch-downloading movies, TV shows, anime, Asian dramas, and Live TV using local external media players—with zero telemetry and 100% local disk storage.

### Complete Feature Inventory

- **Dual Provider System**:
  - **Native Scrapers**: Built-in scrapers for MovieBox, 4KHDHub, Dramachi, BDIX, DhakaFlix, and CircleFTP.
  - **Stremio HTTP Add-ons Mode**: Pre-installed Cinemeta + Addon Manager to add, toggle, or remove any Stremio manifest URL, browse Top Movies/Series catalogs, and search across add-ons.
  - **Live TV / IPTV**: M3U playlist import with channel categories, EPG guide support, and search.
- **Pre-Playback Controls**:
  - **Resolution Picker**: Choose `4K`, `1080p`, `720p`, `480p`, or `Auto` before playback, with parsed quality, codec, and file size labels.
  - **Interactive Subtitle Picker**: Extracts multi-language subtitle tracks so you can pick subtitles before launching playback or downloading.
- **Local `StreamRelay` Loopback Proxy**:
  - Routes DASH `.mpd` and cookie/CloudFront-authenticated streams through a local `127.0.0.1` HTTP/1.1 persistent-connection proxy.
  - Caches rewritten manifests in RAM, warm-prefetches initialization and opening fragments, splits `.m4s` segment fetches into **parallel 95 KB HTTP `Range` sub-requests** (specifically sized to avoid Tengine CDN's `96k` rate-throttling threshold), and lookahead-prefetches the next 3 video segments (`N+1..N+3`) plus matching audio chunks.
- **External Player Orchestration (`mpv`, `IINA`, `VLC`, Android Intents)**:
  - **Priority Auto-Detection**: Detects `IINA → MPV → VLC` on macOS (checking `/Applications`, Homebrew, MacPorts, Nix), `MPV → VLC` on Linux (including Flatpak & Snap) and Windows (WinGet, Scoop, Chocolatey, Registry), and Termux/Android intents on mobile.
  - **Header & Subtitle Forwarding**: Passes `User-Agent`, `Referer`, cookies, and Stremio `proxyHeaders` to players; pre-downloads remote subtitles in parallel and passes `--sub-file=<path>` (with remote URL fallback).
  - **Playback Optimization Flags**: Passes `--hwdec=auto-safe` for 4K 10-bit HEVC HDR and `--ytdl=no` on non-DASH direct streams (`.mkv`, `.mp4`, `.m3u8`) for faster startup.
  - **Zero-Drift Resume Tracking**: Injects a Lua script (`moviebox_tracker.lua`) into `mpv`/`IINA` to write exact timestamps, duration, and chosen mirror to background state files; resumes with `--start=<seconds>` and marks watched at **90% or EOF**.
  - **Detached Session Safety**: Launches players and the `StreamRelay` sidecar in detached OS sessions with stderr redirected to a temporary log file so closing the terminal/app never kills active playback or causes pipe crashes.
- **Multi-Segment HTTP Batch Downloader**:
  - Downloads single episodes or full seasons using 4–16 concurrent HTTP `Range` workers writing in-place to pre-allocated `.part` files + `.part.json` resume metadata (`ETag`, `Last-Modified`, segment offsets), followed by atomic rename on completion.
  - Automatically falls back to single-stream download if a server returns HTTP `200`, `403`, or `429`.
  - Uses `yt-dlp` + `ffmpeg` (with `--concurrent-fragments 32 --http-chunk-size 95K`) specifically for adaptive DASH downloads.
- **Terminal UI & Polish**: Vim keybindings + mouse support, slash command palette (`/help`, `/settings`, `/browse`, `/history`, `/config`), 9 built-in themes (with automatic downgrade on basic terminals), native terminal poster art (Kitty, Sixel, iTerm2 with text fallback), custom DNS fallback, and local Continue Watching / Favorites / History.

---

## 4. nyaa

**GitHub**: [https://github.com/Beastwick18/nyaa](https://github.com/Beastwick18/nyaa)

### What It Is

A lightweight, Vim-inspired terminal browser and dispatcher for searching, filtering, and sending anime and general torrents to your preferred torrent client.

### Complete Feature Inventory

- **Multi-Source Torrent Discovery**: Searches **Nyaa**, **Sukebei**, and **TorrentGalaxy**, supporting both HTML scraping and RSS feeds, with category selection, filter selection, sort/reverse-sort, and multi-page navigation.
- **Mirror & Proxy Support**: Allows overriding `base_url` per source (e.g., `nyaa.land` when `nyaa.si` is ISP-blocked) and routing all requests through an HTTP proxy (`request_proxy`) with configurable timeouts.
- **6 Built-In Download / Dispatch Targets**:
  1. **qBittorrent** (direct Web API integration).
  2. **Transmission** (RPC integration).
  3. **rqbit** (direct HTTP API integration).
  4. **Download `.torrent` to local folder**.
  5. **Open with OS default app**.
  6. **Run custom shell command** with placeholders (`{torrent}`, `{magnet}`, `{title}`, `{file}`).
- **Modal Keybindings & Clipboard Craft**: Vim-style modes (`/` or `i` to search, `Ctrl-s` switch source, `d` switch client, `c`/`s`/`f` category/sort/filter, `t` theme), live config saving (`save_config_on_change`), optional infohash-only magnet yanking (`yank_full_magnet = false` strips trackers/metadata), OSC52 + X11/Wayland/macOS clipboard fallback, and custom themes.

---

## 5. Mangayomi

**GitHub**: [https://github.com/kodjodevf/mangayomi](https://github.com/kodjodevf/mangayomi)

### What It Is

A cross-platform (Android, iOS, macOS, Linux, Windows) all-in-one app for reading **manga, webtoons, comics, and light novels** and watching **anime and movies**, inspired by Tachiyomi/Aniyomi.

### Complete Feature Inventory

- **Unified Media Library**: Organizes manga, novels, anime, and movies into custom categories with local reading/watching support, light/dark themes, and the redesigned **Shiori** UI across phone, desktop, and TV.
- **Dual-Language Extension Ecosystem**: Community sources can be written in either **Dart** (`dart_eval`) or **JavaScript**, keeping all scraping logic completely decoupled from the core app.
- **Built-In Torrent Streaming**: Embeds a local torrent HTTP server (compiled as a native library) for streaming anime/movie torrents directly inside the app.
- **Rebuilt Video Player**: Controls tailored for phone, desktop, and TV, plus video filters, audio delay adjustment, and persistent volume/brightness/aspect-fit settings carried across episodes.
- **Configurable Manga & Novel Reader**: Multiple viewer modes (vertical/webtoon, RTL, LTR, double-page), reading directions, download-ahead while reading, automatic downloading of new chapters, and per-series library-update skip rules.
- **Trackers & Local-Only Tracking**: Syncs with **MyAnimeList, AniList, SIMKL, Trakt, and Kitsu**, plus an **account-free on-device tracker** for tracking status, progress, scores, and dates privately without any external login.
- **Sync, Backups & Reliability**:
  - **Serverless Device-to-Device Sync**: Syncs library/progress directly between devices without needing a central cloud server.
  - Local/cloud backups with **Aniyomi backup** protobuf compatibility, background Android download/update/sync jobs under one notification, and a serialized database write queue to prevent concurrent write corruption.

---

## 6. Zangetsu

**GitHub**: [https://github.com/Spyou/Zangetsu](https://github.com/Spyou/Zangetsu) | **Providers**: [https://github.com/Spyou/zangetsu-providers](https://github.com/Spyou/zangetsu-providers)

### What It Is

An open-source multi-media app for **Android, Android TV, iOS (beta), and Windows (WIP)** divided into three dedicated modes—**Streaming** (anime, movies, series), **Manga**, and **Light Novels**—with a dedicated remote-friendly Android TV interface.

### Complete Feature Inventory

- **Manifest-Based JS Providers**: Add one repository manifest URL (`index.json`) to browse and install self-contained `.js` scraper files running in an embedded QuickJS runtime. Includes custom browser shims so **LNReader** light-novel plugins run cleanly inside QuickJS.
- **Cross-Ecosystem Extension Compatibility**: On Android, supports loading **CloudStream**, **Aniyomi**, and **Mihon** extensions alongside native Zangetsu JS sources and **Stremio** integration (on iOS, Zangetsu's own JS sources, player, downloads, and trackers still work).
- **Smart Player Routing**: Uses native playback for standard streams and **automatically routes all torrents to `libmpv`** (regardless of player toggle state) so MKV containers and anime subtitles play reliably.
- **Library, Profiles & Sync**:
  - My List grouping with unwatched-count chips on library cards.
  - Profile PIN locks synced with device biometrics.
  - Cross-device cloud sync (lists, profiles, watch/reading progress) via the Zangetsu sync server, plus Discord Rich Presence with account status selection.
  - Built-in in-app updater checking GitHub Releases (with separate nightly install package IDs so nightlies don't overwrite stable builds).

---

## 7. Zenshin

**GitHub**: [https://github.com/hitarth-gg/zenshin](https://github.com/hitarth-gg/zenshin)

### What It Is

A desktop (Electron) and web anime list manager paired with P2P torrent streaming and web-scraped anime streaming.

### Complete Feature Inventory

- **AniList Integration**: Browse seasonal/trending anime, manage your AniList watching lists, and track episodes.
- **Dual Stream Acquisition**:
  - **Torrent Streaming**: Searches **Nyaa** and **AnimeTosho** (migrated to `tosho-xyz`), streams selected magnets/torrents via **WebTorrent** over a local Express HTTP server.
  - **Web Scraping**: Scrapes online streaming hosts (e.g., AnimePahe via Puppeteer/cookies) for direct HLS streaming.
- **Playback & Key Limitation**:
  - Built-in Video.js / Plyr HTML5 player for HLS and basic streams.
  - **One-Click External Player Handoff**: Explicitly includes "Open in VLC" / "Open in External Player (MPV)" buttons because extracting and rendering embedded MKV ASS/SSA subtitles inside a browser `<video>` player is notoriously difficult without a native `libmpv`/transcoding layer.

---

## 8. dango

**GitHub**: [https://github.com/serifpersia/dango](https://github.com/serifpersia/dango)

### What It Is

A lightweight, hardware-efficient, local-first self-hosted web app (also runnable via Termux or as a standalone Android APK) focused on fast local anime watching, watchlist tracking, privacy, and low resource usage (runs well on SBCs and older hardware).

### Complete Feature Inventory

- **Expanded Media Hub**: Dedicated sections for **Anime**, **Manga**, **TV/Movies**, and **ASMR**, with release schedules, trending feeds, and catalog search.
- **Multi-Provider HLS Playback**: Switch seamlessly between multiple third-party streaming providers directly inside the player without leaving the episode.
- **Watch Along (No Port Forwarding)**: Create a synchronized watch-party room right below any direct HLS anime, TV, or movie player and share an invite link. Each viewer runs their own Dango instance while the host controls playback; rooms automatically expire after 6 hours.
- **Bidirectional Trackers (AniList & MAL)**:
  - Merges statuses and progress both ways (remote-only imports, local-only pushes, never deletes missing entries, reports unmatched titles instead of guessing).
  - Auto-pushes episode progress once playback reaches **80%**; also supports login-free username and XML list imports.
- **Profiles, Stats & LAN Security**: Multiple local profiles with separate libraries/tracker connections/passwords/avatars, a Library Insights analytics dashboard (viewing habits, completion ratios), and optional LAN password protection.
- **3-Tier Local-First Cloud Sync Fallback**: Primary state lives in local SQLite; backups export JSON snapshots via a priority chain: **1. GitHub Sync** (private `dango-sync-data` repo via OAuth device flow) → **2. Google Drive** (`appDataFolder`) → **3. Rclone** (Mega, Dropbox, WebDAV).

---

## 9. LiSA

**GitHub**: [https://github.com/Cosmicoppai/LiSA](https://github.com/Cosmicoppai/LiSA)

### What It Is

An account-free desktop application for streaming, reading, and batch-downloading anime and manga from supported sites.

### Complete Feature Inventory

- **Anime Streaming & Batch Downloading**: Scrapes AnimePahe with Sub/Dub selection and 720p/1080p resolution choices; supports single-episode or full-page batch downloading via an async download manager.
- **Manga Reading & Downloading**: Scrapes MangaKatana with a built-in E-Reader and downloads chapters/volumes as **JPG** or **PDF**.
- **Built-In + External Players**: Stream in the built-in video player or switch in one click to **MPV** or **VLC**.
- **Discovery & Local Library**: Filterable Explore screens for both anime and manga, a "My List" watchlist, a recommendation tab per series, and a local Library view for pre-downloaded episodes and manga chapters—with zero user account required.

---

## 10. AniWorld-Downloader

**GitHub**: [https://github.com/phoenixthrush/AniWorld-Downloader](https://github.com/phoenixthrush/AniWorld-Downloader)

### What It Is

A versatile multi-interface tool (CLI, interactive terminal menu/TUI, browser-based Web UI, Python library, and Docker container) for finding, downloading, and streaming anime, series, movies, and manga from direct-hoster sites.

### Complete Feature Inventory

- **Two-Layer Scraping (Catalog Sites vs. Stream Hosters)**:
  - **Catalog Sites**: AniWorld, SerienStream, MegaKino, Filmo, Moflix, MangaFire, FilmPalast, Kinox, BurningSeries, plus optional opt-in adult animation sites.
  - **Stream Hoster Extractors**: VOE, Filemoon, Doodstream, MegaKino, Gupload, MoflixClick, Vidara, Vidmoly, and Vidoza (several also support preview extraction).
  - **Automatic Hoster Fallback**: If a preferred hoster fails, automatically tries the next hoster in your configured priority order without switching away from your chosen audio/subtitle language.
- **Downloading & Library Automation**:
  - Downloads single episodes, full seasons, entire series, movies (MKV/MP4 via FFmpeg), and manga chapters/volumes (JPG, CBZ, EPUB).
  - **Auto-Sync**: Scheduled checks for new episodes of titles already in your library.
  - Smart episode numbering (extracting explicit `[Episode N]` / `[JP Episode N]` numbers from English titles with seasonal fallback) and customizable filename templates.
- **Streaming & External Integrations**:
  - Streams via **MPV** or **IINA**, with built-in support for **Syncplay** (watch parties), **AniSkip** (intro/outro skipping), and **Anime4K** shaders.
  - Handles CAPTCHAs via browser visibility / headless Chromium + Xvfb in Docker.
  - **Web UI Extras**: Local accounts, optional **OIDC SSO**, scoped REST **API Keys** (`read`, `read-and-download`, `full-access`), optional **Discord request bot**, and custom CSS + background shaders (with `/settings?nocss=1` recovery URL if a custom theme breaks the UI).

---

## 11. Reiverr

**GitHub**: [https://github.com/aleksilassila/reiverr](https://github.com/aleksilassila/reiverr)

### What It Is

A self-hosted, TV-first ("Selfhosted Netflix") all-in-one replacement for the entire *arr + media server stack (Jellyfin + Sonarr + Radarr + Seerr + torrent client), with packaged **Samsung Tizen** and **Android TV / Fire TV** apps.

### Complete Feature Inventory

- **TMDB Discovery**: Trending movies/TV, search, personalized recommendations from TMDB ratings, cast/crew details, and trailers.
- **Built-In Request & Torrent Engine**:
  - Built-in torrent client with concurrent download limits, speed limits, and RSS feed discovery.
  - **On-Demand Episode Downloading & Auto-Cleanup**: Can download episodes only as needed and automatically delete them after watching.
  - **Per-User Storage Quotas**: Limits how much disk space each user's requests can consume.
- **Smart Local Library**: Automatically discovers existing files on disk (compatible with Jellyfin folder layouts) and **hardlinks** finished downloads into library folders when on the same volume so seeding and library storage use zero duplicate space.
- **Playback**: Direct play, on-the-fly **HLS transcoding**, direct **torrent streaming**, audio track selection, and sidecar VTT/SRT subtitles—built with large TV-remote-friendly UI controls and backwards compatibility down to Chromium 69 (Tizen 5.5).

---

## 12. bobarr-v2

**GitHub**: [https://github.com/iam4x/bobarr](https://github.com/iam4x/bobarr) _(successor repo: `iam4x/bobarr-v2`)_

### What It Is

A zero-configuration, Dockerized all-in-one alternative to running **Sonarr + Radarr + Jackett + torrent client + VPN** separately.

### Complete Feature Inventory

- **Unified Media Automation**: Single web dashboard to search for movies and TV shows, automatically query configured torrent trackers for the best match, and download them into your library.
- **Built-In VPN Routing**: Bundles the torrent downloader and VPN directly inside the stack so users don't have to manually configure Jackett indexers, API keys, or VPN container networking across 4–5 separate services.

---

## 13. WeFlix_v2

**GitHub**: [https://github.com/kweephyo-pmt/WeFlix_v2](https://github.com/kweephyo-pmt/WeFlix_v2)

### What It Is

A clean, modern Netflix-style movie, TV, and anime discovery web application powered by the TMDB API and Firebase, delegating all video playback to an embedded third-party iframe.

### Complete Feature Inventory

- **Catalog & Discovery UX**: Auto-advancing trending hero carousel, collapsible icon sidebar with hover-expand, genre filtering reflected in shareable URL params (`/movies?genre=28`), sort options (Most Popular, Top Rated, Newest, Oldest, Highest Grossing), curated special categories (**Anime, K-Drama, C-Drama, Donghua**), global/multi-country catalog, infinite-scroll poster grids with skeleton loaders, and live search.
- **Detail & Person Pages**: Full-bleed backdrop hero, ratings, genres, tagline, cast/crew row, dedicated Person biography/credits pages, season tabs with episode thumbnail cards, and human-readable slug URLs (`/movie/550-fight-club`) with dynamic SEO meta tags.
- **User Accounts & Playback**: Firebase Auth (login, signup, email verification, password reset), cloud-synced personal Watchlist and Continue Watching row in Firestore, and an embedded in-browser iframe video player (`vidlink.pro`) with a click-to-activate scroll overlay.

---

## 14. BookOrbit

**GitHub**: [https://github.com/bookorbit/bookorbit](https://github.com/bookorbit/bookorbit)

### What It Is

A self-hosted digital library and reading platform for **ebooks, PDFs, comics, and audiobooks** with native iPhone/Apple Watch apps, Kobo sync, and a KOReader plugin. Included here because its library management, metadata aggregation, and sync patterns translate directly to video/anime platforms.

### Complete Feature Inventory

- **Multi-Format Built-In Readers**: Web readers for EPUB/KEPUB/MOBI/AZW3/FB2, PDFs, comics (CBZ/CBR/CB7), and audiobooks (M4B/MP3/M4A/OPUS/OGG/FLAC), plus Read-Along EPUB3 (single file & single position shared across reading and listening).
- **12+ Pluggable Metadata Providers**: Built-in fetchers for Google Books, Open Library, Amazon, Goodreads, Kobo, Hardcover, Audible, Audnexus, Libro.fm, iTunes, ComicVine, and RanobeDB, plus an **external metadata provider** interface (e.g., Douban, FantLab, MangaBaka) and separate cover-art sourcing.
- **Library Dock (Automated Ingestion)**: Each library can have a staging "Dock" folder where newly dropped files receive metadata, get reviewed, and—with auto-file enabled—are automatically renamed and moved into the library.
- **3-Way Sync & External Trackers**: Bidirectional progress, highlight, and annotation sync across the Web Reader, Kobo devices, and KOReader (via a pre-configured downloadable plugin), plus automatic sync to Hardcover, Readwise, and StoryGraph.
- **Stats, Jobs & Enterprise-Grade Admin**: Reading heatmaps, streaks, goals, 50+ achievements, "Reading DNA" profiles, a dedicated **Jobs** page for long-running background scans/bulk actions, multi-user permissions with OIDC/SSO (Authentik, Keycloak, Authelia), OPDS feeds, Send-to-Kindle, and scoped API keys.

---

## Cross-App Feature & Mechanism Comparison

| Capability                         |       Seanime       |           VortX            | MovieBox-TUI  |   nyaa    | Mangayomi  |    Zangetsu     | Zenshin  |    dango    |   LiSA   | AniWorld-DL |  Reiverr  | bobarr-v2 | WeFlix_v2 | BookOrbit |
| ---------------------------------- | :-----------------: | :------------------------: | :-----------: | :-------: | :--------: | :-------------: | :------: | :---------: | :------: | :---------: | :-------: | :-------: | :-------: | :-------: |
| **Local Library Scanning**         |         Yes         |          Plex/SMB          |       —       |     —     | Local read |        —        |    —     |      —      | Local DL |  Local DL   |    Yes    |    Yes    |     —     |    Yes    |
| **Torrent Streaming (P2P)**        |         Yes         |         Yes (Full)         |       —       |     —     |    Yes     |       Yes       |   Yes    |      —      |    —     |      —      |    Yes    |     —     |     —     |     —     |
| **Debrid Streaming**               |         Yes         |            Yes             |  Via Addons   |     —     |     —      |   Via Addons    |    —     |      —      |    —     |      —      |     —     |     —     |     —     |     —     |
| **Web Scrapers / Online Streams**  |     Extensions      |           Addons           |   Built-in    |     —     | Extensions |   Extensions    | Built-in |  Built-in   | Built-in |  Built-in   |     —     |     —     |  Iframe   |     —     |
| **Stremio Addon Support**          |          —          |            Yes             |      Yes      |     —     |     —      |       Yes       |    —     |      —      |    —     |      —      |     —     |     —     |     —     |     —     |
| **Built-In Player**                | VideoCore + MpvCore | libmpv + AVPlayer + Aether |       —       |     —     |    Yes     | Native + libmpv | Video.js |   HLS Web   |   Yes    |      —      |  Web/TV   |     —     |  Iframe   |  Readers  |
| **External Player (MPV/VLC/IINA)** |         Yes         |            Yes             | Yes (Primary) |  Via cmd  |  Optional  |        —        |   Yes    |      —      |   Yes    |     Yes     |     —     |     —     |     —     |     —     |
| **Auto-Downloader / RSS / Sync**   |         Yes         |             —              |       —       |     —     | Auto-ch DL |        —        |    —     |      —      | Batch DL |  Auto-Sync  | Yes (RSS) |    Yes    |     —     |   Dock    |
| **AniList / MAL / Trakt Sync**     |       AniList       |        Trakt/SIMKL         |  Local only   |     —     | 5 Trackers |    Trackers     | AniList  |  AL + MAL   |    —     |      —      |   TMDB    |     —     | Firestore | Hardcover |
| **Watch Party / Social**           |       Nakama        |             —              |       —       |     —     |     —      |        —        |    —     | Watch Along |    —     |  Syncplay   |     —     |     —     |     —     |     —     |
| **Intro / OP / ED Skipping**       |      Chapters       |           SkipDB           |       —       |     —     |     —      |        —        |    —     |      —      |    —     |   AniSkip   |     —     |     —     |     —     |     —     |
| **Live TV / IPTV**                 |          —          |            Yes             |      Yes      |     —     |     —      |        —        |    —     |      —      |    —     |      —      |     —     |     —     |     —     |     —     |
| **Offline Mode / Downloads**       |         Yes         |            Yes             | Multi-seg DL  | Client DL |    Yes     |       Yes       |    —     |      —      | Batch DL |     Yes     | On-demand |    Yes    |     —     |    Yes    |
| **Multi-User Profiles**            |          —          |         Yes (PIN)          |       —       |     —     |     —      |    Yes (PIN)    |    —     |     Yes     |    —     |  Accounts   |  Quotas   |     —     | Firebase  | OIDC/SSO  |

---

## Core Operational Mechanics Observed Across All 14 Projects

1. **How Providers Connect**:
   - **Embedded Script Extensions** (`Seanime`, `Mangayomi`, `Zangetsu`): App ships empty of content sources; users install community JS/Dart scripts from a manifest URL. Keeps the core app legal, modular, and easy to fix when sites change HTML.
   - **Stateless HTTP Manifest Add-ons** (`VortX`, `MovieBox-TUI`, `Zangetsu`): Follows the Stremio protocol (`manifest.json` → `/catalog`, `/meta`, `/stream`, `/subtitles`). Zero local script sandbox needed; add-ons run remotely and return JSON stream objects (direct URLs, magnets, or debrid links + `proxyHeaders`).
   - **Two-Stage Site → Hoster Scraping** (`AniWorld-Downloader`, `MovieBox-TUI`, `LiSA`, `dango`): Stage 1 scrapes a catalog site for episode/title pages; Stage 2 extracts playable `.m3u8`/`.mpd`/`.mp4` URLs from video hosters with automatic fallback when a hoster is down.
   - **Indexer Search + Client Dispatch** (`nyaa`, `Seanime`, `bobarr-v2`): Queries torrent trackers (HTML/RSS/API) with mirror URL and proxy overrides, then either streams via an embedded client or dispatches to external daemons (qBittorrent, Transmission, `rqbit`).

2. **How Streaming Actually Works Under the Hood**:
   - **Torrent-to-HTTP**: Downloads pieces sequentially with priority on the **first and last pieces** (so MKV headers, chapters, and seek tables load immediately), exposes a local `127.0.0.1` HTTP server supporting byte `Range` requests, and feeds that URL to the player.
   - **Loopback Stream Relay Proxy**: Sits on `127.0.0.1` between the remote CDN/hoster and the local player to inject required `Referer`/`User-Agent`/Cookie headers, rewrite DASH/HLS manifests, prefetch upcoming segments, and split range fetches into small chunks to bypass CDN per-connection throttling.
   - **Direct Play vs. Transcode**: Local desktop/native players (`libmpv`, `mpv`, `IINA`, `VLC`) play almost anything natively (10-bit HEVC/AV1 + ASS/SSA subtitles), whereas browser/TV clients require either codec-compatible Direct Play or on-the-fly FFmpeg HLS transcoding + subtitle/font extraction.

3. **Subtle Quality-of-Life Details That Separate Polished Apps from Prototypes**:
   - **Playback Continuity & Thresholds**: Auto-marking watched at **80–90% or EOF**, restoring seek positions only _after_ the player reports media-ready, and tracking exact player timestamps via IPC/Lua instead of wall-clock timers.
   - **Storage & Drive Hygiene**: Hardlinking downloads into library folders (Reiverr), shelving locked entries when external drives disconnect (Seanime), immediately unlinking consumed streaming cache chunks to avoid OOM/disk bloat (VortX), and atomic `.part` → final file renames (MovieBox-TUI).
   - **Resilience to Broken Sources**: Mirror `base_url` overrides and HTTP proxies for ISP-blocked trackers (`nyaa`), automatic hoster fallback (`AniWorld-Downloader`), refreshing expired stream URLs before switching providers (`Seanime`), and detached player processes so closing the main UI never interrupts playback (`MovieBox-TUI`).
