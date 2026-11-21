# What gpui-kit 0.10 gives the Phase 0 shell

Answers issue #9. Researched 2026-11-21 against the published crate sources
resolved by `gpui-kit = "=0.10.0"`: `gpui-kit 0.10.0`, `gpui-component 0.10.0`,
`gpui-base 0.10.0`, `gpui-pre 0.3.12` and its platform crates
(`gpui-pre-macos`, `gpui-pre-windows`, `gpui-pre-linux`, all 0.3.12).

Paths below are relative to `~/.cargo/registry/src/index.crates.io-*/`.
**Verified** means read in that source. **Inferred** means reasoned from the
source but not run.

## 1. `open_window` and the titlebar

`gpui_kit::open_window(options, cx, build)` is a thin wrapper: it calls GPUI's
`cx.open_window` and wraps the built view in `gpui_base::Root`
(`gpui-kit-0.10.0/src/lib.rs`, `open_window`). It returns
`(AnyWindowHandle, Entity<V>)`. All window options are GPUI's `WindowOptions`.
Verified.

| Need                     | API                                                                                                                                                       | Platforms                                                    | Status   |
| ------------------------ | --------------------------------------------------------------------------------------------------------------------------------------------------------- | ------------------------------------------------------------ | -------- |
| Transparent titlebar     | `TitlebarOptions { appears_transparent: true, .. }` (`gpui-pre-0.3.12/src/platform.rs:2825`)                                                              | macOS and Windows. Linux uses `window_decorations` instead.  | Verified |
| Traffic light position   | `TitlebarOptions::traffic_light_position: Option<Point<Pixels>>`, plus `Window::set_traffic_light_position` at runtime (`src/window.rs:3285`, macOS only) | macOS                                                        | Verified |
| App-owned titlebar drag  | `WindowOptions::app_owns_titlebar_drag` (`platform.rs:2636`), then `Window::start_window_move()` (`window.rs:3237`)                                       | macOS flag. `start_window_move` works on macOS, X11, Wayland | Verified |
| Drag / min / max / close | `div().window_control_area(WindowControlArea::Drag \| Min \| Max \| Close)` (`window.rs:752`)                                                             | Windows only (maps to `HTCAPTION` etc.)                      | Verified |
| Client-side decorations  | `WindowOptions::window_decorations: Option<WindowDecorations>` (`platform.rs:2677`), `Window::window_decorations() -> Decorations`                        | X11, Wayland                                                 | Verified |
| CSD helpers              | `start_window_resize(ResizeEdge)`, `show_window_menu(pos)`, `set_client_inset(px)`, `window_controls()` (`window.rs:2721`, `3229`, `3242`, `3273`)        | X11, Wayland                                                 | Verified |
| Background transparency  | `WindowOptions::window_background: WindowBackgroundAppearance::{Opaque, Transparent, Blurred}` (`platform.rs:2899`)                                       | all, "Blurred not always supported"                          | Verified |

Platform details, all verified:

- `WindowControlArea` hit testing is implemented only on Windows
  (`gpui-pre-windows-0.3.12/src/events.rs:979`). macOS, X11 and Wayland
  implement `on_hit_test_window_control` as a no-op
  (`gpui-pre-macos-0.3.12/src/window.rs:2083`,
  `gpui-pre-linux-0.3.12/src/linux/x11/window.rs:1740`,
  `.../wayland/window.rs:2013`). Dragging there needs a mouse-move handler
  that calls `window.start_window_move()`.
- Wayland starts with `WindowDecorations::Client` and switches to server mode
  only when the compositor's `xdg-decoration` says so
  (`gpui-pre-linux-0.3.12/src/linux/wayland/window.rs:626`, `:1184`). GNOME
  has no `xdg-decoration`, so it stays client-side.
- X11 falls back to server-side decorations when no compositor is running
  (`x11/window.rs:1921`).

**`gpui_component::TitleBar` already does all of this.** Verified in
`gpui-component-0.10.0/src/title_bar.rs`:

- `TitleBar::window_options()` returns `appears_transparent: true`,
  `traffic_light_position: (9, 9)`, `app_owns_titlebar_drag: true` (`:61-93`).
- Its mouse handlers call `window.start_window_move()` on drag (`:404`), set
  `WindowControlArea::Drag` for Windows (`:416`), zoom on double-click on Linux
  and call `titlebar_double_click()` on macOS (`:362-365`), and open the window
  menu on right-click under CSD (`:429`).
- It draws min/max/close only when needed: on Linux only under
  `Decorations::Client`, and only the controls the compositor supports
  (`:272-299`).
- `Root` wraps every window in `window_border()`, which draws the CSD shadow
  and resize edges on Linux (`root.rs:750-758`, `window_border.rs:236`).

**A 40px bar:** `TITLE_BAR_HEIGHT` is 34px (`title_bar.rs:18`), but the
element applies the caller's style after its own `.h(...)`
(`.refine_style(&self.style)`, `:360`), so `TitleBar::new().h(px(40.))`
overrides it. Verified in source, not run. On macOS, `move_traffic_light` sizes
the titlebar container to `button_height + 2 * y` and puts the buttons at `x, y`
(`gpui-pre-macos-0.3.12/src/window.rs:710-760`). Centering in a 40px bar
needs `y = (40 - button_height) / 2`, about `13` for the standard 14px
buttons. Inferred: check it on screen. The macOS left padding of 80px
(`title_bar.rs:20`) leaves room for the lights.

## 2. `gpui-component` widgets from ARCHITECTURE §5

Each module listed below exists in `gpui-component-0.10.0/src/lib.rs`.
Verified.

| Widget         | Exists | API and limits                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                                   |
| -------------- | ------ | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `sidebar`      | yes    | `Sidebar::new(id).collapsible(SidebarCollapsible::{Icon, Offcanvas, None}).collapsed(bool)`, with `SidebarGroup` / `SidebarMenu` / `SidebarMenuItem` (`.icon`, `.active`, `.suffix(..)` for badges, `.context_menu`). Width changes are animated. **No hover-expand**: no `on_hover` in `sidebar/` and the collapsed state is a builder argument. **Collapsed width is fixed at 48px** (`sidebar/mod.rs:29`, applied after the caller's style at `:445-447`), so a 64px rail can't be set by style. Expanded width defaults to 255px and can be set with `.w(...)` (`:441-445`). |
| `command`      | yes    | `Command::new(&Entity<CommandState>)` with `.items` / `.group` / `.separator`, `.on_query`, `.on_confirm`, `.filterable(false)` for async results, `CommandState::set_loading`. Rows use `v_virtual_list` (`command/state.rs:1043`). It has **no delegate trait**: items are passed on every render, so "swap the delegate" becomes "swap the item set". It is a widget, not a dialog: wrap it in `open_dialog`.                                                                                                                                                                 |
| `virtual_list` | yes    | `v_virtual_list` / `h_virtual_list(view, id, item_sizes: Rc<Vec<Size<Pixels>>>, render_range)` from `gpui-base` (`gpui-base-0.10.0/src/virtual_list.rs:94-131`). One axis, with a known size for each item. **There is no virtual grid.** A grid is a `v_virtual_list` of rows, each row holding `cols = floor(width / card_w)` posters. GPUI's `uniform_list` (same-height rows) works too. `grid_layout` is a draggable dashboard grid, not virtualized.                                                                                                                       |
| `tab`          | yes    | `TabBar::new(id).children(Tab..).selected_index(i).on_click(..)`. Styles: pill, outline, segmented, underline, folder. `.menu(true)` adds an overflow menu, `.track_scroll(&ScrollHandle)` makes the bar scrollable (`tab/tab_bar.rs`).                                                                                                                                                                                                                                                                                                                                          |
| `dialog`       | yes    | `window.open_dialog(cx, \|dialog, window, cx\| ..)`, `open_alert_dialog`, `close_dialog`, `close_all_dialogs` (`window_ext.rs:75-115`). Dialogs stack (`active_dialogs: Vec`, `root.rs:28`). The builder can run again, so keep it cheap.                                                                                                                                                                                                                                                                                                                                        |
| `sheet`        | yes    | `window.open_sheet(..)` opens on the right, `open_sheet_at(Placement, ..)` elsewhere. `Sheet::size`, `.resizable`, `.overlay_closable`. **Only one sheet at a time** (`active_sheet: Option`, `root.rs:27`).                                                                                                                                                                                                                                                                                                                                                                     |
| `notification` | yes    | `window.push_notification(Notification::success(..).title(..).autohide(..).action(..))`. Keyed with `.id::<T>()` / `.id1::<T>(key)` so a new one replaces the old. `.placement(Anchor)`, and `.system()` / `.in_app_and_system()` for OS notifications (`notification.rs:236-321`).                                                                                                                                                                                                                                                                                              |
| `hover_card`   | yes    | `HoverCard::new(id).trigger(..).content(\|..\| ..).open_delay(..).close_delay(..).anchor(..)` (`hover_card.rs`).                                                                                                                                                                                                                                                                                                                                                                                                                                                                 |
| `menu`         | yes    | `PopupMenu` (`.menu`, `.submenu`, `.separator`, `.scrollable`, `.max_h`), `ContextMenuExt::context_menu`, `DropdownMenu::dropdown_menu`, `AppMenuBar` (`menu/mod.rs`). Enough for the sidebar's "More" menu.                                                                                                                                                                                                                                                                                                                                                                     |
| `skeleton`     | yes    | `Skeleton::new()` (`.secondary()`), styled like any element, with a pulse animation (`skeleton.rs`). A separate `shimmer` module exists too.                                                                                                                                                                                                                                                                                                                                                                                                                                     |
| `carousel`     | yes    | `Carousel::new(id, &Entity<CarouselState>)` with `CarouselContent` / `CarouselItem` / `CarouselPrevious` / `CarouselNext` / `CarouselPagination`. `CarouselState::new(n).with_looping(..).with_axis(..)`. Arrow, Home and End keys are bound. **No autoplay**: the strings `autoplay` and `interval` are absent from `carousel/`. Drive `select_next` from a timer task.                                                                                                                                                                                                         |

**Overlays need no layer calls.** `gpui_component::init` registers a
`gpui_base::Root` plugin (`root.rs:21-23`, `:722`) that renders dialogs,
sheets, notifications and tooltips. Because `gpui_kit::open_window` installs
`gpui_base::Root` (re-exported as `gpui_component::Root`), the `WindowExt`
calls work in any window it opens. Verified.

## 3. `img()` with remote URLs

Verified in `gpui-pre-0.3.12/src/elements/img.rs` and `src/app.rs`:

- **The HTTP loader is GPUI's, but no client is installed.** For a URL
  source, `ImageAssetLoader` calls `cx.http_client().get(uri, .., true)`
  (`img.rs:623-640`). The default client is `NullHttpClient`, which fails
  every request with `"No HttpClient available"` (`app.rs:246`,
  `:3606-3620`). gpui-kit doesn't install one on desktop: `gpui-kit-assets`
  uses reqwest only on wasm (`gpui-kit-assets-0.10.0/Cargo.toml:105`). The
  upstream guide says the same: "On native platforms the default `HttpClient`
  fails every request"
  ([website/docs/image.md](https://github.com/longbridge/gpui-kit/blob/main/website/docs/image.md)).
- **Our client can be shared.** Install it with
  `Application::with_http_client(Arc<dyn HttpClient>)` (`app.rs:282`) or
  `cx.set_http_client(..)` (`app.rs:2090`). The trait is
  `gpui_kit::http_client::HttpClient` (re-exported from `gpui-pre`, `src/gpui.rs:6`),
  with `send(http::Request<AsyncBody>) -> BoxFuture<Result<Response<AsyncBody>>>`
  plus `user_agent()` and `proxy()`
  (`gpui-pre-http-client-0.3.12/src/http_client.rs:118`). The upstream guide
  has a complete adapter from a `reqwest` client (with `http-cache-reqwest`
  disk caching) that runs `request.send()` on a Tokio runtime and reads the
  `RedirectPolicy` request extension
  ([image.md, "Cache remote images over HTTP"](https://github.com/longbridge/gpui-kit/blob/main/website/docs/image.md)).
  The guide is on `main`, but every API it uses exists in 0.3.12. Verified.
  `gpui-pre-reqwest-client` (=0.3.12) is a ready-made client, but it brings
  Zed's reqwest fork and a Tokio dependency of its own
  (crates.io dependencies API).
- **Cache: in memory only, kept until removed.** Without an `image_cache`,
  `img()` goes through `window.use_asset`, and the result is kept in
  `App::loading_assets` "until `remove_asset`" (`app.rs:3124-3150`). Decoded
  frames are never evicted. A bounded cache is opt-in: wrap a subtree in
  `image_cache(provider)` with your own `ImageCache` impl (`elements/image_cache.rs:14`, `:197`). The
  built-in `retain_all` provider never evicts either. There is no disk cache
  and no HTTP freshness handling.
- **Animated GIF and WebP: yes.** Formats are detected from the bytes.
  `ImageFormat::Gif` uses `decode_gif_image`, `WebP` uses `decode_webp_image`,
  and an animated WebP decodes every frame (`img.rs:665-669`,
  `platform.rs:3423-3431`). `img()` advances frames by each frame's delay
  unless `cx.reduce_motion()` (`img.rs:312-327`). Every frame is decoded into
  memory up front. Inferred from `decode_animated_frames`; it matters for
  large GIFs.
- Other sources: `Resource::Path` (a file on disk), `ImageSource::Image(Arc<Image>)`
  (bytes we already hold), `Custom(..)` loader (`img.rs:36-47`).
  `.with_loading(..)` and `.with_fallback(..)` handle placeholders and errors.

## 4. Saving and restoring scroll position per route

Yes, verified:

- `gpui::ScrollHandle` is a cloneable `Rc<RefCell<..>>` with
  `offset() -> Point<Pixels>`, `set_offset(Point<Pixels>)`,
  `logical_scroll_top() -> (usize, Pixels)` and `scroll_to_item(ix)`
  (`gpui-pre-0.3.12/src/elements/div.rs:4310-4467`). Use it with
  `div().overflow_y_scroll().track_scroll(&handle)`.
- `VirtualListScrollHandle` derefs to `ScrollHandle` and adds
  `scroll_to_item(ix, ScrollStrategy)` (`gpui-base-0.10.0/src/virtual_list.rs:40-123`).
  `UniformListScrollHandle` has `logical_scroll_top_index()` and
  `scroll_to_item_strict_with_offset` (`elements/uniform_list.rs:80-222`).
- Two strategies: (a) keep the route view, and so its handle, alive in the
  router's back stack, and position is preserved for free. (b) Store
  `handle.offset()` (or the logical top item and offset, which survives a
  changed column count) per route key and call `set_offset` /
  `scroll_to_item` after the view mounts. For (b), `set_offset` before the
  first layout writes the raw offset, and layout clamps it later. Inferred:
  check with a test.
- `gpui-base` also ships `NavStack` / `NavStackState`, a view stack with
  `push`, `pop`, `forward`, `replace`, `pop_to_root`, push/pop transitions
  and a forward stack (`gpui-base-0.10.0/src/nav_stack.rs:81-248`, exported in
  `lib.rs:149`). It holds `AnyView`s, so popped views keep their scroll state.
  It could be the `RouterState` store, or serve as a model for it.

## 5. Mouse back and forward buttons

Yes, verified. `MouseButton::Navigate(NavigationDirection::{Back, Forward})`
(`gpui-pre-0.3.12/src/interactive.rs:453-483`) is produced by each desktop
backend:

- macOS: `buttonNumber` 3/4, and horizontal swipe events from mice such as the
  MX Master (`gpui-pre-macos-0.3.12/src/events.rs:157-240`).
- Windows: `XBUTTON1` / `XBUTTON2` (`gpui-pre-windows-0.3.12/src/events.rs:545`).
- X11: buttons 8/9 (`gpui-pre-linux-0.3.12/src/linux/x11/event.rs:29`).
- Wayland: `BTN_BACK` / `BTN_SIDE` and their forward equivalents
  (`.../wayland/client.rs:2388`).

Handle them with
`.on_mouse_down(MouseButton::Navigate(NavigationDirection::Back), ..)` on the
shell root (`elements/div.rs:126`).

## 6. `test-support`

Verified in `gpui-kit-0.10.0/src/test.rs`, `TESTING.md` and `tests/integration/`:

- Enable `gpui-kit = { features = ["test-support"] }` in
  `[dev-dependencies]`. `#[gpui_kit::test]` is GPUI's test macro. It takes
  `cx: &mut TestAppContext`, and the test can be sync or `async`
  (`tests/integration/test_macro.rs`). Don't glob-import `gpui_kit::*` in
  test modules: GPUI's `test` attribute would shadow `#[test]`.
- **Headless windows:** tests call the production `gpui_kit::open_window`
  under GPUI's test platform with fixed bounds (`tests/common/mod.rs`).
  GPUI also offers `cx.add_window`, `add_window_view` and `add_empty_window`
  (`gpui-pre-0.3.12/src/app/test_context.rs:284-352`).
- **Input:** `TestWindowExt` provides `find` / `try_find` / `find_all` /
  `within(scope)` (snapshots of bounds, visibility, a11y label and value,
  focus) and `click`, `click_at`, `click_with_options` (any `MouseButton`,
  including `Navigate`, through `ClickOptions::with_button`, `test.rs:111`),
  `right_click`, `double_click`, `hover`, `scroll(ScrollDelta)`, `drag`,
  `drag_to`, `press("cmd-k")`, `input("text")`. Elements must opt in with
  `.test_support()` (inert in production builds). `TestAppContextExt::wait_for`
  waits for async UI. GPUI's own `simulate_keystrokes`, `simulate_input` and
  `simulate_event` also work.
- Limits stated in `TESTING.md`: it checks no pixels, models no OS IME, and
  doesn't automate a packaged app.
- **Tokio in tests:** the test executor is deterministic and
  `run_until_parked` panics when tasks are still pending unless
  `cx.executor().allow_parking()` is called (`executor.rs:230-245`). Awaiting a
  real Tokio `JoinHandle` through `bridge::run` therefore needs
  `allow_parking()` and a live runtime, or a fake engine. Inferred.

## 7. Newer gpui-kit and Tokio integration

- **No newer release.** The crates.io API on 2026-11-21 lists `gpui-kit`
  `max_version` 0.10.0 (published 2026-11-16). `gpui-component` is also at
  0.10.0. The GitHub repo's latest release is `v0.10.0`
  (`gh api repos/longbridge/gpui-kit/releases`). Upstream ships about one
  minor version a week (0.9.0 on 11-02, 0.9.1 on 11-09, 0.10.0 on 11-16).
- `gpui-pre` 0.3.13 exists (2026-11-16), but gpui-kit 0.10 pins `=0.3.12`
  (`gpui-kit-0.10.0/Cargo.toml:270-272`), and so does `main`'s workspace
  `Cargo.toml`.
- **No Tokio integration.** No source in `gpui-kit`, `gpui-base`,
  `gpui-component` or `gpui-pre` 0.3.12/0.3.13 depends on Tokio (the only hits
  are comments). The crates `gpui-pre-tokio`, `gpui-kit-tokio` and
  `gpui-tokio` don't exist on crates.io. Upstream's own image guide creates a
  private Tokio runtime and says "reqwest needs a tokio runtime; GPUI's
  executors are not one". So ARCHITECTURE §4's own bridge is still needed.
- `main` adds unpublished crates (`gpui-shell`, `gpui-webview`, `gpui-fps`)
  in the workspace `Cargo.toml`. They aren't on crates.io and aren't relevant
  to Phase 0.

## Implications for Phase 0

1. **Titlebar:** build on `TitleBar` with `TitleBar::window_options()`,
   override the height with `.h(px(40.))`, and set `traffic_light_position` to
   about `(px(12.), px(13.))`. Check the y value on screen. This covers drag,
   double-click and CSD controls on all three OSes. Don't hand-roll
   `window_control_area`, which is a no-op off Windows.
2. **Sidebar:** the stock `Sidebar` can't give a 64px rail (fixed 48px
   collapsed) and doesn't expand on hover. Either accept 48px and toggle
   `collapsed` from a hover handler on a wrapper `div`, or build the rail from
   `SidebarMenuItem`-like parts. Decide in the shell ticket, and update
   ARCHITECTURE §5 if we take 48px.
3. **Command palette:** `Command` inside `open_dialog`. Slash modes swap the
   item set and use `filterable(false)` with `on_query` and
   `set_loading` for async search. Change "result delegate" in ARCHITECTURE
   §5 to match.
4. **Poster grid:** wrap `v_virtual_list` (or `uniform_list`) rows,
   computing the column count from container width. No grid widget comes for free.
5. **Images:** install one `HttpClient` adapter at boot that forwards to our
   shared `reqwest` client on the bridge's Tokio `Handle`, adding a disk HTTP
   cache. Without it, every remote poster fails. Add a bounded `ImageCache`
   around poster grids, because the default cache never evicts.
6. **Router:** keep route views alive in the back stack, or adopt
   `gpui_base::NavStack`, and save `ScrollHandle::offset()` per route key as a
   fallback. Bind `MouseButton::Navigate` on the shell root.
7. **Tests:** `#[gpui_kit::test]` plus `TestWindowExt` covers the Phase 0
   shell (click a sidebar item, assert the route, press `cmd-j`, send Navigate
   clicks). Tests that cross the Tokio bridge need `allow_parking()` or a fake
   engine.
8. **Versions:** stay on 0.10. Upstream releases weekly, so expect API churn
   and pin `=0.10.0` in `[workspace.dependencies]`.
