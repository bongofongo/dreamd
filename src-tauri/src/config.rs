//! Config loading and saving: global `~/.config/dreamd/config.toml` with a
//! repo-local `.dreamd.toml` override. Everything is optional; sane defaults
//! are used.
//!
//! Reading goes through raw `toml::Table`s rather than deserializing each file
//! into a `Config` and merging structs. That matters: a struct merge cannot
//! tell "the local file set `tmux_autodetect = true`" from "the local file
//! didn't mention it", so it silently reset whatever the global file said. At
//! the table level an absent key is simply absent.
//!
//! Writing patches the *global* table in place and re-serializes it, so keys we
//! never touched — including ones a future version adds — survive a save. TOML
//! comments and key ordering do not; `dreamd config edit` is the escape hatch
//! for hand-maintained files.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use toml::{Table, Value};

/// Written at the top of every config we save, so a user who opens the file
/// after using the settings panel knows what happened to their comments.
const HEADER: &str = "\
# dreamd config. Written by the settings panel and `dreamd config set`.
# Values are preserved across saves; comments and key ordering are not.
# Docs: https://github.com/bongofongo/dreamd#config

";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Named theme: a palette from the bundled set or from
    /// `~/.config/dreamd/themes/<name>.css`. Appended after the base
    /// stylesheet. Ignored when `theme_css` is set.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme: Option<String>,

    /// Path to a complete user stylesheet. Replaces the base stylesheet
    /// outright — no palette is appended. Hot-reloaded by the watcher.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub theme_css: Option<PathBuf>,

    /// Which of a theme's two appearances to show. `System` follows the OS.
    ///
    /// `Option` rather than a plain field with a `System` default, because the
    /// two are not the same thing: a legacy palette name like `gruvbox-dark`
    /// implies an appearance, and that implication has to lose to the user
    /// explicitly choosing *system*. Collapsing "never set" into
    /// `Some(System)` would make the panel's System button do nothing for
    /// anyone still on an old theme name. Read it through [`Config::mode`].
    ///
    /// Unlike `theme_css` this is safe for a repo-local `.dreamd.toml` to set:
    /// it reads no files and injects nothing.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<Mode>,

    /// Extra glob-ish ignore patterns beyond `.gitignore`/`.ignore`.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub extra_ignores: Vec<String>,

    /// tmux target pane for send-to-Claude (e.g. "session:0.1" or "%3").
    /// If set, it is used directly and auto-detection is skipped.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tmux_target: Option<String>,

    /// Whether to auto-detect a pane running `claude` when `tmux_target` is unset.
    pub tmux_autodetect: bool,

    /// Frontend keybinds, surfaced to JS at startup. Values are KeyboardEvent
    /// `key` combos like "Ctrl+P". Unknown actions are ignored by the frontend.
    pub keymap: Keymap,

    /// The embedded Claude Code pane.
    pub agent: Agent,

    /// Chrome the user can drag into a shape and expect to find again.
    pub ui: Ui,
}

/// The embedded Claude Code pane's preferences.
///
/// Both fields are read when the pane is *opened*, not continuously: `position`
/// is applied by the frontend on mount and `permission_mode` reaches the child
/// as a launch flag, so changing either mid-session is a restart, not a live
/// update.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Agent {
    /// Which edge the pane docks to.
    pub position: Position,

    /// The permission mode Claude Code launches in.
    ///
    /// Not settable from a repo-local `.dreamd.toml` — see [`Config::load`].
    pub permission_mode: PermissionMode,

    /// How the agent is drawn.
    pub surface: Surface,
}

/// Which of the two agent surfaces the pane opens.
///
/// The terminal came first and is real Claude Code in xterm.js: its own
/// palette, its own typography, a composer dreamd cannot see into. The native
/// surface is the same agent reached over `stream-json`, drawn with dreamd's own
/// markdown pipeline — which is the point of the whole exercise, so it is the
/// default.
///
/// **`Terminal` is kept, undocumented, as a fallback**, not as a supported
/// choice: a slash command with no native equivalent, or a stream-json shape
/// dreamd has not learned to draw, should cost a config line rather than a
/// release. It goes when nobody reports needing it.
///
/// Settable from a repo-local `.dreamd.toml`, unlike its neighbour: choosing
/// how a conversation is *drawn* is not a decision about what an agent may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Surface {
    #[default]
    Native,
    Terminal,
}

/// Which edge the agent pane docks to. `bottom` is how the pane has always
/// worked; `right` is the tall-window shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Position {
    #[default]
    Bottom,
    Right,
}

/// Claude Code's four permission modes, spelled the way a config file wants to
/// read rather than the way the CLI flag spells them.
///
/// A closed enum rather than a string is what keeps tenet 3 intact: the launch
/// command is a match over four literal `const`s, never a format string with a
/// user value interpolated into it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PermissionMode {
    /// Ask before every edit — Claude Code's own default.
    Default,
    /// Edits go through, everything else still asks. dreamd's default, because
    /// the loop this pane exists for is read → ask → let it edit.
    #[default]
    AcceptEdits,
    /// Plan first, touch nothing.
    Plan,
    /// Ask for nothing at all.
    BypassPermissions,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Ui {
    /// Sidebar width in CSS pixels, as left by the drag handle.
    ///
    /// Clamped on the way in rather than validated: a width outside the range
    /// is a stale or hand-typed number, and the useful answer is the nearest
    /// usable tree, not a rejected config file that takes every other key down
    /// with it.
    #[serde(deserialize_with = "de_tree_width")]
    pub tree_width: u32,

    /// Stack panel width in CSS pixels, as left by its drag handle. Clamped on
    /// the way in for the same reason [`Ui::tree_width`] is.
    #[serde(deserialize_with = "de_stack_width")]
    pub stack_width: u32,

    /// The agent pane's width when it is docked right (`agent.position =
    /// "right"`), in CSS pixels.
    ///
    /// Two numbers rather than one because the pane's drag handle changes
    /// orientation with the dock: the width you left it at on the right is not
    /// a height, and switching position should restore the size you last chose
    /// *there* rather than reinterpret the other one.
    #[serde(deserialize_with = "de_pane_width")]
    pub pane_width: u32,

    /// The agent pane's height when it is docked below the reader, in CSS
    /// pixels. See [`Ui::pane_width`] for why it is its own key.
    #[serde(deserialize_with = "de_pane_height")]
    pub pane_height: u32,

    /// Whether the window draws the native menubar — File / Edit / Help on
    /// Linux, and nothing at all on macOS, where the bar belongs to the
    /// application rather than to the window and cannot be hidden per-window.
    ///
    /// Off by default: on Linux it is a full row of chrome above a reader whose
    /// whole point is the document. Its two accelerators go with it — the bar is
    /// detached, not hidden, for reasons `apply_chrome` in `main.rs` measured —
    /// so the sidebar header's root field is the way to open a folder without
    /// one.
    pub menubar: bool,

    /// Whether the window keeps its native titlebar: the bar the window manager
    /// draws above the window, carrying close / minimize / maximize.
    ///
    /// See [`TITLEBAR_DEFAULT`] for why the default is not the same on every
    /// platform. `ui/index.html`'s `#drag-strip` is what keeps a window with no
    /// titlebar movable, and it predates this setting — it exists for view mode.
    pub titlebar: bool,
}

/// The sidebar's original fixed width, and the range the drag handle may leave
/// it in. Below the minimum the tree is unreadable and the gesture means
/// "collapse" instead; above the maximum it is eating the document.
pub const TREE_WIDTH_DEFAULT: u32 = 260;
pub const TREE_WIDTH_MIN: u32 = 140;
pub const TREE_WIDTH_MAX: u32 = 600;

/// The stack panel and the agent pane, same arrangement as the tree above.
///
/// The three defaults are deliberately modest: all three surfaces open *over*
/// or *beside* the document, and a reader that has to be dismissed before you
/// can read is the failure these sizes are set against. They were 340px, 38% of
/// the window and 40% of it respectively — a right-docked pane on a wide screen
/// took 720px of a document that renders at 700. Every one of them is now a
/// drag away from whatever the reader actually wants, and the drag persists.
pub const STACK_WIDTH_DEFAULT: u32 = 280;
pub const STACK_WIDTH_MIN: u32 = 200;
pub const STACK_WIDTH_MAX: u32 = 720;

pub const PANE_WIDTH_DEFAULT: u32 = 380;
pub const PANE_WIDTH_MIN: u32 = 240;
pub const PANE_WIDTH_MAX: u32 = 1200;

pub const PANE_HEIGHT_DEFAULT: u32 = 240;
pub const PANE_HEIGHT_MIN: u32 = 120;
pub const PANE_HEIGHT_MAX: u32 = 1200;

/// macOS keeps its titlebar; nothing else does.
///
/// This is the one platform difference in the window-chrome settings, and it is
/// a *value* rather than a `cfg` arm around the code that applies it — so both
/// platforms compile and run the same `apply_chrome`, and a macOS user who sets
/// `titlebar = false` by hand gets a frameless window rather than a key that
/// silently does nothing.
///
/// The asymmetry is real, not taste: `tauri.conf.json` asks for
/// `titleBarStyle: "Overlay"` with `hiddenTitle`, so on macOS the traffic
/// lights sit *inside* the reading pane and cost no vertical space. There is no
/// second bar there to reclaim. On Linux the WM stacks a real title bar above
/// the window, directly on top of the menubar — two rows of furniture before
/// the first line of prose.
#[cfg(target_os = "macos")]
pub const TITLEBAR_DEFAULT: bool = true;
#[cfg(not(target_os = "macos"))]
pub const TITLEBAR_DEFAULT: bool = false;

impl Default for Ui {
    fn default() -> Self {
        Self {
            tree_width: TREE_WIDTH_DEFAULT,
            stack_width: STACK_WIDTH_DEFAULT,
            pane_width: PANE_WIDTH_DEFAULT,
            pane_height: PANE_HEIGHT_DEFAULT,
            menubar: false,
            titlebar: TITLEBAR_DEFAULT,
        }
    }
}

fn de_tree_width<'de, D>(de: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(u32::deserialize(de)?.clamp(TREE_WIDTH_MIN, TREE_WIDTH_MAX))
}

fn de_stack_width<'de, D>(de: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(u32::deserialize(de)?.clamp(STACK_WIDTH_MIN, STACK_WIDTH_MAX))
}

fn de_pane_width<'de, D>(de: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(u32::deserialize(de)?.clamp(PANE_WIDTH_MIN, PANE_WIDTH_MAX))
}

fn de_pane_height<'de, D>(de: D) -> Result<u32, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(u32::deserialize(de)?.clamp(PANE_HEIGHT_MIN, PANE_HEIGHT_MAX))
}

/// The user's appearance preference. [`Mode::System`] is not a thing CSS can be
/// sliced for, which is why resolving it produces a [`theme::Scheme`] rather
/// than staying in this type.
///
/// [`theme::Scheme`]: crate::theme::Scheme
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Follow the OS appearance, and keep following it while the app runs.
    #[default]
    System,
    Light,
    Dark,
}

impl Mode {
    /// The appearance to render in. `system` needs the OS's answer, which only
    /// a window can give — see the `.setup()` hook in `main.rs`.
    pub fn resolve(self, system: crate::theme::Scheme) -> crate::theme::Scheme {
        match self {
            Mode::System => system,
            Mode::Light => crate::theme::Scheme::Light,
            Mode::Dark => crate::theme::Scheme::Dark,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Keymap {
    /// Open the Telescope-style file palette.
    pub palette: String,
    /// Previous / next result inside the palette (vim-style).
    pub palette_prev: String,
    pub palette_next: String,
    /// Highlight the current selection.
    pub highlight: String,
    /// Send the stack to the agent.
    pub send_stack: String,
    /// Send the stack down the *tmux* path instead — `send.rs`, a temp file and
    /// `send-keys` into a pane running `claude`.
    ///
    /// Deliberately unbound and deliberately absent from the settings panel's
    /// action list: the embedded pane is the product's send path, and this is
    /// the one that lets you compare the two when the pane misbehaves. Bind it
    /// by hand in `config.toml`; nothing in the UI will ever offer it.
    ///
    /// `Option` rather than an empty string so "never set" is a state the
    /// frontend can test rather than a combo that can never match.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub send_stack_tmux: Option<String>,
    /// Toggle the stack panel.
    pub toggle_stack: String,
    /// Toggle the contents / outline panel.
    pub toggle_outline: String,
    /// Show or hide the embedded Claude Code pane.
    pub toggle_pane: String,
    /// Collapse / restore the sidebar file tree.
    pub toggle_tree: String,
    /// Distraction-free view mode: hide the titlebar, sidebar and side panels
    /// in one flip, leaving only the rendered document.
    pub toggle_view: String,
    /// Jump the reading pane to the top / bottom of the open document.
    pub jump_top: String,
    pub jump_bottom: String,
    /// Open the next / previous markdown file in the sidebar's order.
    pub next_file: String,
    pub prev_file: String,
    /// Set / return to *the* mark. Vim's marks cut down to a single slot, so
    /// both of these are ordinary one-shot combos with no letter argument.
    pub set_mark: String,
    pub jump_mark: String,
    /// Step back and forward through reading positions you were teleported away
    /// from — link clicks, tree and palette opens, outline and mark jumps.
    pub jump_back: String,
    pub jump_forward: String,
    /// Open the find bar, and step through its matches. Frontend-only, over the
    /// one open document — nothing here reaches the fuzzy path index.
    pub find: String,
    pub find_next: String,
    pub find_prev: String,
    /// Copy the assembled stack to the clipboard.
    pub copy_stack: String,
    /// Open the settings panel.
    pub settings: String,
    /// Save the annotation being edited, from inside the textarea.
    pub save_annotation: String,
    /// Also accept a bare `h` for `highlight`, the way the app shipped before
    /// keybinds were configurable. Off if you want `h` back as a plain letter.
    pub quick_highlight: bool,
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            palette: "Ctrl+F".into(),
            palette_prev: "Ctrl+P".into(),
            palette_next: "Ctrl+N".into(),
            highlight: "Ctrl+H".into(),
            send_stack: "Ctrl+Enter".into(),
            send_stack_tmux: None,
            toggle_stack: "Ctrl+O".into(),
            // I for "index"; B is the sidebar, the way every editor spells it.
            toggle_outline: "Ctrl+I".into(),
            // T for terminal. Free here, and unlike the editors' `Ctrl+``,
            // reachable on every layout — backtick is a dead key on several,
            // where it arrives as `e.key === "Dead"` and can never match. This
            // is the one binding checked *above* the `isEditable` guard and
            // above the pane's own key handling, so it both opens the pane and
            // gets you back out of it.
            toggle_pane: "Ctrl+T".into(),
            toggle_tree: "Ctrl+B".into(),
            // M for "minimal". Ctrl+M is free here and unclaimed by the
            // webview; the macOS menubar's Cmd-chords can't reach it because
            // `matchCombo` requires an exact modifier match.
            toggle_view: "Ctrl+M".into(),
            // Vim's `gg`/`G` would be the obvious pair, but `gg` is a two-key
            // sequence and `matchCombo` only knows single combos — see
            // docs/plans/jump-top-bottom-keybind.md. `Home`/`End` are single
            // keys with the same effect, and do nothing else in the app, so
            // they cost no keyspace; a vim user rebinds jump_bottom to
            // "Shift+G" in one line.
            jump_top: "Home".into(),
            jump_bottom: "End".into(),
            // `]`/`[` is the near-universal "next/prev thing" convention, and
            // unlike a bare letter it costs no typing keyspace a reader wants:
            // the dispatch sits below the `isEditable` and overlay guards, so
            // the annotation box and the palette input never see these. They
            // are the only bare-punctuation defaults; rebind if your layout
            // buries the brackets behind a modifier.
            next_file: "]".into(),
            prev_file: "[".into(),
            // Vim's marks. `m` is a bare letter, which is a real claim on the
            // keyspace — but the same claim `quick_highlight`'s bare `h` has
            // made since before keybinds were configurable, and `m` does
            // nothing else in a reader.
            //
            // `'` rather than vim's other spelling, `` ` ``: both are correct
            // vim (`'a` jumps to the line, `` `a `` to the column — a
            // distinction a reader has no use for), and backtick is a dead key
            // on several international layouts, where it arrives as
            // `e.key === "Dead"` and can never match. Rebind in one line if you
            // want it.
            //
            // Neither takes a letter: dreamd keeps one mark, not twenty-six.
            set_mark: "m".into(),
            jump_mark: "'".into(),
            // Vim's jumplist keys, `Ctrl+O`/`Ctrl+I`, are both already spoken
            // for — the stack panel and the outline panel. These are one
            // modifier away from the bare `]`/`[` above and read as the same
            // motion: brackets step, Ctrl-brackets step through history.
            // `Ctrl+[` is `Esc` in a terminal, which would matter if dreamd
            // ever grew a terminal surface; it is a webview and does not.
            jump_back: "Ctrl+[".into(),
            jump_forward: "Ctrl+]".into(),
            // Vim's search keys, unchanged. `/` joins `]` and `[` as bare
            // punctuation and `n`/`N` join `h`, `m` and `'` as bare letters —
            // all of which dispatch below the `isEditable` guard, so none of
            // them can reach a reader typing into a field, the find input very
            // much included. `Shift+N` rather than bare `N` because
            // `matchCombo` requires an exact modifier match: without the
            // modifier named, `N` could never match the event that produces it.
            find: "/".into(),
            find_next: "n".into(),
            find_prev: "Shift+N".into(),
            copy_stack: "Ctrl+C".into(),
            settings: "Ctrl+,".into(),
            save_annotation: "Ctrl+Y".into(),
            quick_highlight: true,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: None,
            theme_css: None,
            mode: None,
            extra_ignores: Vec::new(),
            tmux_target: None,
            tmux_autodetect: true,
            keymap: Keymap::default(),
            agent: Agent::default(),
            ui: Ui::default(),
        }
    }
}

impl Config {
    /// The effective appearance preference, defaulting to following the OS.
    pub fn mode(&self) -> Mode {
        self.mode.unwrap_or_default()
    }

    /// Load the global config, then overlay a repo-local `.dreamd.toml` if present.
    pub fn load(repo_root: &Path) -> Self {
        let mut merged = global_table();
        if let Some(mut local) = read_table(&local_path(repo_root)) {
            for (key, why) in strip_untrusted(&mut local) {
                eprintln!(
                    "dreamd: ignoring {key} in {} — {why}",
                    local_path(repo_root).display()
                );
            }
            // A repo that names a theme means that theme, not the global
            // file's `theme_css` still winning on a technicality.
            if local.contains_key("theme") {
                merged.remove("theme_css");
            }
            deep_merge(&mut merged, local);
        }
        Config::deserialize(Value::Table(merged)).unwrap_or_else(|e| {
            eprintln!("dreamd: ignoring invalid config ({e})");
            Config::default()
        })
    }
}

/// Remove the keys a repo-local `.dreamd.toml` may not set, returning a
/// `(key, why)` pair for each one removed so the caller can name the file it
/// came from.
///
/// `.dreamd.toml` is repo content, and repo content is untrusted (tenet 4) —
/// you get it by cloning. Four keys are more than a preference:
///
/// - `theme_css` reads an arbitrary file and injects it into the webview as a
///   stylesheet, where a `background-image: url(https://…)` would turn that
///   into a read-and-exfiltrate primitive. A repo may still pick a *named*
///   `theme`, which can only resolve to a bundled palette or one the user wrote.
/// - `agent.permission_mode` chooses how much the agent may do without asking.
///   A cloned repo that could set `bypass-permissions` would be deciding that
///   on your behalf, before you had read a line of it.
/// - `ui.menubar` and `ui.titlebar` are the window's own frame. `titlebar =
///   false` takes away the close button, which is a thing a cloned repo should
///   not be able to do to a window — and unlike the keys below, no amount of it
///   is undone by moving to another repo, because the reader has to find the
///   settings panel to get the frame back. The pair travels together: one key
///   deciding your chrome is the shape of the problem, not the direction.
///
/// `agent.position`, `agent.surface` and the four `ui` sizes — `tree_width`,
/// `stack_width`, `pane_width`, `pane_height` — are left alone: they move
/// furniture *inside* the window and read nothing. `surface` in particular
/// only chooses how a conversation is *drawn* — neither of its two values
/// widens what the agent may do, because the permission gate is a hook and
/// outranks the surface entirely.
fn strip_untrusted(local: &mut Table) -> Vec<(&'static str, &'static str)> {
    let mut warnings = Vec::new();
    if local.remove("theme_css").is_some() {
        warnings.push(("theme_css", "repo-local config may only set `theme`"));
    }
    if let Some(Value::Table(agent)) = local.get_mut("agent") {
        if agent.remove("permission_mode").is_some() {
            warnings.push((
                "agent.permission_mode",
                "a repo does not choose what your agent may do unasked",
            ));
        }
    }
    if let Some(Value::Table(ui)) = local.get_mut("ui") {
        const WHY: &str = "a repo does not choose what your window frame looks like";
        if ui.remove("menubar").is_some() {
            warnings.push(("ui.menubar", WHY));
        }
        if ui.remove("titlebar").is_some() {
            warnings.push(("ui.titlebar", WHY));
        }
    }
    warnings
}

// ---- paths ---------------------------------------------------------------

pub fn global_path() -> PathBuf {
    config_dir().join("config.toml")
}

pub fn local_path(repo_root: &Path) -> PathBuf {
    repo_root.join(".dreamd.toml")
}

/// `~/.config/dreamd`. Everything dreamd persists lives under here.
///
/// `dirs::config_dir()` is deliberately the *last* resort: on macOS it resolves
/// to `~/Library/Application Support`, which is not where a tmux + Neovim user
/// looks and not what the README promises. XDG first, then `~/.config`, then
/// the platform answer.
pub fn config_dir() -> PathBuf {
    if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME").filter(|v| !v.is_empty()) {
        return PathBuf::from(xdg).join("dreamd");
    }
    if let Some(home) = dirs::home_dir() {
        return home.join(".config").join("dreamd");
    }
    dirs::config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("dreamd")
}

// ---- raw table access ----------------------------------------------------

/// The global config file as a table, or an empty one if it is missing or
/// unparseable.
pub fn global_table() -> Table {
    read_table(&global_path()).unwrap_or_default()
}

fn read_table(path: &Path) -> Option<Table> {
    let text = std::fs::read_to_string(path).ok()?;
    match text.parse::<Table>() {
        Ok(t) => Some(t),
        Err(e) => {
            eprintln!("dreamd: ignoring invalid config {}: {e}", path.display());
            None
        }
    }
}

/// Write a table to the global config path, creating the directory if needed.
pub fn write_global(table: &Table) -> std::io::Result<()> {
    let path = global_path();
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let body = toml::to_string_pretty(table)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    // Write-then-rename: a crash mid-write must not truncate a config the user
    // hand-maintained.
    let tmp = path.with_extension("toml.tmp");
    std::fs::write(&tmp, format!("{HEADER}{body}"))?;
    std::fs::rename(&tmp, &path)
}

/// Apply `patch` on top of the global config and save it. The result must
/// still deserialize into a `Config`, so a bad value is rejected rather than
/// written out and ignored on next start.
pub fn patch_global(patch: Table) -> Result<Config, String> {
    let mut table = global_table();
    deep_merge(&mut table, patch);
    let cfg =
        Config::deserialize(Value::Table(table.clone())).map_err(|e| format!("rejected: {e}"))?;
    write_global(&table).map_err(|e| format!("cannot write {}: {e}", global_path().display()))?;
    Ok(cfg)
}

/// Set one dotted key (`keymap.palette`) in the global config and save.
pub fn set_global_key(key: &str, value: Value) -> Result<Config, String> {
    patch_global(nest(key, value))
}

/// Build the single-entry table a dotted key describes: `a.b = v` becomes
/// `{a: {b: v}}`.
fn nest(key: &str, value: Value) -> Table {
    let mut parts: Vec<&str> = key.split('.').collect();
    let leaf = parts.pop().unwrap_or(key);
    let mut inner = Table::new();
    inner.insert(leaf.to_string(), value);
    for part in parts.into_iter().rev() {
        let mut outer = Table::new();
        outer.insert(part.to_string(), Value::Table(inner));
        inner = outer;
    }
    inner
}

/// Look up a dotted key in a table.
pub fn get_key<'a>(table: &'a Table, key: &str) -> Option<&'a Value> {
    let mut cur = table;
    let mut parts = key.split('.').peekable();
    while let Some(part) = parts.next() {
        let value = cur.get(part)?;
        if parts.peek().is_none() {
            return Some(value);
        }
        cur = value.as_table()?;
    }
    None
}

/// Every dotted leaf key in a table, e.g. `["theme", "keymap.palette"]`.
pub fn flat_keys(table: &Table) -> Vec<String> {
    fn walk(table: &Table, prefix: &str, out: &mut Vec<String>) {
        for (k, v) in table {
            let path = if prefix.is_empty() {
                k.clone()
            } else {
                format!("{prefix}.{k}")
            };
            match v.as_table() {
                Some(inner) => walk(inner, &path, out),
                None => out.push(path),
            }
        }
    }
    let mut out = Vec::new();
    walk(table, "", &mut out);
    out
}

/// The keys a repo-local `.dreamd.toml` sets, so the settings panel can flag a
/// value it would save to the global file but that this repo shadows.
pub fn local_override_keys(repo_root: &Path) -> Vec<String> {
    read_table(&local_path(repo_root))
        .map(|t| flat_keys(&t))
        .unwrap_or_default()
}

/// Interpret a CLI value the way TOML would (`true`, `12`, `"quoted"`), and
/// fall back to a bare string so `dreamd config set keymap.palette Ctrl+Space`
/// does the obvious thing.
pub fn parse_value(raw: &str) -> Value {
    match format!("v = {raw}").parse::<Table>() {
        Ok(t) => t.get("v").cloned().unwrap_or_else(|| raw.into()),
        Err(_) => raw.into(),
    }
}

/// Overlay `over` onto `base`, recursing into sub-tables so a local file that
/// sets one keybind does not blank the rest of `[keymap]`.
fn deep_merge(base: &mut Table, over: Table) {
    for (key, value) in over {
        match (base.get_mut(&key), value) {
            (Some(Value::Table(existing)), Value::Table(incoming)) => {
                deep_merge(existing, incoming);
            }
            (_, value) => {
                base.insert(key, value);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    //! Only the pure table plumbing is exercised here. Anything reaching
    //! `config_dir()` reads the *real* `~/.config/dreamd` — `config_check.rs`
    //! owns those paths, because it can sandbox `XDG_CONFIG_HOME` for the whole
    //! process without racing tests running in parallel threads.
    use super::*;
    use crate::theme::Scheme;

    fn table(toml: &str) -> Table {
        toml.parse::<Table>().expect("valid toml fixture")
    }

    // ---- deep_merge -------------------------------------------------------

    #[test]
    fn a_local_file_that_never_mentions_a_key_leaves_it_alone() {
        // The regression this whole module is shaped around: merging at the
        // struct level made "absent" and "defaulted" indistinguishable, so a
        // `.dreamd.toml` setting only `theme` blanked the global `[keymap]`.
        let mut base = table(
            "tmux_autodetect = false\n\
             [keymap]\n\
             palette = \"Ctrl+Space\"\n\
             highlight = \"Ctrl+H\"\n",
        );
        deep_merge(&mut base, table("theme = \"nord\"\n"));
        assert_eq!(
            get_key(&base, "keymap.palette").and_then(|v| v.as_str()),
            Some("Ctrl+Space")
        );
        assert_eq!(
            get_key(&base, "tmux_autodetect").and_then(|v| v.as_bool()),
            Some(false)
        );
        assert_eq!(
            get_key(&base, "theme").and_then(|v| v.as_str()),
            Some("nord")
        );
    }

    #[test]
    fn merging_recurses_into_sub_tables_key_by_key() {
        let mut base = table("[keymap]\npalette = \"Ctrl+Space\"\nhighlight = \"Ctrl+H\"\n");
        deep_merge(&mut base, table("[keymap]\nhighlight = \"Ctrl+B\"\n"));
        assert_eq!(
            get_key(&base, "keymap.palette").and_then(|v| v.as_str()),
            Some("Ctrl+Space"),
            "an untouched sibling keybind was lost"
        );
        assert_eq!(
            get_key(&base, "keymap.highlight").and_then(|v| v.as_str()),
            Some("Ctrl+B")
        );
    }

    #[test]
    fn a_non_table_value_is_replaced_wholesale() {
        // Arrays are values, not tables — a local `extra_ignores` overrides
        // rather than appending to the global one.
        let mut base = table("extra_ignores = [\"a\", \"b\"]\n");
        deep_merge(&mut base, table("extra_ignores = [\"c\"]\n"));
        let got = get_key(&base, "extra_ignores").and_then(|v| v.as_array());
        assert_eq!(got.map(|a| a.len()), Some(1));
    }

    #[test]
    fn a_table_replacing_a_scalar_does_not_recurse() {
        let mut base = table("keymap = \"nonsense\"\n");
        deep_merge(&mut base, table("[keymap]\npalette = \"Ctrl+P\"\n"));
        assert_eq!(
            get_key(&base, "keymap.palette").and_then(|v| v.as_str()),
            Some("Ctrl+P")
        );
    }

    // ---- nest / get_key / flat_keys --------------------------------------

    #[test]
    fn a_dotted_key_nests_into_tables() {
        let t = nest("keymap.palette", "Ctrl+P".into());
        assert_eq!(
            get_key(&t, "keymap.palette").and_then(|v| v.as_str()),
            Some("Ctrl+P")
        );
        // A key with no dots is a plain leaf.
        let t = nest("theme", "nord".into());
        assert_eq!(get_key(&t, "theme").and_then(|v| v.as_str()), Some("nord"));
    }

    #[test]
    fn get_key_returns_none_rather_than_walking_off_a_scalar() {
        let t = table("theme = \"nord\"\n[keymap]\npalette = \"Ctrl+P\"\n");
        assert!(get_key(&t, "theme.deeper").is_none());
        assert!(get_key(&t, "keymap.missing").is_none());
        assert!(get_key(&t, "nothing").is_none());
        // A sub-table is itself addressable.
        assert!(get_key(&t, "keymap").and_then(|v| v.as_table()).is_some());
    }

    #[test]
    fn flat_keys_lists_leaves_not_tables() {
        let t = table("theme = \"nord\"\n[keymap]\npalette = \"Ctrl+P\"\nhighlight = \"Ctrl+H\"\n");
        let mut keys = flat_keys(&t);
        keys.sort();
        assert_eq!(keys, vec!["keymap.highlight", "keymap.palette", "theme"]);
    }

    // ---- parse_value ------------------------------------------------------

    #[test]
    fn a_cli_value_is_parsed_as_toml_when_it_can_be() {
        assert_eq!(parse_value("true").as_bool(), Some(true));
        assert_eq!(parse_value("42").as_integer(), Some(42));
        assert_eq!(parse_value("\"nord\"").as_str(), Some("nord"));
        assert_eq!(
            parse_value("[\"a\", \"b\"]").as_array().map(|a| a.len()),
            Some(2)
        );
    }

    #[test]
    fn an_unparseable_value_falls_back_to_a_bare_string() {
        // `dreamd config set theme nord` — no quotes typed, and none needed.
        assert_eq!(parse_value("nord").as_str(), Some("nord"));
        assert_eq!(parse_value("Ctrl+Space").as_str(), Some("Ctrl+Space"));
        assert_eq!(parse_value("").as_str(), Some(""));
    }

    // ---- Mode -------------------------------------------------------------

    #[test]
    fn system_mode_defers_to_the_os_and_the_others_do_not() {
        assert_eq!(Mode::System.resolve(Scheme::Light), Scheme::Light);
        assert_eq!(Mode::System.resolve(Scheme::Dark), Scheme::Dark);
        assert_eq!(Mode::Light.resolve(Scheme::Dark), Scheme::Light);
        assert_eq!(Mode::Dark.resolve(Scheme::Light), Scheme::Dark);
    }

    #[test]
    fn mode_defaults_to_system() {
        assert_eq!(Mode::default(), Mode::System);
    }

    // ---- agent / ui -------------------------------------------------------

    /// What `Config::load` does after the two files are merged, minus the
    /// filesystem — `config_dir()` is `config_check`'s to touch, not a unit
    /// test's.
    fn config_of(toml: &str) -> Config {
        Config::deserialize(Value::Table(table(toml))).expect("valid config fixture")
    }

    #[test]
    fn the_agent_and_ui_keys_default_without_being_written() {
        let cfg = config_of("");
        assert_eq!(cfg.agent.position, Position::Bottom);
        assert_eq!(cfg.agent.permission_mode, PermissionMode::AcceptEdits);
        assert_eq!(cfg.ui.tree_width, TREE_WIDTH_DEFAULT);
    }

    #[test]
    fn the_new_keys_layer_global_under_local_like_every_other_key() {
        let mut base = table(
            "[agent]\nposition = \"bottom\"\npermission_mode = \"plan\"\n\
             [ui]\ntree_width = 300\n",
        );
        deep_merge(&mut base, table("[agent]\nposition = \"right\"\n"));
        let cfg = Config::deserialize(Value::Table(base)).expect("merged config");
        assert_eq!(cfg.agent.position, Position::Right, "local value applies");
        assert_eq!(
            cfg.agent.permission_mode,
            PermissionMode::Plan,
            "an untouched sibling in the same section was lost"
        );
        assert_eq!(cfg.ui.tree_width, 300, "an untouched section was lost");
    }

    #[test]
    fn a_local_file_mentioning_only_agent_does_not_wipe_the_global_keymap() {
        // The `deep_merge` regression, re-asserted for a new section: a
        // sub-table arriving from the local file must not replace a sibling
        // sub-table it never mentioned.
        let mut base = table("[keymap]\npalette = \"Ctrl+Space\"\n");
        deep_merge(&mut base, table("[agent]\nposition = \"right\"\n"));
        let cfg = Config::deserialize(Value::Table(base)).expect("merged config");
        assert_eq!(cfg.keymap.palette, "Ctrl+Space");
        assert_eq!(cfg.agent.position, Position::Right);
    }

    #[test]
    fn a_repo_may_move_the_pane_but_not_choose_its_permissions() {
        let mut local = table(
            "[agent]\nposition = \"right\"\npermission_mode = \"bypass-permissions\"\n\
             [ui]\ntree_width = 200\n",
        );
        let warnings = strip_untrusted(&mut local);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(warnings[0].0, "agent.permission_mode");

        let cfg = Config::deserialize(Value::Table(local)).expect("stripped config");
        assert_eq!(
            cfg.agent.permission_mode,
            PermissionMode::AcceptEdits,
            "a repo chose the permission mode"
        );
        assert_eq!(cfg.agent.position, Position::Right, "position is harmless");
        assert_eq!(cfg.ui.tree_width, 200, "tree_width is harmless");
    }

    #[test]
    fn stripping_leaves_a_local_file_that_oversteps_nothing_alone() {
        let mut local = table("theme = \"nord\"\n[agent]\nposition = \"right\"\n");
        assert!(strip_untrusted(&mut local).is_empty());
        assert_eq!(local.len(), 2);
    }

    #[test]
    fn a_repo_may_choose_how_a_conversation_is_drawn_but_not_what_it_may_do() {
        // `surface` sits in the same table as `permission_mode` and is
        // deliberately *not* stripped beside it. Neither of its values widens
        // what the agent may do: the gate is a PreToolUse hook and outranks the
        // surface entirely. This test exists so that "it's in [agent], strip it
        // too" has to argue with something.
        let mut local =
            table("[agent]\nsurface = \"terminal\"\npermission_mode = \"bypass-permissions\"\n");
        let warnings = strip_untrusted(&mut local);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(warnings[0].0, "agent.permission_mode");

        let cfg = Config::deserialize(Value::Table(local)).expect("stripped config");
        assert_eq!(cfg.agent.surface, Surface::Terminal, "surface survives");
        assert_eq!(
            cfg.agent.permission_mode,
            PermissionMode::default(),
            "the mode does not"
        );
    }

    #[test]
    fn the_native_surface_is_the_default() {
        assert_eq!(Config::default().agent.surface, Surface::Native);
        assert_eq!(
            config_of("[agent]\nsurface = \"terminal\"\n").agent.surface,
            Surface::Terminal
        );
    }

    #[test]
    fn theme_css_is_still_stripped_and_still_says_so() {
        let mut local = table("theme_css = \"/etc/passwd\"\n");
        let warnings = strip_untrusted(&mut local);
        assert_eq!(warnings.len(), 1);
        assert_eq!(warnings[0].0, "theme_css");
        assert!(local.is_empty());
    }

    #[test]
    fn a_repo_may_resize_the_tree_but_not_take_away_the_close_button() {
        let mut local = table("[ui]\ntree_width = 200\nmenubar = true\ntitlebar = false\n");
        let warnings = strip_untrusted(&mut local);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert_eq!(warnings[0].0, "ui.menubar");
        assert_eq!(warnings[1].0, "ui.titlebar");

        let cfg = Config::deserialize(Value::Table(local)).expect("stripped config");
        assert!(!cfg.ui.menubar, "a repo turned the menubar on");
        assert_eq!(
            cfg.ui.titlebar, TITLEBAR_DEFAULT,
            "a repo chose the window frame"
        );
        assert_eq!(cfg.ui.tree_width, 200, "tree_width is still harmless");
    }

    #[test]
    fn the_window_chrome_defaults_are_off_except_the_mac_titlebar() {
        // The pair the settings panel toggles, pinned so a change to either
        // default is a deliberate edit here. `menubar` is unconditional
        // because `hide_menu` is a documented no-op on macOS — the bar there
        // belongs to the app, not the window, and this key never touches it.
        let ui = Ui::default();
        assert!(!ui.menubar);
        assert_eq!(ui.titlebar, cfg!(target_os = "macos"));
    }

    #[test]
    fn a_chrome_toggle_from_the_panel_arrives_as_a_bool() {
        // `renderWindow`'s exact path, for the same reason
        // `a_width_from_the_drag_handle_arrives_as_an_integer` exists: a JS
        // boolean has to survive the JSON -> TOML hop, or the toggle would be
        // rejected by `patch_global` and silently snap back.
        let patch: Table =
            Table::deserialize(serde_json::json!({"ui": {"menubar": true, "titlebar": true}}))
                .expect("json patch");
        let mut table = Table::new();
        deep_merge(&mut table, patch);
        let cfg = Config::deserialize(Value::Table(table)).expect("deserialize");
        assert!(cfg.ui.menubar);
        assert!(cfg.ui.titlebar);
    }

    #[test]
    fn tree_width_is_clamped_on_the_way_in_rather_than_rejected() {
        assert_eq!(config_of("[ui]\ntree_width = 10\n").ui.tree_width, 140);
        assert_eq!(config_of("[ui]\ntree_width = 0\n").ui.tree_width, 140);
        assert_eq!(config_of("[ui]\ntree_width = 99999\n").ui.tree_width, 600);
        assert_eq!(config_of("[ui]\ntree_width = 320\n").ui.tree_width, 320);
    }

    #[test]
    fn the_pane_and_stack_sizes_are_clamped_the_same_way() {
        // Same rule as the tree, and pinned separately because each one has its
        // own deserializer: a range copied onto the wrong key is exactly the
        // kind of mistake a shared test would not catch.
        let ui = |s: &str| config_of(s).ui;
        assert_eq!(ui("[ui]\nstack_width = 1\n").stack_width, STACK_WIDTH_MIN);
        assert_eq!(
            ui("[ui]\nstack_width = 99999\n").stack_width,
            STACK_WIDTH_MAX
        );
        assert_eq!(ui("[ui]\nstack_width = 300\n").stack_width, 300);

        assert_eq!(ui("[ui]\npane_width = 1\n").pane_width, PANE_WIDTH_MIN);
        assert_eq!(ui("[ui]\npane_width = 99999\n").pane_width, PANE_WIDTH_MAX);
        assert_eq!(ui("[ui]\npane_width = 500\n").pane_width, 500);

        assert_eq!(ui("[ui]\npane_height = 1\n").pane_height, PANE_HEIGHT_MIN);
        assert_eq!(
            ui("[ui]\npane_height = 99999\n").pane_height,
            PANE_HEIGHT_MAX
        );
        assert_eq!(ui("[ui]\npane_height = 300\n").pane_height, 300);
    }

    #[test]
    fn the_pane_keeps_a_width_and_a_height_rather_than_one_size() {
        // The dock decides which of the two a drag writes, so switching
        // `agent.position` has to find the size last left on *that* edge. One
        // shared key would make a tall bottom pane into a wide right one.
        let cfg = config_of("[ui]\npane_width = 500\npane_height = 300\n");
        assert_eq!(cfg.ui.pane_width, 500);
        assert_eq!(cfg.ui.pane_height, 300);
    }

    #[test]
    fn a_width_from_the_drag_handle_arrives_as_an_integer() {
        // The exact path `set_config` takes: a Tauri command argument is
        // deserialized out of a `serde_json::Value`, so a JS number has to
        // land in the table as a TOML *integer* — a float would be rejected by
        // `patch_global` after the drag, at which point the tree would silently
        // stop remembering its width.
        let patch: Table =
            Table::deserialize(serde_json::json!({"ui": {"tree_width": 320}})).expect("json patch");
        let mut table = Table::new();
        deep_merge(&mut table, patch);
        let cfg = Config::deserialize(Value::Table(table)).expect("deserialize");
        assert_eq!(cfg.ui.tree_width, 320);

        // And the other half of the same rule: a fractional width is refused
        // rather than rounded, which is why the drag rounds before it sends.
        let fractional = Table::deserialize(serde_json::json!({"ui": {"tree_width": 320.5}}))
            .expect("json patch");
        assert!(Config::deserialize(Value::Table(fractional)).is_err());
    }

    #[test]
    fn an_unknown_position_or_permission_mode_is_rejected() {
        // Not clamped, unlike a width: there is no nearest sensible edge, and a
        // typo that silently ran the agent in the wrong mode is the failure
        // this enum exists to prevent.
        assert!(Config::deserialize(Value::Table(table("[agent]\nposition = \"top\"\n"))).is_err());
        assert!(Config::deserialize(Value::Table(table(
            "[agent]\npermission_mode = \"acceptEdits\"\n"
        )))
        .is_err());
    }

    #[test]
    fn every_permission_mode_round_trips_through_its_config_spelling() {
        for (text, want) in [
            ("default", PermissionMode::Default),
            ("accept-edits", PermissionMode::AcceptEdits),
            ("plan", PermissionMode::Plan),
            ("bypass-permissions", PermissionMode::BypassPermissions),
        ] {
            let cfg = config_of(&format!("[agent]\npermission_mode = \"{text}\"\n"));
            assert_eq!(cfg.agent.permission_mode, want);
            assert_eq!(
                toml::Value::try_from(want).expect("serialize").as_str(),
                Some(text),
                "the value dreamd writes back is not the one it reads"
            );
        }
    }

    // ---- the hidden tmux keybind -----------------------------------------

    #[test]
    fn the_tmux_send_keybind_is_unbound_by_default() {
        assert_eq!(Keymap::default().send_stack_tmux, None);
        // And is not written out, so "reset all shortcuts" — which patches the
        // global file with `default_keymap()` — cannot clear a binding the user
        // set by hand.
        let text = toml::to_string(&Keymap::default()).expect("serialize");
        assert!(
            !text.contains("send_stack_tmux"),
            "the hidden binding leaked into a written keymap:\n{text}"
        );
    }

    #[test]
    fn the_tmux_send_keybind_is_read_when_a_user_binds_it_by_hand() {
        let cfg = config_of("[keymap]\nsend_stack_tmux = \"Ctrl+Alt+Enter\"\n");
        assert_eq!(
            cfg.keymap.send_stack_tmux.as_deref(),
            Some("Ctrl+Alt+Enter")
        );
        assert_eq!(cfg.keymap.send_stack, "Ctrl+Enter", "a sibling was lost");
    }
}
