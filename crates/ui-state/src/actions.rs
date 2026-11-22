//! Workspace actions and their key bindings. See ARCHITECTURE §5.

use gpui_kit::{App, KeyBinding, actions};

/// The key context of the main window's shell.
pub const WORKSPACE: &str = "Workspace";

actions!(
    workspace,
    [
        /// Returns to the previous route.
        GoBack,
        /// Redoes the last [`GoBack`].
        GoForward
    ]
);

/// Back and forward: `cmd-[` / `cmd-]` on macOS, `ctrl` elsewhere.
#[cfg(target_os = "macos")]
const NAVIGATION_KEYS: (&str, &str) = ("cmd-[", "cmd-]");
#[cfg(not(target_os = "macos"))]
const NAVIGATION_KEYS: (&str, &str) = ("ctrl-[", "ctrl-]");

/// Registers the workspace key bindings. Call it once at boot.
pub fn bind_keys(cx: &mut App) {
    cx.bind_keys([
        KeyBinding::new(NAVIGATION_KEYS.0, GoBack, Some(WORKSPACE)),
        KeyBinding::new(NAVIGATION_KEYS.1, GoForward, Some(WORKSPACE)),
    ]);
}
