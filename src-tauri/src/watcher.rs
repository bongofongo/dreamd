//! Filesystem watching via `notify`. Emits Tauri events so the tree and the
//! open preview stay live while the user edits in Neovim in another pane. Also
//! watches the active theme CSS for hot-reload.

use crate::{is_markdown, theme};
use notify::{EventKind, RecursiveMode, Watcher};
use serde::Serialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
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

/// How long the pump waits before rechecking `cancel` while nothing is
/// happening. Only reached on an idle repo, and only costs an atomic load.
const CANCEL_POLL: Duration = Duration::from_millis(500);

/// Start watching in a background thread that owns the watcher for its lifetime.
///
/// Setting `cancel` retires the thread: File -> Open moves the tree root, and a
/// watcher left running on the previous root would keep emitting `file-changed`
/// for files the window is no longer showing.
pub fn spawn(
    app: AppHandle,
    repo_root: PathBuf,
    theme_path: Option<PathBuf>,
    cancel: Arc<AtomicBool>,
) {
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
        let themes = ThemeWatch::new(theme_path);
        if let Some(tp) = &themes.file {
            let _ = watcher.watch(tp, RecursiveMode::NonRecursive);
        }
        // Best-effort: the directory only exists once the user has saved a
        // theme. Bundled palettes live in the binary and cannot be watched at
        // all — editing `ui/themes/*.css` needs a rebuild.
        let _ = watcher.watch(&themes.dir, RecursiveMode::NonRecursive);

        pump(&app, &rx, &themes, &cancel);
    });
}

/// The theme paths worth watching: an explicit `theme_css` file, and the user
/// themes directory. Paths are canonicalized because macOS FSEvents reports
/// resolved paths, so a theme under `/tmp` (really `/private/tmp`) or behind a
/// symlinked home would never compare equal otherwise.
struct ThemeWatch {
    file: Option<PathBuf>,
    dir: PathBuf,
}

impl ThemeWatch {
    fn new(theme_path: Option<PathBuf>) -> Self {
        let dir = theme::user_dir();
        Self {
            file: theme_path.map(|p| p.canonicalize().unwrap_or(p)),
            dir: dir.canonicalize().unwrap_or(dir),
        }
    }

    /// Whether a changed path restyles the app: the watched stylesheet itself,
    /// or any `.css` in the user themes directory. The directory is matched
    /// wholesale rather than against the *active* palette, so saving a theme
    /// you are not currently using still refreshes the settings panel's list —
    /// and re-resolving is a file read, not a re-render.
    fn matches(&self, path: &Path) -> bool {
        if self.file.as_deref() == Some(path) {
            return true;
        }
        path.parent() == Some(&self.dir) && path.extension().is_some_and(|e| e == "css")
    }
}

/// Drain the watcher channel, coalescing bursts, until the channel closes or
/// this watcher is retired.
fn pump(
    app: &AppHandle,
    rx: &Receiver<notify::Result<notify::Event>>,
    themes: &ThemeWatch,
    cancel: &AtomicBool,
) {
    let mut pending: HashMap<PathBuf, Acc> = HashMap::new();

    loop {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        // A timeout rather than a bare `recv` so an idle watcher still notices
        // it has been retired.
        let first = match rx.recv_timeout(CANCEL_POLL) {
            Ok(ev) => ev,
            Err(RecvTimeoutError::Timeout) => continue,
            Err(RecvTimeoutError::Disconnected) => return,
        };
        let mut theme_touched = false;
        absorb(first, themes, &mut pending, &mut theme_touched);

        // Keep absorbing while events are still arriving inside the window.
        let disconnected = loop {
            match rx.recv_timeout(DEBOUNCE) {
                Ok(ev) => absorb(ev, themes, &mut pending, &mut theme_touched),
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
    themes: &ThemeWatch,
    pending: &mut HashMap<PathBuf, Acc>,
    theme_touched: &mut bool,
) {
    let Ok(event) = event else { return };
    for path in event.paths {
        // Checked before the markdown filter, which is what lets a `.css`
        // inside the repo root be seen at all.
        if themes.matches(&path) {
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
