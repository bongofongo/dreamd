#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

//! dreamd — lightweight GUI markdown reader with a highlight/annotation ->
//! agent loop. Launched from the CLI: `dreamd [path]`.
//!
//! This file is deliberately thin: CLI, `AppState`, the Tauri commands, and the
//! builder. The work those commands do lives in the `dreamd` library crate
//! (`src/lib.rs`) so benches and tests can drive it without a window.

use clap::Parser;
use dreamd::annotations::{Highlight, Pair, Store};
use dreamd::catalog::Catalog;
use dreamd::config::{Config, Keymap};
use dreamd::fs_walk::FileNode;
use dreamd::send::SendResult;
use dreamd::{cli, config, home_relative, markdown, perf, read_source, send, theme, watcher};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use tauri::{Manager, State};

struct AppState {
    repo_root: PathBuf,
    /// A file to open on load (nvim-style `dreamd file.md`), if any.
    initial_file: Option<String>,
    /// Behind a lock because the settings panel rewrites it at runtime — the
    /// only mutable-at-runtime configuration dreamd has.
    config: Mutex<Config>,
    store: Mutex<Store>,
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
}

impl AppState {
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
    let repo_root = state.repo_root.clone();
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
    let config = state.config.lock().unwrap().clone();
    send::send(&config, &state.repo_root, &pairs)
}

#[tauri::command]
fn get_keymap(state: State<AppState>) -> Keymap {
    state.config.lock().unwrap().keymap.clone()
}

/// The assembled stack query as markdown (for copy-to-clipboard).
#[tauri::command]
fn stack_query_text(state: State<AppState>) -> String {
    let pairs = state.store.lock().unwrap().stack_pairs();
    send::assemble_query(&state.repo_root, &pairs)
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
fn set_appearance(app: tauri::AppHandle, state: State<AppState>, scheme: theme::Scheme) -> ThemeView {
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
        local_overrides: config::local_override_keys(&state.repo_root),
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
    let cfg = Config::load(&state.repo_root);
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

    let (repo_root, initial) = dreamd::resolve_target(cli.path);
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

    // `dreamd file.md` is a Preview-style "open this one document" gesture: the
    // document is the only thing on screen, the sidebar starts collapsed, and
    // the repo walk has no business on the critical path. Anything else — a
    // directory, no argument, or a non-markdown file, all of which leave the
    // sidebar as the only usable surface — builds synchronously exactly as
    // before.
    let deferred = initial.is_some();
    let catalog = Arc::new(Catalog::pending());
    if !deferred {
        catalog.build(&repo_root, &cfg.extra_ignores);
    }

    // Above the spawn deliberately: on the deferred path there is no pre-window
    // work left to measure, and exiting into a live walk thread would time the
    // teardown racing it.
    if cli.bench_startup {
        perf::mark("bench_startup_exit");
        return;
    }

    if deferred {
        let (c, root, ignores) = (catalog.clone(), repo_root.clone(), cfg.extra_ignores.clone());
        std::thread::spawn(move || c.build(&root, &ignores));
    }

    let theme_path = cfg.theme_css.clone();

    // `mut` is only needed by the seeding call below, which compiles out.
    #[allow(unused_mut)]
    let mut store = Store::default();
    #[cfg(feature = "perf")]
    seed_highlights(&mut store, &initial);

    let state = AppState {
        repo_root: repo_root.clone(),
        initial_file: initial,
        config: Mutex::new(cfg),
        store: Mutex::new(store),
        catalog,
        // Corrected in `.setup()`, which is the first place a window exists to
        // ask the OS. Dark is what dreamd has always painted before the theme
        // lands, so this is the status quo for the handful of statements in
        // between.
        appearance: AtomicU8::new(1),
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
            delete_file,
            open_external,
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
            }
            watcher::spawn(app.handle().clone(), repo_root.clone(), theme_path.clone());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running dreamd");
}
