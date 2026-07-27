//! The native menubar.
//!
//! dreamd inherited Tauri's default menu until packaging forced the issue: a
//! `.app` launched from Finder gets no argv, so there has to be a way to open a
//! repo from inside the window. Tauri's default File menu is only "Close
//! Window", and there is no way to add one item to it — supplying a menu at all
//! replaces the whole bar, so the default is rebuilt here (mirroring
//! `tauri::menu::Menu::default`) with two items added.
//!
//! On macOS the accelerators are `Cmd+O` / `Cmd+Shift+O`. That does *not*
//! collide with the `Ctrl+O` bound to `toggle_stack`: `matchCombo` in
//! `ui/app.js` requires exact modifier equality including `metaKey`, so a
//! Cmd-chord never reaches a Ctrl-chord binding. O means Open in the menubar and
//! nothing else changed in the webview.
//!
//! On Linux the same `CmdOrCtrl+O` string would resolve to `Ctrl+O` and collide
//! head-on, so the Linux menu uses `Ctrl+Shift+O` / `Ctrl+Alt+O` instead. See
//! `build`'s Linux arm for why that is a silent breakage rather than a
//! double-fire.
//!
//! ## Why the shape differs by platform
//!
//! muda exposes every `PredefinedMenuItem` constructor on every platform, so the
//! *API* is portable — but its GTK backend then drops most of them. The
//! `is_item_supported!` macro in muda's `platform_impl/gtk` admits only
//! `Separator`, `Cut`, `Copy`, `Paste`, `SelectAll`, `About` and custom items;
//! `Undo`, `Redo`, `Minimize`, `Maximize`, `Fullscreen`, `Hide`, `HideOthers`,
//! `ShowAll`, `Services`, `CloseWindow` and `Quit` are silently skipped rather
//! than rendered dead. Handing GTK the macOS menu would therefore produce a
//! "dreamd" submenu with nothing in it and a Window submenu with nothing in it.
//!
//! So Linux gets what GTK can actually draw: File, Edit, Help. Window management
//! is the window manager's job there anyway, and Quit is the titlebar's.
//!
//! The other GTK asymmetry is which items claim a key. Cut/Copy/Paste/SelectAll
//! only `set_accel` on their label — cosmetic, so `Ctrl+C` still reaches the
//! webview — while **custom items register a real accelerator** on the window's
//! accel group. That is why the two Open items may not use `CmdOrCtrl+O` here.

use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Runtime};

/// Menu ids the builder's `on_menu_event` matches on.
pub const OPEN_FOLDER: &str = "open_folder";
pub const OPEN_FILE: &str = "open_file";

/// Tauri's default macOS menu, plus File -> Open.
#[cfg(target_os = "macos")]
pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let pkg = app.package_info();
    let about = AboutMetadata {
        name: Some(pkg.name.clone()),
        version: Some(pkg.version.to_string()),
        ..Default::default()
    };

    let app_menu = Submenu::with_items(
        app,
        &pkg.name,
        true,
        &[
            &PredefinedMenuItem::about(app, None, Some(about))?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::services(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, None)?,
            &PredefinedMenuItem::hide_others(app, None)?,
            &PredefinedMenuItem::show_all(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;

    let file_menu = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &MenuItem::with_id(app, OPEN_FOLDER, "Open Folder…", true, Some("CmdOrCtrl+O"))?,
            &MenuItem::with_id(
                app,
                OPEN_FILE,
                "Open File…",
                true,
                Some("CmdOrCtrl+Shift+O"),
            )?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;

    let edit_menu = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;

    let view_menu = Submenu::with_items(
        app,
        "View",
        true,
        &[&PredefinedMenuItem::fullscreen(app, None)?],
    )?;

    let window_menu = Submenu::with_items(
        app,
        "Window",
        true,
        &[
            &PredefinedMenuItem::minimize(app, None)?,
            &PredefinedMenuItem::maximize(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::close_window(app, None)?,
        ],
    )?;

    Menu::with_items(
        app,
        &[&app_menu, &file_menu, &edit_menu, &view_menu, &window_menu],
    )
}

/// The GTK menubar: only what muda's GTK backend will actually draw.
///
/// The accelerators deliberately differ from macOS's. `CmdOrCtrl+O` resolves to
/// `Ctrl+O` here, which is `keymap.toggle_stack` — and a registered GTK
/// accelerator is consumed before the webview sees the key, so the collision
/// would not double-fire, it would make `Ctrl+O` stop toggling the stack. Shift
/// and Alt variants are unclaimed by the default keymap and stay clear of it.
#[cfg(not(target_os = "macos"))]
pub fn build<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let pkg = app.package_info();
    let about = AboutMetadata {
        name: Some(pkg.name.clone()),
        version: Some(pkg.version.to_string()),
        ..Default::default()
    };

    let file_menu = Submenu::with_items(
        app,
        "File",
        true,
        &[
            &MenuItem::with_id(app, OPEN_FOLDER, "Open Folder…", true, Some("Ctrl+Shift+O"))?,
            &MenuItem::with_id(app, OPEN_FILE, "Open File…", true, Some("Ctrl+Alt+O"))?,
        ],
    )?;

    // Undo/Redo are omitted, not forgotten: GTK skips them, so listing them
    // would render an Edit menu shorter than this one reads.
    let edit_menu = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;

    let help_menu = Submenu::with_items(
        app,
        "Help",
        true,
        &[&PredefinedMenuItem::about(app, None, Some(about))?],
    )?;

    Menu::with_items(app, &[&file_menu, &edit_menu, &help_menu])
}
