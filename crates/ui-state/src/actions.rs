//! Workspace actions and their key bindings. See ARCHITECTURE §5.

use gpui_kit::{App, KeyBinding, actions};

/// The key context of the main window's shell.
pub const WORKSPACE: &str = "Workspace";

/// The key context of the command palette.
pub const COMMAND_PALETTE: &str = "CommandPalette";

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
    #[cfg(any(target_os = "windows", target_os = "linux"))]
    cx.bind_keys([
        KeyBinding::new(ALT_NAVIGATION_KEYS.0, GoBack, Some(WORKSPACE)),
        KeyBinding::new(ALT_NAVIGATION_KEYS.1, GoForward, Some(WORKSPACE)),
    ]);
}
