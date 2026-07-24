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
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
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
    // Fenced blocks are collected during the parse and highlighted afterwards,
    // in parallel — syntect dominates render time on any document with code
    // (`render/code/2m` vs `render/prose/2m` is a ~750x spread) and each block
    // is independent of every other.
    let mut blocks: Vec<Block> = Vec::new();

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
                Some((lang, text)) => {
                    // Placeholder; filled in below once highlighting is done.
                    blocks.push(Block {
                        at: events.len(),
                        lang,
                        text,
                    });
                    events.push(Event::Html("".into()));
                }
                None => events.push(Event::End(TagEnd::CodeBlock)),
            },
            // Untrusted raw HTML from the source -> render as escaped text.
            Event::Html(h) | Event::InlineHtml(h) => events.push(Event::Text(h)),
            other => events.push(other),
        }
    }

    for (at, html) in highlight_blocks(&blocks) {
        events[at] = Event::Html(html.into());
    }

    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, events.into_iter());
    html
}

/// A fenced code block awaiting highlighting, and the event slot it belongs in.
struct Block {
    at: usize,
    lang: String,
    text: String,
}

/// Highlight every block, spreading them across the available cores. Returns
/// `(event index, html)` pairs in arbitrary order.
fn highlight_blocks(blocks: &[Block]) -> Vec<(usize, String)> {
    if blocks.len() < 2 {
        return blocks
            .iter()
            .map(|b| (b.at, highlight_code(&b.lang, &b.text)))
            .collect();
    }

    // Force the lazy syntect statics here rather than letting the workers race
    // into `OnceLock::get_or_init`, which would just serialize them again.
    syntaxes();
    themes();

    let next = AtomicUsize::new(0);
    let out = Mutex::new(Vec::with_capacity(blocks.len()));
    let workers = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(1)
        .min(blocks.len());

    // Blocks vary hugely in size, so workers pull the next index rather than
    // taking a fixed slice.
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| loop {
                let Some(b) = blocks.get(next.fetch_add(1, Ordering::Relaxed)) else {
                    break;
                };
                let html = highlight_code(&b.lang, &b.text);
                out.lock().unwrap().push((b.at, html));
            });
        }
    });

    out.into_inner().unwrap()
}

#[derive(Debug, Clone, Serialize)]
pub struct Location {
    /// 1-based line of the first line the quote touches.
    pub line_start: usize,
    /// 1-based line of the last line the quote touches.
    pub line_end: usize,
}

/// Locate a highlighted quote within the current source, using surrounding
/// context (`prefix`/`suffix`) to disambiguate. Returns `None` when the quoted
/// text can no longer be found — i.e. the highlighted text itself was edited,
/// which the caller treats as a *stale* highlight.
///
/// To locate many quotes in the same document, build a [`SourceIndex`] once and
/// call [`SourceIndex::locate`] instead — this function throws away every index
/// it builds.
pub fn locate(source: &str, prefix: &str, quote: &str, suffix: &str) -> Option<Location> {
    SourceIndex::new(source).locate(prefix, quote, suffix)
}

/// Reusable per-document scratch for [`SourceIndex::locate`].
///
/// `Store::reanchor_file` locates every highlight in a file against one source
/// string. Both of the indexes below are `O(len)` to build and were previously
/// rebuilt *per highlight*, against documents that can be megabytes — which is
/// why re-anchoring cost the same whether a quote resolved or not.
///
/// Line starts are built eagerly: every successful locate needs two line
/// lookups, and one pass is cheaper than the two scans it replaces. The
/// whitespace-stripped index is built lazily, on the first quote that actually
/// reaches tier 3.
pub struct SourceIndex<'a> {
    source: &'a str,
    /// Byte offset of the start of each line; `line_starts[0] == 0`.
    line_starts: Vec<usize>,
    stripped: Option<Stripped>,
}

impl<'a> SourceIndex<'a> {
    pub fn new(source: &'a str) -> Self {
        let mut line_starts = Vec::with_capacity(source.len() / 32 + 1);
        line_starts.push(0);
        line_starts.extend(source.match_indices('\n').map(|(i, _)| i + 1));
        Self {
            source,
            line_starts,
            stripped: None,
        }
    }

    /// 1-based line containing `byte_idx`.
    fn line_at(&self, byte_idx: usize) -> usize {
        // `line_starts[0]` is 0, so the result is always >= 1.
        self.line_starts.partition_point(|&s| s <= byte_idx)
    }

    /// The line span of the byte range `[start, start + len)`.
    fn span(&self, start: usize, len: usize) -> Location {
        Location {
            line_start: self.line_at(start),
            line_end: self.line_at((start + len).saturating_sub(1)),
        }
    }

    /// See [`locate`]. Tiers are tried in order; a rendered selection (what
    /// `getSelection().toString()` yields) normally falls through to tier 3.
    pub fn locate(&mut self, prefix: &str, quote: &str, suffix: &str) -> Option<Location> {
        if quote.trim().is_empty() {
            return None;
        }

        // 1) Exact match with context.
        let needle = format!("{prefix}{quote}{suffix}");
        if let Some(pos) = self.source.find(&needle) {
            return Some(self.span(pos + prefix.len(), quote.len()));
        }

        // 2) Exact match of the quote alone.
        if let Some(pos) = self.source.find(quote) {
            return Some(self.span(pos, quote.len()));
        }

        // 3) Whitespace-normalized match (rendered selections collapse whitespace).
        let quote_ns: String = quote.chars().filter(|c| !c.is_whitespace()).collect();
        if quote_ns.is_empty() {
            return None;
        }
        let source = self.source;
        let stripped = self
            .stripped
            .get_or_insert_with(|| Stripped::build(source));
        let (start, end) = stripped.find(&quote_ns)?;
        Some(Location {
            line_start: self.line_at(start),
            line_end: self.line_at(end),
        })
    }
}

/// The source with all whitespace removed, plus the offset table needed to map
/// a hit inside it back to a byte range in the original.
struct Stripped {
    text: String,
    /// For each *byte* of `text`, the byte offset in the original source of the
    /// char that byte belongs to. Keying by byte rather than by char index is
    /// what makes the lookup O(1): a multi-byte char simply repeats its offset,
    /// so any byte position inside a match maps to the right source char.
    source_offsets: Vec<usize>,
}

impl Stripped {
    fn build(source: &str) -> Self {
        let mut text = String::with_capacity(source.len());
        // Whitespace is typically a fifth to a third of a markdown document.
        let mut source_offsets = Vec::with_capacity(source.len() * 3 / 4);
        for (i, ch) in source.char_indices() {
            if !ch.is_whitespace() {
                text.push(ch);
                source_offsets.resize(text.len(), i);
            }
        }
        Self {
            text,
            source_offsets,
        }
    }

    /// Byte offsets in the *original* source of the first and last char of
    /// `quote_ns` (itself already whitespace-stripped).
    fn find(&self, quote_ns: &str) -> Option<(usize, usize)> {
        let at = self.text.find(quote_ns)?;
        let last = at + quote_ns.len().saturating_sub(1);
        let start = *self.source_offsets.get(at)?;
        let end = self.source_offsets.get(last).copied().unwrap_or(start);
        Some((start, end))
    }
}
