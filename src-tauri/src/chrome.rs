//! The window's own frame — the half of `apply_chrome` that is not the menubar.
//!
//! `ui.titlebar` is the window manager's bar: the row it stacks above the
//! window, carrying close / minimize / maximize. Off is a frameless window that
//! `#drag-strip` still moves.
//!
//! **On macOS this does nothing, and that is the point.** `tauri.conf.json`
//! asks for `titleBarStyle: "Overlay"` with `hiddenTitle`, so there is no bar
//! there — only the three traffic lights, floating over the page and costing no
//! vertical space. There is nothing to reclaim, so there is nothing to toggle,
//! and the settings panel hides the row (`WINDOW_TOGGLES`, `mac: false`) the way
//! it already hides the menubar's.
//!
//! Calling `set_decorations` on macOS anyway is the bug this module was written
//! for. tao rebuilds the style mask from scratch on every call — `Closable |
//! Miniaturizable | Resizable | Titled` for `true` — which drops the
//! `FullSizeContentView` and transparent-titlebar bits the window was *created*
//! with. So turning the titlebar off and back on did not restore the overlay: it
//! produced an opaque native bar sitting on top of a page laid out for no bar,
//! with no way back short of editing `config.toml` by hand. A `cfg` arm that
//! never makes the call is the only fix, because there is no second call that
//! puts those bits back.
//!
//! What macOS *does* have its own preference for is `ui.titlebar_fade`, and that
//! one never reaches this module: it is how dreamd paints its own bar, so it is
//! CSS (`body.chrome-fade` in `ui/index.html`) and the window knows nothing
//! about it.

/// Apply `ui.titlebar` to the live window. Called from `.setup()`, from
/// `set_config` so the settings toggle is instant, and from `adopt_root` with
/// the config it just re-read. Idempotent, and a failure to reach the window is
/// silently nothing — the preference is chrome, and losing it must not cost the
/// caller.
#[cfg(not(target_os = "macos"))]
pub fn set_titlebar(win: &tauri::WebviewWindow, on: bool) {
    // Supported on every non-macOS platform, applied immediately, and Tauri
    // does its own hop to the main thread.
    let _ = win.set_decorations(on);
}

/// See the module docs: on macOS there is no bar to toggle, and the call that
/// looks like it would toggle one destroys the overlay titlebar instead.
#[cfg(target_os = "macos")]
pub fn set_titlebar(_win: &tauri::WebviewWindow, _on: bool) {}
