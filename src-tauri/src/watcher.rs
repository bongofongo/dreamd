//! Filesystem watching via `notify`. Emits Tauri events so the tree and the
//! open preview stay live while the user edits in Neovim in another pane. Also
//! watches the active theme CSS for hot-reload.

use crate::is_markdown;
use notify::{EventKind, RecursiveMode, Watcher};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::Duration;
use tauri::{AppHandle, Emitter};

/// How long to keep absorbing events before emitting. macOS FSEvents reports a
/// single editor `:w` as several events — measured at 1.58 per save, each of
/// which used to cost a full re-render — and a `git checkout` reports one per
/// changed file, each of which costs two full repo walks plus a sidebar
/// rebuild. Coalescing within this window collapses both.
const DEBOUNCE: Duration = Duration::from_millis(60);

#[derive(Clone, Serialize)]
struct PathPayload {
    path: String,
}

/// Everything seen for one path within a window. Kinds are accumulated rather
/// than overwritten because they are not mutually exclusive: an editor that
/// saves by writing a temp file and renaming it over the original produces a
/// remove *and* a create for a file that very much still exists, and picking
/// either one alone would be wrong.
#[derive(Default)]
struct Acc {
    created: bool,
    modified: bool,
    removed: bool,
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

        pump(&app, &rx, theme_path.as_deref());
    });
}

/// Drain the watcher channel, coalescing bursts, until the channel closes.
fn pump(
    app: &AppHandle,
    rx: &Receiver<notify::Result<notify::Event>>,
    theme_path: Option<&Path>,
) {
    let mut pending: HashMap<PathBuf, Acc> = HashMap::new();

    while let Ok(first) = rx.recv() {
        let mut theme_touched = false;
        absorb(first, theme_path, &mut pending, &mut theme_touched);

        // Keep absorbing while events are still arriving inside the window.
        let disconnected = loop {
            match rx.recv_timeout(DEBOUNCE) {
                Ok(ev) => absorb(ev, theme_path, &mut pending, &mut theme_touched),
                Err(RecvTimeoutError::Timeout) => break false,
                Err(RecvTimeoutError::Disconnected) => break true,
            }
        };

        if theme_touched {
            let _ = app.emit("theme-reloaded", ());
        }
        for (path, acc) in pending.drain() {
            emit(app, &path, &acc);
        }

        if disconnected {
            return;
        }
    }
}

/// Turn one window's worth of accumulated kinds into the smallest set of
/// frontend events that still describes what happened. The filesystem is the
/// tiebreaker: whether the path exists *now* is more reliable than the order
/// FSEvents happened to report things in.
fn emit(app: &AppHandle, path: &Path, acc: &Acc) {
    let payload = PathPayload {
        path: path.to_string_lossy().into_owned(),
    };

    if !path.exists() {
        let _ = app.emit("file-removed", payload);
        return;
    }
    // The tree and the search index only learn about new files from this.
    if acc.created {
        let _ = app.emit("file-added", payload.clone());
    }
    // A remove on a path that still exists is a rename-style save, so the open
    // document has to repaint just as it would for a plain write.
    if acc.modified || acc.removed {
        let _ = app.emit("file-changed", payload);
    }
}

fn absorb(
    event: notify::Result<notify::Event>,
    theme_path: Option<&Path>,
    pending: &mut HashMap<PathBuf, Acc>,
    theme_touched: &mut bool,
) {
    let Ok(event) = event else { return };
    for path in event.paths {
        if theme_path == Some(path.as_path()) {
            *theme_touched = true;
            continue;
        }
        if !is_markdown(&path) {
            continue;
        }
        // Matched before the insert so unhandled kinds (access, etc.) don't
        // create an entry that would then be resolved against the filesystem.
        let field: fn(&mut Acc) = match event.kind {
            EventKind::Create(_) => |a| a.created = true,
            EventKind::Remove(_) => |a| a.removed = true,
            EventKind::Modify(_) => |a| a.modified = true,
            _ => continue,
        };
        field(pending.entry(path).or_default());
    }
}
