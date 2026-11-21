# Seanime — Complete UI & UX Specification Document

A full, screen-by-screen and component-by-component breakdown of everything Seanime offers in its user interface (`seanime-web` and desktop `seanime-denshi`), covering design tokens, global shell & navigation, modular home widgets, media cards, detail pages, streaming/torrent/debrid modals, the built-in video player (`VideoCore` / `MpvCore`), manga reader, command palette, theme customization, spoiler protection, social watch parties, and all 15 settings panels.

---

## 1. Visual Design System & Tokens

### Core Dark-First Color Palette (`globals.css`)

Seanime is dark-mode-first with an ultra-deep obsidian canvas, subtle translucent hairline borders, and a signature indigo-violet brand accent:

| Token                                | Default Value                                                                                                                                                                             | Visual Role                                              |
| ------------------------------------ | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------- |
| `--background`                       | `#070707`                                                                                                                                                                                 | Main application canvas behind all views                 |
| `--paper`                            | `rgb(11, 11, 11)` (`#0B0B0B`)                                                                                                                                                             | Cards, modals, drawers, popovers, media popup cards      |
| `--paper-lighter`                    | `rgb(13, 13, 13)` (`#0D0D0D`)                                                                                                                                                             | Elevated surfaces, hover states, nested panels           |
| `--foreground`                       | `#D1D1D1` (`gray-200`)                                                                                                                                                                    | Primary readable body text                               |
| `--muted`                            | `rgba(255, 255, 255, 0.40)`                                                                                                                                                               | Secondary labels, timestamps, metadata subtitles         |
| `--muted-highlight`                  | `rgba(255, 255, 255, 0.60)`                                                                                                                                                               | Hovered secondary labels, active subtle icons            |
| `--border`                           | `rgba(255, 255, 255, 0.10)`                                                                                                                                                               | Crisp 1px hairline separators & card outlines            |
| `--subtle`                           | `rgba(255, 255, 255, 0.06)`                                                                                                                                                               | Soft button fills, inactive pill backgrounds, hover rows |
| `--subtle-highlight`                 | `rgba(255, 255, 255, 0.08)`                                                                                                                                                               | Hovered subtle rows and cards                            |
| `--color-brand-500` (Default Accent) | `#6152DF` (`rgb(97, 82, 223)`)                                                                                                                                                            | Primary accent, active focus rings, selected states      |
| Brand Ramp (`50` → `950`)            | `#F2F0FF` → `#C7C2FF` (`300`) → `#9F92FF` (`400`) → `#6152DF` (`500`) → `#3F2EB2` (`700`) → `#1A144F` (`950`)                                                                             | Gradients, active badges, light/dark semantic accents    |
| Gray Ramp (`50` → `950`)             | `#E6E6E6` → `#8F8F8F` (`400`) → `#5A5A5A` (`500`) → `#282828` (`700`) → `#141414` (`800`) → `#101010` (`900`) → `#0B0B0B` (`950`)                                                         | Neutral surfaces and text hierarchy                      |
| Semantic Status Colors               | Red (errors / missing episodes), Green/Emerald (active downloads / online / watched), Amber/Yellow (warnings / ratings), Blue/Sky (offline sync / info), Indigo/Purple (Nakama / plugins) | Badges, toast alerts, progress bars, status dots         |

### Geometry, Typography & Effects

- **Border Radius**: Base `--radius: 0.5rem` (`8px`) for inputs, buttons, cards, and popovers; `12px` (`rounded-xl`) for expanded floating sidebar and larger modals; `9999px` (`rounded-full`) for icon buttons, status pills, and scrollbars.
- **Typography**: Clean geometric sans-serif (`Inter` / system fallback) with tight tracking (`tracking-tight`) on headings:
  - `h1`: `36px–48px` ExtraBold (`font-extrabold`)
  - `h2`: `30px` Bold (`font-bold`)
  - `h3`: `24px` SemiBold (`font-semibold`)
  - `h4`: `20px` SemiBold
  - `h5`: `18px` SemiBold
  - `h6`: `16px` SemiBold
  - Inline code / filenames: `14px` monospace in dark rounded pills (`.code`).
- **Custom Scrollbars**: Slim `8px` width/height, transparent track, rounded `rgba(255, 255, 255, 0.20)` thumb (`0.30` on hover) with padding clip so scrollbars never look heavy. Horizontal carousels hide scrollbars completely (`.hide-scrollbar`).
- **Ambient Halo Gradients**: Subtle multi-stop radial color blur (`blur(100px) saturate(150%)`, `opacity: 0.10`) behind hero headers and feature sections to prevent flat black monotony.
- **View Transitions**:
  - **Route Transitions (Desktop)**: `200ms ease` cross-fade on main content (`denshi-main`) while keeping the sidebar (`denshi-sidebar`), window titlebar (`denshi-titlebar`), and active mini-player (`denshi-vc-player`) completely pinned and flicker-free.
  - **Player ↔ Mini-Player Morph**: `320ms cubic-bezier(0.22, 1, 0.36, 1)` smooth geometric morph between full-screen/drawer player and floating corner mini-player.
  - Respects `prefers-reduced-motion: reduce` everywhere.

---

## 2. Global App Shell, Navigation & Desktop Chrome

### Desktop Window & Top Chrome

- **Custom Electron/Desktop Titlebar (`--titlebar-h: 40px`)**: Frameless draggable top bar on desktop with native window controls and pinned view-transition layer.
- **Top Navbar (`top-navbar.tsx`)**: Optional (can be hidden via `hideTopNavbar` theme setting); hosts quick actions, library scan trigger, notification/status indicators, and contextual top-level links.
- **Global Top Indefinite Loader (`top-indefinite-loader.tsx`)**: Ultra-thin animated progress line pinned to the very top edge of the viewport during background fetches/scans.
- **Rate-Limit Loader (`rate-limit-loader.tsx`)**: Subtle visual indicator when AniList API rate-limiting triggers a cooldown retry.
- **Announcements Banner (`announcements.tsx`)**: Dismissible in-app release/announcement notifications.
- **Guided Onboarding & Tour (`getting-started/`, `tour/`)**: First-run setup wizard (library path, AniList login, player/torrent/debrid choices) plus interactive feature walkthrough overlays.

### Main Sidebar (`main-sidebar.tsx`)

A sleek, vertical icon rail on the left that supports **transparent overlay mode** (blending over full-bleed page banners) and **hover-to-expand (`expandSidebarOnHover`)** from icon-only to `260px` wide with a smooth `300ms` width transition and right-edge gradient fade:

#### Top Section — Brand & Primary Routes

1. **Seanime Logo** centered at top (`mt-2` on desktop).
2. **Primary Navigation Items** (dynamically shown based on enabled features):
   - **Home** (`/`) — Modular dashboard / library home.
   - **Schedule** (`/schedule`) — Airing calendar + **red badge** showing missing downloaded episode count.
   - **Manga** (`/manga`) — Shown when Manga is enabled.
   - **My Lists** (`/lists`) — Full AniList/local list manager.
   - **Discover** (`/discover`) — Trending, popular, seasonal, upcoming, movies, missed sequels.
   - **Search** (`/search`) — Advanced filterable catalog search.
   - **Torrent List / Built-In Torrent Client** (`/torrent-list` or `/torrent-client`) — Icon dynamically changes to qBittorrent, Transmission, or BitTorrent icon; shows **green badge** with `downloading + paused` count and appends `"(N seeding)"` to tooltip/label.
   - **Debrid** (`/debrid`) — Shown when Debrid is enabled.
   - **Scan Summaries** (`/scan-summaries`) — Shown when a local library path is configured.
   - **Auto Downloader** (`/auto-downloader`) — Shown when torrent provider + library path are configured; displays **red badge** with queued download count.
3. **Plugin Webview Sidebar Items**: Extensions can inject their own pinned sidebar routes.
4. **Automatic Overflow Management (`ResizeObserver`)**: If the window height becomes too short to fit all sidebar icons, Seanime automatically moves trailing plugin items and menu items into a collapsible **"More" (`>`)** submenu—and restores them on window resize. Users can also manually unpin items into "More" via settings.
5. **Refresh AniList Action**: One-click manual sync button directly in the sidebar.
6. **Desktop Back / Forward Navigation**: On desktop builds, a subtle Back arrow button appears below the nav; hovering over it reveals a floating Forward arrow button (`HoverCard`), keeping chrome minimal.
7. **Plugin Sidebar Tray (`PluginSidebarTray`)**: Small icon tray for background plugin widgets with badge counts.

#### Bottom Section — System, Social & User Profile

1. **Update Modal Trigger** (Web or Electron auto-updater banner when a new version is available).
2. **Nakama (Watch Party / Library Sharing)**: Shown when enabled; displays an **info badge** with connected peer count when hosting, or a **pulsing green dot** when connected to a host.
3. **Extensions** (`/extensions`): Displays a **pulsing red badge** with available extension updates or plugin issue counts.
4. **Offline / Sync** (`/sync`): Displays a **blue badge** when offline sync is active.
5. **Settings** (`/settings`).
6. **User Avatar & Account Menu**:
   - Displays the user's AniList avatar—including a clever **animated GIF avatar probe** that checks if a `.gif` version exists on AniList's CDN and upgrades static avatars automatically.
   - Clicking opens a dropdown to **Sign Out** (with confirmation dialog) or **Log in with AniList** (OAuth token modal with direct link to AniList PIN URL + token paste box).
7. **Offline Sidebar (`offline-sidebar.tsx`)**: A streamlined sidebar variant automatically swapped in when Offline Mode is active.

---

## 3. Global Command Palette (`SeaCommand` — `Cmd/Ctrl+J` or `Q`)

One of Seanime's best power-user features—a spotlight-style command dialog (`max-w-2xl`, `300px` scrollable result list) accessible from anywhere:

- **Default Shortcuts**: `Meta+J` (`Cmd+J` / `Ctrl+J`) or `q` opens the palette; pressing `s` anywhere opens it pre-filled with `/search`.
- **Built-In Slash Commands**:
  - `/search <query>` — Live search across AniList anime/manga without leaving the current page.
  - `/anime <title>` — Instant fuzzy jump to any anime in your collection.
  - `/manga <title>` — Instant jump to any manga in your collection.
  - `/library <title>` — Instant jump to locally downloaded library titles.
  - `/magnet <link>` — Multi-step magnet link ingestion workflow (pick anime → pick episode → stream or download).
  - `/spoilers` — Quick toggle for Anime Spoiler Protection settings.
  - `/logs` — Open server/client logs viewer.
  - `/issue` — Launch the built-in Issue Recorder.
  - `/droptorrent` — Immediately drop the active Torrentstream session.
  - `/reload` — Trigger module/page reload.
  - `/devtool` — Developer / debug actions.
- **Global Magnet Paste Detection**: Pasting a `magnet:?xt=...` URL _anywhere_ in the app (outside text inputs) automatically intercepts the paste, opens the Command Palette in `/magnet` mode, and:
  - If you are already on an `/entry?id=...` page, skips straight to **Episode Selection**.
  - Otherwise, opens **Anime Selection → Episode Selection**.
- **Contextual & Plugin Injectables**: Pages and plugins can dynamically inject quick actions into the command palette.

---

## 4. Modular Home Screen (`home-screen.tsx` & `home-settings-modal.tsx`)

Instead of a hardcoded dashboard, Seanime's Home screen is a **user-customizable stack of 15 widget types**, configurable via the **Home Settings Modal** (drag/reorder, add, remove, configure per-widget layout and filters):

### Available Home Widgets (`HOME_ITEMS`)

| Widget ID                        | Kind         | Configurable Options & Purpose                                                                                                                                                         |
| -------------------------------- | ------------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `anime-continue-watching-header` | Hero Header  | Full-bleed hero carousel of anime you are currently watching with backdrop artwork and quick play                                                                                      |
| `discover-header`                | Hero Header  | Full-bleed trending anime hero carousel with synopsis, metadata badges, and jump-to-entry                                                                                              |
| `anime-continue-watching`        | Row / Header | Horizontal episode carousel of the next unwatched episodes across your active shows                                                                                                    |
| `anime-library`                  | Row          | Downloaded / currently watching anime grouped by status (`CURRENT`, `PAUSED`, `PLANNING`, `COMPLETED`, `DROPPED`) in **Grid** or **Carousel** layout                                   |
| `my-lists`                       | Row          | Media from your AniList/custom lists filtered by status (including `REPEATING`), Type (`Anime` or `Manga`), optional Custom List Name, in **Grid** or **Carousel** layout              |
| `local-anime-library`            | Row          | Complete **Grid** or **Carousel** of every anime detected on your local disk                                                                                                           |
| `local-anime-library-stats`      | Row          | Summary stat strip of your local library (total files, total GB/TB, matched shows, unmatched files)                                                                                    |
| `library-upcoming-episodes`      | Row          | Countdown carousel of upcoming episodes specifically for shows in your library                                                                                                         |
| `aired-recently`                 | Row          | Global carousel of anime episodes that aired recently                                                                                                                                  |
| `missed-sequels`                 | Row          | Carousel highlighting sequels/seasons of shows you watched that aren't in your list yet                                                                                                |
| `anime-schedule-calendar`        | Row          | Interactive airing schedule calendar (`My Lists` or `Global` mode)                                                                                                                     |
| `anime-carousel`                 | Row          | Custom user-defined anime carousel with custom Name, Sort (Popular, Trending, Romaji/English A–Z/Z–A, Score), Status, Format (TV/Movie/OVA/ONA/Special), Genres, Season, Year, Country |
| `manga-carousel`                 | Row          | Same custom filterable carousel for Manga / One-Shots                                                                                                                                  |
| `manga-library`                  | Row / Header | Currently Reading / Paused manga in **Grid** or **Carousel** layout                                                                                                                    |
| `centered-title`                 | Row          | Centered typographic section divider / custom heading                                                                                                                                  |

### Home Toolbar (`home-toolbar.tsx`)

- Floating/sticky action bar on the Home screen providing:
  - **Scan Library** button (with live scan progress & unmatched file indicator).
  - **Library Explorer** button (opens file-tree drawer).
  - **Playlists** button (opens binge-watch playlist drawer).
  - **Bulk Actions / Missing Episodes** trigger.
  - **Customize Home** button (opens Home Settings Modal).
  - **Genre Selector Strip** (optional horizontal genre filter pills, toggleable via `disableLibraryScreenGenreSelector`).
- **Horizontal Carousels**: Support click-and-drag scrolling (`horizontal-draggable-scroll`), left/right arrow controls, optional auto-scroll (`disableCarouselAutoScroll`), and **`Alt + Mouse Wheel`** horizontal scrolling.

---

## 5. Media Cards, Episode Cards & Hover Interactions

### Media Entry Card (`media-entry-card.tsx` & `media-entry-card-components.tsx`)

Used everywhere across Home, My Lists, Discover, Search, and Manga:

- **Poster Aspect Ratio**: Standard vertical 2:3 anime/manga cover art with subtle dark bottom gradient (`item-bottom-gradients.tsx`) and optional blurred-background card glow (`enableMediaCardBlurredBackground`).
- **Overlay Badges**:
  - **Top-Left**: Watch/read progress badge (`media-entry-progress-badge.tsx`, e.g., `5 / 12` or `EP 5`), plus optional **unwatched episode count** (`showAnimeUnwatchedCount`) or **unread chapter count** (`showMangaUnreadCount`) pill.
  - **Top-Right**: Community/personal score badge (`media-entry-score-badge.tsx`) and airing/status dot.
  - **Bottom**: 1–2 line clamped title + format/year/episode subtitle.
- **Hover Popup / Action Card**: Hovering over a card smoothly elevates it and reveals a rich quick-action popup (`--media-card-popup-background`) with:
  - Quick **Play Next Episode** button.
  - **List Status / Score / Progress** quick editor modal trigger (`anilist` list editor).
  - Right-click **Context Menu** (`context-menu/`) for opening entry, opening folder, refreshing metadata, or adding to playlist.
- **Skeleton Loaders (`media-entry-card-skeleton.tsx`)**: Exact-proportion shimmer placeholders during loading.

### Episode Cards (`episode-list/`)

- **Modern vs. Legacy Card Toggle (`useLegacyEpisodeCard`)**:
  - **Modern Card**: Wide 16:9 episode thumbnail with bottom gradient, episode number pill, episode title, optional anime title (`showEpisodeCardAnimeInfo`), optional description (`hideEpisodeCardDescription`), optional local filename badge (`hideDownloadedEpisodeCardFilename`), watched checkmark / continuity progress bar along the bottom edge, and filler badge.
  - **Spoiler Protection Integration**: When `hideAnimeSpoilers` is enabled, unwatched episode cards can independently blur/hide thumbnails (`hideAnimeSpoilerThumbnails`), mask episode titles (`hideAnimeSpoilerTitles`), hide episode descriptions (`hideAnimeSpoilerDescriptions`), and optionally exempt the immediate next episode (`hideAnimeSpoilerSkipNextEpisode`).
- **Episode Info Modal (`media-episode-info-modal.tsx`)**: Clicking info on any episode opens full synopsis, air date, runtime, AniDB/AniZip metadata, and file path details.

---

## 6. Anime Entry Detail Page (`entry/`)

The centerpiece screen of Seanime, designed with cinematic full-bleed artwork and tabbed playback modes:

### Page Header & Banner (`media-page-header-components.tsx`)

- **7 Banner Display Modes (`mediaPageBannerType`)**:
  1. `default` — Full banner image (falls back to cover art).
  2. `blur-when-unavailable` — Crisp banner if present, blurred cover art fallback.
  3. `dim-when-unavailable` — Dimmed fallback.
  4. `hide-when-unavailable` — Hides banner when missing.
  5. `blur` — Always blurred.
  6. `dim` — Always dimmed.
  7. `hide` — Always hidden.
- **2 Banner Sizes**: `Large` (immersive hero height) or `Small` (`media-entry-page-small-banner.tsx`).
- **2 Info Box Layouts**: `Fluid` (full-width modern arrangement) or `Boxed`.
- **Optional Blurred Page Background** (`enableMediaPageBlurredBackground`).
- **Header Content**:
  - Cover poster (clickable/enlargeable), primary + native/romaji titles, studio pill (`anime-entry-studio.tsx`), status/season/year/format/duration metadata strip, genre tags (`media-genre-selector.tsx`), mean score & popularity, collapsible synopsis, and **AniList Entry Editor** button (status, progress, score, start/finish dates, repeats, notes, custom lists).
  - **Entry Actions Bar (`entry-actions/`)**: Open local folder, refresh metadata, match/unmatch local files, lock/unlock files, view AniList/MAL/AniDB external links, Nakama share/watch-party trigger, and plugin-injected dropdown actions.
- **Layout Modes**: Supports `stacked` layout (header above full-width episodes) and responsive drawer mode (`anime-entry-drawer.tsx`).

### Playback Source Tabs on the Entry Page

Depending on what is enabled, the Anime Entry page cleanly separates playback sources via tabs:

1. **Library (Local Files) (`episode-list/`)**:
   - Shows matched local episodes, missing episodes banner (with one-click "Download Missing" button), episode range/chunk pagination for long-running shows (100+ eps), filter/sort controls, bulk file manager (delete, lock, move, re-match), and special/OVA/NCOP/NCED tabs.
   - Clicking an episode plays via your configured player (Built-in `VideoCore`/`MpvCore`, Mediastream transcode, or External MPV/VLC/MPC-HC/IINA).
2. **Torrent Streaming (`torrent-stream/`)**:
   - Episode list synced to AniZip/AniList metadata.
   - **Auto-Select** button (one click finds & streams the best torrent using your profile) OR **Manual Torrent Search** drawer (`torrent-search/`) showing indexer results with resolution/codec/group/seeders/leechers/size/date columns, batch indicator, file-inside-batch picker, and search/sort filters.
   - Live torrent buffer overlay showing peer count, download/upload speed, and piece readiness while starting playback.
3. **Debrid Streaming (`debrid-stream/`)**:
   - Same clean episode list with **cached instant-play badges**, auto-select or manual torrent picker, batch file selector, and one-click "Download to Library" option.
4. **Online Streaming (`onlinestream/`)**:
   - Provider selector dropdown, Sub/Dub toggle, server/mirror selector, episode grid/list, and inline or full-screen player launch.

### Secondary Detail Sections

- **Characters & Voice Actors (`media-entry-characters-section.tsx`)**: Horizontal row/grid of character portraits with role and Japanese VA.
- **Relations & Recommendations**: Linked prequels, sequels, side stories, spin-offs, and community recommendations as clickable Media Entry Cards.

---

## 7. Built-In Video Player (`VideoCore` & `MpvCore` — 42 Components)

Seanime's built-in player (`_features/video-core/` and `_features/mpv-core/`) is a full cinema-grade overlay UI shared across local files, Mediastream, Torrentstream, Debridstream, and Onlinestream:

### Player Chrome & Layout

- **Discrete Auto-Hiding Controls**: Controls and mouse cursor automatically fade out during playback/buffering unless hovering over the control bar (`.discrete-controls`), while **JASSUB ASS/SSA subtitles always stay 100% visible**.
- **Top Overlay Bar (`video-core-top-section.tsx`)**:
  - Anime title + Episode number & title.
  - **Mini-Player Toggle** (morphs player into a floating bottom-right corner window via CSS View Transitions so you can keep browsing the app while watching; in mini-player mode, all secondary controls hide and only a centered play/pause button + expand/close controls remain).
  - **Close / Back** button.
- **Bottom Seekbar (`video-core-time-range.tsx`)**:
  - Slim `3–5px` track (`8px` in fullscreen) that expands on hover with a smooth scaling thumb (`scale(0) → scale(1)`).
  - Displays played progress (accent color), buffered range (`rgba(255,255,255,0.4)`), pointer hover track, and **Chapter Cue Markers** (vertical ticks along the timeline + highlighted OP/ED segments).
  - **Hover Scrub Preview (`video-core-preview.ts`)**: Floating frame thumbnail preview (`2px` white border) + exact timestamp + floating **Chapter Name Pill** (`media-preview-chapter-display`).
- **Control Bar (`video-core-control-bar.tsx`)**:
  - **Left Group**: Morphing Play/Pause button (SVG `clip-path` animation), Previous Episode & Next Episode buttons, Mute button with **hover-expanding Volume Slider (`0px → 70px`)** + **Volume Boost** (>100%), and clickable **Timestamp** toggling between `Elapsed / Total` and `-Remaining / Total`.
  - **Right Group**:
    - **Episode Playlist Drawer (`video-core-playlist.tsx`)**: Slide-out episode list inside the player to jump episodes without leaving fullscreen.
    - **In-Sight Panel (`video-core-in-sight.tsx`)**: Contextual overlay showing characters/cast/info for the anime you're watching right inside the player.
    - **Watch Party Chat (`video-core-watch-party-chat.tsx`)**: Live Nakama chat drawer overlaid on the player.
    - **Audio Track Menu (`video-core-audio-menu.tsx`)**: Switch Japanese/English/dual audio tracks.
    - **Subtitle Menu (`video-core-subtitle-menu.tsx`)**: Switch embedded ASS/SSA (JASSUB), Blu-ray PGS, WebVTT/SRT tracks, load external subtitle files, toggle captions (with animated accent underline bar when active), and adjust subtitle delay/size/offset.
    - **Resolution / Quality Menu (`video-core-resolution-menu.tsx`)**: Switch HLS/transcode/online quality levels.
    - **Settings Menu (`video-core-settings-menu.tsx` & `video-core-preferences.tsx`)**: Playback speed, **Anime4K Shader presets** (`video-core-anime-4k-manager.ts`), aspect ratio/zoom, auto-play next episode, auto-skip OP/ED & custom skip chapter patterns, screenshot capture (`video-core-screenshot.ts` + confirmation prompt), keybinds customization, and **Stats for Nerds (`video-core-stats.tsx`)** (codec, resolution, bitrate, dropped frames, buffer health, torrent speeds).
    - **Cast Button (`video-core-cast.tsx`)**, **Picture-in-Picture (`video-core-pip.ts`)**, and **Fullscreen (`video-core-fullscreen.ts`)**.
- **On-Screen Overlays (`video-core-overlay-display.tsx`)**:
  - Center animated loading spinner (300ms delay so quick seeks don't flash a spinner).
  - Floating **"Skip Intro / Skip Outro / Skip Chapter"** button when inside a tagged chapter.
  - **Auto-Play Next Episode Countdown** card (`autoplay/`) near the end of an episode with Cancel / Play Now buttons.
  - Visual HUD feedback on keyboard shortcuts (volume %, seek +/- seconds, speed change, screenshot saved).
- **Mobile Touch Gestures (`video-core-mobile-gestures.ts`)**: Dedicated mobile control bar layout, double-tap left/right to seek, vertical swipe for brightness/volume, and iOS fullscreen subtitle handling.

---

## 8. All Other Full-Page Screens

### Discover (`/discover`)

- Full-bleed trending hero carousel at top.
- Stacked horizontal carousels (`discover/_containers/`):
  - **Trending Anime**, **Popular Anime**, **Upcoming Season**, **Trending Movies**, **Airing Schedule**, **Missed Sequels**, **Trending Manga (All)**, and **Trending by Country** (Japan, South Korea, China).
- Filterable genre chips and quick jump into Advanced Search.

### Advanced Search (`/search`)

- Full filter bar: Search query input, Type (Anime / Manga), Sort order, Format, Status, Season, Year, Genres (multi-select), Country of Origin, and adult-content filter.
- Responsive poster grid with infinite scroll / pagination and preserved scroll position when navigating back from an entry.

### My Lists (`/lists`)

- Complete AniList + Custom Source list viewer organized by status tabs (`Watching`, `Planning`, `Completed`, `Paused`, `Dropped`, `Repeating`, plus custom lists).
- Inline search/filter bar, sort selector, and quick edit modal on every card.

### Airing Schedule (`/schedule`)

- Weekly / monthly airing calendar showing exact local broadcast times for episodes of shows in your lists (and global airing schedule).
- **Missing Episodes Section**: Dedicated alert panel listing aired episodes that are not yet in your local library, with one-click download/stream actions.

### Manga Hub & Built-In Reader (`/manga` & `/manga/entry`)

- **Manga Library**: Grid/carousel of currently reading & downloaded manga with unread chapter count badges.
- **Manga Entry Page**: Cover art, metadata, AniList sync, provider selector dropdown, **"Compare All Providers"** view, source refresh, chapter list with read/unread checkmarks, batch chapter download queue, and local chapter storage management.
- **Manga Reader**: Full-screen distraction-free reader supporting:
  - Multiple reading modes (Long Strip / Webtoon vertical scroll, Single Page, Double Page spread, RTL / LTR).
  - Zoom controls, image fit modes (fit width, fit height, original), page progress bar, keyboard navigation, and automatic AniList chapter progress update.

### Torrent List & Built-In Torrent Client (`/torrent-list`, `/torrent-client`)

- Real-time table/cards of active torrents (downloading, seeding, paused, completed) showing progress bars, download/upload speeds, ETA, ratio, peers/seeds, file size, and one-click Pause, Resume, Delete (with/without files), and Open Folder actions.

### Debrid Dashboard (`/debrid`)

- Shows active/cached Debrid downloads and transfers, instant download-to-library actions, file browser for multi-file debrid torrents, and cleanup controls.

### Auto-Downloader (`/auto-downloader`)

- **Rules List**: Per-anime automation rules (release group, resolution, title comparison, providers, min/max episode, destination folder, active/paused toggle).
- **Profiles Tab**: Reusable scoring/filter profiles (providers, codecs, release groups, min seeders, min/max size, excluded terms, download delay timer).
- **Queue & History Tab**: Pending/delayed torrents awaiting download (with manual Approve/Reject) and log of past automatic downloads.

### Scan Summaries (`/scan-summaries`)

- Detailed post-scan audit log showing every scanned folder, matched anime & file count, confidence scores, and a highlighted list of unmatched/problematic files with direct links to fix matches.

### Library Explorer (`library-explorer/`)

- Slide-over file browser showing your actual disk folders side-by-side with Seanime's matched anime entries, allowing quick renaming, locking, unlocking, and file deletion.

### Playlists (`playlists/`)

- Create, reorder, and play custom multi-show episode queues for binge-watching, with automatic sequential playback across different series.

### Extensions Marketplace (`/extensions`)

- **Installed Tab**: Manage installed Torrent, Online Streaming, Manga, and Plugin extensions; inspect settings, reload, view errors/issues, and check code diffs before updating.
- **Marketplace Tab**: Browse community repositories by category, search, and install extensions in one click.
- **Extension Playground / Code Editor**: Built-in editor for developing/testing custom extensions locally.

### Nakama Social Modal (`nakama/`)

- Modal/drawer to host a room or join a friend's server (room URL + passcode), view connected peers, toggle **Host Library Sharing**, and start/join synchronized **Watch Parties** with live chat.

### Offline & Sync (`/sync`)

- Select specific anime/manga entries to snapshot and cache for Offline Mode, toggle Offline Mode on/off, and inspect local offline storage usage.

### Custom Sources (`/custom-sources`)

- UI for creating and managing non-AniList anime/manga entries (custom titles, covers, episode counts, and local file bindings).

### Diagnostics & Support UI

- **Error Explainer (`error-explainer/`)**: Human-readable troubleshooting modal translating cryptic network/player/torrent errors into actionable steps.
- **Issue Recorder (`issue-report/`)**: Built-in `rrweb` session recorder + anonymized log bundler to export reproducible bug reports.

---

## 9. Settings Page — All 15 Tabs (`settings/`)

Seanime organizes all configuration into a clean tabbed Settings screen with a sticky **Save** button (`settings-submit-button.tsx`) that applies changes live without restarting:

1. **Anime Library (`anime-library-settings.tsx`)**: Library paths, auto-scan, matching algorithm & threshold, legacy matching toggle, auto-update AniList progress.
2. **Video Playback (`playback-settings.tsx` & `mediaplayer-settings.tsx`)**: Default playback source, external player selection (MPV/VLC/MPC-HC/IINA paths, host/port/password), built-in `MpvCore` / `VideoCore` preferences, auto-play next episode, watch continuity, chapter skip patterns, screenshot directory, log export.
3. **Mediastream (`mediastream-settings.tsx`)**: Direct play vs. FFmpeg transcoding, hardware acceleration (NVENC/QSV/VAAPI/VideoToolbox), bitrate/preset, subtitle/font extraction.
4. **Torrent Client & Search**: Default torrent provider, qBittorrent / Transmission / Built-in client credentials, active torrent count badge toggle, **Auto-Select Profile Editor (`autoselect-profile-form.tsx` — 45 KB)** for ranking providers, release groups, codecs, sources, and languages.
5. **Torrentstream (`torrentstream-settings.tsx`)**: Enable Torrentstream, auto-select, download directory, preload next episode, preferred player.
6. **Debrid (`debrid-settings.tsx`)**: Provider selector (Real-Debrid, Torbox, AllDebrid, Premiumize), API key, Debridstream settings.
7. **Manga (`manga-settings.tsx`)**: Enable manga, default providers, download directory, reader defaults.
8. **AniList (`anilist-settings.tsx`)**: Account connection, list sync preferences, fallback metadata provider.
9. **UI & Appearance (`ui-settings.tsx` — 69 KB)**: Full theme & layout studio (detailed in Section 10 below).
10. **Nakama (`nakama-settings.tsx`)**: Enable Nakama, host/peer mode, server URL, passcode, library sharing toggle.
11. **Discord Rich Presence (`discord-rich-presence-settings.tsx`)**: Enable RPC, show anime/manga activity, show timestamps/buttons.
12. **Desktop / Denshi (`denshi-settings.tsx`)**: Window behavior, view transitions, update channel (`GitHub`, `Seanime`, `Canary`).
13. **Server (`server-settings.tsx`)**: Port, host, password protection, CORS, mobile server options.
14. **File Cache (`filecache-settings.tsx`)**: Cache size inspection and one-click clear for posters/metadata/transcode temp files.
15. **Logs & Data (`logs-settings.tsx`, `data-settings.tsx`, `local-settings.tsx`)**: Live log viewer with anonymization, database/settings backup & reset.

---

## 10. Complete Theme & UI Customization Matrix (`theme-hooks.ts`)

Every visual toggle Seanime exposes in `Settings → UI` (38 distinct UI settings):

| Category                     | Setting Keys & Exact Behavior                                                                                                                                                                                                               |
| ---------------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Colors**                   | `enableColorSettings`, `backgroundColor` (`#070707`), `accentColor` (`#6152df`), `sidebarBackgroundColor` (`#070707`), plus curated presets (`theme-bank.ts`)                                                                               |
| **Sidebar & Chrome**         | `expandSidebarOnHover`, `disableSidebarTransparency`, `hideTopNavbar`, `unpinnedMenuItems[]`                                                                                                                                                |
| **Background & Blur**        | `libraryScreenCustomBackgroundImage`, `libraryScreenCustomBackgroundOpacity` (default `10%`), `libraryScreenCustomBackgroundBlur`, `enableBlurringEffects`, `enableMediaCardBlurredBackground`, `enableMediaPageBlurredBackground`          |
| **Library Banner**           | `libraryScreenBannerType` (`dynamic` vs `custom`), `libraryScreenCustomBannerImage`, `libraryScreenCustomBannerPosition` (`50% 50%`), `libraryScreenCustomBannerOpacity` (`100%`), `disableLibraryScreenGenreSelector`                      |
| **Media Entry Banner**       | `mediaPageBannerType` (7 modes: Default, Blur/Dim/Hide when unavailable, always Blur/Dim/Hide), `mediaPageBannerSize` (`default` Large vs `small`), `mediaPageBannerInfoBoxSize` (`fluid` vs `boxed`), `animeEntryScreenLayout` (`stacked`) |
| **Cards & Carousels**        | `smallerEpisodeCarouselSize`, `useLegacyEpisodeCard`, `disableCarouselAutoScroll`, `showEpisodeCardAnimeInfo`, `showAnimeUnwatchedCount`, `showMangaUnreadCount`, `hideEpisodeCardDescription`, `hideDownloadedEpisodeCardFilename`         |
| **Default Sorting**          | `continueWatchingDefaultSorting` (`AIRDATE_DESC`), `animeLibraryCollectionDefaultSorting` (`TITLE`), `mangaLibraryCollectionDefaultSorting` (`TITLE`)                                                                                       |
| **Spoiler Protection Suite** | `hideAnimeSpoilers` (master toggle), `hideAnimeSpoilerThumbnails`, `hideAnimeSpoilerTitles`, `hideAnimeSpoilerDescriptions`, `hideAnimeSpoilerSkipNextEpisode`                                                                              |
| **Custom CSS**               | `customCSS` (desktop/web) and `mobileCustomCSS` (mobile-specific overrides)                                                                                                                                                                 |

---

## 11. Reusable UI Component Library Inventory (`components/ui/` — 54 Primitives)

To replicate Seanime's visual consistency across every screen, its design system is built on these **54 core UI primitives**:

- **Layout & Navigation**: `app-layout` (AppSidebar + AppLayoutContent), `page-header`, `vertical-menu`, `navigation-menu`, `tabs`, `breadcrumbs`, `scroll-area`, `horizontal-draggable-scroll`, `separator`, `collapsible`, `accordion`, `disclosure`.
- **Overlays & Feedback**: `modal`, `drawer`, `dropdown-menu`, `context-menu`, `popover`, `hover-card`, `tooltip`, `command` (CommandDialog), `alert`, `toaster`, `loading-spinner`, `skeleton`, `progress-bar`.
- **Data Display**: `card`, `carousel`, `datagrid`, `table`, `pagination`, `stats`, `timeline`, `calendar`, `charts`, `avatar`, `badge`.
- **Forms & Inputs**: `form`, `basic-field`, `button` (Button + IconButton), `input`, `text-input`, `textarea`, `number-input`, `select`, `native-select`, `combobox`, `autocomplete`, `checkbox`, `radio-group`, `switch`, `slider`, `date-picker`, `simple-dropzone`.

---

## 12. Subtle UX Polish Details Worth Copying

1. **Zero-Jank Scrollbar Locking**: Reserves exact `8px` right-padding offsets (`scroll-locked-offset`) when modals/drawers open so fixed headers and layouts never shift horizontally.
2. **Scroll Position Restoration**: Remembers your exact scroll offset when clicking into an anime from Search or My Lists and hitting Back.
3. **Graceful Sidebar Overflow**: Never clips icons or forces awkward scrolling on smaller laptops—overflowing items automatically fold into a `"More"` menu via `ResizeObserver`.
4. **Paste-a-Magnet Anywhere**: Pasting a magnet link anywhere in the window immediately launches the stream/download flow without hunting for a "Manual Magnet" button.
5. **Spoiler-Safe Browsing**: Granular independent toggles for blurring episode thumbnails, hiding titles, and hiding descriptions while optionally leaving the very next episode unblurred.
6. **Live-Updating Badges Everywhere**: Sidebar badges reflect live WebSocket state (missing episodes, active torrent count, seeding count, auto-downloader queue, Nakama peers, extension updates, offline sync).
7. **Animated Player Micro-Interactions**: Play/pause SVG `clip-path` morph, hover-expanding volume bar, rotating settings gear on open, animated caption underline, chapter ticks on seekbar, and `320ms` View Transition morph to mini-player.
8. **Fallback-First Artwork**: Every banner and avatar degrades cleanly (banner → blurred cover art; static AniList avatar → probed animated GIF avatar; missing thumbnail → styled skeleton/placeholder).
