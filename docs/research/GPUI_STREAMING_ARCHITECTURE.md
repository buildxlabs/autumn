# Seanime-Inspired Desktop Streaming Platform — Rust + GPUI (`gpui-kit`) Architecture & Overall Structure

A complete blueprint for building a native, GPU-accelerated desktop anime & media streaming platform in **Rust** using **[`longbridge/gpui-kit`](https://github.com/longbridge/gpui-kit)** (Zed's GPUI + `gpui-base` + `gpui-component` + optional `gpui-shell`).

---

## 1. High-Level System Architecture

GPUI runs its UI and rendering loop on a **`smol`**-backed foreground/background executor, while Rust's networking, BitTorrent, SQLite, and IPC crates run on **Tokio**. The cleanest architecture decouples the **pure Rust core** from the **GPUI presentation layer** and bridges them with [`gpui-tokio-bridge`](https://github.com/AprilNEA/gpui-tokio-bridge) and typed GPUI `Entity` stores.

```text
┌────────────────────────────────────────────────────────────────────────────┐
│                        GPUI Window (Root via gpui-kit)                     │
│  ┌──────────────┐  ┌────────────────────────────────────────────────────┐  │
│  │ Custom       │  │ Active Screen (Router / NavStack)                  │  │
│  │ Titlebar     │  │  • Home (Modular Widgets)   • Anime Entry + Tabs   │  │
│  ├──────────────┤  │  • Discover / Search        • Schedule / My Lists  │  │
│  │ Collapsible  │  │  • Torrent / Debrid / Auto  • Manga / Reader       │  │
│  │ Icon Sidebar │  │  • Extensions / Marketplace • Settings (15 Tabs)   │  │
│  │ (Hover 260px)│  └────────────────────────────────────────────────────┘  │
│  │ + Live Badges│  Overlays hosted by Root:                                │
│  │ + Overflow   │  • SeaCommand Palette (Cmd+J / Q / Global Magnet Paste)  │
│  └──────────────┘  • Modals / Drawers / Context Menus / Toasts / HUD       │
└───────────────────────────────────┬────────────────────────────────────────┘
                                    │ Entity<T> / EventEmitter / Actions
                                    ▼
┌────────────────────────────────────────────────────────────────────────────┐
│               GPUI State & Bridge Layer (gpui-tokio-bridge)                │
│  • AppState (Global)    • ThemeStore (38 Seanime tokens)                   │
│  • LibraryStore         • PlaybackStore (live position / chapters / sync)  │
│  • TorrentStore         • ProviderStore (extensions + Stremio add-ons)     │
└───────────────────────────────────┬────────────────────────────────────────┘
                                    │ async channels / Tokio::spawn
                                    ▼
┌────────────────────────────────────────────────────────────────────────────┐
│                     Tokio Background Engine (No GPUI deps)                 │
│  ┌──────────────┐ ┌──────────────┐ ┌──────────────┐ ┌───────────────────┐  │
│  │ Metadata     │ │ Providers    │ │ Torrent      │ │ Local Library     │  │
│  │ • AniList GQL│ │ • rquickjs JS│ │ • librqbit   │ │ • Fuzzy Filename  │  │
│  │ • AniZip/MAL │ │ • Stremio    │ │ • qBit / RPC │ │ • MKV Inspector   │  │
│  │ • TMDB       │ │ • Nyaa HTML  │ │ • Debrid API │ │ • File Watcher    │  │
│  └──────────────┘ └──────────────┘ └──────┬───────┘ └───────────────────┘  │
│                                           │                                │
│  ┌────────────────────────────────────────▼─────────────────────────────┐  │
│  │ Local Loopback Servers (127.0.0.1) + SQLite (sqlx / rusqlite)        │  │
│  │ • librqbit HTTP Range Streamer  • StreamRelay Proxy (Headers/DASH)   │  │
│  └────────────────────────────────────────┬─────────────────────────────┘  │
└───────────────────────────────────────────┼────────────────────────────────┘
                                            ▼
                   ┌──────────────────────────────────────────┐
                   │ Playback Layer                           │
                   │ • v1: External mpv / IINA via JSON IPC   │
                   │       (detached setsid(), Lua/IPC sync)  │
                   │ • v2: Embedded libmpv2 / GPU surface     │
                   └──────────────────────────────────────────┘
```

## 2. Cargo Workspace & Multi-Crate Layout

Using a **hexagonal multi-crate workspace** keeps incremental Rust compile times fast (editing UI never recompiles `librqbit` or `rquickjs`) and enforces a strict rule: **backend crates never import `gpui`**.

```text
autumn-player/
├── Cargo.toml                      # Workspace manifest + fast dev profiles
├── assets/
│   ├── icons/                      # Custom SVG icons (AniList, Nyaa, qBit, etc.)
│   ├── fonts/                      # Inter / JetBrains Mono
│   └── mpv/
│       └── tracker.lua             # Optional fallback tracker script
├── crates/
│   ├── domain/                     # Pure data models, IDs, enums, error types (0 I/O deps)
│   │   └── src/
│   │       ├── media.rs            # Anime, Episode, Manga, Chapter, MediaId
│   │       ├── stream.rs           # StreamSource (Local, Torrent, Debrid, Online, Addon)
│   │       ├── library.rs          # LocalFile, MatchResult, ScanSummary, ShelfState
│   │       ├── torrent.rs          # TorrentInfo, AutoSelectProfile, ScoringRule
│   │       ├── theme.rs            # All 38 Seanime theme & spoiler settings
│   │       └── events.rs           # AppEvent enum pushed from Tokio -> GPUI
│   ├── storage/                    # SQLite (sqlx or rusqlite) + disk cache (lru + rmp-serde)
│   │   └── src/
│   │       ├── db.rs               # Migrations, settings, continuity, history, playlists
│   │       └── cache.rs            # Poster/metadata MessagePack cache
│   ├── metadata/                   # AniList GraphQL (cynic/reqwest), AniZip, TMDB, SkipDB
│   ├── providers/                  # Stream & indexer providers
│   │   └── src/
│   │       ├── traits.rs           # TorrentProvider, OnlineProvider, MangaProvider traits
│   │       ├── nyaa.rs             # Nyaa / Sukebei / mirror base_url + proxy support
│   │       ├── stremio.rs          # Stateless Stremio HTTP manifest add-on client
│   │       ├── debrid.rs           # Real-Debrid, Torbox, AllDebrid, Premiumize
│   │       └── js_runtime.rs       # rquickjs sandboxed JS extension runner
│   ├── torrent/                    # Embedded librqbit streaming engine + external qBit/Transmission/rqbit clients
│   ├── proxy/                      # Local 127.0.0.1 StreamRelay (header injection, DASH/HLS prefetch)
│   ├── player/                     # Player detection (mpv/IINA/VLC), detached spawn, async JSON IPC, continuity
│   ├── library/                    # Filename parser (Habari-style), folder scanner, notify file watcher, MKV track reader
│   ├── ui_theme/                   # Seanime theme -> gpui-component ThemeColor mapping + presets
│   ├── ui_components/              # Reusable Seanime-style GPUI widgets
│   │   └── src/
│   │       ├── media_card.rs       # 2:3 Poster card, hover popup, badges, spoiler blur
│   │       ├── episode_card.rs     # 16:9 Episode card, continuity bar, spoiler masks
│   │       ├── carousel.rs         # Horizontal smooth/drag carousel + Alt+Wheel scroll
│   │       ├── hero_banner.rs      # Full-bleed banner with 7 Seanime banner modes
│   │       ├── media_grid.rs       # Virtualized responsive poster grid
│   │       ├── command_palette.rs  # SeaCommand (Cmd+J / Q / S + global magnet paste)
│   │       ├── sidebar.rs          # Collapsible icon rail (hover-expand 260px + overflow More)
│   │       └── playback_hud.rs     # Active playback status bar / mini-control overlay
│   └── ui_screens/                 # Full pages (each owns its Entity state & Render impl)
│       └── src/
│           ├── home.rs             # Modular 15-widget Home screen + Home Settings modal
│           ├── entry.rs            # Anime detail page (Banner + Local/Torrent/Debrid/Online tabs)
│           ├── discover.rs         # Trending, Popular, Upcoming, Missed Sequels, Movies
│           ├── search.rs           # Advanced multi-filter search with scroll restoration
│           ├── lists.rs            # My Lists (status tabs + quick editor)
│           ├── schedule.rs         # Airing calendar + Missing Episodes alert panel
│           ├── torrents.rs         # Live torrent manager + manual torrent search drawer
│           ├── autodownloader.rs   # Rules, Profiles (scoring), Queue & History
│           ├── debrid.rs           # Cached debrid transfers & library downloads
│           ├── extensions.rs       # Installed extensions, Marketplace, diff viewer
│           ├── manga.rs            # Manga library + distraction-free reader
│           └── settings.rs         # 15-tab Settings studio + live module reload
└── bins/
    └── app/                        # Thin binary entrypoint: init gpui-kit, tokio bridge, Root window
        └── src/main.rs
```

### Root `Cargo.toml` Tips for GPUI

- **Single Facade Pinning**: Depend on [`gpui-kit`](https://github.com/longbridge/gpui-kit) as your single GPUI facade (it pins matching `gpui-pre` / optional `gpui-fast`, `gpui-base`, `gpui-component`, and default icon assets so you never hit duplicate-type version mismatches).

- **Fast Dev Compilation** (Zed's workspace pattern):

  ```toml
  [profile.dev.package."*"]
  opt-level = 2 # Keep your own crates fast to compile, optimize taffy/resvg/image/librqbit in dev
  ```

- **Release Profile** (from `MovieBox-TUI`): `lto = true`, `codegen-units = 1`, `strip = true`, plus optional `mimalloc` allocator on non-Android targets.

## 3. GPUI + `gpui-kit` Bootstrap & Window Lifecycle

[`gpui-kit`](https://gpui-kit.com/component/root) requires a strict initialization order so dialogs, sheets/drawers, notifications/toasts, tooltips, popovers, focus traversal, and window chrome work everywhere:

1. **Application Entry**:

   - Create `Application::new().with_assets(CombinedAssets)` (layering your custom SVGs before `gpui_kit::assets::Assets`).

   - Register your `reqwest`-backed `HttpClient` on `App` so GPUI's native `img("https://...")` + `ImageCache` loads anime posters asynchronously.

2. **Initialize Subsystems (in order)**:

   - `gpui_component::init(cx);` — Registers styled overlays, tooltips, dialogs, sheets, theme, and window chrome.

   - `gpui_tokio_bridge::init(cx);` — Spawns the background Tokio runtime bridged to GPUI.

   - `ui_theme::init(cx, saved_theme_settings);` — Applies Seanime's `#070707` / `#6152df` palette to `gpui-component`'s `Theme`.

   - `app_state::init(cx);` — Initializes `Global` `AppState` holding handles to SQLite, `librqbit`, `PlaybackManager`, and `ProviderRegistry`, plus a background task forwarding `AppEvent`s into GPUI `Entity` stores.

   - Register global `KeyBinding`s (`cmd-j` / `ctrl-j`, `q`, `s`, `cmd-v` magnet intercept, `escape`, `cmd-[` / `cmd-]`).

3. **Open the Main Window**:

   - Always open windows with `**gpui_kit::open_window**` (which wraps your root `AppShell` view inside `gpui_component::Root`).

   - Configure `TitlebarOptions` with `appears_transparent: true` and `traffic_light_position` on macOS so your custom `40px` titlebar and full-bleed banners blend right up to the top window edge like Seanime Denshi.

## 4. Mapping Seanime's UI System to `gpui-kit`

### Layering Strategy (3 Layers of `gpui-kit`)

- Use `**gpui-component**` for all standard controls (Buttons, Inputs, Selects, Switches, Sliders, Tabs, Tables, Modals/Dialogs, Drawers/Sheets, Popovers, Tooltips, Toasts, Context Menus, Virtual Lists).

- Drop down to `**gpui-base**` or raw GPUI `div()` + `canvas()` only for bespoke media visuals (poster cards with gradient overlays, full-bleed blurred hero banners, seekbar chapter ticks, custom horizontal carousels).

- Optionally adopt `**gpui-shell**` later if you want Seanime-style **UI plugins** (JS scripts that describe native GPUI elements with no DOM!).

### Seanime → `gpui-kit` Component Mapping

| Seanime UI Element                          | `gpui-kit` / GPUI Implementation                                                                                                                                                                                                 |
| ------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **App Shell (`AppLayout`)**                 | Root `div().flex().size_full().bg(theme.background)` containing `Sidebar` (left) + `MainColumn` (`Titlebar` + active screen + optional bottom `PlaybackHud`).                                                                    |
| **Collapsible Hover Sidebar**               | Custom `Entity<Sidebar>` with fixed `64px` collapsed rail that animates to `260px` on `on_hover` when `expand_sidebar_on_hover` is enabled. Uses `gpui` bounds tracking to fold overflowing items into a `"More"` `PopoverMenu`. |
| **Live Sidebar Badges**                     | Small absolute-positioned `Badge` pills driven by `cx.observe(&torrent_store)` / `cx.observe(&library_store)` / `cx.observe(&autodownloader_store)`.                                                                             |
| **Modular Home Screen (15 Widgets)**        | Vertical `ScrollArea` rendering a `Vec<HomeWidgetConfig>` stored in SQLite; reordered/configured via a `Dialog` (`HomeSettingsModal`).                                                                                           |
| **Poster Grids (Home / Lists / Search)**    | `**uniform_list**` / `gpui-component` Virtual List grouping cards into responsive rows calculated from container width (`available_width / (card_width + gap)`), guaranteeing 120 FPS even with 2,000+ anime.                    |
| **Horizontal Carousels**                    | Custom `ScrollHandle`-backed flex row with left/right overlay arrows, mouse-drag scrolling, and `on_scroll_wheel` translating `Alt + Wheel` into horizontal delta.                                                               |
| **Media Entry Card (2:3)**                  | `RenderOnce` component using `img(url).object_fit(ObjectFit::Cover)` + bottom linear gradient overlay + top-left progress/unwatched badge + top-right score badge + `HoverCard` / `ContextMenu`.                                 |
| **Episode Card (16:9)**                     | `RenderOnce` component with 16:9 thumbnail, optional spoiler blur/mask (`hide_anime_spoilers`), bottom continuity progress bar, filler badge, and click-to-play / info `Dialog`.                                                 |
| **Anime Entry Hero Banner**                 | Relative container (`height: 360px` Large / `220px` Small) with background `img` + gradient scrim + optional blur filter implementing all 7 Seanime `mediaPageBannerType` modes.                                                 |
| **SeaCommand (`Cmd+J` / `Q`)**              | Centered `Dialog` with focused `TextInput` + filtered `List` (`Picker` pattern). Slash prefix (`/search`, `/anime`, `/magnet`, `/spoilers`, `/logs`) swaps the active result delegate dynamically.                               |
| **Global Magnet Paste**                     | Window-level paste action handler checking `cx.read_from_clipboard()` when no text input has focus; if it starts with `magnet:?`, opens `SeaCommand` directly on `/magnet`.                                                      |
| **Torrent Search / Auto-Downloader Tables** | `gpui-component` `Table` (virtualized columns: Title, Group, Res, Codec, Size, Seeds, Date, Actions) with sortable headers and filter bar.                                                                                       |
| **Settings (15 Tabs)**                      | Split view: left `VerticalMenu` of 15 tabs + right scrollable settings form + sticky bottom "Unsaved Changes / Save" bar that triggers live module refresh.                                                                      |
| **Manga Reader**                            | Dedicated full-window route with `List` (Long Strip vertical scroll) or single/double-page `img` view with zoom transform, keyboard page turns (`Left`/`Right`/`Space`), and floating auto-hide HUD.                             |
| **Toasts & Error Explainer**                | `gpui_component::Root` notification stack for non-blocking toasts + structured `Dialog` for actionable error diagnostics.                                                                                                        |

### Translating Seanime's Theme Tokens to `gpui-component`

Map Seanime's exact defaults (`#070707` background, `#0B0B0B` paper, `#0D0D0D` elevated, `rgba(255,255,255,0.10)` border, `#6152DF` brand accent, `8px` radius) into `gpui_component::theme::ThemeColor` inside `crates/ui_theme`:

- Expose a live `Entity<ThemeStore>` holding all 38 `ThemeSettings` fields (colors, sidebar transparency, expand-on-hover, banner modes, card options, default sorts, and all 5 spoiler protection toggles).

- Whenever a toggle changes in `Settings → UI`, `ThemeStore` updates `Theme::global_mut(cx)` and calls `cx.refresh_windows()` so every open view re-renders on the very next frame with zero restart.

## 5. State Management, Routing & Focus Architecture

### Navigation & Scroll Restoration

```rust
#[derive(Clone, Debug, PartialEq)]
pub enum Route {
    Home,
    AnimeEntry { media_id: u32, initial_tab: EntryTab },
    Discover,
    Search(SearchFilters),
    MyLists { status: Option<ListStatus> },
    Schedule,
    MangaHome,
    MangaEntry { media_id: u32 },
    MangaReader { media_id: u32, chapter_id: String },
    Torrents,
    Debrid,
    AutoDownloader,
    ScanSummaries,
    Extensions,
    OfflineSync,
    Settings { tab: SettingsTab },
}
```

- Maintain a `RouterState` `Entity` with `back_stack: Vec<HistoryEntry>`, `forward_stack: Vec<HistoryEntry>`, and a `HashMap<RouteKey, ScrollOffset>` so hitting Back from an `AnimeEntry` restores your exact scroll position on `Search` or `MyLists` (just like Seanime Denshi).

- Wire macOS/Windows/Linux back/forward buttons and the sidebar's hover Back/Forward icon directly to `RouterState`.

### Entity Ownership & Reactivity Rules

1. **Store Entities live in `AppState` (Global)**:

   - `library: Entity<LibraryStore>`

   - `playback: Entity<PlaybackStore>`

   - `torrents: Entity<TorrentStore>`

   - `providers: Entity<ProviderStore>`

   - `theme: Entity<ThemeStore>`

   - `router: Entity<RouterState>`

2. **Views subscribe via `cx.observe(...)` / `cx.subscribe(...)`**: Never poll inside `render()`. Background Tokio tasks send `AppEvent`s over a bounded `flume` or `async_channel` to one GPUI foreground consumer task that updates the relevant `Entity` and calls `cx.notify()`.

3. **Never drop active `Task`s**: Store background GPUI `Task<()>` handles in the owning `Entity` (e.g., `_search_task: Option<Task<()>>`). Assigning a new search task automatically cancels the previous in-flight search!

### Actions & Keybindings (`actions!` Macro)

Define strongly typed GPUI actions scoped by focus context (`Workspace`, `Player`, `MangaReader`, `CommandPalette`):

- Global (`Workspace`): `OpenCommandPalette` (`cmd-j` / `q`), `QuickSearch` (`s`), `NavigateBack` (`cmd-[`), `NavigateForward` (`cmd-]`), `ScanLibrary` (`cmd-shift-s`), `ToggleSidebar` (`cmd-b`), `OpenSettings` (`cmd-,`).

- Active Playback (`PlaybackHud` / `Player`): `TogglePlayPause` (`space`), `SeekForward` (`right`), `SeekBackward` (`left`), `SkipIntro` (`enter` when in OP/ED chapter), `NextEpisode` (`shift-n`), `PreviousEpisode` (`shift-p`).

- Manga Reader (`MangaReader`): `NextPage`, `PrevPage`, `ZoomIn`, `ZoomOut`, `ToggleReadingMode`.

## 6. Performance, Image Caching & Async Guardrails

1. **Zero I/O on the UI Thread**: Every HTTP request, SQLite query, torrent operation, and filesystem scan runs on Tokio via `gpui-tokio-bridge` or `cx.background_executor()`.

2. **Two-Tier Poster & Metadata Cache**:

   - **L1 (RAM)**: GPUI's `ImageCache` (bounded) + `lru::LruCache` for parsed metadata.

   - **L2 (Disk)**: MessagePack (`rmp-serde`) cached metadata + downloaded poster/thumbnail files in OS cache dir (`dirs::cache_dir()`) so cold restarts render the Home screen instantaneously.

3. **Throttle High-Frequency Backend Events**:

   - Torrent speeds (`librqbit` stats) and MPV playback timestamps (`time-pos`) update at most **2–4 Hz** (every 250–500 ms) before notifying GPUI entities—preventing unnecessary 120 FPS re-layouts while idle.

4. **Detached Player & Sidecar Safety**:

   - Spawn `mpv` / `IINA` in detached OS sessions (`setsid()` on Unix, `DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP` on Windows, following `MovieBox-TUI`), redirect stderr to a temp log file, and use a **fresh IPC socket path per launch** (following Seanime) so rapid episode switching never collides.

## 7. Phased Build Roadmap

| Phase       | Milestone                                     | Concrete Deliverables                                                                                                                                                                                                                                                                           |
| ----------- | --------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| **Phase 0** | **Workspace & Shell Skeleton**                | Cargo workspace with hexagonal crate split, `gpui-kit` + `gpui-tokio-bridge` bootstrap, custom `40px` titlebar, `#070707` / `#6152df` theme applied to `gpui-component`, collapsible hover Sidebar with `"More"` overflow, and `RouterState`.                                                   |
| **Phase 1** | **Catalog, Home & Command Palette**           | AniList GraphQL client + SQLite/MessagePack cache, virtualized `MediaEntryCard` grid + horizontal carousels, modular `HomeScreen`, `Discover`, `Search` (with scroll restore), `MyLists`, and `SeaCommand` (`Cmd+J`/`Q`/`S`).                                                                   |
| **Phase 2** | **Anime Entry & Local Playback**              | Full `AnimeEntry` page (7 banner modes, episode cards, spoiler toggles, characters/relations), local library scanner + fuzzy filename matcher (`Habari`-style) + MKV inspection, and **external `mpv` launcher with async JSON IPC** (resume, 80–90% auto-sync to AniList, OP/ED chapter skip). |
| **Phase 3** | **Torrent Streaming (`librqbit`) & Indexers** | Embedded `librqbit` HTTP `Range` streamer with head/tail piece priority, Nyaa/mirror/proxy search drawer, global magnet-paste intercept in `SeaCommand`, Auto-Select scoring profiles, and live Torrent Manager screen.                                                                         |
| **Phase 4** | **Debrid, Stremio Add-ons & `StreamRelay`**   | Real-Debrid/Torbox/AllDebrid/Premiumize integration, stateless Stremio HTTP manifest add-ons, and `127.0.0.1` `StreamRelay` loopback proxy for header/cookie forwarding and DASH/HLS prefetching.                                                                                               |
| **Phase 5** | **JS Extensions, Auto-Downloader & Manga**    | `rquickjs` sandboxed extension runtime + Marketplace UI, Auto-Downloader rules/profiles/queue, Airing Schedule + Missing Episodes, and Manga Hub + Reader.                                                                                                                                      |
| **Phase 6** | **Polish, Customization & In-App Video R&D**  | Full 15-tab Settings live-reload, 38-toggle UI studio, Offline Mode (`Platform Abstraction Layer`), Playlists, Discord RPC, and optional embedded `libmpv2` / GPU surface player exploration.                                                                                                   |

Whenever you're ready to start building, tell me if you want to **scaffold Phase 0** (the Cargo workspace, `gpui-kit` window shell, Seanime theme tokens, collapsible sidebar, and router) or dive deeper into any specific subsystem first.
