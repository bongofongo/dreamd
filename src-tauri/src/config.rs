//! Config loading: global `~/.config/dreamd/config.toml` with a repo-local
//! `.dreamd.toml` override. Everything is optional; sane defaults are used.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    /// Path to the user-editable theme CSS. Hot-reloaded by the watcher.
    /// If unset or missing, the bundled default `theme.css` is used.
    pub theme_css: Option<PathBuf>,

    /// Extra glob-ish ignore patterns beyond `.gitignore`/`.ignore`.
    pub extra_ignores: Vec<String>,

    /// tmux target pane for send-to-Claude (e.g. "session:0.1" or "%3").
    /// If set, it is used directly and auto-detection is skipped.
    pub tmux_target: Option<String>,

    /// Whether to auto-detect a pane running `claude` when `tmux_target` is unset.
    pub tmux_autodetect: bool,

    /// Frontend keybinds, surfaced to JS at startup. Values are KeyboardEvent
    /// `key` combos like "Ctrl+P". Unknown actions are ignored by the frontend.
    pub keymap: Keymap,
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
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme_css: None,
            extra_ignores: Vec::new(),
            tmux_target: None,
            tmux_autodetect: true,
            keymap: Keymap::default(),
        }
    }
}

impl Config {
    /// Load the global config, then overlay a repo-local `.dreamd.toml` if present.
    pub fn load(repo_root: &Path) -> Self {
        let mut cfg = Self::read_file(&Self::global_path()).unwrap_or_default();
        if let Some(local) = Self::read_file(&repo_root.join(".dreamd.toml")) {
            cfg.merge(local);
        }
        cfg
    }

    fn global_path() -> PathBuf {
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("dreamd")
            .join("config.toml")
    }

    fn read_file(path: &Path) -> Option<Config> {
        let text = std::fs::read_to_string(path).ok()?;
        match toml::from_str::<Config>(&text) {
            Ok(c) => Some(c),
            Err(e) => {
                eprintln!("dreamd: ignoring invalid config {}: {e}", path.display());
                None
            }
        }
    }

    /// Repo-local values win over global where explicitly set.
    fn merge(&mut self, local: Config) {
        if local.theme_css.is_some() {
            self.theme_css = local.theme_css;
        }
        if !local.extra_ignores.is_empty() {
            self.extra_ignores = local.extra_ignores;
        }
        if local.tmux_target.is_some() {
            self.tmux_target = local.tmux_target;
        }
        self.tmux_autodetect = local.tmux_autodetect;
        self.keymap = local.keymap;
    }
}
