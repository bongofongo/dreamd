//! The native macOS menubar.
//!
//! dreamd inherited Tauri's default menu until packaging forced the issue: a
//! `.app` launched from Finder gets no argv, so there has to be a way to open a
//! repo from inside the window. Tauri's default File menu is only "Close
//! Window", and there is no way to add one item to it — supplying a menu at all
//! replaces the whole bar, so the default is rebuilt here (mirroring
//! `tauri::menu::Menu::default`) with two items added.
//!
//! The accelerators are `Cmd+O` / `Cmd+Shift+O`. Note that this does *not*
//! collide with the `Ctrl+O` bound to `toggle_stack`: `matchCombo` in
//! `ui/app.js` requires exact modifier equality including `metaKey`, so a
//! Cmd-chord never reaches a Ctrl-chord binding. O means Open in the menubar
//! and nothing else changed in the webview.

use tauri::menu::{AboutMetadata, Menu, MenuItem, PredefinedMenuItem, Submenu};
use tauri::{AppHandle, Runtime};

/// Menu ids the builder's `on_menu_event` matches on.
pub const OPEN_FOLDER: &str = "open_folder";
pub const OPEN_FILE: &str = "open_file";

/// Tauri's default macOS menu, plus File -> Open.
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
