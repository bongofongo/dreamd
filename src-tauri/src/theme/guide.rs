//! `dreamd theme guide`: the theming sheet, printed from the binary so an
//! agent on a machine that has only the installed `dreamd` can read the
//! contract before writing a palette.
//!
//! Three sources, none of which can drift from the code on its own:
//! `ui/themes/README.md` is the prose and is `include_str!`'d; the variable
//! table inside it is generated from [`contract::VARS`] and a test asserts the
//! committed file matches (`--readme` prints the regenerated document to paste
//! back); the selector list is scanned out of `ui/index.html` by `build.rs`.

use super::contract::{Scope, VARS};
use super::BUNDLED;

pub const README: &str = include_str!("../../../ui/themes/README.md");

/// `#id`s, a blank line, `.class`es — everything the chrome and the base
/// stylesheet style, as `build.rs` wrote it.
pub const SELECTORS: &str = include_str!(concat!(env!("OUT_DIR"), "/selectors.txt"));

const OPEN: &str = "<!-- variables -->";
const CLOSE: &str = "<!-- /variables -->";

/// The markdown table between the README's markers.
pub fn variables_table() -> String {
    let mut out =
        String::from("| variable | kind | block | required | paints |\n|---|---|---|---|---|\n");
    for v in VARS {
        let block = match v.scope {
            Scope::Shared => "shared",
            Scope::PerMode => "per mode",
        };
        out.push_str(&format!(
            "| `{}` | {} | {} | {} | {} |\n",
            v.name,
            v.kind.label(),
            block,
            if v.required { "yes" } else { "no" },
            v.paints
        ));
    }
    out
}

/// The README with its variable table regenerated from the contract. Equal to
/// [`README`] whenever the committed file is current — a test says so.
pub fn readme() -> String {
    let (head, rest) = README
        .split_once(OPEN)
        .expect("README carries the open marker");
    let (_, tail) = rest
        .split_once(CLOSE)
        .expect("README carries the close marker");
    format!("{head}{OPEN}\n{}{CLOSE}{tail}", variables_table())
}

/// The whole guide: the README, then the lists only the binary knows.
pub fn render(syntax_themes: &[String]) -> String {
    let mut out = readme();
    out.push_str("\n## Syntect themes\n\nValues `--syntax-theme` may take, quoted:\n\n");
    for t in syntax_themes {
        out.push_str(&format!("- `\"{t}\"`\n"));
    }
    out.push_str("\n## Bundled themes\n\n`dreamd theme new <name> --from <one of these>`; the first is the default:\n\n");
    for (name, _) in BUNDLED {
        out.push_str(&format!("- `{name}`\n"));
    }
    out.push_str("\n## Selectors\n\nEvery id and class the chrome and the base stylesheet style — the surface a `theme_css` stylesheet can override. Scanned from the page at build time.\n\n```\n");
    out.push_str(SELECTORS);
    out.push_str("```\n");
    out
}

/// The same facts as one document, for a caller that would rather not parse
/// markdown. `themes_dir` and `active` are the two runtime facts the README's
/// prose can only describe.
pub fn json(
    syntax_themes: &[String],
    themes_dir: &std::path::Path,
    active: Option<&str>,
) -> serde_json::Value {
    let vars: Vec<_> = VARS
        .iter()
        .map(|v| {
            serde_json::json!({
                "name": v.name,
                "kind": v.kind.label(),
                "block": match v.scope { Scope::Shared => "shared", Scope::PerMode => "per-mode" },
                "required": v.required,
                "paints": v.paints,
            })
        })
        .collect();
    let (ids, classes) = SELECTORS
        .split_once("\n\n")
        .map(|(i, c)| (lines(i), lines(c)))
        .unwrap_or_default();
    serde_json::json!({
        "themes_dir": themes_dir,
        "active": active,
        "bundled": BUNDLED.iter().map(|(n, _)| *n).collect::<Vec<_>>(),
        "syntax_themes": syntax_themes,
        "variables": vars,
        "internal": super::contract::INTERNAL,
        "selectors": { "ids": ids, "classes": classes },
        "guide": readme(),
    })
}

fn lines(s: &str) -> Vec<&str> {
    s.lines().filter(|l| !l.is_empty()).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The committed README's table is the contract's. Regenerate with
    /// `dreamd theme guide --readme > ui/themes/README.md`.
    #[test]
    fn the_readme_table_is_current() {
        // `assert!`, not `assert_eq!`: the diff is a 12KB document and the
        // remedy is one command.
        assert!(
            README == readme(),
            "ui/themes/README.md is stale: dreamd theme guide --readme > ui/themes/README.md"
        );
    }

    /// Every `#id` and `.class` the README's region table names exists in the
    /// page — a region map that points at a renamed element misleads every
    /// agent that reads it.
    #[test]
    fn every_selector_the_region_table_names_exists() {
        let (ids, classes) = super::super::selectors::selectors(&format!(
            "{}\n{}",
            super::super::selectors::style_blocks(super::super::INDEX_HTML),
            super::super::BASE_CSS
        ));
        // Styled selectors, plus every id in the markup: an element the
        // stylesheet only reaches through its parent is still real and still
        // targetable, and the map is allowed to name it.
        let mut known: std::collections::HashSet<String> =
            ids.iter().chain(classes.iter()).cloned().collect();
        for id in super::super::INDEX_HTML.split("id=\"").skip(1) {
            if let Some((id, _)) = id.split_once('"') {
                known.insert(format!("#{id}"));
            }
        }
        let table = README
            .split("## Going further")
            .nth(1)
            .and_then(|s| s.split("Four things").next())
            .expect("the region table");
        let mut named = 0;
        let mut missing = Vec::new();
        for row in table.lines().filter(|l| l.starts_with("| ")) {
            for span in row.split('`').skip(1).step_by(2) {
                for tok in
                    span.split(|c: char| c.is_whitespace() || c == ',' || c == '(' || c == ')')
                {
                    // `mark.hl`, `button.code-copy`, `body.chrome-fade` name
                    // the class; `#id`/`.class` are themselves. Anything
                    // else in a span is a value, not a selector.
                    let Some(at) = tok.find(['#', '.']) else {
                        continue;
                    };
                    if !tok.starts_with(['#', '.']) && !tok[..at].chars().all(char::is_alphabetic) {
                        continue;
                    }
                    let sel = tok[at..].split([':', '[']).next().unwrap_or("");
                    if sel.len() < 2 {
                        continue;
                    }
                    // `mark.hl.stale` is two classes; each must exist.
                    let mut parts = Vec::new();
                    let mut rest = sel;
                    while let Some(next) = rest[1..].find(['#', '.']) {
                        parts.push(&rest[..next + 1]);
                        rest = &rest[next + 1..];
                    }
                    parts.push(rest);
                    for part in parts {
                        named += 1;
                        if !known.contains(part) {
                            missing.push(tok.to_string());
                        }
                    }
                }
            }
        }
        assert!(
            named > 50,
            "the region table scan found only {named} selectors"
        );
        assert!(
            missing.is_empty(),
            "README names selectors the page does not style: {missing:?}"
        );
    }

    #[test]
    fn the_guide_carries_the_lists_only_the_binary_knows() {
        let g = render(&["InspiredGitHub".to_string()]);
        assert!(g.contains("- `\"InspiredGitHub\"`"));
        assert!(g.contains("- `dreamd`\n"));
        assert!(g.contains("#workspace"));
        assert!(!SELECTORS.is_empty());
    }

    #[test]
    fn json_names_every_variable_and_the_selector_surface() {
        let j = json(&[], std::path::Path::new("/x"), Some("dreamd"));
        assert_eq!(j["variables"].as_array().unwrap().len(), VARS.len());
        assert!(j["selectors"]["ids"]
            .as_array()
            .unwrap()
            .iter()
            .any(|v| v == "#content"));
        assert_eq!(j["active"], "dreamd");
    }
}
