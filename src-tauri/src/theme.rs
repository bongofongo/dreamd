//! Themes: the registry of palettes, resolving the active stylesheet, and
//! pulling out the two values that are not CSS at all — the window background
//! and the syntect theme for code blocks.
//!
//! A theme is split in two. `ui/theme.css` holds the *rules* and is embedded as
//! [`BASE_CSS`]; a *palette* is a bare `:root { --bg: … }` block, either bundled
//! (`ui/themes/*.css`) or written by the user into `~/.config/dreamd/themes/`.
//! The palette is appended after the base so its custom properties win. Setting
//! `theme_css` to a path opts out of the split entirely: that file is used
//! alone, with nothing appended, which is what a hand-written full stylesheet
//! wants.
//!
//! The resolved CSS is injected into `#user-theme` from JS, several IPC
//! round-trips into boot, so `--bg` does not exist while the window is first
//! painted. The window and webview are given the parsed background up front so
//! that gap reads as the theme's colour instead of a white flash. Parsing the
//! user's own file (tenet 5) rather than hardcoding a colour means a light
//! theme flashes light, not dark.

use crate::config::{config_dir, Config};
use serde::Serialize;
use std::path::PathBuf;

/// The base reading stylesheet, embedded so it works in a packaged binary.
pub const BASE_CSS: &str = include_str!("../../ui/theme.css");

/// Palettes that ship in the binary. The first entry is the default.
pub const BUNDLED: &[(&str, &str)] = &[
    ("dreamd", include_str!("../../ui/themes/dreamd.css")),
    ("gruvbox-dark", include_str!("../../ui/themes/gruvbox-dark.css")),
    (
        "catppuccin-mocha",
        include_str!("../../ui/themes/catppuccin-mocha.css"),
    ),
    ("tokyo-night", include_str!("../../ui/themes/tokyo-night.css")),
    ("nord", include_str!("../../ui/themes/nord.css")),
    (
        "gruvbox-light",
        include_str!("../../ui/themes/gruvbox-light.css"),
    ),
    (
        "catppuccin-latte",
        include_str!("../../ui/themes/catppuccin-latte.css"),
    ),
    (
        "solarized-light",
        include_str!("../../ui/themes/solarized-light.css"),
    ),
    (
        "high-contrast-dark",
        include_str!("../../ui/themes/high-contrast-dark.css"),
    ),
    (
        "high-contrast-light",
        include_str!("../../ui/themes/high-contrast-light.css"),
    ),
];

pub const DEFAULT_THEME: &str = BUNDLED[0].0;

/// Where user palettes live. Saving one here shadows a bundled theme of the
/// same name.
pub fn user_dir() -> PathBuf {
    config_dir().join("themes")
}

#[derive(Debug, Clone, Serialize)]
pub struct ThemeInfo {
    pub name: String,
    /// "bundled" or "user" — a user file of the same name wins.
    pub kind: &'static str,
    /// Present for user themes, so the panel can point at the file on disk.
    pub path: Option<PathBuf>,
}

/// The active theme, ready to hand to the frontend.
#[derive(Debug, Clone, Serialize)]
pub struct Resolved {
    /// The palette name, or `None` when `theme_css` bypassed the registry.
    pub name: Option<String>,
    pub css: String,
    /// The syntect theme for fenced code blocks, if the CSS named one.
    pub syntax_theme: Option<String>,
}

/// Every theme dreamd can offer, user files first so a shadowed bundled name
/// appears once. Sorted by name.
pub fn list() -> Vec<ThemeInfo> {
    let mut out: Vec<ThemeInfo> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(user_dir()) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("css") {
                continue;
            }
            if let Some(name) = path.file_stem().and_then(|s| s.to_str()) {
                out.push(ThemeInfo {
                    name: name.to_string(),
                    kind: "user",
                    path: Some(path.clone()),
                });
            }
        }
    }
    for (name, _) in BUNDLED {
        if !out.iter().any(|t| t.name == *name) {
            out.push(ThemeInfo {
                name: (*name).to_string(),
                kind: "bundled",
                path: None,
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// A palette's CSS by name: the user's file if there is one, else the bundled
/// copy.
pub fn palette(name: &str) -> Option<String> {
    if let Some(path) = user_path(name) {
        if let Ok(css) = std::fs::read_to_string(&path) {
            return Some(css);
        }
    }
    // Bundled palettes are `include_str!`'d, so in a release build editing
    // `ui/themes/*.css` needs a rebuild while a *user* theme hot-reloads
    // instantly. That asymmetry is confusing while working on the themes
    // themselves, so a dev build reads them off disk first.
    #[cfg(debug_assertions)]
    if let Some(css) = source_palette(name) {
        return Some(css);
    }
    BUNDLED
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, css)| (*css).to_string())
}

#[cfg(debug_assertions)]
fn source_palette(name: &str) -> Option<String> {
    if !BUNDLED.iter().any(|(n, _)| *n == name) {
        return None;
    }
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../ui/themes")
        .join(format!("{name}.css"));
    std::fs::read_to_string(path).ok()
}

/// The full stylesheet for a named palette: base rules plus the palette.
pub fn css_for(name: &str) -> Option<String> {
    palette(name).map(|p| format!("{BASE_CSS}\n{p}"))
}

/// Resolve the active theme from config. `theme_css` wins and is used alone;
/// otherwise the named palette is appended to the base rules, falling back to
/// the default palette when the name is unknown.
pub fn resolve(cfg: &Config) -> Resolved {
    if let Some(path) = &cfg.theme_css {
        if let Ok(css) = std::fs::read_to_string(path) {
            let syntax_theme = syntax_theme(&css);
            return Resolved {
                name: None,
                css,
                syntax_theme,
            };
        }
        eprintln!("dreamd: cannot read theme_css {}", path.display());
    }
    let name = cfg.theme.as_deref().unwrap_or(DEFAULT_THEME);
    let (name, css) = match css_for(name) {
        Some(css) => (name.to_string(), css),
        None => {
            eprintln!("dreamd: unknown theme {name:?}, using {DEFAULT_THEME}");
            (
                DEFAULT_THEME.to_string(),
                css_for(DEFAULT_THEME).unwrap_or_else(|| BASE_CSS.to_string()),
            )
        }
    };
    let syntax_theme = syntax_theme(&css);
    Resolved {
        name: Some(name),
        css,
        syntax_theme,
    }
}

// ---- user palettes -------------------------------------------------------

/// The path a user palette would live at, or `None` if the name could escape
/// the themes directory. Names are file stems, not paths: no separators, no
/// `..`, nothing exotic.
pub fn user_path(name: &str) -> Option<PathBuf> {
    let ok = !name.is_empty()
        && name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.'))
        && name != "."
        && name != "..";
    ok.then(|| user_dir().join(format!("{name}.css")))
}

pub fn save_user(name: &str, css: &str) -> Result<PathBuf, String> {
    let path = user_path(name).ok_or_else(|| format!("invalid theme name {name:?}"))?;
    std::fs::create_dir_all(user_dir()).map_err(|e| format!("cannot create themes dir: {e}"))?;
    std::fs::write(&path, css).map_err(|e| format!("cannot write {}: {e}", path.display()))?;
    Ok(path)
}

pub fn delete_user(name: &str) -> Result<(), String> {
    let path = user_path(name).ok_or_else(|| format!("invalid theme name {name:?}"))?;
    if !path.exists() {
        return Err(format!("no user theme named {name:?}"));
    }
    std::fs::remove_file(&path).map_err(|e| format!("cannot delete {}: {e}", path.display()))
}

// ---- values parsed out of the CSS ----------------------------------------

/// The `--bg` custom property as `(r, g, b)`. Only hex forms (`#rgb`,
/// `#rrggbb`) are understood; anything else yields `None` and the caller keeps
/// whatever default it had.
pub fn background(css: &str) -> Option<(u8, u8, u8)> {
    parse_hex(&custom_property(css, "--bg")?)
}

/// The `--syntax-theme` custom property: the syntect theme for fenced code
/// blocks. Not used by CSS at all — it rides along in the palette so one file
/// describes the whole look. Quotes are stripped.
pub fn syntax_theme(css: &str) -> Option<String> {
    let raw = custom_property(css, "--syntax-theme")?;
    let trimmed = raw.trim_matches(|c| c == '"' || c == '\'').trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The last declaration of a custom property, which is the one CSS would use.
///
/// The match has to start on a property boundary: a bare substring search for
/// `--bg:` would also find `--panel--bg:`, and `--syntax-theme` is the more
/// collidable of the two names we look for.
fn custom_property(css: &str, name: &str) -> Option<String> {
    let css = strip_comments(css);
    let needle = format!("{name}:");
    let mut from = css.len();
    while let Some(at) = css[..from].rfind(&needle) {
        let boundary = css[..at]
            .chars()
            .next_back()
            .map_or(true, |c| !c.is_ascii_alphanumeric() && c != '_' && c != '-');
        if boundary {
            let start = at + needle.len();
            return Some(css[start..].split(';').next()?.trim().to_string());
        }
        from = at;
    }
    None
}

/// Drop `/* ... */` blocks so a commented-out declaration cannot win over the
/// real one.
fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(open) = rest.find("/*") {
        out.push_str(&rest[..open]);
        rest = match rest[open + 2..].find("*/") {
            Some(close) => &rest[open + 2 + close + 2..],
            None => "", // unterminated comment: everything after it is comment
        };
    }
    out.push_str(rest);
    out
}

fn parse_hex(value: &str) -> Option<(u8, u8, u8)> {
    let hex = value.strip_prefix('#')?;
    let pair = |i: usize| u8::from_str_radix(&hex[i..i + 2], 16).ok();
    match hex.len() {
        6 | 8 => Some((pair(0)?, pair(2)?, pair(4)?)),
        3 | 4 => {
            let dup = |i: usize| u8::from_str_radix(&hex[i..i + 1].repeat(2), 16).ok();
            Some((dup(0)?, dup(1)?, dup(2)?))
        }
        _ => None,
    }
}
