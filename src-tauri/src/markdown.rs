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
use std::collections::HashSet;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, OnceLock};
use syntect::highlighting::{Theme, ThemeSet};
use syntect::html::highlighted_html_for_string;
use syntect::parsing::SyntaxSet;

/// Code-block color theme. Chosen to match the bundled dark `theme.css`.
/// Used when a palette names no syntect theme, or names one this build of
/// syntect does not carry.
pub const CODE_THEME: &str = "base16-ocean.dark";

/// The syntect themes available to `--syntax-theme`, so the settings panel can
/// only offer names that will actually work.
pub fn syntax_theme_names() -> Vec<String> {
    themes().themes.keys().cloned().collect()
}

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

/// Look a syntect theme up by name, falling back to [`CODE_THEME`] so a palette
/// naming a theme this build of syntect doesn't carry degrades to the default
/// rather than to unhighlighted text.
fn code_theme(name: &str) -> Option<&'static Theme> {
    let set = &themes().themes;
    set.get(name).or_else(|| set.get(CODE_THEME))
}

fn highlight_code(lang: &str, code: &str, theme: Option<&Theme>) -> String {
    let ss = syntaxes();
    let syntax = ss
        .find_syntax_by_token(lang)
        .unwrap_or_else(|| ss.find_syntax_plain_text());
    let Some(theme) = theme else {
        return fallback_code(code);
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

/// Render markdown source to a sanitized HTML string, with fenced code blocks
/// highlighted using the default syntect theme.
pub fn render(source: &str) -> String {
    render_with(source, CODE_THEME)
}

/// As [`render`], but with the syntect theme named by the active dreamd
/// palette's `--syntax-theme`. An unknown name falls back to [`CODE_THEME`].
pub fn render_with(source: &str, code_theme: &str) -> String {
    let parser = Parser::new_ext(source, options());

    let mut events: Vec<Event> = Vec::new();
    // (lang, text) of the fence currently being read.
    let mut code_buf: Option<(String, String)> = None;
    // Fenced blocks are collected during the parse and highlighted afterwards,
    // in parallel — syntect dominates render time on any document with code
    // (`render/code/2m` vs `render/prose/2m` is a ~750x spread) and each block
    // is independent of every other.
    let mut blocks: Vec<Block> = Vec::new();
    // The open heading's slot in `events` plus its text so far. A heading's id
    // is a slug of its own text, which is only known at the *end* tag, so the
    // start event is patched in place once the text has been seen. Headings do
    // not nest, so one slot is enough.
    let mut heading: Option<(usize, String)> = None;
    let mut slugger = Slugger::default();

    for ev in parser {
        match ev {
            e @ Event::Start(Tag::Heading { .. }) => {
                heading = Some((events.len(), String::new()));
                events.push(e);
            }
            Event::End(TagEnd::Heading(level)) => {
                if let Some((at, text)) = heading.take() {
                    if let Event::Start(Tag::Heading { id, .. }) = &mut events[at] {
                        // `ENABLE_HEADING_ATTRIBUTES` is off, so `id` is always
                        // `None` today; honour an explicit one anyway rather
                        // than silently overwriting it if that ever changes.
                        let slug = match id.take() {
                            Some(explicit) => slugger.reserve(&explicit),
                            None => slugger.slug(&text),
                        };
                        *id = Some(slug.into());
                    }
                }
                events.push(Event::End(TagEnd::Heading(level)));
            }
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
                None => {
                    if let Some((_, h)) = &mut heading {
                        h.push_str(&t);
                    }
                    events.push(Event::Text(t));
                }
            },
            // Inline code is part of the text a reader sees in a heading, so it
            // is part of the slug.
            Event::Code(c) => {
                if let Some((_, h)) = &mut heading {
                    h.push_str(&c);
                }
                events.push(Event::Code(c));
            }
            // A setext heading can span lines; the break reads as a space.
            e @ (Event::SoftBreak | Event::HardBreak) => {
                if let Some((_, h)) = &mut heading {
                    h.push(' ');
                }
                events.push(e);
            }
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
            // Because it *is* text once rendered, it also feeds the slug.
            Event::Html(h) | Event::InlineHtml(h) => {
                if let Some((_, acc)) = &mut heading {
                    acc.push_str(&h);
                }
                events.push(Event::Text(h));
            }
            other => events.push(other),
        }
    }

    for (at, html) in highlight_blocks(&blocks, code_theme) {
        events[at] = Event::Html(html.into());
    }

    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, events.into_iter());
    html
}

/// Fallback id for a heading whose text slugs to nothing — `## ***`, or a
/// heading that is only an image.
const EMPTY_SLUG: &str = "section";

/// Mints the `id` attribute for each heading in one document.
///
/// The scheme is GitHub's, because a `[jump](#some-heading)` written inside a
/// repo's own markdown was written against GitHub's:
///
/// 1. trim, then lowercase (`str::to_lowercase`, so it is Unicode-aware);
/// 2. keep alphanumerics, `-` and `_`; turn each whitespace char into a `-`
///    (runs are *not* collapsed, matching `github-slugger`); drop the rest;
/// 3. an empty result becomes [`EMPTY_SLUG`].
///
/// Repeats are then disambiguated by appending `-1`, `-2`, … — the same problem
/// `locate` has with a repeated heading, and the same answer: the first
/// occurrence keeps the bare name and later ones are numbered in document
/// order. Unlike GitHub, the numbered candidate is itself checked for a clash,
/// so a document containing both `## A` twice and a literal `## A 1` still
/// comes out with every id distinct — ids are what anchoring will key on, so
/// uniqueness is a guarantee here rather than a near-certainty.
///
/// Slugs are stable across renders of the same source: nothing here depends on
/// anything but the heading text and what came before it in the document.
#[derive(Default)]
pub struct Slugger {
    seen: HashSet<String>,
}

impl Slugger {
    /// The id for a heading whose rendered text is `text`.
    pub fn slug(&mut self, text: &str) -> String {
        let mut out = String::with_capacity(text.len());
        for ch in text.trim().to_lowercase().chars() {
            if ch.is_alphanumeric() || ch == '-' || ch == '_' {
                out.push(ch);
            } else if ch.is_whitespace() {
                out.push('-');
            }
        }
        if out.is_empty() {
            out.push_str(EMPTY_SLUG);
        }
        self.reserve(&out)
    }

    /// Claim `base` verbatim if it is free, otherwise the first `base-N` that
    /// is. Used directly for an id the source stated itself.
    pub fn reserve(&mut self, base: &str) -> String {
        if self.seen.insert(base.to_string()) {
            return base.to_string();
        }
        for n in 1.. {
            let candidate = format!("{base}-{n}");
            if self.seen.insert(candidate.clone()) {
                return candidate;
            }
        }
        unreachable!("the range is unbounded")
    }
}

/// A fenced code block awaiting highlighting, and the event slot it belongs in.
struct Block {
    at: usize,
    lang: String,
    text: String,
}

/// Highlight every block, spreading them across the available cores. Returns
/// `(event index, html)` pairs in arbitrary order.
fn highlight_blocks(blocks: &[Block], theme_name: &str) -> Vec<(usize, String)> {
    if blocks.len() < 2 {
        let theme = code_theme(theme_name);
        return blocks
            .iter()
            .map(|b| (b.at, highlight_code(&b.lang, &b.text, theme)))
            .collect();
    }

    // Force the lazy syntect statics here rather than letting the workers race
    // into `OnceLock::get_or_init`, which would just serialize them again.
    syntaxes();
    let theme = code_theme(theme_name);

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
            scope.spawn(|| {
                while let Some(b) = blocks.get(next.fetch_add(1, Ordering::Relaxed)) {
                    let html = highlight_code(&b.lang, &b.text, theme);
                    out.lock().unwrap().push((b.at, html));
                }
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
/// The span reported is that of the first and last **non-whitespace** character
/// of the quote. A selection that happens to end in a newline does not claim
/// the following line.
///
/// To locate many quotes in the same document, build a [`SourceIndex`] once and
/// call [`SourceIndex::locate`] instead — this function throws away every index
/// it builds.
pub fn locate(source: &str, prefix: &str, quote: &str, suffix: &str) -> Option<Location> {
    SourceIndex::new(source).locate(prefix, quote, suffix)
}

/// How much context either side is used to disambiguate, in bytes. The
/// frontend sends more than this; anything past the window buys nothing, and
/// the comparison runs once per candidate occurrence.
const CONTEXT_WINDOW: usize = 64;

/// The last `max` bytes of `s`, rounded outward to a char boundary.
fn tail(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut i = s.len() - max;
    while !s.is_char_boundary(i) {
        i += 1;
    }
    &s[i..]
}

/// The first `max` bytes of `s`, rounded inward to a char boundary.
fn head(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut i = max;
    while !s.is_char_boundary(i) {
        i -= 1;
    }
    &s[..i]
}

fn common_suffix_len(a: &[u8], b: &[u8]) -> usize {
    a.iter()
        .rev()
        .zip(b.iter().rev())
        .take_while(|(x, y)| x == y)
        .count()
}

fn common_prefix_len(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b.iter()).take_while(|(x, y)| x == y).count()
}

/// Byte offsets of every occurrence of `needle` in `hay`, **including
/// overlapping ones**.
///
/// `str::match_indices` resumes after each match and so cannot see an
/// occurrence that begins inside the previous one. That is not a corner case
/// here: a quote dragged across a repeated block (a config sample, a table, a
/// repeated heading) is periodic, and the copy the reader actually selected is
/// routinely one of the overlapping ones `match_indices` skips.
fn occurrences<'h>(hay: &'h str, needle: &'h str) -> impl Iterator<Item = usize> + 'h {
    let mut from = 0;
    std::iter::from_fn(move || {
        let pos = from + hay.get(from..)?.find(needle)?;
        from = pos + 1;
        while from < hay.len() && !hay.is_char_boundary(from) {
            from += 1;
        }
        Some(pos)
    })
}

/// Byte offset of the occurrence of `needle` in `hay` that best fits the
/// evidence an anchor carries: the context either side, and — on a re-anchor —
/// where the quote was last found.
///
/// Without context this is `hay.find(needle)`: the first occurrence, which is
/// only right by luck when the quote is repeated. With context, occurrences are
/// scored by how many bytes of `before` they share with the text immediately to
/// their left plus how many bytes of `after` they share with the text to their
/// right, and the best-scoring one wins.
///
/// Scoring rather than requiring an exact context match matters because the
/// context the frontend can supply comes from the rendered DOM: it has lost the
/// markdown syntax (`**`, `` ` ``, link brackets) that the source still carries,
/// so it agrees with the source for a few characters and then diverges. A
/// partial agreement is still enough to pick between two copies of a quote.
///
/// Two copies of the same block are byte-identical *including* their context,
/// though, and no amount of scoring separates those. `hint` does: an anchor
/// that has not moved should not jump to a different copy just because that
/// copy comes first in the file. Occurrences are produced in order, so the
/// nearest full context match is either the last one before the hint or the
/// first one at or after it, and the scan can stop as soon as it has both.
fn best_match(
    hay: &str,
    needle: &str,
    before: &str,
    after: &str,
    hint: Option<usize>,
) -> Option<usize> {
    if before.is_empty() && after.is_empty() {
        return hay.find(needle);
    }
    let before = tail(before, CONTEXT_WINDOW);
    let after = head(after, CONTEXT_WINDOW);
    let perfect = before.len() + after.len();

    let dist = |pos: usize| hint.map_or(0, |h| h.abs_diff(pos));
    let mut best: Option<(usize, usize)> = None; // (score, position)
    let mut perfect_below: Option<usize> = None;

    for pos in occurrences(hay, needle) {
        let score = context_score(hay, pos, needle.len(), before, after);
        if score >= perfect {
            match hint {
                None => return Some(pos),
                Some(h) if pos < h => perfect_below = Some(pos),
                Some(h) => {
                    return Some(match perfect_below {
                        Some(b) if h - b < pos - h => b,
                        _ => pos,
                    })
                }
            }
        }
        // Better context wins; between equal contexts, the copy nearer to
        // where the highlight already was.
        if best.map_or(true, |(s, p)| {
            score > s || (score == s && dist(pos) < dist(p))
        }) {
            best = Some((score, pos));
        }
    }
    perfect_below.or(best.map(|(_, pos)| pos))
}

fn context_score(hay: &str, pos: usize, needle_len: usize, before: &str, after: &str) -> usize {
    common_suffix_len(&hay.as_bytes()[..pos], before.as_bytes())
        + common_prefix_len(&hay.as_bytes()[pos + needle_len..], after.as_bytes())
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
    ///
    /// The quote is trimmed first, so all three tiers agree on what the span
    /// of a selection is: the first through the last non-whitespace character.
    /// Tier 3 could never report anything else — it works from a source with
    /// the whitespace removed — and an untrimmed tier 2 disagreed with it
    /// whenever a selection began or ended on a line break.
    pub fn locate(&mut self, prefix: &str, quote: &str, suffix: &str) -> Option<Location> {
        self.locate_near(prefix, quote, suffix, 0)
    }

    /// [`SourceIndex::locate`], told where the quote was last found.
    ///
    /// `hint_line` is a 1-based line number, or 0 for "no idea" — the first
    /// anchoring of a fresh highlight. Re-anchoring always has one, and it is
    /// the only thing that can separate two byte-identical copies of a block,
    /// which quote plus context cannot. It is a hint in the strict sense: a
    /// wrong or stale one costs a little time and never a worse answer.
    pub fn locate_near(
        &mut self,
        prefix: &str,
        quote: &str,
        suffix: &str,
        hint_line: usize,
    ) -> Option<Location> {
        let quote = quote.trim();
        if quote.is_empty() {
            return None;
        }

        let has_context = !prefix.is_empty() || !suffix.is_empty();

        // 1) Exact match with context. Only worth trying when there *is*
        // context — otherwise the needle is just the quote and this repeats
        // tier 2's scan verbatim.
        if has_context {
            let needle = format!("{prefix}{quote}{suffix}");
            if let Some(pos) = self.source.find(&needle) {
                return Some(self.span(pos + prefix.len(), quote.len()));
            }
        }

        // 2) Exact match of the quote alone — but only when there is no
        // context, because then there is nothing better to go on. With context
        // this tier is actively harmful: it takes the first exact occurrence
        // while ignoring the context that says the quote came from a later
        // copy, and the context it would have to weigh is *rendered*, which
        // only tier 3 can compare against the source.
        if !has_context {
            if let Some(pos) = self.source.find(quote) {
                return Some(self.span(pos, quote.len()));
            }
        }

        // 3) Whitespace-normalized match (rendered selections collapse
        // whitespace). The context is stripped the same way as the quote —
        // collapsed context no more matches raw source than a collapsed quote
        // does, so tier 3 has to do its own disambiguation or the quote lands
        // on whichever copy comes first in the file.
        let quote_ns = strip_ws(quote);
        if quote_ns.is_empty() {
            return None;
        }
        let (prefix_ns, suffix_ns) = (strip_ws(prefix), strip_ws(suffix));
        // Where the hint line begins in the source, before the index is
        // borrowed — `line_starts` and `stripped` are both fields of `self`.
        let hint_src = hint_line
            .checked_sub(1)
            .and_then(|i| self.line_starts.get(i).copied());
        let source = self.source;
        let stripped = self.stripped.get_or_insert_with(|| Stripped::build(source));
        let hint = hint_src.map(|b| stripped.offset_of(b));
        let (start, end) = stripped.find(&quote_ns, &prefix_ns, &suffix_ns, hint)?;
        Some(Location {
            line_start: self.line_at(start),
            line_end: self.line_at(end),
        })
    }
}

/// Every non-whitespace char of `s`, in order.
fn strip_ws(s: &str) -> String {
    s.chars().filter(|c| !c.is_whitespace()).collect()
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

    /// Offset into `text` of the first char at or after source byte `src`.
    /// `source_offsets` is non-decreasing, so this is a binary search.
    ///
    /// The result needs no rounding to a char boundary: every byte of a char
    /// repeats that char's offset, so the predicate cannot flip partway through
    /// one and the partition point can only land on a boundary.
    fn offset_of(&self, src: usize) -> usize {
        self.source_offsets.partition_point(|&o| o < src)
    }

    /// Byte offsets in the *original* source of the first and last char of
    /// `quote_ns` (itself already whitespace-stripped). `prefix_ns`/`suffix_ns`
    /// are the stripped context, used to pick between repeated occurrences, and
    /// `hint` is where the quote was last found, as an offset into `text`.
    fn find(
        &self,
        quote_ns: &str,
        prefix_ns: &str,
        suffix_ns: &str,
        hint: Option<usize>,
    ) -> Option<(usize, usize)> {
        let at = best_match(&self.text, quote_ns, prefix_ns, suffix_ns, hint)?;
        let last = at + quote_ns.len().saturating_sub(1);
        let start = *self.source_offsets.get(at)?;
        let end = self.source_offsets.get(last).copied().unwrap_or(start);
        Some((start, end))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- tenet 4: raw HTML is escaped, never executed ---------------------

    #[test]
    fn block_html_is_escaped_not_emitted() {
        let html = render("<script>alert(1)</script>\n");
        assert!(!html.contains("<script"), "live script tag in {html:?}");
        assert!(html.contains("&lt;script&gt;"), "not escaped: {html:?}");
    }

    #[test]
    fn inline_html_is_escaped_not_emitted() {
        let html = render("Some *text* <img src=x onerror=alert(1)> more.\n");
        assert!(!html.contains("<img"), "live img tag in {html:?}");
        assert!(html.contains("&lt;img"), "not escaped: {html:?}");
        // The surrounding markdown still renders — escaping is not a bail-out.
        assert!(html.contains("<em>text</em>"), "markdown lost: {html:?}");
    }

    #[test]
    fn escaping_does_not_double_encode_its_own_output() {
        // `&` must be replaced first, or `<` -> `&lt;` -> `&amp;lt;`.
        assert_eq!(escape_html("a & b < c > d"), "a &amp; b &lt; c &gt; d");
        assert_eq!(escape_html("&lt;"), "&amp;lt;");
    }

    #[test]
    fn a_link_destination_is_not_a_way_to_smuggle_markup() {
        // pulldown-cmark escapes attribute values itself; assert it, because a
        // future switch of renderer is exactly when this would regress.
        let html = render("[x](https://e.com/\"onmouseover=\"alert(1))\n");
        assert!(!html.contains("onmouseover=\"alert"), "{html:?}");
    }

    // ---- heading slugs ----------------------------------------------------

    #[test]
    fn slug_follows_the_github_scheme() {
        let mut s = Slugger::default();
        assert_eq!(s.slug("  Hello, World!  "), "hello-world");
        assert_eq!(s.slug("Keep_under-scores"), "keep_under-scores");
        // Whitespace runs are *not* collapsed, matching github-slugger.
        assert_eq!(s.slug("a  b"), "a--b");
        // Punctuation is dropped, not turned into a separator.
        assert_eq!(s.slug("C++ / Rust"), "c--rust");
        // Lowercasing is Unicode-aware.
        assert_eq!(s.slug("ÉCOLE"), "école");
    }

    #[test]
    fn a_heading_that_slugs_to_nothing_gets_the_fallback() {
        let mut s = Slugger::default();
        assert_eq!(s.slug("***"), EMPTY_SLUG);
        // ...and a second one is still distinct.
        assert_eq!(s.slug("!!!"), format!("{EMPTY_SLUG}-1"));
    }

    #[test]
    fn repeats_are_numbered_in_document_order() {
        let mut s = Slugger::default();
        assert_eq!(s.slug("Intro"), "intro");
        assert_eq!(s.slug("Intro"), "intro-1");
        assert_eq!(s.slug("Intro"), "intro-2");
    }

    #[test]
    fn a_numbered_candidate_is_itself_collision_checked() {
        // The guarantee that goes beyond GitHub: `## A`, `## A 1`, `## A`
        // must still produce three distinct ids.
        let mut s = Slugger::default();
        assert_eq!(s.slug("A"), "a");
        assert_eq!(s.slug("A 1"), "a-1");
        assert_eq!(s.slug("A"), "a-2");
    }

    #[test]
    fn slugs_are_stable_across_renders_of_the_same_source() {
        let src = "# Intro\n\ntext\n\n## Intro\n\nmore\n";
        assert_eq!(render(src), render(src));
        assert!(render(src).contains("id=\"intro-1\""), "{}", render(src));
    }

    // ---- occurrences ------------------------------------------------------

    #[test]
    fn occurrences_includes_overlapping_matches() {
        // `str::match_indices` yields only 0 and 4 here.
        let hits: Vec<_> = occurrences("aaaa", "aa").collect();
        assert_eq!(hits, vec![0, 1, 2]);
    }

    #[test]
    fn occurrences_steps_over_whole_chars() {
        // Advancing by one *byte* into a multi-byte char would panic on the
        // next slice; the scan rounds up to a char boundary instead.
        let hits: Vec<_> = occurrences("ééé", "é").collect();
        assert_eq!(hits, vec![0, 2, 4]);
    }

    // ---- locate: the three tiers -----------------------------------------

    #[test]
    fn tier1_exact_match_with_context_picks_the_right_copy() {
        let src = "one dup two\n\nthree dup four\n";
        let loc = locate(src, "three ", "dup", " four").expect("located");
        assert_eq!((loc.line_start, loc.line_end), (3, 3));
    }

    #[test]
    fn tier2_exact_quote_alone_when_there_is_no_context() {
        let src = "alpha\nbeta\ngamma\n";
        let loc = locate(src, "", "beta", "").expect("located");
        assert_eq!((loc.line_start, loc.line_end), (2, 2));
    }

    #[test]
    fn tier3_matches_a_rendered_selection_across_a_line_break() {
        // What `getSelection().toString()` yields: whitespace collapsed, so it
        // matches no substring of the source. Only the stripped index can.
        let src = "a paragraph that\nwraps across lines\n";
        let loc = locate(src, "", "paragraph that wraps", "").expect("located");
        assert_eq!((loc.line_start, loc.line_end), (1, 2));
    }

    #[test]
    fn the_span_runs_first_to_last_non_whitespace_char() {
        // A selection ending in a newline must not claim the following line.
        let src = "first line\nsecond line\nthird line\n";
        let loc = locate(src, "", "second line\n", "").expect("located");
        assert_eq!((loc.line_start, loc.line_end), (2, 2));
        // Leading whitespace likewise does not claim the line above.
        let loc = locate(src, "", "\nsecond line", "").expect("located");
        assert_eq!((loc.line_start, loc.line_end), (2, 2));
    }

    #[test]
    fn an_edited_quote_is_not_found() {
        // What makes a highlight Stale rather than silently re-anchoring.
        assert!(locate("alpha\nbeta\n", "", "no such text", "").is_none());
        assert!(locate("alpha\n", "", "   ", "").is_none());
    }

    #[test]
    fn a_hint_separates_two_byte_identical_copies() {
        // Identical block *including* its context — scoring cannot separate
        // these, so the hint is the only evidence left.
        let block = "before para\n\nshared line one\nshared line two\n\nafter para\n";
        let src = format!("{block}\n---\n\n{block}");
        let quote = "shared line one shared line two";
        let (prefix, suffix) = ("before para ", " after para");
        // Copies start on lines 3 and 12.
        let at = |hint| {
            SourceIndex::new(&src)
                .locate_near(prefix, quote, suffix, hint)
                .expect("located")
                .line_start
        };
        assert_eq!(at(3), 3);
        assert_eq!(at(12), 12);
        // A hint is a hint: a stale one that lands nearer the second copy
        // still resolves there, and never produces a worse answer than none.
        assert_eq!(at(11), 12);
    }

    #[test]
    fn without_context_the_first_occurrence_wins() {
        // `best_match` short-circuits to `find` when there is nothing to score
        // against, so a hint alone cannot move the answer. Documented here
        // because it is the difference between this and the test above.
        let src = "dup\n\ndup\n";
        let mut index = SourceIndex::new(src);
        assert_eq!(index.locate_near("", "dup", "", 3).unwrap().line_start, 1);
    }

    #[test]
    fn a_shared_index_agrees_with_a_one_shot_locate() {
        // `reanchor_file` reuses one index across every highlight in a file;
        // that must not change any answer.
        let src = "alpha beta\n\ngamma delta\n\nalpha beta\n";
        let mut index = SourceIndex::new(src);
        for (prefix, quote, suffix) in [
            ("", "gamma delta", ""),
            ("", "alpha beta", ""),
            ("delta\n\n", "alpha beta", "\n"),
        ] {
            assert_eq!(
                index.locate(prefix, quote, suffix).map(|l| l.line_start),
                locate(src, prefix, quote, suffix).map(|l| l.line_start),
                "disagreement on {quote:?}",
            );
        }
    }
}
