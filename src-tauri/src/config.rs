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
    /// Toggle the stack panel.
    pub toggle_stack: String,
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
            toggle_stack: "Ctrl+O".into(),
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
            // `.dreamd.toml` is repo content, and repo content is untrusted
            // (tenet 4) — you get it by cloning. `theme_css` reads an arbitrary
            // file and injects it into the webview as a stylesheet, where a
            // `background-image: url(https://…)` would turn that into a
            // read-and-exfiltrate primitive. A repo may pick a *named* theme,
            // which can only resolve to a bundled palette or one the user
            // wrote themselves.
            if local.remove("theme_css").is_some() {
                eprintln!(
                    "dreamd: ignoring theme_css in {} — repo-local config may only set `theme`",
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
    let cfg = Config::deserialize(Value::Table(table.clone()))
        .map_err(|e| format!("rejected: {e}"))?;
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
