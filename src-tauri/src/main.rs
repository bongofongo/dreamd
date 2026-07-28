#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! dreamd — lightweight GUI markdown reader with a highlight/annotation ->
//! agent loop. Launched from the CLI: `dreamd [path]`.
//!
//! This file is deliberately thin: CLI, `AppState`, the Tauri commands, and the
//! builder. The work those commands do lives in the `dreamd` library crate
//! (`src/lib.rs`) so benches and tests can drive it without a window.

use clap::Parser;
use dreamd::annotations::{Highlight, Id, Origin, Pair, Store};
use dreamd::catalog::Catalog;
use dreamd::config::{Config, Keymap};
use dreamd::fs_walk::FileNode;
use dreamd::send::SendResult;
use dreamd::{
    cli, config, guard, home_relative, markdown, marks_file, mcp, notify, perf, pty, read_source,
    rootfield, send, theme, watcher,
};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tauri::{Emitter, Manager, State};

/// How long a mutation waits before it reaches disk.
///
/// Saves are coalesced off the command thread because serialising the whole
/// store inside `set_annotation` would put a file write on a UI-latency path —
/// the one path `perf/` measures as `save_to_paint`. Half a second is short
/// enough that the only way to lose a mark is `kill -9`, and long enough that
/// dragging a selection across a paragraph writes once rather than per chip.
const SAVE_DEBOUNCE: Duration = Duration::from_millis(500);

struct AppState {
    /// Behind a lock because File -> Open can move it: a `.app` launched from
    /// Finder starts with no repo at all, and picking one is how it gets one.
    /// `RwLock` rather than `Mutex` because every command reads it and only the
    /// menu handler writes.
    repo_root: RwLock<PathBuf>,
    /// Whether `repo_root` means anything yet. False only when dreamd was
    /// launched with no path and the walk-up found no `.git` — the Finder
    /// double-click case, where the resolved "root" would be `/`. While false
    /// nothing walks and the window stays hidden; File -> Open flips it.
    has_repo: AtomicBool,
    /// Retires the watcher thread for the previous root when File -> Open moves
    /// it. `None` until something is being watched.
    watcher_cancel: Mutex<Option<Arc<AtomicBool>>>,
    /// The same, for the MCP socket thread. Separate from `watcher_cancel`
    /// because the two are armed under different conditions — a repo with no
    /// `.git` still gets neither, but a secondary dreamd gets a watcher and no
    /// socket.
    mcp_cancel: Mutex<Option<Arc<AtomicBool>>>,
    /// A file to open on load (nvim-style `dreamd file.md`), if any.
    initial_file: Option<String>,
    /// Behind a lock because the settings panel rewrites it at runtime — the
    /// only mutable-at-runtime configuration dreamd has.
    config: Mutex<Config>,
    /// `Arc` because the MCP socket thread holds the same store the commands
    /// do — that sharing is the entire point of the design, and it is the
    /// `catalog: Arc<Catalog>` precedent below. Command bodies are unchanged:
    /// `Arc<Mutex<T>>` derefs.
    store: Arc<Mutex<Store>>,
    /// The tree and the search index, from one walk behind one readiness gate.
    /// Caching them is what stops a cold start walking the repo twice — once
    /// for the index, once for the frontend's first `loadTree`. On a
    /// single-file launch the walk runs on a background thread and this is
    /// what the commands wait on. `Arc` because those waits happen off the
    /// command thread, in `spawn_blocking`.
    catalog: Arc<Catalog>,
    /// The appearance currently on screen, with `system` already resolved.
    ///
    /// An atomic rather than another `Mutex` so `syntax_theme` — which runs on
    /// every render, holding the config lock — never has to reason about lock
    /// order. Written by `.setup()` at startup and by `set_appearance` when the
    /// frontend notices the OS change.
    appearance: AtomicU8,
    /// Whether this process is the one that writes this repo's marks file.
    ///
    /// False for a repo with no `.git` (there is nothing to be the marks *of*)
    /// and for a **secondary** — a second dreamd on a repo another one already
    /// claimed. Two writers would be last-write-wins on a whole file, so the
    /// second window keeps its marks in memory and says so on stderr rather
    /// than clobbering the first's. `adopt_root` re-decides it, because the
    /// claim is per repo.
    persists: AtomicBool,
    /// Set by every mutation, cleared by the save thread. An atomic rather than
    /// a channel because the interesting question is only ever "has anything
    /// changed since the last write?", and coalescing is the entire point.
    dirty: Arc<AtomicBool>,
    /// Files whose loaded marks have not been checked against this run's bytes
    /// yet.
    ///
    /// Seeded from the marks file at startup and re-seeded by `adopt_root`;
    /// `get_highlights` drains it one path at a time. This exists so the
    /// steady state costs **nothing**: `Store::ensure_reanchored` is idempotent
    /// but takes a `&str` of source, so calling it unconditionally would mean
    /// re-reading the open document on every repaint. A mark created this
    /// session was anchored against these bytes and is never in here.
    pending_reanchor: Mutex<HashSet<String>>,
    /// The embedded Claude Code pane's pty, or `None` while the pane has never
    /// been opened — which is the overwhelmingly common case, and why this is
    /// an `Option` created on first open rather than at boot. One per window:
    /// the pane is a single dock, not a tab strip.
    pty: Mutex<Option<pty::Pty>>,
}

impl AppState {
    /// The current tree root. Cloned out rather than handed a guard, so no
    /// command holds the lock across a walk or a render.
    fn root(&self) -> PathBuf {
        self.repo_root.read().unwrap().clone()
    }

    fn scheme(&self) -> theme::Scheme {
        match self.appearance.load(Ordering::Relaxed) {
            0 => theme::Scheme::Light,
            _ => theme::Scheme::Dark,
        }
    }

    fn set_scheme(&self, scheme: theme::Scheme) {
        let v = match scheme {
            theme::Scheme::Light => 0,
            theme::Scheme::Dark => 1,
        };
        self.appearance.store(v, Ordering::Relaxed);
    }

    /// Note that the store changed, so the next debounce tick writes it.
    ///
    /// Called by the mutating commands and — through
    /// [`marking_dirty`] — by the MCP layer's notifier, which already fires on
    /// exactly the calls that changed something.
    fn touch(&self) {
        self.dirty.store(true, Ordering::Relaxed);
    }

    /// The syntect theme the active palette asks for, or syntect's default.
    fn syntax_theme(&self) -> String {
        let scheme = self.scheme();
        theme::resolve(&self.config.lock().unwrap(), scheme)
            .syntax_theme
            .unwrap_or_else(|| markdown::CODE_THEME.to_string())
    }
}

#[derive(clap::Parser)]
#[command(name = "dreamd", version, about = "Lightweight GUI markdown reader")]
#[command(args_conflicts_with_subcommands = true)]
struct Cli {
    /// A markdown file to open, or a directory to browse. Like nvim: passing a
    /// file opens it, but the file tree is still rooted at the repo of the
    /// current directory. Defaults to the current directory.
    path: Option<PathBuf>,

    /// Use this theme for one run, without saving it. See `dreamd theme list`.
    #[arg(long)]
    theme: Option<String>,

    /// Use this appearance for one run, without saving it.
    #[arg(long, value_parser = parse_mode)]
    mode: Option<config::Mode>,

    /// Run the full pre-window startup sequence, then exit without opening a
    /// window. Lets hyperfine measure the Rust half of cold start on its own.
    /// Only emits timings when built with `--features perf`.
    ///
    /// On a *file* argument the walk is deferred to a background thread, so
    /// there is nothing pre-window to measure and this exits without either
    /// walking or spawning — timing a teardown that races a live thread would
    /// measure nothing meaningful.
    #[arg(long, hide = true)]
    bench_startup: bool,

    /// A directory literally named `theme` or `config` is shadowed by these;
    /// pass it as `./theme` to open it instead.
    #[command(subcommand)]
    command: Option<cli::Cmd>,
}

/// `--mode` values, spelled the same way `config.toml` spells them.
fn parse_mode(raw: &str) -> Result<config::Mode, String> {
    match raw {
        "system" => Ok(config::Mode::System),
        "light" => Ok(config::Mode::Light),
        "dark" => Ok(config::Mode::Dark),
        other => Err(format!("expected system, light or dark, got {other:?}")),
    }
}

// ---- persistence ---------------------------------------------------------

/// Every distinct file the loaded marks name, as the set `get_highlights`
/// drains.
fn files_with_marks(store: &Store) -> HashSet<String> {
    store
        .parts()
        .0
        .iter()
        .map(|h| h.file_path.clone())
        .collect()
}

/// Wrap a notifier so an agent's mutation dirties the store on its way to the
/// window.
///
/// The MCP layer is the one mutator that never touches a Tauri command, so
/// nothing in here would otherwise know it happened. `notify` already fires on
/// exactly the calls that changed something — a read tool and a refused write
/// emit nothing, and `mcp/server.rs` has tests holding that line — which makes
/// it the right signal to hang the dirty flag off, rather than a second filter
/// that could disagree with it.
fn marking_dirty(dirty: Arc<AtomicBool>, inner: notify::Notifier) -> notify::Notifier {
    Arc::new(move |change| {
        dirty.store(true, Ordering::Relaxed);
        inner(change);
    })
}

/// Write the marks if anything changed. The debounce tick, the quit hook and
/// `adopt_root` all land here.
///
/// **Lock order is store, then `config` / `repo_root` / `pending_reanchor`**,
/// and `adopt_root` takes them in the same order for the same reason: those two
/// are the only places holding more than one, and inverting them here would
/// deadlock a File -> Open against a save.
///
/// Every decision except the first is made *under* the store lock, and that is
/// load-bearing rather than tidy. `adopt_root` mutates `persists`, `repo_root`
/// and the store together beneath that lock, so a tick that read them outside
/// it could pass the gate as a primary on repo Y, block on the lock, and wake
/// up to write repo X's file — as a secondary that had just stood down. The
/// unlocked `dirty` read is only a hint, and the safe direction: it can miss a
/// mutation that lands microseconds later, which the next tick catches.
fn flush_marks(app: &tauri::AppHandle) {
    let state = app.state::<AppState>();
    if !state.dirty.load(Ordering::Relaxed) {
        return;
    }
    let store = state.store.lock().unwrap();
    if !state.persists.load(Ordering::Relaxed) || !state.dirty.swap(false, Ordering::Relaxed) {
        return;
    }
    let root = state.root();
    if let Err(e) = marks_file::save(&root, &store) {
        eprintln!("dreamd: cannot save marks for {}: {e}", root.display());
        // Put the flag back: a full disk that clears itself should not have
        // cost the session's marks.
        state.dirty.store(true, Ordering::Relaxed);
    }
}

// ---- commands ------------------------------------------------------------

/// The three commands below are `async` and do their waiting inside
/// `spawn_blocking` because on a single-file launch the catalog may not exist
/// yet: blocking a synchronous command would block whatever thread Tauri runs
/// it on, and the frontend can't tell a late-resolving promise from a slow one
/// anyway.
#[tauri::command]
async fn list_markdown_files(state: State<'_, AppState>) -> Result<FileNode, String> {
    let catalog = state.catalog.clone();
    tauri::async_runtime::spawn_blocking(move || catalog.wait_tree())
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn repo_info(state: State<AppState>) -> serde_json::Value {
    let root = state.root();
    serde_json::json!({
        "root": root.to_string_lossy(),
        "name": root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        "display": home_relative(&root),
        // False means "nothing opened yet" — the frontend shows the empty state
        // rather than a tree it is about to find empty.
        "hasRepo": state.has_repo.load(Ordering::Relaxed),
        // False means this window is a secondary: its marks are real for as
        // long as it runs and are never written. Exposed so the frontend can
        // say so; nothing in `ui/` reads it yet.
        "persists": state.persists.load(Ordering::Relaxed),
    })
}

/// The `[ui]` section on its own.
///
/// Separate from `get_settings`, which lists every bundled palette and walks
/// the user themes directory, because this one is on the boot path: the tree
/// cannot lay itself out at the persisted width without it, and asking for the
/// whole settings payload to learn one integer would put that walk in front of
/// the first paint.
#[tauri::command]
fn get_ui(state: State<AppState>) -> config::Ui {
    state.config.lock().unwrap().ui.clone()
}

/// Directory *names* under what has been typed into the root field, for its
/// tab completion. See `rootfield` for what this surface may and may not do —
/// notably that it never returns a file name and never a path.
#[tauri::command]
fn complete_directories(path: String) -> Result<Vec<String>, String> {
    rootfield::complete(&path, dirs::home_dir().as_deref())
}

/// Point the app at a path typed into the root field. The heavy lifting is
/// `adopt_root`'s — config swap, re-walk, watcher re-arm, marks flush, MCP
/// socket retire and re-bind — and this only decides whether the line is a
/// path at all, so a typo leaves the reader in the root they were in.
#[tauri::command]
fn set_root(app: tauri::AppHandle, path: String) -> Result<(), String> {
    let target = rootfield::target(&path, dirs::home_dir().as_deref())?;
    adopt_root(&app, target);
    Ok(())
}

#[tauri::command]
fn initial_file(state: State<AppState>) -> Option<String> {
    state.initial_file.clone()
}

#[tauri::command]
fn render_markdown(state: State<AppState>, path: String) -> Result<String, String> {
    let source = read_source(&path)?;
    // The palette names the syntect theme for fenced code, so switching themes
    // has to re-render: the code colours are baked into the HTML, not CSS.
    let code_theme = state.syntax_theme();
    Ok(markdown::render_with(&source, &code_theme))
}

#[tauri::command]
async fn fuzzy_search(state: State<'_, AppState>, query: String) -> Result<Vec<FileNode>, String> {
    let catalog = state.catalog.clone();
    tauri::async_runtime::spawn_blocking(move || catalog.wait_query(&query))
        .await
        .map_err(|e| e.to_string())
}

/// Re-walk the repo after a file appears or disappears, and hand the fresh tree
/// straight back. The frontend used to follow this with `list_markdown_files`,
/// which walked the whole repo a second time for the same answer.
#[tauri::command]
async fn rebuild_index(state: State<'_, AppState>) -> Result<FileNode, String> {
    let catalog = state.catalog.clone();
    let repo_root = state.root();
    let ignores = state.config.lock().unwrap().extra_ignores.clone();
    tauri::async_runtime::spawn_blocking(move || catalog.rebuild(&repo_root, &ignores))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
fn add_highlight(
    state: State<AppState>,
    file_path: String,
    quote: String,
    prefix: String,
    suffix: String,
) -> Result<Id, String> {
    let source = read_source(&file_path)?;
    // Anchoring lives in `Store::add_anchored`, not here: `main.rs` cannot be
    // imported, so the (0, 0) fallback had no test, and the agent's
    // `mark_passage` needs the same behaviour.
    let id = state.store.lock().unwrap().add_anchored(
        file_path,
        &source,
        quote,
        prefix,
        suffix,
        Origin::Human,
    );
    state.touch();
    Ok(id)
}

#[tauri::command]
fn set_annotation(state: State<AppState>, id: String, text: String) -> bool {
    let changed = state.store.lock().unwrap().set_annotation(&id, text);
    if changed {
        state.touch();
    }
    changed
}

#[tauri::command]
fn remove_highlight(state: State<AppState>, id: String) {
    state.store.lock().unwrap().remove(&id);
    state.touch();
}

#[tauri::command]
fn remove_pair(state: State<AppState>, id: String) {
    state.store.lock().unwrap().remove_from_stack(&id);
    state.touch();
}

/// The overlay for one document — and the first thing `renderCurrent` calls
/// after `innerHTML`, which is why the lazy re-anchor hangs off it.
///
/// Marks that came off disk carry line numbers from whenever they were last
/// saved, and the file may have been edited by anything since. Re-anchoring
/// them all at startup would land straight on the cold-start number, so it is
/// paid per file, on first sight of that file, inside a path that was already
/// scanning the store for it. A repo where you open two documents pays for
/// two; one where you open two hundred pays for two hundred, which is the
/// right way round.
#[tauri::command]
fn get_highlights(state: State<AppState>, path: String) -> Vec<Highlight> {
    // Read the file *before* taking the store lock, and only when this path is
    // still owed a re-anchor. The guard is a `contains`, not a `remove`: a read
    // that fails — Neovim's atomic-rename save has the file unlinked for an
    // instant, and a `git checkout` for longer — must leave the debt standing,
    // or the marks render at the previous session's line numbers, `Active`, for
    // the rest of the run.
    let owed = state.pending_reanchor.lock().unwrap().contains(&path);
    let source = owed.then(|| read_source(&path).ok()).flatten();
    let mut store = state.store.lock().unwrap();
    if let Some(source) = source {
        store.ensure_reanchored(&path, &source);
        // Under the store lock, so the order is store -> pending_reanchor here
        // and in `adopt_root`. The `contains` above is the only place that
        // takes `pending_reanchor` alone, and it releases before the store lock
        // is taken; keep it that way.
        state.pending_reanchor.lock().unwrap().remove(&path);
    }
    store.for_file(&path)
}

#[tauri::command]
fn get_highlight(state: State<AppState>, id: String) -> Option<Highlight> {
    state.store.lock().unwrap().get(&id)
}

/// Re-read the file and re-anchor its highlights (Active/Stale). Called by the
/// frontend when a `file-changed` event arrives for the open document.
///
/// Deliberately **does not** dirty the store, and it is the only mutation that
/// doesn't. What it changes — line numbers and Active/Stale — is derived from
/// the file's current bytes, and `get_highlights` derives it again on the next
/// cold start from whatever the bytes are *then*. Marking it dirty would put a
/// marks write on every `:w` in Neovim, in the middle of the `save_to_paint`
/// loop, to persist an answer that is recomputed anyway.
#[tauri::command]
fn reanchor(state: State<AppState>, path: String) -> Result<Vec<Highlight>, String> {
    let source = read_source(&path)?;
    // This *is* the re-anchor the pending set exists to force, so nothing is
    // owed for this path any more.
    state.pending_reanchor.lock().unwrap().remove(&path);
    Ok(state.store.lock().unwrap().reanchor_file(&path, &source))
}

#[tauri::command]
fn get_stack(state: State<AppState>) -> Vec<Pair> {
    state.store.lock().unwrap().stack_pairs()
}

/// One-button send. `ids` empty/absent = send the whole stack.
///
/// The stack is cleared from what was *actually* sent, not from `ids` and not
/// wholesale: `selected_pairs` skips an id naming nothing and an unannotated
/// mark, and only a send that succeeded is a send at all. A failure leaves the
/// stack exactly as it was, which is the state the user can retry from.
///
/// The clipboard fallback counts as a send. It is still `Ok`, the questions
/// still left the app, and the alternative — a stack that looks unsent while the
/// text sits on the clipboard — is the one that gets asked twice.
#[tauri::command]
fn send_stack(state: State<AppState>, ids: Vec<Id>) -> Result<SendResult, String> {
    let pairs = {
        let store = state.store.lock().unwrap();
        if ids.is_empty() {
            store.stack_pairs()
        } else {
            store.selected_pairs(&ids)
        }
    };
    let config = state.config.lock().unwrap().clone();
    let result = send::send(&config, &state.root(), &pairs)?;
    let sent: Vec<Id> = pairs.iter().map(|p| p.highlight.id.clone()).collect();
    state.store.lock().unwrap().mark_sent(&sent);
    state.touch();
    Ok(result)
}

#[tauri::command]
fn get_keymap(state: State<AppState>) -> Keymap {
    state.config.lock().unwrap().keymap.clone()
}

/// The assembled stack query as markdown (for copy-to-clipboard).
#[tauri::command]
fn stack_query_text(state: State<AppState>) -> String {
    let pairs = state.store.lock().unwrap().stack_pairs();
    send::assemble_query(&state.root(), &pairs)
}

/// The stylesheet plus the appearance to render it in.
///
/// One command rather than a stylesheet call and a mode call: this runs on the
/// boot path, where every extra round trip is time the window spends in the
/// fallback colours rather than the user's theme.
#[derive(serde::Serialize)]
struct ThemeView {
    css: String,
    /// The *preference*, not the resolved scheme. The OS appearance can change
    /// while the app runs and only the frontend is placed to notice, so it gets
    /// told what to watch for rather than a single already-collapsed answer.
    mode: config::Mode,
    /// What `system` currently resolves to, so the panel can label the toggle.
    scheme: theme::Scheme,
    syntax_theme: Option<String>,
}

fn theme_view(state: &AppState) -> ThemeView {
    let cfg = state.config.lock().unwrap();
    let scheme = state.scheme();
    let resolved = theme::resolve(&cfg, scheme);
    ThemeView {
        css: resolved.css,
        mode: cfg.mode(),
        scheme,
        syntax_theme: resolved.syntax_theme,
    }
}

#[tauri::command]
fn get_theme(state: State<AppState>) -> ThemeView {
    theme_view(&state)
}

/// Adopt a new appearance, pushed by the frontend when the OS switches under a
/// `system` preference.
///
/// Returns the new view rather than being fire-and-forget: `render_markdown`
/// reads the scheme to pick the syntect theme, so a caller that re-rendered
/// before this landed would bake the old code colours into the new palette.
/// Awaiting the return value is what closes that window.
#[tauri::command]
fn set_appearance(
    app: tauri::AppHandle,
    state: State<AppState>,
    scheme: theme::Scheme,
) -> ThemeView {
    state.set_scheme(scheme);
    let view = theme_view(&state);
    // The native window background follows too, so an auto-switch doesn't leave
    // the frame in the old appearance.
    if let (Some(win), Some((r, g, b))) = (
        app.get_webview_window("main"),
        theme::background(&view.css, scheme),
    ) {
        let _ = win.set_background_color(Some(tauri::window::Color(r, g, b, 255)));
    }
    view
}

// ---- settings ------------------------------------------------------------

/// Everything the settings panel needs in one round trip: the effective
/// config, which theme is active, what else is on offer, and which keys a
/// repo-local `.dreamd.toml` would shadow if we saved them globally.
#[derive(serde::Serialize)]
struct Settings {
    config: Config,
    theme: Option<String>,
    /// What `config.mode` resolves to right now, so the toggle can say
    /// "System (dark)" instead of just "System".
    scheme: theme::Scheme,
    themes: Vec<theme::ThemeInfo>,
    syntax_themes: Vec<String>,
    config_path: String,
    themes_dir: String,
    local_overrides: Vec<String>,
}

#[tauri::command]
fn get_settings(state: State<AppState>) -> Settings {
    let config = state.config.lock().unwrap().clone();
    let scheme = state.scheme();
    Settings {
        theme: theme::resolve(&config, scheme).name,
        scheme,
        config,
        themes: theme::list(),
        syntax_themes: markdown::syntax_theme_names(),
        config_path: config::global_path().to_string_lossy().into_owned(),
        themes_dir: theme::user_dir().to_string_lossy().into_owned(),
        local_overrides: config::local_override_keys(&state.root()),
    }
}

/// Merge a partial config into the global file and adopt the result. The patch
/// is a TOML table in the shape of the config, e.g. `{keymap: {palette: "…"}}`;
/// anything that fails to deserialize is rejected before the file is written.
#[tauri::command]
fn set_config(
    app: tauri::AppHandle,
    state: State<AppState>,
    patch: toml::Table,
) -> Result<Settings, String> {
    config::patch_global(patch)?;
    let cfg = Config::load(&state.root());
    // What the OS says, as far as we still know it: the scheme in hand is the
    // last one the frontend pushed, which under `system` *is* the OS's answer.
    let system = state.scheme();
    let (mode, scheme) = (cfg.mode(), theme::scheme_for(&cfg, system));
    *state.config.lock().unwrap() = cfg;
    state.set_scheme(scheme);
    if let Some(win) = app.get_webview_window("main") {
        pin_native_theme(&win, native_pin(mode, scheme, system));
    }
    Ok(get_settings(state))
}

/// Pin the *native* appearance, so the traffic lights, scrollbars and the
/// webview's own `prefers-color-scheme` agree with the palette. `None` hands
/// control back to the OS.
///
/// On macOS tao implements this as `NSApplication.appearance` — it is app-wide
/// rather than per-window, and while it is pinned `Window::theme()` reports the
/// pin rather than the system value. Never read the OS appearance back through
/// it without un-pinning first.
/// Whether the native appearance needs pinning, and to what.
///
/// `None` — let the OS drive — is right only when we are actually showing what
/// the OS asked for. A legacy theme name pins a scheme while `mode` is still
/// `System` (see `theme::scheme_for`), and that case has to pin natively too or
/// the scrollbars disagree with the palette.
fn native_pin(
    mode: config::Mode,
    scheme: theme::Scheme,
    system: theme::Scheme,
) -> Option<theme::Scheme> {
    (mode != config::Mode::System || scheme != system).then_some(scheme)
}

fn pin_native_theme(win: &tauri::WebviewWindow, pinned: Option<theme::Scheme>) {
    let _ = win.set_theme(pinned.map(|s| match s {
        theme::Scheme::Light => tauri::Theme::Light,
        theme::Scheme::Dark => tauri::Theme::Dark,
    }));
}

/// The built-in keybinds, so "reset shortcuts" in the panel writes the same
/// values `Keymap::default()` would rather than a copy that can drift.
#[tauri::command]
fn default_keymap() -> Keymap {
    Keymap::default()
}

#[tauri::command]
fn list_themes() -> Vec<theme::ThemeInfo> {
    theme::list()
}

/// A theme's stylesheet without saving anything — this is what makes hovering
/// the theme list a live preview.
#[tauri::command]
fn theme_css(name: String) -> Result<String, String> {
    theme::css_for(&name).ok_or_else(|| format!("no theme named {name:?}"))
}

/// A palette's own file, without the base rules — what the Custom tab edits.
///
/// The panel used to recover this by regexing the first `:root` block back out
/// of `theme_css`, which finds the shared block of a family and silently drops
/// its mode blocks. It also had to dodge the `:root { --bg: … }` example inside
/// `ui/theme.css`'s own header comment. Asking for the file is both correct and
/// less code.
#[tauri::command]
fn palette_css(name: String) -> Result<String, String> {
    theme::palette(&name).ok_or_else(|| format!("no theme named {name:?}"))
}

#[tauri::command]
fn save_theme(name: String, css: String) -> Result<String, String> {
    theme::save_user(&name, &css).map(|p| p.to_string_lossy().into_owned())
}

#[tauri::command]
fn delete_theme(name: String) -> Result<(), String> {
    theme::delete_user(&name)
}

#[tauri::command]
fn copy_to_clipboard(text: String) -> Result<(), String> {
    send::copy_clipboard(&text)
}

/// Open the OS print dialog over the rendered document — the "export to PDF"
/// path, via the dialog's own Save-as-PDF destination.
///
/// This exists as a command rather than a bare `window.print()` in `app.js`
/// because `window.print()` **does nothing at all in WKWebView**. WebKit routes
/// it to the UI delegate's `_webView:printFrame:`, and wry's `WKUIDelegate`
/// implements only the file-upload, media-permission and new-window callbacks —
/// so the JS call returns having silently printed nothing, on the one platform
/// dreamd is built for. `WebviewWindow::print` goes the other way, straight to
/// `printOperationWithPrintInfo:` / `NSPrintOperation`; on WebKitGTK it runs
/// `WebKitPrintOperation`'s dialog. Tauri's own docs claim `window.print()`
/// works everywhere; on macOS + wry it does not.
///
/// No PDF crate is involved, nothing is written here, and no path is chosen
/// here — the dialog is the OS's and the destination is the user's, which is
/// what keeps this inside tenet 1. What the page looks like is entirely the
/// `#print-css` block in `ui/index.html`.
#[tauri::command]
fn print_document(app: tauri::AppHandle) -> Result<(), String> {
    app.get_webview_window("main")
        .ok_or_else(|| "no window to print".to_string())?
        .print()
        .map_err(|e| e.to_string())
}

/// Move a file to the OS trash. The path must resolve to inside the repo root.
#[tauri::command]
fn delete_file(state: State<AppState>, path: String) -> Result<(), String> {
    let target = std::path::Path::new(&path)
        .canonicalize()
        .map_err(|e| format!("cannot resolve {path}: {e}"))?;
    let root = state.root();
    let root = root.canonicalize().unwrap_or(root);
    if !guard::inside_root(&root, &target) {
        return Err("refusing to delete outside the repo root".into());
    }
    trash_context().delete(&target).map_err(|e| e.to_string())
}

/// A trash handle that does not need permission to automate Finder.
///
/// `trash`'s macOS default is `DeleteMethod::Finder`, which `osascript`s a
/// `tell application "Finder" to delete`. Under the hardened runtime a notarized
/// build would then need `NSAppleEventsUsageDescription` plus the
/// `com.apple.security.automation.apple-events` entitlement, and the user would
/// get a "dreamd wants to control Finder" prompt the first time they deleted
/// anything. `NsFileManager` calls `trashItemAtURL` directly: no entitlement, no
/// prompt, no Finder sound, and faster. The cost is that Trash's "Put Back" does
/// not appear (a macOS bug, not the crate's) — recovery is dragging the file out
/// of the Trash instead of one click, which is why `delete_file` is behind a
/// confirm in the UI.
fn trash_context() -> trash::TrashContext {
    #[allow(unused_mut)]
    let mut ctx = trash::TrashContext::default();
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        ctx.set_delete_method(DeleteMethod::NsFileManager);
    }
    ctx
}

/// File -> Open. Picks a folder (or a markdown file), makes it the tree root,
/// and shows the window if it was still hidden.
///
/// The picker is `rfd` called straight from Rust rather than
/// `tauri-plugin-dialog`: the trigger is a native menu event that is already on
/// the Rust side, so routing it through IPC would mean a plugin registration and
/// an ACL entry to reach the same NSOpenPanel. `rfd` drives GTK3 on Linux — the
/// toolkit tauri already links there — so the same call covers both.
fn open_target(app: &tauri::AppHandle, want_file: bool) {
    let app = app.clone();
    // NSOpenPanel must run its modal loop on the main thread, and so must a GTK
    // dialog. Menu events already arrive there, but going through
    // `run_on_main_thread` says so.
    let handle = app.clone();
    let _ = handle.run_on_main_thread(move || {
        let dialog = rfd::FileDialog::new().set_title(if want_file {
            "Open a markdown file"
        } else {
            "Open a repository"
        });
        let picked = if want_file {
            dialog
                .add_filter("Markdown", &["md", "markdown", "mdown", "mkd"])
                .pick_file()
        } else {
            dialog.pick_folder()
        };
        if let Some(path) = picked {
            adopt_root(&app, path);
        }
    });
}

/// Point the whole app at `path`: swap the root, re-read config for the new
/// repo, re-walk, re-arm the watcher, and tell the frontend to reload.
fn adopt_root(app: &tauri::AppHandle, path: PathBuf) {
    // A picked *file* roots the tree at its own repo, not at the process cwd
    // the way the CLI does. The cwd is meaningless here — for a Finder launch
    // it is `/` — and the containing directory is what the user pointed at.
    let (root, initial) = if path.is_file() {
        let parent = path.parent().map(|p| p.to_path_buf());
        let root = dreamd::resolve_repo_root(parent);
        let file = dreamd::is_markdown(&path).then(|| path.to_string_lossy().into_owned());
        (root, file)
    } else {
        (dreamd::resolve_repo_root(Some(path)), None)
    };

    let state = app.state::<AppState>();
    let previous_root = state.root();
    let moved = previous_root != root;

    // Re-read config: the new repo may carry its own `.dreamd.toml`, and the
    // settings panel reports which keys it overrides. A `--theme`/`--mode` given
    // on the command line for this run is dropped here, which is the right
    // trade — those are a one-run convenience and this is a deliberate move to
    // a different repo.
    let cfg = Config::load(&root);
    let ignores = cfg.extra_ignores.clone();
    let theme_path = cfg.theme_css.clone();

    // Whether the *new* repo is already someone else's. Probed before the swap
    // and only when the root actually moved: probing a root we already own
    // would find our own socket and demote us.
    let claimed = moved && cli::repo_is_claimed(&root);

    // Marks move with the root, in the same block that swaps config — a window
    // that kept the previous repo's marks would paint them over documents that
    // never carried them, and the file they belong to would then be written
    // back under the new repo's name.
    //
    // Lock order is store -> config -> repo_root, matching `flush_marks`; it
    // is the only other holder of more than one of them.
    {
        let mut store = state.store.lock().unwrap();
        if state.persists.load(Ordering::Relaxed) && state.has_repo.load(Ordering::Relaxed) {
            if let Err(e) = marks_file::save(&previous_root, &store) {
                eprintln!(
                    "dreamd: cannot save marks for {}: {e}",
                    previous_root.display()
                );
            }
        }
        *state.config.lock().unwrap() = cfg;
        *state.repo_root.write().unwrap() = root.clone();
        state.has_repo.store(true, Ordering::Relaxed);
        if moved {
            state.persists.store(!claimed, Ordering::Relaxed);
        }
        *store = marks_file::load(&root);
        // The old root's dirt was just written, and the new root's marks came
        // straight off disk — neither is owed a save.
        state.dirty.store(false, Ordering::Relaxed);
        *state.pending_reanchor.lock().unwrap() = files_with_marks(&store);
    }
    if claimed {
        eprintln!(
            "dreamd: another dreamd already owns {} — this window will not save marks",
            root.display()
        );
    }

    // Retire the watcher on the old root before arming one on the new.
    {
        let mut slot = state.watcher_cancel.lock().unwrap();
        if let Some(old) = slot.take() {
            old.store(true, Ordering::Relaxed);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        watcher::spawn(app.clone(), root.clone(), theme_path, cancel.clone());
        *slot = Some(cancel);
    }

    // And the MCP socket, for the same reason and in the same shape. This is
    // the hazard the whole design is most likely to break on: the socket is
    // named after the repo root, `repo_root` mutates here, and a server left
    // listening on the old one would hand an agent asking about *this* repo the
    // previous repo's marks — an answer that looks entirely plausible. The old
    // thread unlinks its own socket file on the way out.
    //
    // Skipped when the root did not actually move, and that guard is not
    // cosmetic: the retiring thread only notices its cancel flag on the next
    // accept poll, so a rebind on the *same* path would find the old server
    // still listening, read that as another live dreamd, and stand down as a
    // secondary — silently losing MCP for the rest of the session.
    if moved {
        let mut slot = state.mcp_cancel.lock().unwrap();
        if let Some(old) = slot.take() {
            old.store(true, Ordering::Relaxed);
        }
        let cancel = Arc::new(AtomicBool::new(false));
        mcp::server::spawn(
            state.store.clone(),
            root.clone(),
            marking_dirty(state.dirty.clone(), notify::to_window(app.clone())),
            cancel.clone(),
        );
        *slot = Some(cancel);
    }

    // Off the main thread: this walks the repo, and the menu event is holding
    // the event loop.
    let catalog = state.catalog.clone();
    let handle = app.clone();
    std::thread::spawn(move || {
        catalog.rebuild(&root, &ignores);
        let _ = handle.emit("repo-changed", initial);
        if let Some(win) = handle.get_webview_window("main") {
            let _ = win.show();
            let _ = win.set_focus();
        }
    });
}

#[tauri::command]
fn open_external(url: String) -> Result<(), String> {
    // Markdown content is untrusted, so restrict to safe schemes: a document
    // must not be able to hand `file:`, `javascript:`, or arbitrary app URL
    // schemes to the OS opener. The allowlist lives in `guard` so it is
    // reachable from a test; this function is the I/O half.
    guard::allowed_scheme(&url)?;
    open::that(&url).map_err(|e| e.to_string())
}

// ---- the embedded Claude Code pane -----------------------------------------
//
// Four commands over `pty::Pty`, all under `core:default` — no plugin, no
// capability entry. The pty is created here on the pane's *first open*, never at
// boot: `portable-pty` costs nothing in a session that never opens a terminal,
// and a cold-start regression would mean the pane is being constructed eagerly.
//
// These are the only commands that emit. Every other one in this file answers
// with a return value, which is the frontend's truth for the mutation it asked
// for (`notify`'s module docs are the long form). A terminal has no such shape:
// its output arrives when the child feels like producing it, and there is no
// call to hang it off. `marks-changed` is still the only *store* push.

/// Event carrying a base64 chunk of the child's output.
const PTY_DATA: &str = "pty-data";
/// Event announcing the child is gone. Payload is its exit code, if it had one.
const PTY_EXIT: &str = "pty-exit";

/// Start the pane's process, or do nothing if it is already running.
///
/// Idempotent because the frontend calls it on every open and the pane's
/// scrollback is meant to survive being hidden: closing the pane is
/// `display: none`, not a kill.
#[tauri::command]
fn pty_spawn(
    rows: u16,
    cols: u16,
    app: tauri::AppHandle,
    state: State<AppState>,
) -> Result<bool, String> {
    let mut slot = state.pty.lock().unwrap();
    if slot.is_some() {
        return Ok(false);
    }
    let handle = app.clone();
    let sink: pty::Sink = Arc::new(move |ev| match ev {
        pty::PtyEvent::Data(chunk) => {
            let _ = handle.emit(PTY_DATA, chunk);
        }
        pty::PtyEvent::Exit(code) => {
            let _ = handle.emit(PTY_EXIT, code);
        }
    });
    // Read here rather than accepted as an argument: the mode is a launch flag
    // chosen from a closed enum in `pty::pane_command`, and taking it off the
    // wire would put a caller-supplied value on the path to a shell for no gain
    // (tenet 3). The frontend's mode control writes the config first, then
    // restarts — so by the time it reaches here the preference *is* the config.
    let mode = state.config.lock().unwrap().agent.permission_mode;
    *slot = Some(pty::Pty::spawn(rows, cols, &state.root(), mode, sink)?);
    Ok(true)
}

/// The pane's preferences, fetched on its *first open* rather than at boot.
///
/// Its own command instead of a slice of `get_settings` because that one loads
/// the theme list and walks the user themes directory — work a terminal has no
/// use for, on a path whose whole contract is that a session which never opens
/// the pane pays nothing for it.
#[tauri::command]
fn agent_prefs(state: State<AppState>) -> config::Agent {
    state.config.lock().unwrap().agent.clone()
}

/// Keystrokes from xterm.js, base64 so a paste of arbitrary bytes survives the
/// JSON trip intact.
#[tauri::command]
fn pty_write(data: String, state: State<AppState>) -> Result<(), String> {
    let bytes = pty::from_b64(&data)?;
    match state.pty.lock().unwrap().as_mut() {
        Some(p) => p.write(&bytes),
        None => Err("the pane is not running".into()),
    }
}

/// Called from the pane's `ResizeObserver`. A resize with no pane running is
/// success, not an error: the postcondition — "the child, if any, knows its
/// size" — already holds, and the observer fires during teardown.
#[tauri::command]
fn pty_resize(rows: u16, cols: u16, state: State<AppState>) -> Result<(), String> {
    match state.pty.lock().unwrap().as_ref() {
        Some(p) => p.resize(rows, cols),
        None => Ok(()),
    }
}

/// End the pane's process. Dropping the handle is what actually reaps it —
/// `Pty::drop` kills — and clearing the slot is what lets the next open start a
/// fresh session.
#[tauri::command]
fn pty_kill(state: State<AppState>) {
    *state.pty.lock().unwrap() = None;
}

/// Frontend-side timing mark, forwarded into the same NDJSON stream as the Rust
/// marks. A no-op unless built with `--features perf`; `console.log` inside
/// WKWebView never reaches our stdout, so this is the only way to get webview
/// timings out of a real run.
#[tauri::command]
fn perf_mark(phase: String, ms: f64) {
    perf::emit(&phase, ms);
}

/// Whether this binary carries timing instrumentation. The frontend checks this
/// once at startup and skips its own `perf_mark` calls entirely when false.
#[tauri::command]
fn perf_enabled() -> bool {
    perf::enabled()
}

/// Preload the store with highlights so the save→repaint loop can be measured
/// at a realistic highlight count without driving the UI.
///
/// Reads a corpus fixture (`perf/corpus/generated/highlights/N.json`) named by
/// `DREAMD_PERF_SEED`, and anchors every entry against the initially-opened
/// file using its `rendered` quote and whitespace-collapsed context — the form
/// the frontend actually sends, which is what forces `locate` down its
/// expensive path.
///
/// Only compiled with `--features perf`.
#[cfg(feature = "perf")]
fn seed_highlights(store: &mut Store, file: &Option<String>) {
    let (Ok(path), Some(target)) = (std::env::var("DREAMD_PERF_SEED"), file.as_ref()) else {
        return;
    };
    let Ok(raw) = std::fs::read_to_string(&path) else {
        eprintln!("perf: cannot read seed fixture {path}");
        return;
    };
    let Ok(fixtures) = serde_json::from_str::<Vec<serde_json::Value>>(&raw) else {
        eprintln!("perf: seed fixture {path} is not a JSON array");
        return;
    };
    // Whatever the DOM would have handed us: whitespace collapsed to single
    // spaces, no leading or trailing run.
    let collapsed = |f: &serde_json::Value, key: &str| {
        f.get(key)
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    let mut seeded = 0;
    for f in &fixtures {
        let quote = f
            .get("rendered")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if quote.is_empty() {
            continue;
        }
        store.add_highlight(
            target.clone(),
            0,
            0,
            quote.to_string(),
            collapsed(f, "prefix"),
            collapsed(f, "suffix"),
        );
        seeded += 1;
    }
    eprintln!("perf: seeded {seeded} highlights against {target}");
}

fn main() {
    // Before anything else, and before any thread exists: on an NVIDIA
    // proprietary driver, WebKitGTK's DMA-BUF renderer kills the Wayland
    // connection during GTK init and there is no window left to report it in.
    dreamd::webkit::prepare_env();

    perf::init();
    perf::mark("process_start");

    let cli = Cli::parse();

    // Headless subcommands exit here: after the config read they need, before
    // the repo walk and index build they don't. `--bench-startup` deliberately
    // sits further down, past the walk, because that is what it measures
    // (except on a file argument, where there is no pre-window walk left).
    if let Some(cmd) = cli.command {
        if let Err(e) = cli::run(cmd) {
            eprintln!("dreamd: {e}");
            std::process::exit(1);
        }
        return;
    }

    let (repo_root, initial, has_repo) = dreamd::resolve_target(cli.path);
    perf::mark("target_resolved");

    let mut cfg = Config::load(&repo_root);
    // `--theme` / `--mode` override for this run only, and never reach the
    // config file.
    if let Some(name) = cli.theme {
        cfg.theme_css = None;
        cfg.theme = Some(name);
    }
    if let Some(mode) = cli.mode {
        cfg.mode = Some(mode);
    }
    perf::mark("config_loaded");

    // Marks come back before the walk, so the window has them the first time
    // the frontend asks — `refreshStack` runs at boot and a badge painted from
    // an empty store would have to be corrected. Nothing is re-anchored here;
    // that is `get_highlights`'s job, per file, on first sight.
    //
    // `has_repo` guards it for the same reason it guards the watcher and the
    // socket: with no repo the root is whatever the walk-up fell back to, and
    // a marks file named after it would be claiming a repo that doesn't exist.
    let marks_started = std::time::Instant::now();
    // `mut` is only needed by the perf seeding below, which compiles out.
    #[allow(unused_mut)]
    let mut store = if has_repo {
        marks_file::load(&repo_root)
    } else {
        Store::default()
    };
    perf::emit(
        "d:marks_loaded",
        marks_started.elapsed().as_secs_f64() * 1000.0,
    );
    perf::mark("marks_loaded");
    let pending_reanchor = files_with_marks(&store);

    // Persistence follows the same claim MCP does: the socket bind is the lock
    // (`mcp::server`'s module doc), and this is a read of it. A second dreamd
    // on the same repo keeps its marks in memory rather than racing the first
    // one's writes, which would be last-write-wins over a whole file.
    //
    // The gap between this probe and the bind the socket thread performs in
    // `.setup()` is real but narrow: two dreamds started on the same repo
    // inside the same few milliseconds could both read "unclaimed". The cost
    // is one save overwriting another's, not a corrupt file — `marks_file`
    // writes a temp sibling and renames. Closing it properly means the bind
    // itself reporting back, which is a change to `mcp::server`'s interface.
    let persists = has_repo && !cli::repo_is_claimed(&repo_root);
    if has_repo && !persists {
        eprintln!(
            "dreamd: another dreamd already owns {} — this window will not save marks",
            repo_root.display()
        );
    }

    // `dreamd file.md` is a Preview-style "open this one document" gesture: the
    // document is the only thing on screen, the sidebar starts collapsed, and
    // the repo walk has no business on the critical path. Anything else — a
    // directory, no argument, or a non-markdown file, all of which leave the
    // sidebar as the only usable surface — builds synchronously exactly as
    // before.
    //
    // `has_repo` is the other guard, and it is the one that matters for a
    // packaged `.app`: LaunchServices gives a Finder launch cwd `/`, the
    // walk-up finds no `.git`, and `repo_root` would be `/`. Walking that is
    // unbounded — `WalkBuilder` here has `hidden(false)` and no depth limit —
    // and it happens *before* the Tauri builder exists, so the window never
    // appears. With no repo, nothing walks and nothing is shown until
    // File -> Open says where to look.
    let deferred = initial.is_some();
    let catalog = Arc::new(Catalog::pending());
    if !deferred && has_repo {
        catalog.build(&repo_root, &cfg.extra_ignores);
    }

    // Above the spawn deliberately: on the deferred path there is no pre-window
    // work left to measure, and exiting into a live walk thread would time the
    // teardown racing it.
    if cli.bench_startup {
        perf::mark("bench_startup_exit");
        return;
    }

    if deferred && has_repo {
        let (c, root, ignores) = (
            catalog.clone(),
            repo_root.clone(),
            cfg.extra_ignores.clone(),
        );
        std::thread::spawn(move || c.build(&root, &ignores));
    } else if !has_repo {
        // Nothing to walk, so nothing will ever resolve the catalog's readiness
        // gate — and `list_markdown_files` waits on it. Settle it empty so the
        // frontend's boot completes and shows the empty state.
        catalog.settle_empty(&repo_root);
    }

    let theme_path = cfg.theme_css.clone();

    #[cfg(feature = "perf")]
    seed_highlights(&mut store, &initial);

    let watcher_cancel = has_repo.then(|| Arc::new(AtomicBool::new(false)));
    // Armed on the same condition as the watcher: with no repo, `repo_root` is
    // whatever the walk-up fell back to and a socket named after it would be
    // claiming a repo that does not exist.
    let mcp_cancel = has_repo.then(|| Arc::new(AtomicBool::new(false)));
    let store = Arc::new(Mutex::new(store));
    let dirty = Arc::new(AtomicBool::new(false));
    let state = AppState {
        repo_root: RwLock::new(repo_root.clone()),
        has_repo: AtomicBool::new(has_repo),
        watcher_cancel: Mutex::new(watcher_cancel.clone()),
        mcp_cancel: Mutex::new(mcp_cancel.clone()),
        initial_file: initial,
        config: Mutex::new(cfg),
        store: store.clone(),
        catalog,
        // Corrected in `.setup()`, which is the first place a window exists to
        // ask the OS. Dark is what dreamd has always painted before the theme
        // lands, so this is the status quo for the handful of statements in
        // between.
        appearance: AtomicU8::new(1),
        persists: AtomicBool::new(persists),
        dirty: dirty.clone(),
        pending_reanchor: Mutex::new(pending_reanchor),
        pty: Mutex::new(None),
    };

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            list_markdown_files,
            repo_info,
            get_ui,
            complete_directories,
            set_root,
            initial_file,
            render_markdown,
            fuzzy_search,
            rebuild_index,
            add_highlight,
            set_annotation,
            remove_highlight,
            remove_pair,
            get_highlights,
            get_highlight,
            reanchor,
            get_stack,
            send_stack,
            get_keymap,
            stack_query_text,
            get_theme,
            set_appearance,
            get_settings,
            set_config,
            default_keymap,
            list_themes,
            theme_css,
            palette_css,
            save_theme,
            delete_theme,
            copy_to_clipboard,
            print_document,
            delete_file,
            open_external,
            agent_prefs,
            pty_spawn,
            pty_write,
            pty_resize,
            pty_kill,
            perf_mark,
            perf_enabled,
        ])
        .setup(move |app| {
            perf::mark("setup");
            // The window declared in tauri.conf.json is created inside Tauri's
            // own setup, which runs before this hook — so there is one to ask
            // here, and `Window::theme()` is the only way to learn the OS
            // appearance at all: Tauri 2 has no pre-window getter. It resolves
            // inline when called from the main thread, which this is.
            if let Some(win) = app.get_webview_window("main") {
                let state = app.state::<AppState>();
                let system = match win.theme() {
                    Ok(tauri::Theme::Light) => theme::Scheme::Light,
                    // An unknown or errored theme keeps dreamd's historical
                    // dark default rather than guessing light.
                    _ => theme::Scheme::Dark,
                };
                let (mode, scheme) = {
                    let cfg = state.config.lock().unwrap();
                    (cfg.mode(), theme::scheme_for(&cfg, system))
                };
                state.set_scheme(scheme);
                pin_native_theme(&win, native_pin(mode, scheme, system));

                // Paint the window with the theme's own `--bg` before the
                // frontend has injected theme.css; without this the reading
                // pane is white for as long as boot takes. `backgroundColor` in
                // tauri.conf.json covers the frame before this runs — it is a
                // static dark value and cannot follow the mode, which is why it
                // is only ever on screen for this gap.
                let css = theme::resolve(&state.config.lock().unwrap(), state.scheme()).css;
                if let Some((r, g, b)) = theme::background(&css, state.scheme()) {
                    let _ = win.set_background_color(Some(tauri::window::Color(r, g, b, 255)));
                }

                // The window is declared `visible: false` so a launch with
                // nothing to show shows nothing: a `.app` double-clicked from
                // Finder sits in the Dock with its menubar until File -> Open
                // gives it a repo. Showing it here, after the background colour
                // is set, also closes the white-flash gap the comment above
                // describes for every other launch.
                if state.has_repo.load(Ordering::Relaxed) || state.initial_file.is_some() {
                    let _ = win.show();
                }
            }
            // Not armed when there is no repo: `watch` is recursive, and the
            // root would be `/`.
            if let Some(cancel) = watcher_cancel.clone() {
                watcher::spawn(
                    app.handle().clone(),
                    repo_root.clone(),
                    theme_path.clone(),
                    cancel,
                );
            }
            // The agent side. `notify::to_window` is the only Tauri that
            // reaches the server, and it is why an agent resolving a mark
            // repaints a window nobody touched. A second dreamd on the same
            // repo finds the socket taken and says so on stderr rather than
            // stealing it.
            if let Some(cancel) = mcp_cancel.clone() {
                mcp::server::spawn(
                    store.clone(),
                    repo_root.clone(),
                    marking_dirty(dirty.clone(), notify::to_window(app.handle().clone())),
                    cancel,
                );
            }
            // The debounce thread. It reads the root and the claim through the
            // handle on every tick rather than capturing them, because
            // File -> Open moves both — a window launched with no repo at all
            // starts as no writer and becomes one — and `adopt_root` has
            // already flushed and cleared the flag by the time the next tick
            // sees the new root. Unconditional for that reason; a tick that
            // owns nothing is one relaxed load. No cancel token: the process
            // exiting is its only exit condition.
            let handle = app.handle().clone();
            std::thread::spawn(move || loop {
                std::thread::sleep(SAVE_DEBOUNCE);
                flush_marks(&handle);
            });
            Ok(())
        })
        .menu(dreamd::menu::build)
        .on_menu_event(|app, event| match event.id().as_ref() {
            dreamd::menu::OPEN_FOLDER => open_target(app, false),
            dreamd::menu::OPEN_FILE => open_target(app, true),
            _ => {}
        })
        // `build` + `run` rather than `run` alone, for the one thing the short
        // form cannot do: a last flush on the way out. Quitting inside the
        // debounce window would otherwise lose the annotation the user typed
        // just before Cmd+Q — the most likely mark in the store to matter.
        // Both events, because `ExitRequested` can be cancelled by a listener
        // that doesn't exist yet, and a second flush costs one atomic read.
        .build(tauri::generate_context!())
        .expect("error while building dreamd")
        .run(|app, event| {
            if matches!(
                event,
                tauri::RunEvent::ExitRequested { .. } | tauri::RunEvent::Exit
            ) {
                flush_marks(app);
                // And take the pane's process with us. `Pty::drop` kills, but
                // only if something drops it — a `claude` reparented to launchd
                // would keep running with no window to show it.
                *app.state::<AppState>().pty.lock().unwrap() = None;
            }
        });
}
