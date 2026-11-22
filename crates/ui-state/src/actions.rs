//! Workspace actions and their key bindings. See ARCHITECTURE §5.

use gpui_kit::{App, KeyBinding, actions};

/// The key context of the main window's shell.
pub const WORKSPACE: &str = "Workspace";

/// The key context of the command palette.
pub const COMMAND_PALETTE: &str = "CommandPalette";

/// The shell's context while no text input has focus, so single-letter
/// shortcuts never eat typed text.
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

/// Back and forward: `cmd-[` / `cmd-]` on macOS, `ctrl` elsewhere.
#[cfg(target_os = "macos")]
const NAVIGATION_KEYS: (&str, &str) = ("cmd-[", "cmd-]");
#[cfg(not(target_os = "macos"))]
const NAVIGATION_KEYS: (&str, &str) = ("ctrl-[", "ctrl-]");

/// The command palette: `cmd-j` on macOS, `ctrl-j` elsewhere.
#[cfg(target_os = "macos")]
const PALETTE_KEY: &str = "cmd-j";
#[cfg(not(target_os = "macos"))]
const PALETTE_KEY: &str = "ctrl-j";

/// Registers the workspace key bindings. Call it once at boot.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(NAVIGATION_KEYS.0, GoBack, Some(WORKSPACE)),
        KeyBinding::new(NAVIGATION_KEYS.1, GoForward, Some(WORKSPACE)),
        KeyBinding::new(PALETTE_KEY, OpenCommandPalette, Some(WORKSPACE)),
        KeyBinding::new("q", OpenCommandPalette, Some(WORKSPACE_NOT_TYPING)),
        KeyBinding::new("s", OpenSearch, Some(WORKSPACE_NOT_TYPING)),
    ]);
}
