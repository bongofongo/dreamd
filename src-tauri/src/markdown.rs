//! Markdown -> HTML rendering.
//!
//! Security: raw HTML embedded in the markdown source is **escaped**, never
//! passed through, so a `<script>` in a document cannot execute inside the
//! webview (which has IPC access). Only syntect's code-block HTML — which we
//! generate ourselves — is emitted as trusted markup.
//!
//! Fenced code blocks are syntax-highlighted server-side via syntect.

use pulldown_cmark::{CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use serde::Serialize;
use std::sync::OnceLock;
use syntect::highlighting::ThemeSet;
use syntect::html::highlighted_html_for_string;
use syntect::parsing::SyntaxSet;

/// Code-block color theme. Chosen to match the bundled dark `theme.css`.
const CODE_THEME: &str = "base16-ocean.dark";

fn syntaxes() -> &'static SyntaxSet {
    static S: OnceLock<SyntaxSet> = OnceLock::new();
    S.get_or_init(SyntaxSet::load_defaults_newlines)
}

fn themes() -> &'static ThemeSet {
    static T: OnceLock<ThemeSet> = OnceLock::new();
    T.get_or_init(ThemeSet::load_defaults)
}

fn options() -> Options {
    let mut o = Options::empty();
    o.insert(Options::ENABLE_TABLES);
    o.insert(Options::ENABLE_STRIKETHROUGH);
    o.insert(Options::ENABLE_TASKLISTS);
    o.insert(Options::ENABLE_FOOTNOTES);
    o.insert(Options::ENABLE_SMART_PUNCTUATION);
    o
}

fn highlight_code(lang: &str, code: &str) -> String {
    let ss = syntaxes();
    let syntax = ss
        .find_syntax_by_token(lang)
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let theme = match themes().themes.get(CODE_THEME) {
        Some(t) => t,
        None => return fallback_code(code),
    };
    highlighted_html_for_string(code, ss, syntax, theme).unwrap_or_else(|_| fallback_code(code))
}

fn fallback_code(code: &str) -> String {
    format!(
        "<pre class=\"code\"><code>{}</code></pre>",
        escape_html(code)
    )
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Render markdown source to a sanitized HTML string.
pub fn render(source: &str) -> String {
    let parser = Parser::new_ext(source, options());

    let mut events: Vec<Event> = Vec::new();
    let mut code_buf: Option<(String, String)> = None; // (lang, text)

    for ev in parser {
        match ev {
            Event::Start(Tag::CodeBlock(kind)) => {
                let lang = match kind {
                    CodeBlockKind::Fenced(l) => l.to_string(),
                    CodeBlockKind::Indented => String::new(),
                };
                code_buf = Some((lang, String::new()));
            }
            // Inside a fence, text accumulates into the buffer instead of
            // being emitted; the whole block is replaced by syntect's HTML.
            Event::Text(t) => match &mut code_buf {
                Some((_, buf)) => buf.push_str(&t),
                None => events.push(Event::Text(t)),
            },
            Event::End(TagEnd::CodeBlock) => match code_buf.take() {
                Some((lang, text)) => events.push(Event::Html(highlight_code(&lang, &text).into())),
                None => events.push(Event::End(TagEnd::CodeBlock)),
            },
            // Untrusted raw HTML from the source -> render as escaped text.
            Event::Html(h) | Event::InlineHtml(h) => events.push(Event::Text(h)),
            other => events.push(other),
        }
    }

    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, events.into_iter());
    html
}

#[derive(Debug, Clone, Serialize)]
pub struct Location {
    /// 1-based line of the first line the quote touches.
    pub line_start: usize,
    /// 1-based line of the last line the quote touches.
    pub line_end: usize,
}

fn line_at(source: &str, byte_idx: usize) -> usize {
    source[..byte_idx.min(source.len())]
        .bytes()
        .filter(|&b| b == b'\n')
        .count()
        + 1
}

/// The line span of the byte range `[start, start + len)`.
fn span(source: &str, start: usize, len: usize) -> Location {
    Location {
        line_start: line_at(source, start),
        line_end: line_at(source, (start + len).saturating_sub(1)),
    }
}

/// Locate a highlighted quote within the current source, using surrounding
/// context (`prefix`/`suffix`) to disambiguate. Returns `None` when the quoted
/// text can no longer be found — i.e. the highlighted text itself was edited,
/// which the caller treats as a *stale* highlight.
pub fn locate(source: &str, prefix: &str, quote: &str, suffix: &str) -> Option<Location> {
    if quote.trim().is_empty() {
        return None;
    }

    // 1) Exact match with context.
    let needle = format!("{prefix}{quote}{suffix}");
    if let Some(pos) = source.find(&needle) {
        return Some(span(source, pos + prefix.len(), quote.len()));
    }

    // 2) Exact match of the quote alone.
    if let Some(pos) = source.find(quote) {
        return Some(span(source, pos, quote.len()));
    }

    // 3) Whitespace-normalized match (rendered selections collapse whitespace).
    locate_normalized(source, quote)
}

fn locate_normalized(source: &str, quote: &str) -> Option<Location> {
    let (src_ns, map) = strip_ws_indexed(source);
    let quote_ns: String = quote.chars().filter(|c| !c.is_whitespace()).collect();
    if quote_ns.is_empty() {
        return None;
    }
    let byte_off = src_ns.find(&quote_ns)?;
    let start_char = src_ns[..byte_off].chars().count();
    let end_char = (start_char + quote_ns.chars().count()).saturating_sub(1);

    let start_byte = *map.get(start_char)?;
    let end_byte = map.get(end_char).copied().unwrap_or(start_byte);
    Some(Location {
        line_start: line_at(source, start_byte),
        line_end: line_at(source, end_byte),
    })
}

/// Return the source with whitespace removed, plus a map from each retained
/// char's index to its original byte offset in `source`.
fn strip_ws_indexed(source: &str) -> (String, Vec<usize>) {
    let mut out = String::new();
    let mut map = Vec::new();
    for (i, ch) in source.char_indices() {
        if !ch.is_whitespace() {
            out.push(ch);
            map.push(i);
        }
    }
    (out, map)
}
