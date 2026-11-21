# Seanime — Complete Master Feature List (Backend + UI/UX)

---

## 1. Library Management & Local Files

- **Multi-Directory Library Support**: Configure one primary anime library path plus multiple secondary library directories.
- **Smart Fuzzy Library Scanner**: Scans arbitrary folder structures without requiring strict Plex/Jellyfin naming conventions.
- **Fansub Filename Parser (`Habari`)**: Extracts title, release group, episode number, season, resolution, codec, dual-audio, and batch tags from anime filenames.
- **Configurable Matching Engine**: Adjustable matching algorithm, confidence threshold (`0.0–1.0`), and legacy matching fallback toggle.
- **Live Filesystem Watcher**: Automatically detects added/deleted files in library folders and triggers background scans.
- **External Drive Resilience ("Shelving")**: Shelves locked library files when an external drive disconnects instead of wiping matched metadata.
- **Library Explorer (UI Drawer)**: Browse raw disk folders side-by-side with matched anime entries; lock, unlock, re-match, rename, or delete files directly.
- **Scan Summaries Page**: Post-scan audit log showing matched shows, confidence scores, library size stats, and a dedicated list of unmatched/problematic files to fix.
- **Bulk File Management**: Bulk lock/unlock, re-match, or delete episodes on any anime entry page.
- **Specials / OVA / NCOP / NCED Separation**: Automatically separates main episodes from specials, openings, endings, and extras.

---

## 2. Streaming & Playback Sources (4 Unified Modes)

- **Local Direct Play & Transcoding (`Mediastream` + `Directstream`)**:
  - Automatic codec/container detection: Direct Play for compatible formats, on-the-fly FFmpeg HLS transcoding for incompatible codecs.
  - Hardware-accelerated transcoding (NVENC, Intel QSV including Low-Power QSV, VAAPI, VideoToolbox).
  - Native MKV parsing (`mkvparser` / `matroska`) for chapters, multi-edition seekbar accuracy, embedded subtitle extraction, and embedded font extraction (including filenames with spaces).
  - Blu-ray PGS bitmap subtitle parsing (`pgs`).
- **Torrent Streaming (`Torrentstream`)**:
  - Stream P2P torrents directly without waiting for full downloads.
  - Head + tail piece prioritization so MKV headers, chapters, and seeking work immediately.
  - Automatic season batch detection and per-episode file selection inside multi-file torrents.
  - Next-episode torrent preloading while watching the current episode.
  - Automatic switch to direct disk streaming once a torrent finishes downloading; immediate disk cleanup when dropping a torrent.
  - Real-time torrent status overlay (peers, download/upload speeds, buffer progress).
- **Debrid Streaming (`Debridstream`)**:
  - Integrations for **Real-Debrid**, **Torbox**, **AllDebrid**, and **Premiumize**.
  - Cached instant-play detection for single episodes and batch torrents.
  - Direct Debrid-to-local-library downloads and a dedicated `/debrid` transfer management dashboard.
- **Online Web Streaming (`Onlinestream`)**:
  - Stream from web anime sources via installable extensions.
  - Sub/Dub toggle, provider selector, mirror/server switcher, and automatic stale-URL refresh before falling back to another provider.
  - Optional toggle to merge online streaming availability directly into the main library view.

---

## 3. Video Players & Playback Intelligence

- **Built-In Web Player (`VideoCore`)**:
  - HTML5 + HLS.js playback with **JASSUB** (full ASS/SSA anime subtitle typesetting) and **PGS** bitmap subtitle rendering.
  - Hover seekbar with frame thumbnail previews, exact timestamps, chapter cue ticks, and floating chapter title pills.
  - Morphing Play/Pause button, Previous/Next Episode controls, hover-expanding Volume Slider + Volume Boost (>100%), clickable Elapsed/Remaining timestamp toggle.
  - Slide-out **Episode Playlist Drawer** inside the player.
  - **In-Sight Overlay**: View characters, cast, and episode info without leaving playback.
  - **Anime4K Real-Time Upscaling Shaders** directly in the player.
  - **Stats for Nerds Overlay** (codec, resolution, bitrate, dropped frames, buffer/torrent metrics).
  - **Audio Track Menu**, **Subtitle Menu** (embedded tracks, external subtitle upload, timing delay/size/offset), and **Resolution/Quality Menu**.
  - **Screenshot Capture** with configurable screenshot directory and confirmation prompt.
  - **Picture-in-Picture (PiP)**, **Chromecast / Cast**, **Fullscreen**, and **Discrete Auto-Hiding Controls** (hiding cursor/HUD while keeping subtitles visible).
  - **Mini-Player Mode**: Smooth `320ms` View Transition morphing the full player into a floating corner player so you can keep browsing while watching.
  - **Mobile Touch Gestures**: Dedicated mobile control bar, double-tap left/right to seek, vertical swipe for brightness/volume, iOS fullscreen subtitle support.
- **Built-In Desktop `libmpv` Player (`MpvCore` in Seanime Denshi)**:
  - Embedded `libmpv` (`mpv-prism`) with native 10-bit HEVC/AV1 decoding, `libass` subtitles, GLSL shaders, dual-GPU support, and log export.
- **External Desktop & Mobile Players**:
  - Full IPC/API integration with **MPV**, **VLC**, **MPC-HC**, and **IINA** (macOS), plus mobile intents/deep links (**Outplayer**, **VLC**).
  - Fresh IPC socket generated per MPV launch to prevent socket collisions.
- **Playback Continuity & Automation**:
  - Exact timestamp resume (`continuity.Manager`)—waits until MPV/IINA media is ready before seeking.
  - Auto-update AniList watch progress automatically upon episode completion.
  - Auto-play next episode with on-screen countdown (Cancel / Play Now).
  - **Chapter & OP/ED Skipping**: Auto-skip or floating "Skip Intro / Outro" button with custom chapter-name skip patterns and timeline highlighting.
  - Custom player keybinds and persistent playback preferences.

---

## 4. Torrent Search, Clients & Auto-Downloader

- **Built-In Torrent Search Drawer**: Search multiple torrent indexers via extensions with resolution, codec, release group, batch badge, seeders/leechers, file size, date, and availability badges for newly aired episodes.
- **Auto-Select Scoring Profiles**: Customizable scoring system ranking preferred/avoided/blocked providers, release groups, resolutions, codecs, sources, and languages.
- **Torrent Client Integrations**:
  - **qBittorrent** (with automatic re-authentication).
  - **Transmission**.
  - **Built-In Experimental Torrent Client** (classic file-based storage).
  - Dedicated `/torrent-list` & `/torrent-client` dashboard with live progress bars, speeds, ETA, ratio, peers, Pause/Resume/Delete/Open Folder controls, and live sidebar badges (`downloading + paused` count + `seeding` count).
- **Auto-Downloader (`/auto-downloader`)**:
  - Per-anime automation rules (release group, resolution, episode range, title filters, destination folder, active/paused toggle).
  - Reusable **Auto-Downloader Profiles** with multi-indexer comparison, scoring, min seeders, min/max size, excluded terms, and **download delay timers** (waiting for higher-priority releases).
  - Pending Queue (manual Approve/Reject) and Download History log, plus red sidebar badge for queued items.

---

## 5. Metadata, Trackers, Discovery & Lists

- **AniList Integration**: Full OAuth/PIN login, animated GIF avatar support, collection sync, list editor (status, progress, score, start/finish dates, repeats, notes, custom lists), and one-click manual refresh in the sidebar.
- **Custom Sources (`/custom-sources`)**: Add and track non-AniList anime and manga series with custom metadata and local files.
- **Fallback Metadata & AniZip**: Episode titles, thumbnails, synopses, air dates, and AniDB/AniZip ID mapping.
- **Discover Page (`/discover`)**: Full-bleed trending hero carousel + carousels for Trending Anime, Popular Anime, Upcoming Season, Trending Movies, Airing Schedule, Missed Sequels, Trending Manga, and Trending by Country (JP/KR/CN).
- **Advanced Search (`/search`)**: Multi-filter search (Query, Anime/Manga type, Sort, Format, Status, Season, Year, Multi-Genres, Country of Origin, Adult filter) with infinite scroll and **scroll-position restoration** when returning from an entry.
- **My Lists (`/lists`)**: Tabbed collection manager (`Watching`, `Planning`, `Completed`, `Paused`, `Dropped`, `Repeating`, Custom Lists) with inline search, sorting, and quick-edit popups.
- **Airing Schedule & Missing Episodes (`/schedule`)**: Weekly/monthly broadcast calendar (My Lists or Global) + Missing Episodes alert panel with one-click download/stream and red sidebar badge count.
- **Playlists (`playlists/`)**: Custom cross-series episode queues for sequential binge-watching.

---

## 6. Manga Management & Built-In Reader

- **Manga Hub (`/manga`)**: Dedicated manga library with unread chapter count badges, Grid/Carousel layouts, and AniList manga sync.
- **Manga Entry Page (`/manga/entry`)**: Provider selector, **"Compare All Providers"** view, source refresh, chapter list with read/unread indicators, batch chapter downloads (with AVIF handling), and local offline chapter management.
- **Built-In Manga Reader**: Long Strip (Webtoon), Single Page, Double Page spread, RTL/LTR reading directions, zoom controls, fit width/height/original, keyboard navigation, and automatic chapter progress tracking.

---

## 7. Extensions, Plugins & Developer Tools

- **In-App Extension Marketplace (`/extensions`)**: Browse, search, install, update, reload, and view unified code diffs before updating extensions.
- **4 Extension Types (Sandboxed `Goja` JS/TS Runtime)**:
  1. **Torrent Providers**
  2. **Online Streaming Providers**
  3. **Manga Providers**
  4. **App Plugins** (lifecycle hooks via `hook`/`hook_resolver`, custom sidebar Webview pages, plugin tray icons with badge counts, dropdown/button actions on anime & manga entries, DOM/Viewport APIs, ChromeDP APIs, `$habari` filename parser, CryptoJS, custom Discord RPC activities, settings access).
- **Extension Playground (`extension_playground`)**: Built-in editor and test runner for developing extensions directly inside the app.
- **Extension Health Alerts**: Pulsing red sidebar badge showing pending updates or plugins with errors.

---

## 8. Social (`Nakama`), Offline Mode & Integrations

- **Nakama Watch Parties**: Synchronized multi-user playback across local files, torrents, debrid, online streams, and custom sources via host relay, complete with **in-player live Watch Party Chat**.
- **Nakama Library Sharing**: Share your local anime library with connected friends; sidebar displays live connected peer count or pulsing green connected dot.
- **Offline Mode (`/sync` + Platform Abstraction Layer)**: Snapshot selected anime/manga metadata + chapters locally and switch seamlessly into Offline Mode (with a streamlined `OfflineSidebar`) when disconnected from the internet.
- **Discord Rich Presence (`discordrpc`)**: Automatic watching/reading activity display with timestamps and plugin customization hooks.
- **DNS-over-HTTPS (`doh`)**: Built-in DoH resolver for bypassing ISP blocks/throttling.
- **Cron Scheduler & Notifications (`cron`, `notifier`, `notification`)**: Scheduled background tasks, OS desktop notifications, and in-app toast notifications.
- **Security & Multi-Device Access**: Optional server password protection, strict/privileged mode guards, mobile companion app (**Seanime Tenji**), and mobile embedded server (`gomobile`).

---

## 9. UI/UX Shell, Navigation & Power-User Features

- **Modular Home Screen (15 Configurable Widgets)**: Add, remove, reorder, and customize `Continue Watching Header`, `Discover Header`, `Continue Watching Row`, `Anime Library`, `My Lists`, `Local Anime Library`, `Local Library Stats`, `Upcoming Library Episodes`, `Aired Recently`, `Missed Sequels`, `Schedule Calendar`, `Custom Anime Carousel`, `Custom Manga Carousel`, `Manga Library`, and `Centered Title` (each with Grid/Carousel layout and filter options).
- **Collapsible Hover Sidebar**: Icon-only rail expanding smoothly to `260px` on hover, optional transparency over banners, live status badges, **automatic `ResizeObserver` overflow folding into a `"More"` menu**, manual item unpinning, and desktop Back/Forward HoverCard buttons.
- **Global Command Palette (`SeaCommand` — `Cmd/Ctrl+J`, `Q`, `S`)**:
  - Slash commands: `/search`, `/anime`, `/manga`, `/library`, `/magnet`, `/spoilers`, `/logs`, `/issue`, `/droptorrent`, `/reload`, `/devtool`.
  - **Global Magnet Paste Intercept**: Pasting a `magnet:` link anywhere in the app automatically opens the `/magnet` stream/download picker (auto-selecting the current anime if you're on an `/entry` page).
- **Rich Media Cards & Hover Popups**: 2:3 poster cards with progress/unwatched/score badges, hover action popups, right-click context menus, skeleton loaders, and `Alt + Mouse Wheel` / click-and-drag horizontal carousels.
- **5-Toggle Anime Spoiler Protection Suite**: Master spoiler toggle + independent toggles to blur/hide unwatched episode thumbnails, mask episode titles, hide descriptions, and optionally exempt the immediate next episode.
- **Full Theme & Appearance Studio (38 Settings)**:
  - Custom background, accent, and sidebar colors + preset theme bank.
  - 7 Media Page Banner modes (`default`, `blur/dim/hide when unavailable`, always `blur/dim/hide`), Banner size (`Large`/`Small`), Info Box (`Fluid`/`Boxed`).
  - Dynamic vs. custom Library banner image/position/opacity + custom full-page background image/opacity/blur.
  - Toggleable card blur, legacy vs. modern episode cards, smaller episode carousels, hide top navbar, disable carousel auto-scroll, hide episode descriptions/filenames, unwatched/unread count toggles, default sorts per collection, and **Desktop + Mobile Custom CSS**.
- **Desktop Polish & Diagnostics**:
  - Smooth `200ms` route View Transitions keeping sidebar, titlebar, and mini-player pinned.
  - Zero-jank scrollbar locking, top indefinite loading bar, AniList rate-limit loader, guided onboarding wizard, interactive feature tour, announcements banner.
  - **Live Settings Reload**: All 15 Settings tabs apply changes immediately without restarting.
  - **Error Explainer Modal**, **Anonymized Log Viewer**, **Multi-Channel Auto-Updater** (`GitHub`, `Seanime`, `Canary`), and **Built-In `rrweb` Issue Recorder**.
