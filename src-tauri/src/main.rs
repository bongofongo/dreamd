#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! dreamd — lightweight GUI markdown reader with a highlight/annotation ->
//! agent loop. Launched from the CLI: `dreamd [path]`.
//!
//! This file is deliberately thin: CLI, `AppState`, the Tauri commands, and the
//! builder. The work those commands do lives in the `dreamd` library crate
//! (`src/lib.rs`) so benches and tests can drive it without a window.

use clap::Parser;
use dreamd::annotations::{Highlight, Pair, Store};
use dreamd::config::{Config, Keymap};
use dreamd::fs_walk::FileNode;
use dreamd::search::SearchIndex;
use dreamd::send::SendResult;
use dreamd::{fs_walk, home_relative, markdown, perf, read_source, send, theme, watcher};
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::{Manager, State};

struct AppState {
    repo_root: PathBuf,
    /// A file to open on load (nvim-style `dreamd file.md`), if any.
    initial_file: Option<String>,
    config: Config,
    store: Mutex<Store>,
    index: Mutex<SearchIndex>,
    /// The file tree, kept alongside the index because both come from the same
    /// walk. Caching it is what stops a cold start walking the repo twice —
    /// once here for the index, once for the frontend's first `loadTree`.
    tree: Mutex<FileNode>,
}

#[derive(clap::Parser)]
#[command(name = "dreamd", version, about = "Lightweight GUI markdown reader")]
struct Cli {
    /// A markdown file to open, or a directory to browse. Like nvim: passing a
    /// file opens it, but the file tree is still rooted at the repo of the
    /// current directory. Defaults to the current directory.
    path: Option<PathBuf>,

    /// Run the full pre-window startup sequence, then exit without opening a
    /// window. Lets hyperfine measure the Rust half of cold start on its own.
    /// Only emits timings when built with `--features perf`.
    #[arg(long, hide = true)]
    bench_startup: bool,
}

// ---- commands ------------------------------------------------------------

#[tauri::command]
fn list_markdown_files(state: State<AppState>) -> FileNode {
    state.tree.lock().unwrap().clone()
}

#[tauri::command]
fn repo_info(state: State<AppState>) -> serde_json::Value {
    serde_json::json!({
        "root": state.repo_root.to_string_lossy(),
        "name": state.repo_root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        "display": home_relative(&state.repo_root),
    })
}

#[tauri::command]
fn initial_file(state: State<AppState>) -> Option<String> {
    state.initial_file.clone()
}

#[tauri::command]
fn render_markdown(path: String) -> Result<String, String> {
    let source = read_source(&path)?;
    Ok(markdown::render(&source))
}

#[tauri::command]
fn fuzzy_search(state: State<AppState>, query: String) -> Vec<FileNode> {
    state.index.lock().unwrap().query(&query)
}

/// Re-walk the repo after a file appears or disappears, and hand the fresh tree
/// straight back. The frontend used to follow this with `list_markdown_files`,
/// which walked the whole repo a second time for the same answer.
#[tauri::command]
fn rebuild_index(state: State<AppState>) -> FileNode {
    let paths = fs_walk::markdown_paths(&state.repo_root, &state.config.extra_ignores);
    let tree = fs_walk::build_tree(&state.repo_root, &paths);
    *state.index.lock().unwrap() = SearchIndex::build(&state.repo_root, &paths);
    *state.tree.lock().unwrap() = tree.clone();
    tree
}

#[tauri::command]
fn add_highlight(
    state: State<AppState>,
    file_path: String,
    quote: String,
    prefix: String,
    suffix: String,
) -> Result<u64, String> {
    let source = read_source(&file_path)?;
    let (line_start, line_end) = markdown::locate(&source, &prefix, &quote, &suffix)
        .map_or((0, 0), |loc| (loc.line_start, loc.line_end));
    Ok(state
        .store
        .lock()
        .unwrap()
        .add_highlight(file_path, line_start, line_end, quote, prefix, suffix))
}

#[tauri::command]
fn set_annotation(state: State<AppState>, id: u64, text: String) -> bool {
    state.store.lock().unwrap().set_annotation(id, text)
}

#[tauri::command]
fn remove_highlight(state: State<AppState>, id: u64) {
    state.store.lock().unwrap().remove(id);
}

#[tauri::command]
fn remove_pair(state: State<AppState>, id: u64) {
    state.store.lock().unwrap().remove_from_stack(id);
}

#[tauri::command]
fn get_highlights(state: State<AppState>, path: String) -> Vec<Highlight> {
    state.store.lock().unwrap().for_file(&path)
}

#[tauri::command]
fn get_highlight(state: State<AppState>, id: u64) -> Option<Highlight> {
    state.store.lock().unwrap().get(id)
}

/// Re-read the file and re-anchor its highlights (Active/Stale). Called by the
/// frontend when a `file-changed` event arrives for the open document.
#[tauri::command]
fn reanchor(state: State<AppState>, path: String) -> Result<Vec<Highlight>, String> {
    let source = read_source(&path)?;
    Ok(state.store.lock().unwrap().reanchor_file(&path, &source))
}

#[tauri::command]
fn get_stack(state: State<AppState>) -> Vec<Pair> {
    state.store.lock().unwrap().stack_pairs()
}

/// One-button send. `ids` empty/absent = send the whole stack.
#[tauri::command]
fn send_stack(state: State<AppState>, ids: Vec<u64>) -> Result<SendResult, String> {
    let pairs = {
        let store = state.store.lock().unwrap();
        if ids.is_empty() {
            store.stack_pairs()
        } else {
            store.selected_pairs(&ids)
        }
    };
    send::send(&state.config, &state.repo_root, &pairs)
}

#[tauri::command]
fn get_keymap(state: State<AppState>) -> Keymap {
    state.config.keymap.clone()
}

/// The assembled stack query as markdown (for copy-to-clipboard).
#[tauri::command]
fn stack_query_text(state: State<AppState>) -> String {
    let pairs = state.store.lock().unwrap().stack_pairs();
    send::assemble_query(&state.repo_root, &pairs)
}

#[tauri::command]
fn get_theme_css(state: State<AppState>) -> String {
    theme::resolve(state.config.theme_css.as_deref())
}

#[tauri::command]
fn copy_to_clipboard(text: String) -> Result<(), String> {
    send::copy_clipboard(&text)
}

/// Move a file to the OS trash. The path must resolve to inside the repo root.
#[tauri::command]
fn delete_file(state: State<AppState>, path: String) -> Result<(), String> {
    let target = std::path::Path::new(&path)
        .canonicalize()
        .map_err(|e| format!("cannot resolve {path}: {e}"))?;
    let root = state
        .repo_root
        .canonicalize()
        .unwrap_or_else(|_| state.repo_root.clone());
    if !target.starts_with(&root) {
        return Err("refusing to delete outside the repo root".into());
    }
    trash::delete(&target).map_err(|e| e.to_string())
}

#[tauri::command]
fn open_external(url: String) -> Result<(), String> {
    // Markdown content is untrusted, so restrict to safe schemes: a document
    // must not be able to hand `file:`, `javascript:`, or arbitrary app URL
    // schemes to the OS opener.
    let scheme = url.split_once(':').map(|(s, _)| s.to_ascii_lowercase());
    match scheme.as_deref() {
        Some("http" | "https" | "mailto") => open::that(&url).map_err(|e| e.to_string()),
        Some(other) => Err(format!("refusing to open scheme: {other}")),
        None => Err("refusing to open URL without a scheme".into()),
    }
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
    perf::init();
    perf::mark("process_start");

    let cli = Cli::parse();
    let (repo_root, initial) = dreamd::resolve_target(cli.path);
    perf::mark("target_resolved");

    let cfg = Config::load(&repo_root);
    perf::mark("config_loaded");

    let paths = fs_walk::markdown_paths(&repo_root, &cfg.extra_ignores);
    perf::mark("walk_done");

    let index = SearchIndex::build(&repo_root, &paths);
    perf::mark("index_built");

    let tree = fs_walk::build_tree(&repo_root, &paths);
    perf::mark("tree_built");

    if cli.bench_startup {
        perf::mark("bench_startup_exit");
        return;
    }

    let theme_path = cfg.theme_css.clone();
    let theme_bg = theme::background(&theme::resolve(theme_path.as_deref()));

    // `mut` is only needed by the seeding call below, which compiles out.
    #[allow(unused_mut)]
    let mut store = Store::default();
    #[cfg(feature = "perf")]
    seed_highlights(&mut store, &initial);

    let state = AppState {
        repo_root: repo_root.clone(),
        initial_file: initial,
        config: cfg,
        store: Mutex::new(store),
        index: Mutex::new(index),
        tree: Mutex::new(tree),
    };

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            list_markdown_files,
            repo_info,
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
            get_theme_css,
            copy_to_clipboard,
            delete_file,
            open_external,
            perf_mark,
            perf_enabled,
        ])
        .setup(move |app| {
            perf::mark("setup");
            // Paint the window with the theme's own `--bg` before the frontend
            // has injected theme.css; without this the reading pane is white
            // for as long as boot takes. `backgroundColor` in tauri.conf.json
            // covers the frame before this runs.
            if let (Some(win), Some((r, g, b))) = (app.get_webview_window("main"), theme_bg) {
                let _ = win.set_background_color(Some(tauri::window::Color(r, g, b, 255)));
            }
            watcher::spawn(app.handle().clone(), repo_root.clone(), theme_path.clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running dreamd");
}
