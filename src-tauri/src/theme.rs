//! Themes: the registry of palettes, resolving the active stylesheet, and
//! pulling out the two values that are not CSS at all — the window background
//! and the syntect theme for code blocks.
//!
//! A theme is split in two. `ui/theme.css` holds the *rules* and is embedded as
//! [`BASE_CSS`]; a *palette* lives in `ui/themes/*.css` or in the user's
//! `~/.config/dreamd/themes/`, and is appended after the base so its custom
//! properties win. Setting `theme_css` to a path opts out of the split
//! entirely: that file is used alone, with nothing appended, which is what a
//! hand-written full stylesheet wants.
//!
//! A palette is a *family* — one file carrying both appearances:
//!
//! ```css
//! :root                     { /* shared: type metrics */ }
//! :root[data-mode="light"]  { /* colours + --syntax-theme */ }
//! :root[data-mode="dark"]   { /* … */ }
//! ```
//!
//! The frontend switches by setting `data-mode` on `<html>`, so a mode toggle
//! is one attribute rather than a re-fetch. CSS handles that on its own; the
//! two values *this* module has to read out by hand do not, which is what
//! [`mode_slice`] is for. A palette with no `[data-mode]` block at all — every
//! user file written before families existed, and every `theme_css` stylesheet
//! — is passed through untouched and reads identically in both schemes.
//!
//! The resolved CSS is injected into `#user-theme` from JS, several IPC
//! round-trips into boot, so `--bg` does not exist while the window is first
//! painted. The window and webview are given the parsed background up front so
//! that gap reads as the theme's colour instead of a white flash. Parsing the
//! user's own file (tenet 5) rather than hardcoding a colour means a light
//! theme flashes light, not dark.

use crate::config::{config_dir, Config};
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::path::PathBuf;

/// A resolved appearance — what a stylesheet is actually sliced for.
///
/// Deliberately not the same type as [`crate::config::Mode`], which also has
/// `System`. Keeping them apart makes "system has to be resolved before it
/// reaches CSS" a compile error rather than a silently wrong block read.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scheme {
    Light,
    Dark,
}

/// The base reading stylesheet, embedded so it works in a packaged binary.
pub const BASE_CSS: &str = include_str!("../../ui/theme.css");

/// Theme families that ship in the binary, each carrying both appearances. The
/// first entry is the default; the reading themes lead, the programmer-coded
/// ones follow.
pub const BUNDLED: &[(&str, &str)] = &[
    ("dreamd", include_str!("../../ui/themes/dreamd.css")),
    ("manuscript", include_str!("../../ui/themes/manuscript.css")),
    (
        "letterpress",
        include_str!("../../ui/themes/letterpress.css"),
    ),
    ("athenaeum", include_str!("../../ui/themes/athenaeum.css")),
    ("gruvbox", include_str!("../../ui/themes/gruvbox.css")),
    ("catppuccin", include_str!("../../ui/themes/catppuccin.css")),
    ("tokyo-night", include_str!("../../ui/themes/tokyo-night.css")),
    ("nord", include_str!("../../ui/themes/nord.css")),
    ("solarized", include_str!("../../ui/themes/solarized.css")),
    (
        "high-contrast",
        include_str!("../../ui/themes/high-contrast.css"),
    ),
];

pub const DEFAULT_THEME: &str = BUNDLED[0].0;

/// Palette names from before families, each mapping to a family plus the
/// appearance the old name meant. An existing `config.toml` keeps working, and
/// `dreamd theme set <old-name>` rewrites itself into the new spelling.
///
/// Note what is *absent*: `dreamd`, `nord` and `tokyo-night` are family names
/// now, not legacy ones. Listing them here would pin dark for every config that
/// already names them and make `mode = "system"` a no-op for the three
/// most-used themes — the opposite of the point.
pub const ALIASES: &[(&str, &str, Scheme)] = &[
    ("gruvbox-dark", "gruvbox", Scheme::Dark),
    ("gruvbox-light", "gruvbox", Scheme::Light),
    ("catppuccin-mocha", "catppuccin", Scheme::Dark),
    ("catppuccin-latte", "catppuccin", Scheme::Light),
    ("solarized-light", "solarized", Scheme::Light),
    ("high-contrast-dark", "high-contrast", Scheme::Dark),
    ("high-contrast-light", "high-contrast", Scheme::Light),
];

/// The family and appearance a legacy palette name stands for.
pub fn dealias(name: &str) -> Option<(&'static str, Scheme)> {
    ALIASES
        .iter()
        .find(|(alias, _, _)| *alias == name)
        .map(|(_, family, scheme)| (*family, *scheme))
}

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
    /// The appearance this was resolved for. The CSS carries both, but
    /// `syntax_theme` and [`background`] are single-valued and had to pick.
    pub scheme: Scheme,
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
/// copy, else — last of all — the family a legacy name aliases.
///
/// The alias step is deliberately last. Someone who ran
/// `dreamd theme new gruvbox-dark` has a real file of that name, and it must
/// win over the compatibility shim.
pub fn palette(name: &str) -> Option<String> {
    named_palette(name).or_else(|| named_palette(dealias(name)?.0))
}

fn named_palette(name: &str) -> Option<String> {
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

/// The name a theme appears under in [`list`]. A legacy alias reports the
/// family it resolves to; anything with a real palette of its own — including a
/// user file that shadows an alias — reports itself.
fn canonical(name: &str) -> String {
    if named_palette(name).is_some() {
        return name.to_string();
    }
    dealias(name).map_or_else(|| name.to_string(), |(family, _)| family.to_string())
}

/// The appearance to render in, given the config and what the OS says.
///
/// A legacy palette name carries an implicit appearance, and it applies only
/// when `mode` was never set at all: someone who wrote `theme = "gruvbox-dark"`
/// and nothing else meant dark and gets dark, while someone who has since
/// written `mode = "system"` meant system and gets it. That distinction is why
/// `Config::mode` is an `Option`.
pub fn scheme_for(cfg: &Config, system: Scheme) -> Scheme {
    if cfg.mode.is_none() {
        if let Some(name) = &cfg.theme {
            // Only when the alias isn't shadowed by a real theme of that name.
            if named_palette(name).is_none() {
                if let Some((_, scheme)) = dealias(name) {
                    return scheme;
                }
            }
        }
    }
    cfg.mode().resolve(system)
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
pub fn resolve(cfg: &Config, scheme: Scheme) -> Resolved {
    if let Some(path) = &cfg.theme_css {
        if let Ok(css) = std::fs::read_to_string(path) {
            let syntax_theme = syntax_theme(&css, scheme);
            return Resolved {
                name: None,
                css,
                scheme,
                syntax_theme,
            };
        }
        eprintln!("dreamd: cannot read theme_css {}", path.display());
    }
    let name = cfg.theme.as_deref().unwrap_or(DEFAULT_THEME);
    let (name, css) = match css_for(name) {
        // Reported under the name it is *listed* under, so a config still
        // saying `gruvbox-dark` marks the `gruvbox` card as active rather than
        // marking nothing at all.
        Some(css) => (canonical(name), css),
        None => {
            eprintln!("dreamd: unknown theme {name:?}, using {DEFAULT_THEME}");
            (
                DEFAULT_THEME.to_string(),
                css_for(DEFAULT_THEME).unwrap_or_else(|| BASE_CSS.to_string()),
            )
        }
    };
    let syntax_theme = syntax_theme(&css, scheme);
    Resolved {
        name: Some(name),
        css,
        scheme,
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
pub fn background(css: &str, scheme: Scheme) -> Option<(u8, u8, u8)> {
    parse_hex(&custom_property(css, "--bg", scheme)?)
}

/// The `--syntax-theme` custom property: the syntect theme for fenced code
/// blocks. Not used by CSS at all — it rides along in the palette so one file
/// describes the whole look. Quotes are stripped.
pub fn syntax_theme(css: &str, scheme: Scheme) -> Option<String> {
    let raw = custom_property(css, "--syntax-theme", scheme)?;
    let trimmed = raw.trim_matches(|c| c == '"' || c == '\'').trim();
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// Whether a stylesheet declares any `[data-mode]` block — i.e. whether it is a
/// family or a flat pre-family palette. Public so `theme_check` can insist that
/// every bundled palette is one.
pub fn has_mode_blocks(css: &str) -> bool {
    strip_comments(css).contains("data-mode")
}

/// The last declaration of a custom property *in `scheme`*, which is the one
/// CSS would use.
///
/// The match has to start on a property boundary: a bare substring search for
/// `--bg:` would also find `--panel--bg:`, and `--syntax-theme` is the more
/// collidable of the two names we look for.
///
/// Public because `theme_check` should assert against the parser the app
/// actually uses rather than its own approximation of it — a `css.contains`
/// test passes the light pass for a variable only the dark block declares.
pub fn custom_property(css: &str, name: &str, scheme: Scheme) -> Option<String> {
    let css = strip_comments(css);
    let css = mode_slice(&css, scheme);
    let needle = format!("{name}:");
    let mut from = css.len();
    while let Some(at) = css[..from].rfind(&needle) {
        let boundary = css[..at]
            .chars()
            .next_back()
            .map_or(true, |c| !c.is_ascii_alphanumeric() && c != '_' && c != '-');
        if boundary {
            let start = at + needle.len();
            // `;` *or* `}`: a final declaration written without its semicolon
            // would otherwise swallow every block after it — which is only
            // harmless while a palette is a single block.
            let value = css[start..].split(|c| c == ';' || c == '}').next()?;
            return Some(value.trim().to_string());
        }
        from = at;
    }
    None
}

/// The stylesheet as it applies in `scheme`: the other appearance's blocks
/// dropped, and this one's moved to the end.
///
/// Two rules carry the whole design.
///
/// **A stylesheet with no `[data-mode]` block is returned unchanged.** That is
/// the compatibility guarantee, and it is why the test is "has no mode blocks"
/// rather than "keep the `:root` blocks": a hand-written `theme_css` may well
/// declare `--bg` on `body` or `.reader`, and a `:root` filter would throw it
/// away.
///
/// **This scheme's blocks are re-appended, not merely left in place.**
/// [`custom_property`] is a last-wins *textual* scan, but real CSS gives
/// `:root[data-mode="dark"]` (0,1,1) a higher specificity than `:root` (0,0,1)
/// whatever the source order. Without the move, a palette that put its mode
/// blocks above the shared block would give one answer here and another in the
/// webview — a disagreement visible only as a window frame in the wrong colour.
///
/// A mode block nested inside an `@media` loses its condition when it is moved.
/// Only the two values parsed here are affected; the CSS handed to the webview
/// is never rewritten.
fn mode_slice<'a>(css: &'a str, scheme: Scheme) -> Cow<'a, str> {
    if !css.contains("data-mode") {
        return Cow::Borrowed(css);
    }
    let b = css.as_bytes();
    let mut kept = String::with_capacity(css.len());
    let mut mine = String::new();
    // Everything before `copied` is already in `kept`; `prelude` is where the
    // current selector started.
    let (mut copied, mut prelude, mut i) = (0, 0, 0);
    while i < b.len() {
        match b[i] {
            // A brace inside a string is not a brace. `content: "}"` is legal
            // in a hand-written stylesheet and would desynchronise the depth
            // count.
            b'"' | b'\'' => i = skip_string(b, i),
            b'{' => match mode_attr(&css[prelude..i]) {
                // Not a mode block: descend into it, so a mode block nested in
                // an `@media` is still found. No special case needed.
                None => {
                    i += 1;
                    prelude = i;
                }
                Some(named) => {
                    let end = block_end(b, i);
                    kept.push_str(&css[copied..prelude]);
                    if named == Some(scheme) {
                        mine.push_str(&css[prelude..end]);
                    }
                    i = end;
                    copied = end;
                    prelude = end;
                }
            },
            b'}' | b';' => {
                i += 1;
                prelude = i;
            }
            _ => i += 1,
        }
    }
    kept.push_str(&css[copied..]);
    kept.push_str(&mine);
    Cow::Owned(kept)
}

/// The scheme a selector's `[data-mode=…]` names. `None` means the selector has
/// no `data-mode` at all; `Some(None)` means it has one naming something we
/// don't recognise, which belongs to neither scheme and is dropped from both.
///
/// The `:root` prefix is deliberately not required, so `html[data-mode="dark"]`,
/// `:root[data-mode='dark']`, `:root[data-mode = "dark"]` and selector lists all
/// classify correctly.
fn mode_attr(prelude: &str) -> Option<Option<Scheme>> {
    let lower = prelude.to_ascii_lowercase();
    let at = lower.find("data-mode")?;
    let rest = lower[at + "data-mode".len()..].trim_start();
    // CSS allows `~=`, `|=`, `^=`, `$=`, `*=` here as well as a bare `=`.
    let rest = rest.strip_prefix(['~', '|', '^', '$', '*']).unwrap_or(rest);
    let rest = rest.trim_start().strip_prefix('=')?.trim_start();
    let value = match rest.chars().next() {
        Some(q @ ('"' | '\'')) => rest[1..].split(q).next()?,
        _ => rest.split(|c: char| c == ']' || c.is_whitespace()).next()?,
    };
    Some(match value.trim() {
        "dark" => Some(Scheme::Dark),
        "light" => Some(Scheme::Light),
        _ => None,
    })
}

/// The index just past the `}` closing the block opened at `open`. An
/// unterminated block runs to the end of the file.
fn block_end(b: &[u8], open: usize) -> usize {
    let mut depth = 0usize;
    let mut i = open;
    while i < b.len() {
        match b[i] {
            b'"' | b'\'' => {
                i = skip_string(b, i);
                continue;
            }
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return i + 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    b.len()
}

/// The index just past the quote closing the string opened at `start`.
fn skip_string(b: &[u8], start: usize) -> usize {
    let quote = b[start];
    let mut i = start + 1;
    while i < b.len() {
        match b[i] {
            b'\\' => i += 2,
            c if c == quote => return i + 1,
            _ => i += 1,
        }
    }
    b.len()
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
