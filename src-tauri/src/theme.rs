//! Theme CSS: resolving the user's stylesheet, and pulling out the one value
//! the native shell needs before the frontend is alive.
//!
//! `theme.css` is injected into `#user-theme` from JS, several IPC round-trips
//! into boot, so `--bg` does not exist while the window is first painted. The
//! window and webview are given the parsed background up front so that gap
//! reads as the theme's colour instead of a white flash. Parsing the user's
//! own file (tenet 5) rather than hardcoding a colour means a light theme
//! flashes light, not dark.

use crate::DEFAULT_THEME;
use std::path::Path;

/// The active theme CSS: the user's file if it is set and readable, else the
/// bundled default.
pub fn resolve(path: Option<&Path>) -> String {
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_else(|| DEFAULT_THEME.to_string())
}

/// The `--bg` custom property as `(r, g, b)`. Only hex forms (`#rgb`,
/// `#rrggbb`) are understood; anything else yields `None` and the caller keeps
/// whatever default it had.
pub fn background(css: &str) -> Option<(u8, u8, u8)> {
    let css = strip_comments(css);
    let start = css.find("--bg:")? + "--bg:".len();
    parse_hex(css[start..].split(';').next()?.trim())
}

/// Drop `/* ... */` blocks so a commented-out `--bg` cannot win over the real
/// declaration.
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
