//! The palette contract: every custom property a stylesheet may declare, what
//! each one paints, and [`check`], which holds a stylesheet to it.
//!
//! This is the one place the vocabulary is written down. `ui/theme.css` and
//! `ui/index.html` *consume* these variables, `theme_check` asserts the bundled
//! families declare them, `dreamd theme check` asserts a user's file does, and
//! `dreamd theme guide` prints the table an agent reads before writing one —
//! all off [`VARS`], so a variable the chrome learned to read cannot go
//! undocumented without `every_consumed_variable_is_in_the_contract` failing.
//! Before this module the list lived in the example harness, the fallbacks in
//! two stylesheets, and nowhere an agent could ask.
//!
//! Nothing here is enforced at *load*: a palette that fails `check` still
//! applies, with the base stylesheet's fallbacks standing in for whatever it
//! left out. `check` is advice, and the CLI's exit status is how an agent
//! reads it.

use super::{background, custom_property, has_mode_blocks, parse_hex, prior_fade, Scheme};

/// What kind of value a variable takes. Only [`Kind::Color`] on `--bg`,
/// [`Kind::Percent`] and [`Kind::SyntaxTheme`] are parsed by Rust; the rest
/// are the webview's business and the kind is documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// A CSS colour. `--bg` must be hex (`#rgb`/`#rrggbb`) because the native
    /// window is painted from it before the webview exists.
    Color,
    /// `N%`, a strength.
    Percent,
    /// A CSS length (`17px`, `1.2em`).
    Length,
    /// A unitless number.
    Number,
    /// A `font-family` stack.
    Font,
    /// A syntect theme name, quoted. Not CSS; Rust reads it.
    SyntaxTheme,
    /// A CSS keyword, e.g. `justify` or `auto`.
    Keyword,
    /// A `box-shadow` value.
    Shadow,
    /// A `border` shorthand or `none`.
    Border,
}

impl Kind {
    pub fn label(self) -> &'static str {
        match self {
            Kind::Color => "colour",
            Kind::Percent => "percent",
            Kind::Length => "length",
            Kind::Number => "number",
            Kind::Font => "font stack",
            Kind::SyntaxTheme => "syntect theme",
            Kind::Keyword => "keyword",
            Kind::Shadow => "box-shadow",
            Kind::Border => "border",
        }
    }
}

/// Where a variable belongs in a family. Advisory — `check` resolves every
/// variable per scheme through the same slice the app uses, so a colour in
/// the shared block passes, it is just a colour that cannot differ by mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    /// The bare `:root` block: type metrics, shared by both appearances.
    Shared,
    /// A `:root[data-mode="…"]` block: colours, one value per appearance.
    PerMode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Var {
    pub name: &'static str,
    pub kind: Kind,
    pub scope: Scope,
    /// A required variable has a fallback in `ui/theme.css` or `index.html`,
    /// but the fallback belongs to the default theme: a palette that omits one
    /// is painting part of the window in someone else's colour.
    pub required: bool,
    /// What it paints, in the words the guide prints.
    pub paints: &'static str,
}

const fn var(
    name: &'static str,
    kind: Kind,
    scope: Scope,
    required: bool,
    paints: &'static str,
) -> Var {
    Var {
        name,
        kind,
        scope,
        required,
        paints,
    }
}

use Kind::*;
use Scope::*;

/// Every variable dreamd reads from a palette, in the order the guide prints
/// them: the document's type, then the surfaces, then the text on them, then
/// the two accent roles, then the highlight colours, then shape.
pub const VARS: &[Var] = &[
    // ---- type (shared) ----
    var("--font-body", Font, Shared, true, "the document's prose"),
    var("--font-mono", Font, Shared, true, "inline code, fenced blocks, the terminal pane, and every monospaced field in the chrome"),
    var("--font-size", Length, Shared, true, "the document's base font size; `ui.zoom` multiplies it"),
    var("--line-height", Number, Shared, true, "the document's line height"),
    var("--content-width", Length, Shared, true, "the document's measure — its max width; `ui.zoom` multiplies it so the width in characters stays put"),
    var("--ui-font-size", Length, Shared, true, "the chrome's font size: sidebar, buttons, panels, modals"),
    var("--font-ui", Font, Shared, false, "the chrome's font family; defaults to the system UI face"),
    var("--font-heading", Font, Shared, false, "headings, when they should differ from `--font-body`"),
    var("--heading-weight", Number, Shared, false, "heading font weight (default 650)"),
    var("--heading-rule", Border, Shared, false, "the rule under h1 and h2; `none` for a reading theme (default `1px solid var(--border)`)"),
    var("--para-spacing", Length, Shared, false, "vertical margin between paragraphs (default 0.8em)"),
    var("--text-align", Keyword, Shared, false, "paragraph alignment; `justify` wants `--hyphens: auto` beside it"),
    var("--hyphens", Keyword, Shared, false, "`manual` (default) or `auto`"),
    var("--letter-spacing", Length, Shared, false, "tracking on the document's prose"),
    // ---- surfaces (per mode) ----
    var("--bg", Color, PerMode, true, "the page and the native window behind it — hex only, Rust paints the window from it"),
    var("--sidebar-bg", Color, PerMode, true, "the sidebar, the stack panel, the agent pane and card, the find bar, menus and the outline"),
    var("--btn-bg", Color, PerMode, true, "buttons, inputs, table headers, inline code and the code-block slab (unless `--code-bg`)"),
    var("--hover", Color, PerMode, true, "hover state on buttons, tree rows, menu items, and the agent's tool cards"),
    var("--border", Color, PerMode, true, "every 1px rule: panel edges, buttons, inputs, table cells, heading rules"),
    var("--code-bg", Color, PerMode, false, "the fenced code block's slab instead of `--btn-bg`; `transparent` restores syntect's own"),
    // ---- text (per mode) ----
    var("--text", Color, PerMode, true, "body text, in the document and the chrome"),
    var("--muted", Color, PerMode, true, "secondary text: blockquotes, hints, placeholders, timestamps"),
    var("--link", Color, PerMode, true, "links in the document"),
    // ---- accent (per mode) ----
    var("--accent", Color, PerMode, true, "the primary button, the blockquote bar, the caret, focus rings, selected tabs' edges"),
    var("--accent-dim", Color, PerMode, true, "a wash of the accent: the open file in the tree, the selected palette row, tab and theme card"),
    // ---- highlights (per mode) ----
    var("--hl", Color, PerMode, true, "the highlight fill on a marked passage and the highlight-mode button"),
    var("--hl-text", Color, PerMode, false, "text on a highlight fill (default near-black)"),
    var("--hl-prior", Percent, PerMode, true, "how much of `--hl` survives on a mark from an earlier session; differs per mode because the same strength is a whisper on paper and a lit bar on black"),
    var("--stale", Color, PerMode, true, "a mark whose passage was edited away: the rail chip's edge, the danger button, error text"),
    var("--stale-bg", Color, PerMode, true, "the stale chip's fill and the danger button's hover"),
    var("--stale-text", Color, PerMode, false, "text on a stale fill (default near-black)"),
    // ---- code ----
    var("--syntax-theme", SyntaxTheme, PerMode, true, "the syntect theme for tokens in fenced code, quoted; one per mode or a light theme gets dark code"),
    // ---- shape (shared) ----
    var("--radius", Length, Shared, false, "corner radius on buttons, inputs, chips, inline code and images (each site defaults to 4–6px)"),
    var("--radius-lg", Length, Shared, false, "corner radius on cards, modals, the agent pop-out, menus and code blocks (each site defaults to 8–12px)"),
    var("--shadow", Shadow, Shared, false, "the shadow under every floating surface — modals, menus, the pop-out, the toast; `none` flattens them"),
];

/// Variables the stylesheets consume that are *not* a palette's: dreamd sets
/// them from config or measurement at runtime, inline on an element. A
/// palette declaring one is overwritten before it paints.
pub const INTERNAL: &[&str] = &[
    "--zoom",
    "--img-w",
    "--tree-width",
    "--stack-width",
    "--pane-width",
    "--pane-height",
    "--pane-w",
    "--pane-h",
];

pub fn find(name: &str) -> Option<&'static Var> {
    VARS.iter().find(|v| v.name == name)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    /// The palette will paint wrong somewhere: a required variable missing, a
    /// value Rust cannot parse.
    Error,
    /// Worth a look: a probable typo, weak contrast, a value that cannot be
    /// right for both appearances.
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    pub level: Level,
    pub message: String,
}

impl Finding {
    fn error(message: String) -> Self {
        Finding {
            level: Level::Error,
            message,
        }
    }
    fn warning(message: String) -> Self {
        Finding {
            level: Level::Warning,
            message,
        }
    }
}

/// WCAG AA for body text. Nord's `--link` sits under it on purpose, which is
/// why contrast is a warning and not an error.
pub const MIN_CONTRAST: f64 = 4.5;

fn label(scheme: Scheme) -> &'static str {
    match scheme {
        Scheme::Light => "light",
        Scheme::Dark => "dark",
    }
}

/// Hold `css` to the contract. `syntax_themes` is `markdown::syntax_theme_names()`,
/// passed in so this stays free of syntect's dump load and testable with a
/// list of one.
///
/// Every per-scheme rule resolves the variable through [`custom_property`] —
/// the parser the app uses — so a variable only the dark block declares is
/// reported missing from light, which a substring test would miss.
pub fn check(css: &str, syntax_themes: &[String]) -> Vec<Finding> {
    let mut out = Vec::new();
    let family = has_mode_blocks(css);

    for scheme in [Scheme::Light, Scheme::Dark] {
        let m = label(scheme);
        for v in VARS.iter().filter(|v| v.required) {
            if custom_property(css, v.name, scheme).is_none() {
                out.push(Finding::error(format!(
                    "{m}: missing {} ({})",
                    v.name, v.paints
                )));
            }
        }
        if custom_property(css, "--bg", scheme).is_some() && background(css, scheme).is_none() {
            out.push(Finding::error(format!(
                "{m}: --bg is not a hex colour; the native window is painted from it and understands only #rgb or #rrggbb"
            )));
        }
        if let Some(name) = super::syntax_theme(css, scheme) {
            if !syntax_themes.contains(&name) {
                out.push(Finding::error(format!(
                    "{m}: --syntax-theme {name:?} is not a syntect theme; one of: {}",
                    syntax_themes.join(", ")
                )));
            }
        }
        if let Some(raw) = custom_property(css, "--hl-prior", scheme) {
            if parse_percent(&raw).is_none() {
                out.push(Finding::error(format!(
                    "{m}: --hl-prior {raw:?} is not a percentage between 0% and 100%"
                )));
            }
        }
        for (fg, what) in [("--text", "body text"), ("--link", "links")] {
            if let (Some(f), Some(b)) = (
                custom_property(css, fg, scheme).and_then(|v| parse_hex(&v)),
                background(css, scheme),
            ) {
                let ratio = contrast(f, b);
                if ratio < MIN_CONTRAST {
                    out.push(Finding::warning(format!(
                        "{m}: {fg} on --bg is {ratio:.1}:1; {what} wants {MIN_CONTRAST}:1"
                    )));
                }
            }
        }
    }

    if family {
        // A value copied across the two blocks is a value that was looked at
        // in one of them. Each of these is invisible except as "a bit off".
        let same = |name: &str| {
            custom_property(css, name, Scheme::Light) == custom_property(css, name, Scheme::Dark)
        };
        if background(css, Scheme::Light) == background(css, Scheme::Dark) {
            out.push(Finding::error(
                "--bg is the same in both modes; a family paints each appearance its own ground"
                    .into(),
            ));
        }
        if same("--syntax-theme") {
            out.push(Finding::error(
                "--syntax-theme is the same in both modes; that is how a light theme gets dark code blocks".into(),
            ));
        }
        if prior_fade(css, Scheme::Light) == prior_fade(css, Scheme::Dark) {
            out.push(Finding::error(
                "--hl-prior is the same in both modes; the same fade strength cannot be right on paper and on black".into(),
            ));
        }
    } else {
        out.push(Finding::warning(
            "no :root[data-mode=\"light\"] / [data-mode=\"dark\"] blocks: this palette reads the same in both appearances".into(),
        ));
    }

    for name in declared(css) {
        if find(&name).is_some() || INTERNAL.contains(&name.as_str()) {
            continue;
        }
        let hint = nearest(&name)
            .map(|n| format!(" (did you mean {n}?)"))
            .unwrap_or_default();
        out.push(Finding::warning(format!(
            "{name} is not a variable dreamd reads{hint}"
        )));
    }

    out
}

/// Every custom property declared anywhere in `css`, in first-seen order, each
/// once. Comments are stripped first so a commented-out `--old-name:` is not
/// reported as a typo.
pub fn declared(css: &str) -> Vec<String> {
    let css = super::strip_comments(css);
    let bytes = css.as_bytes();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while let Some(at) = css[i..].find("--") {
        let start = i + at;
        let boundary = start == 0
            || !(bytes[start - 1].is_ascii_alphanumeric()
                || bytes[start - 1] == b'_'
                || bytes[start - 1] == b'-');
        let end = start
            + css[start..]
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
                .unwrap_or(css.len() - start);
        // A declaration, not a `var(--x)` reference: the name is followed by a
        // colon, and it opens with a boundary rather than the tail of
        // `--panel--bg`.
        let is_decl = boundary
            && end > start + 2
            && css[end..].trim_start().starts_with(':')
            && !css[end..].trim_start().starts_with("::");
        if is_decl {
            let name = &css[start..end];
            if !out.iter().any(|n| n == name) {
                out.push(name.to_string());
            }
        }
        i = end.max(start + 2);
    }
    out
}

/// The contract variable closest to `name` by edit distance, if any is close
/// enough to be a plausible slip.
fn nearest(name: &str) -> Option<&'static str> {
    VARS.iter()
        .map(|v| (levenshtein(name, v.name), v.name))
        .filter(|(d, _)| *d <= 3)
        .min_by_key(|(d, _)| *d)
        .map(|(_, n)| n)
}

fn levenshtein(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut prev: Vec<usize> = (0..=b.len()).collect();
    let mut cur = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        cur[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let cost = usize::from(ca != cb);
            cur[j + 1] = (prev[j + 1] + 1).min(cur[j] + 1).min(prev[j] + cost);
        }
        std::mem::swap(&mut prev, &mut cur);
    }
    prev[b.len()]
}

fn parse_percent(raw: &str) -> Option<f64> {
    let n: f64 = raw.trim().strip_suffix('%')?.trim().parse().ok()?;
    (0.0..=100.0).contains(&n).then_some(n)
}

/// WCAG relative luminance.
fn luminance((r, g, b): (u8, u8, u8)) -> f64 {
    let lin = |c: u8| {
        let c = f64::from(c) / 255.0;
        if c <= 0.03928 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b)
}

/// WCAG contrast ratio, `1.0..=21.0`.
pub fn contrast(a: (u8, u8, u8), b: (u8, u8, u8)) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    let (hi, lo) = if la > lb { (la, lb) } else { (lb, la) };
    (hi + 0.05) / (lo + 0.05)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn themes() -> Vec<String> {
        vec!["InspiredGitHub".into(), "base16-ocean.dark".into()]
    }

    fn errors(f: &[Finding]) -> Vec<&str> {
        f.iter()
            .filter(|f| f.level == Level::Error)
            .map(|f| f.message.as_str())
            .collect()
    }

    fn warnings(f: &[Finding]) -> Vec<&str> {
        f.iter()
            .filter(|f| f.level == Level::Warning)
            .map(|f| f.message.as_str())
            .collect()
    }

    #[test]
    fn every_bundled_family_passes_with_no_errors() {
        let available = crate::markdown::syntax_theme_names();
        for (name, css) in super::super::BUNDLED {
            let f = check(css, &available);
            assert!(errors(&f).is_empty(), "{name}: {:?}", errors(&f));
        }
    }

    #[test]
    fn a_variable_only_the_dark_block_declares_is_missing_from_light() {
        let css = format!(
            "{}\n:root[data-mode=\"dark\"] {{ --link: #fff; }}",
            super::super::BUNDLED[0].1.replace("--link:", "--link-x:")
        );
        let f = check(&css, &themes());
        assert!(errors(&f)
            .iter()
            .any(|m| m.starts_with("light: missing --link")));
        assert!(!errors(&f)
            .iter()
            .any(|m| m.starts_with("dark: missing --link")));
    }

    #[test]
    fn a_typo_is_a_warning_naming_the_nearest_variable() {
        let css = ":root { --acent: #fff; --bg: #000; }";
        let f = check(css, &themes());
        assert!(warnings(&f)
            .iter()
            .any(|m| m.contains("--acent") && m.contains("did you mean --accent")));
    }

    #[test]
    fn a_runtime_variable_is_not_a_typo() {
        let f = check(":root { --zoom: 2; }", &themes());
        assert!(!warnings(&f).iter().any(|m| m.contains("--zoom")));
    }

    #[test]
    fn a_commented_out_declaration_is_not_reported() {
        let f = check(":root { /* --old: 1; */ }", &themes());
        assert!(!warnings(&f).iter().any(|m| m.contains("--old")));
    }

    #[test]
    fn a_reference_is_not_a_declaration() {
        assert_eq!(declared(":root { --a: var(--b); }"), vec!["--a"]);
    }

    #[test]
    fn a_non_hex_background_is_an_error_because_the_window_reads_it() {
        let f = check(":root { --bg: rgb(0,0,0); }", &themes());
        assert!(errors(&f)
            .iter()
            .any(|m| m.contains("--bg is not a hex colour")));
    }

    #[test]
    fn an_unknown_syntect_theme_is_an_error_that_lists_the_real_ones() {
        let f = check(":root { --syntax-theme: \"Dracula\"; }", &themes());
        let m = errors(&f)
            .into_iter()
            .find(|m| m.contains("Dracula"))
            .expect("reported");
        assert!(m.contains("InspiredGitHub"));
    }

    #[test]
    fn a_prior_fade_that_is_not_a_percentage_is_an_error() {
        let f = check(":root { --hl-prior: 0.2; }", &themes());
        assert!(errors(&f).iter().any(|m| m.contains("--hl-prior")));
        let f = check(":root { --hl-prior: 20%; }", &themes());
        assert!(!errors(&f).iter().any(|m| m.contains("not a percentage")));
    }

    #[test]
    fn weak_contrast_is_a_warning_not_an_error() {
        let f = check(":root { --bg: #ffffff; --text: #dddddd; }", &themes());
        assert!(warnings(&f).iter().any(|m| m.contains("--text on --bg")));
        assert!(!errors(&f).iter().any(|m| m.contains("contrast")));
    }

    #[test]
    fn a_family_with_one_ground_for_both_modes_is_an_error() {
        let css =
            ":root[data-mode=\"light\"] { --bg: #111; } :root[data-mode=\"dark\"] { --bg: #111; }";
        let f = check(css, &themes());
        assert!(errors(&f).iter().any(|m| m.contains("--bg is the same")));
    }

    #[test]
    fn a_flat_palette_is_a_warning_and_never_a_same_in_both_modes_error() {
        let f = check(":root { --bg: #111; }", &themes());
        assert!(warnings(&f).iter().any(|m| m.contains("reads the same")));
        assert!(!errors(&f).iter().any(|m| m.contains("same in both")));
    }

    #[test]
    fn contrast_is_the_wcag_ratio() {
        assert!((contrast((0, 0, 0), (255, 255, 255)) - 21.0).abs() < 0.01);
        assert!((contrast((255, 255, 255), (255, 255, 255)) - 1.0).abs() < 0.01);
    }

    #[test]
    fn names_are_unique_and_required_ones_are_what_theme_check_used_to_insist_on() {
        let mut seen = std::collections::HashSet::new();
        for v in VARS {
            assert!(seen.insert(v.name), "{} listed twice", v.name);
        }
        assert_eq!(VARS.iter().filter(|v| v.required).count(), 21);
    }

    /// The contract's whole reason to exist: a `var(--x)` the stylesheets read
    /// that the contract does not name is a variable no palette can learn
    /// about. `--danger` and `--fg` were both this once.
    #[test]
    fn every_consumed_variable_is_in_the_contract() {
        let mut consumed = std::collections::BTreeSet::new();
        for css in [super::super::BASE_CSS, super::super::INDEX_HTML] {
            let mut rest = css;
            while let Some(at) = rest.find("var(--") {
                let start = at + 4;
                let end = rest[start..]
                    .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_' || c == '-'))
                    .map_or(rest.len(), |e| start + e);
                consumed.insert(rest[start..end].to_string());
                rest = &rest[end..];
            }
        }
        let unknown: Vec<_> = consumed
            .iter()
            .filter(|n| find(n).is_none() && !INTERNAL.contains(&n.as_str()))
            // `--x` is theme.css's header comment's placeholder.
            .filter(|n| n.as_str() != "--x")
            .collect();
        assert!(
            unknown.is_empty(),
            "consumed but not in the contract: {unknown:?}"
        );
        // And the reverse: a contract entry nothing reads is a lie in the guide.
        let unread: Vec<_> = VARS
            .iter()
            .filter(|v| !consumed.contains(v.name) && v.kind != SyntaxTheme)
            .map(|v| v.name)
            .collect();
        assert!(
            unread.is_empty(),
            "in the contract but never consumed: {unread:?}"
        );
    }
}
