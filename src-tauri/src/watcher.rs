//! Filesystem watching via `notify`. Emits Tauri events so the tree and the
//! open preview stay live while the user edits in Neovim in another pane. Also
//! watches the active theme CSS for hot-reload.

use crate::is_markdown;
use notify::{EventKind, RecursiveMode, Watcher};
use serde::Serialize;
use std::path::PathBuf;
use tauri::{AppHandle, Emitter};

#[derive(Clone, Serialize)]
struct PathPayload {
    path: String,
}

/// Start watching in a background thread that owns the watcher for its lifetime.
pub fn spawn(app: AppHandle, repo_root: PathBuf, theme_path: Option<PathBuf>) {
    std::thread::spawn(move || {
        let (tx, rx) = std::sync::mpsc::channel();
        let mut watcher = match notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        }) {
            Ok(w) => w,
            Err(e) => {
                eprintln!("dreamd: failed to start file watcher: {e}");
                return;
            }
        };

        if let Err(e) = watcher.watch(&repo_root, RecursiveMode::Recursive) {
            eprintln!("dreamd: failed to watch {}: {e}", repo_root.display());
        }
        if let Some(tp) = &theme_path {
            let _ = watcher.watch(tp, RecursiveMode::NonRecursive);
        }

        for event in rx.into_iter().flatten() {
            for path in &event.paths {
                // Theme hot-reload.
                if theme_path.as_deref() == Some(path.as_path()) {
                    let _ = app.emit("theme-reloaded", ());
                    continue;
                }
                if !is_markdown(path) {
                    continue;
                }
                let payload = PathPayload {
                    path: path.to_string_lossy().into_owned(),
                };
                let name = match event.kind {
                    EventKind::Create(_) => "file-added",
                    EventKind::Remove(_) => "file-removed",
                    EventKind::Modify(_) => "file-changed",
                    _ => continue,
                };
                let _ = app.emit(name, payload);
            }
        }
    });
}
