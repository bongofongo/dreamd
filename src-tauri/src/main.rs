#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! dreamd — lightweight GUI markdown reader with a highlight/annotation ->
//! agent loop. Launched from the CLI: `dreamd [path]`.

mod annotations;
mod config;
mod fs_walk;
mod markdown;
mod search;
mod send;
mod watcher;

use annotations::{Highlight, Pair, Store};
use clap::Parser;
use config::{Config, Keymap};
use fs_walk::FileNode;
use search::SearchIndex;
use send::SendResult;
use std::path::PathBuf;
use std::sync::Mutex;
use tauri::State;

/// The bundled default theme, embedded so it works in a packaged binary.
const DEFAULT_THEME: &str = include_str!("../../ui/theme.css");

struct AppState {
    repo_root: PathBuf,
    /// A file to open on load (nvim-style `dreamd file.md`), if any.
    initial_file: Option<String>,
    config: Config,
    store: Mutex<Store>,
    index: Mutex<SearchIndex>,
}

#[derive(clap::Parser)]
#[command(name = "dreamd", version, about = "Lightweight GUI markdown reader")]
struct Cli {
    /// A markdown file to open, or a directory to browse. Like nvim: passing a
    /// file opens it, but the file tree is still rooted at the repo of the
    /// current directory. Defaults to the current directory.
    path: Option<PathBuf>,
}

fn is_markdown(path: &std::path::Path) -> bool {
    matches!(
        path.extension().and_then(|e| e.to_str()),
        Some("md") | Some("markdown") | Some("mdown") | Some("mkd")
    )
}

fn resolve_repo_root(arg: Option<PathBuf>) -> PathBuf {
    let start = arg.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
    let start = start.canonicalize().unwrap_or(start);
    let mut cur = start.as_path();
    loop {
        if cur.join(".git").exists() {
            return cur.to_path_buf();
        }
        match cur.parent() {
            Some(p) => cur = p,
            None => break,
        }
    }
    start
}

/// Resolve the CLI argument into a (tree root, file-to-open) pair.
/// - a file arg  -> root = repo of the current directory; open the file if markdown.
/// - a dir arg   -> root = repo of that directory; nothing pre-opened.
/// - no arg      -> root = repo of the current directory.
fn resolve_target(arg: Option<PathBuf>) -> (PathBuf, Option<String>) {
    match arg {
        Some(p) => {
            let abs = p.canonicalize().unwrap_or(p);
            if abs.is_file() {
                let root = resolve_repo_root(None);
                let file = is_markdown(&abs).then(|| abs.to_string_lossy().into_owned());
                (root, file)
            } else {
                (resolve_repo_root(Some(abs)), None)
            }
        }
        None => (resolve_repo_root(None), None),
    }
}

fn read_source(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("cannot read {path}: {e}"))
}

// ---- commands ------------------------------------------------------------

#[tauri::command]
fn list_markdown_files(state: State<AppState>) -> FileNode {
    fs_walk::scan(&state.repo_root, &state.config.extra_ignores)
}

#[tauri::command]
fn repo_info(state: State<AppState>) -> serde_json::Value {
    serde_json::json!({
        "root": state.repo_root.to_string_lossy(),
        "name": state.repo_root.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        "display": home_relative(&state.repo_root),
    })
}

/// Render a path home-relative (`~/foo` instead of `/Users/me/foo`).
fn home_relative(path: &std::path::Path) -> String {
    if let Some(home) = dirs::home_dir() {
        if let Ok(rest) = path.strip_prefix(&home) {
            return format!("~/{}", rest.to_string_lossy());
        }
    }
    path.to_string_lossy().into_owned()
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

#[tauri::command]
fn rebuild_index(state: State<AppState>) {
    let paths = fs_walk::markdown_paths(&state.repo_root, &state.config.extra_ignores);
    *state.index.lock().unwrap() = SearchIndex::build(&state.repo_root, &paths);
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
    let (line_start, line_end) = match markdown::locate(&source, &prefix, &quote, &suffix) {
        Some(loc) => (loc.line_start, loc.line_end),
        None => (0, 0),
    };
    let id = state.store.lock().unwrap().add_highlight(
        file_path, line_start, line_end, quote, prefix, suffix,
    );
    Ok(id)
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
    if let Some(p) = &state.config.theme_css {
        if let Ok(css) = std::fs::read_to_string(p) {
            return css;
        }
    }
    DEFAULT_THEME.to_string()
}

#[tauri::command]
fn copy_to_clipboard(text: String) -> Result<(), String> {
    let mut cb = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    cb.set_text(text).map_err(|e| e.to_string())
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
        Some("http") | Some("https") | Some("mailto") => {
            open::that(&url).map_err(|e| e.to_string())
        }
        Some(other) => Err(format!("refusing to open scheme: {other}")),
        None => Err("refusing to open URL without a scheme".into()),
    }
}

fn main() {
    let cli = Cli::parse();
    let (repo_root, initial) = resolve_target(cli.path);
    let cfg = Config::load(&repo_root);
    let paths = fs_walk::markdown_paths(&repo_root, &cfg.extra_ignores);
    let index = SearchIndex::build(&repo_root, &paths);
    let theme_path = cfg.theme_css.clone();

    let state = AppState {
        repo_root: repo_root.clone(),
        initial_file: initial,
        config: cfg.clone(),
        store: Mutex::new(Store::default()),
        index: Mutex::new(index),
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
        ])
        .setup(move |app| {
            watcher::spawn(app.handle().clone(), repo_root.clone(), theme_path.clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running dreamd");
}
