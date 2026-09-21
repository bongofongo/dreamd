//! The selector surface: every `#id` and `.class` the chrome's stylesheet and
//! `ui/theme.css` style, which is exactly the set a `theme_css` stylesheet can
//! override. Scanned rather than hand-listed so it cannot drift.
//!
//! Included **by path** from `build.rs` as well as through `theme`, which is
//! why it depends on nothing — not even the rest of this crate. The build
//! script runs it over `ui/index.html` and `ui/theme.css` and writes the list
//! to `OUT_DIR`, so `dreamd theme guide` prints it from a few kilobytes of
//! text rather than carrying a second copy of the 120KB page in the binary.

/// The contents of every `<style>` element in `html`, concatenated.
pub fn style_blocks(html: &str) -> String {
    let mut out = String::new();
    let mut rest = html;
    while let Some(open) = rest.find("<style") {
        let Some(gt) = rest[open..].find('>') else {
            break;
        };
        let body_start = open + gt + 1;
        let Some(close) = rest[body_start..].find("</style>") else {
            break;
        };
        out.push_str(&rest[body_start..body_start + close]);
        out.push('\n');
        rest = &rest[body_start + close..];
    }
    out
}

/// Every id and class in selector position in `css`: `(ids, classes)`,
/// each sorted and unique. Comments and quoted strings are skipped where they
/// sit; at-rule preludes (`@media …`) contribute nothing but their blocks are
/// entered.
pub fn selectors(css: &str) -> (Vec<String>, Vec<String>) {
    let css = strip_comments(css);
    let mut ids = Vec::new();
    let mut classes = Vec::new();
    let mut prelude = 0;
    let mut i = 0;
    let b = css.as_bytes();
    while i < b.len() {
        match b[i] {
            b'"' | b'\'' => {
                let q = b[i];
                i += 1;
                while i < b.len() && b[i] != q {
                    i += if b[i] == b'\\' { 2 } else { 1 };
                }
                i += 1;
            }
            b'{' => {
                let sel = css[prelude..i].trim();
                if !sel.starts_with('@') {
                    tokens(sel, &mut ids, &mut classes);
                }
                i += 1;
                prelude = i;
            }
            b'}' | b';' => {
                i += 1;
                prelude = i;
            }
            _ => i += 1,
        }
    }
    for v in [&mut ids, &mut classes] {
        v.sort();
        v.dedup();
    }
    (ids, classes)
}

fn tokens(sel: &str, ids: &mut Vec<String>, classes: &mut Vec<String>) {
    let b = sel.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if (b[i] == b'#' || b[i] == b'.')
            && i + 1 < b.len()
            && (b[i + 1].is_ascii_alphabetic() || b[i + 1] == b'_' || b[i + 1] == b'-')
        {
            let start = i;
            i += 1;
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'_' || b[i] == b'-') {
                i += 1;
            }
            let tok = sel[start..i].to_string();
            if b[start] == b'#' {
                ids.push(tok);
            } else {
                classes.push(tok);
            }
        } else {
            i += 1;
        }
    }
}

fn strip_comments(css: &str) -> String {
    let mut out = String::with_capacity(css.len());
    let mut rest = css;
    while let Some(open) = rest.find("/*") {
        out.push_str(&rest[..open]);
        match rest[open + 2..].find("*/") {
            Some(close) => rest = &rest[open + 2 + close + 2..],
            None => return out,
        }
    }
    out.push_str(rest);
    out
}

/// The list as the build script writes it and the guide prints it: ids, a
/// blank line, classes, one per line.
pub fn render(css: &str) -> String {
    let (ids, classes) = selectors(css);
    let mut out = ids.join("\n");
    out.push_str("\n\n");
    out.push_str(&classes.join("\n"));
    out.push('\n');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_and_classes_are_taken_from_selectors_only() {
        let (ids, classes) = selectors(
            "#a .b, .c:hover > #d { color: #fff; background: url(#e); } @media (x) { .f { } }",
        );
        assert_eq!(ids, vec!["#a", "#d"]);
        assert_eq!(classes, vec![".b", ".c", ".f"]);
    }

    #[test]
    fn a_brace_in_a_string_or_comment_does_not_desynchronise() {
        let (ids, classes) = selectors(".a::before { content: \"{\"; } /* .zzz { */ #b { }");
        assert_eq!(ids, vec!["#b"]);
        assert_eq!(classes, vec![".a"]);
    }

    #[test]
    fn a_number_after_a_dot_is_not_a_class() {
        let (_, classes) = selectors(".a { margin: .5em; } .b{}");
        assert_eq!(classes, vec![".a", ".b"]);
    }

    #[test]
    fn style_blocks_are_all_collected() {
        let html = "<style>.a{}</style><p>x</p><style id=\"b\">#c{}</style>";
        let (ids, classes) = selectors(&style_blocks(html));
        assert_eq!(ids, vec!["#c"]);
        assert_eq!(classes, vec![".a"]);
    }
}
