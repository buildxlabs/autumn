//! Workspace actions and their key bindings. See ARCHITECTURE §5.

use gpui_kit::{App, KeyBinding, actions};

/// The key context of the main window's shell.
pub const WORKSPACE: &str = "Workspace";

/// The key context of the command palette.
pub const COMMAND_PALETTE: &str = "CommandPalette";

/// The key context of the playback HUD while it has focus.
pub const PLAYER: &str = "Player";

/// The shell's context while no text input has focus, so single-letter
/// shortcuts never eat typed text. Must name [`WORKSPACE`].
const WORKSPACE_NOT_TYPING: &str = "Workspace && !Input";

actions!(
    workspace,
    [
        /// Returns to the previous route.
        GoBack,
        /// Redoes the last [`GoBack`].
        GoForward,
        /// Opens the command palette with an empty query.
        OpenCommandPalette,
        /// Opens the command palette in `/search` mode.
        OpenSearch
    ]
);

actions!(
    player,
    [
        /// Plays or pauses the active session.
        PlayPause,
        /// Seeks ten seconds back.
        SeekBack,
        /// Seeks ten seconds forward.
        SeekForward,
        /// Stops the active session.
        StopPlayback,
        /// Skips the opening or ending chapter currently playing.
        SkipChapter
    ]
);

#[cfg(debug_assertions)]
actions!(
    workspace,
    [
        /// Sends a slow loopback request to exercise the loader.
        DebugSlowFetch,
        /// Triggers a loopback 429 to exercise the countdown.
        DebugRateLimited,
        /// Triggers a loopback 403 to exercise section recovery.
        DebugAniListUnavailable
    ]
);

/// Back and forward: `cmd-[` / `cmd-]` on macOS, `ctrl-[` / `ctrl-]`
/// elsewhere. Windows and Linux also bind `alt-left` / `alt-right`.
#[cfg(target_os = "macos")]
const NAVIGATION_KEYS: (&str, &str) = ("cmd-[", "cmd-]");
#[cfg(not(target_os = "macos"))]
const NAVIGATION_KEYS: (&str, &str) = ("ctrl-[", "ctrl-]");

/// The command palette: `cmd-j` on macOS, `ctrl-j` elsewhere.
#[cfg(target_os = "macos")]
const PALETTE_KEY: &str = "cmd-j";
#[cfg(not(target_os = "macos"))]
const PALETTE_KEY: &str = "ctrl-j";

/// Arrow-key chords to walk the same history as [`NAVIGATION_KEYS`] on
/// Windows and Linux, where `ctrl-[` is awkward on many keyboard layouts.
#[cfg(any(target_os = "windows", target_os = "linux"))]
const ALT_NAVIGATION_KEYS: (&str, &str) = ("alt-left", "alt-right");

/// Registers the workspace key bindings. Call it once at boot.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(NAVIGATION_KEYS.0, GoBack, Some(WORKSPACE)),
        KeyBinding::new(NAVIGATION_KEYS.1, GoForward, Some(WORKSPACE)),
        KeyBinding::new(PALETTE_KEY, OpenCommandPalette, Some(WORKSPACE)),
        KeyBinding::new("q", OpenCommandPalette, Some(WORKSPACE_NOT_TYPING)),
        KeyBinding::new("s", OpenSearch, Some(WORKSPACE_NOT_TYPING)),
    ]);
    cx.bind_keys([
        KeyBinding::new("space", PlayPause, Some(PLAYER)),
        KeyBinding::new("left", SeekBack, Some(PLAYER)),
        KeyBinding::new("right", SeekForward, Some(PLAYER)),
        KeyBinding::new("enter", SkipChapter, Some(PLAYER)),
        KeyBinding::new("escape", StopPlayback, Some(PLAYER)),
    ]);
    #[cfg(debug_assertions)]
    cx.bind_keys([
        KeyBinding::new("f8", DebugSlowFetch, Some(WORKSPACE)),
        KeyBinding::new("f9", DebugRateLimited, Some(WORKSPACE)),
        KeyBinding::new("f10", DebugAniListUnavailable, Some(WORKSPACE)),
    ]);
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    cx.bind_keys([
        KeyBinding::new(ALT_NAVIGATION_KEYS.0, GoBack, Some(WORKSPACE)),
        KeyBinding::new(ALT_NAVIGATION_KEYS.1, GoForward, Some(WORKSPACE)),
    ]);
}
