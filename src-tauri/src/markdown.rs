//! Markdown -> HTML rendering.
//!
//! Security: raw HTML embedded in the markdown source is **escaped**, never
//! passed through, so a `<script>` in a document cannot execute inside the
//! webview (which has IPC access). Only syntect's code-block HTML — which we
//! generate ourselves — is emitted as trusted markup.
//!
//! Fenced code blocks are syntax-highlighted server-side via syntect.

use pulldown_cmark::{CodeBlockKind, CowStr, Event, Options, Parser, Tag, TagEnd};
use serde::Serialize;
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet, VecDeque};
use std::hash::{Hash, Hasher};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
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

/// Pay the syntect dump load on a background thread instead of the first paint.
///
/// `syntaxes()`/`themes()` are lazy, so without this the *first document
/// render* pays the flate2+bincode load of the bundled dumps — serial with
/// first paint, which is the one place it is visible (`syntect_cold` in
/// `benches/render.rs` is what it costs). Called from `.setup()`, after the
/// window exists, beside `agent::claude::warm` and for the same reason. A
/// render that arrives before the thread finishes blocks on the `OnceLock`
/// exactly as it always did — never slower, usually fully overlapped.
pub fn warm() {
    std::thread::spawn(|| {
        syntaxes();
        themes();
    });
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

/// Syntect emits one `<span style="color:…">` per token, and on the 2MB mixed
/// corpus doc that is 43,557 spans and 4.1MB of HTML for 2MB of source. Two
/// ways of shrinking that were measured in Chromium (n=15 per arm, a fresh page
/// per sample, arm order alternated — reusing one page swung the same
/// comparison by 20 points in *both* directions) and **neither is worth
/// taking**:
///
/// - **Classes instead of inline styles.** Only 9 distinct style values appear
///   in the whole document, so this is 13.1% fewer bytes. Parse -5%, layout
///   +1%, total a wash: the bytes come back as selector matching against 44k
///   elements, which an inline style skips entirely.
/// - **Dropping the spans whose colour is already the theme foreground.** Those
///   are 23,643 of the 43,557 (54%) and 20.7% of the bytes. Parse -20%, layout
///   -4% — real, but ~34ms of a ~600ms operation, and it would require dreamd
///   to emit and escape that markup itself. Today only syntect's own output is
///   trusted (tenet 4), and moving the escaping here to save 4% of a cost that
///   is dominated by text layout is the wrong trade.
///
/// The number that dominates is the webview laying out the document, and it is
/// insensitive to the shape of the markup. See perf/README.md.
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

// ---- the code-block memo -------------------------------------------------
//
// A `:w` in Neovim re-renders the whole document, and syntect dominates that
// cost on anything with code — yet a save typically changes one block and
// leaves every other one byte-identical. So highlighted blocks are memoized
// process-wide, next to `syntaxes()`/`themes()` and for the same reason: this
// is a property of the binary, not of a window, and putting it in `AppState`
// would thread a Tauri handle through `render_with` into a module the benches
// and `mcp` use.
//
// Soundness rests on `highlight_code` being a pure function of
// `(lang, code, theme)`: `find_syntax_by_token` reads the immutable static
// `SyntaxSet` and `highlighted_html_for_string` builds a fresh `HighlightLines`
// per call. The key carries the theme **name** rather than the resolved theme,
// which is what makes a light/dark switch actually re-colour the code — the
// colours are baked into the HTML, not into CSS. Two names that both fall back
// to `CODE_THEME` cost one duplicate entry and never a wrong one.

/// Cap on what the memo holds, summed over every stored key *and* its html.
/// Not optional: `render_agent_text` feeds this cache from an agent's replies,
/// which are unbounded in a way a repo's files are not.
const CODE_CACHE_MAX_BYTES: usize = 16 * 1024 * 1024;

/// One memoized block. The **whole** key is stored and compared on every hit —
/// the hash only narrows the search, so a 64-bit collision is a miss rather
/// than someone else's code block rendered in place of this one.
struct CodeEntry {
    theme: String,
    lang: String,
    code: String,
    /// `Arc` so a hit is a refcount bump rather than a clone of the rendered
    /// HTML — on the warm save path every block is a hit, and copying several
    /// MB under the cache mutex was most of what the lock protected. An
    /// evicted entry a render still holds stays alive until that render is
    /// done with it; the cap bounds what the cache *retains*, not what is
    /// momentarily in flight.
    html: Arc<str>,
}

impl CodeEntry {
    fn matches(&self, theme: &str, lang: &str, code: &str) -> bool {
        self.theme == theme && self.lang == lang && self.code == code
    }

    fn bytes(&self) -> usize {
        self.theme.len() + self.lang.len() + self.code.len() + self.html.len()
    }
}

#[derive(Default)]
struct CodeCache {
    /// Key hash -> every entry that hashed to it, oldest first.
    buckets: HashMap<u64, Vec<CodeEntry>>,
    /// One hash per live entry, in insertion order: the FIFO eviction queue.
    order: VecDeque<u64>,
    bytes: usize,
}

impl CodeCache {
    fn get(&self, hash: u64, theme: &str, lang: &str, code: &str) -> Option<Arc<str>> {
        self.buckets
            .get(&hash)?
            .iter()
            .find(|e| e.matches(theme, lang, code))
            .map(|e| Arc::clone(&e.html))
    }

    fn insert(&mut self, hash: u64, entry: CodeEntry) {
        // A block repeated within one document misses twice and comes back
        // twice; storing it twice would also make the byte count a lie.
        let bucket = self.buckets.entry(hash).or_default();
        if bucket
            .iter()
            .any(|e| e.matches(&entry.theme, &entry.lang, &entry.code))
        {
            return;
        }
        let bytes = entry.bytes();
        if bytes > CODE_CACHE_MAX_BYTES {
            return;
        }
        bucket.push(entry);
        self.order.push_back(hash);
        self.bytes += bytes;
        while self.bytes > CODE_CACHE_MAX_BYTES && !self.order.is_empty() {
            self.evict_oldest();
        }
    }

    fn evict_oldest(&mut self) {
        let Some(hash) = self.order.pop_front() else {
            return;
        };
        let Some(bucket) = self.buckets.get_mut(&hash) else {
            return;
        };
        if !bucket.is_empty() {
            // Entries are pushed to the back of their bucket, so index 0 is the
            // oldest insertion carrying this hash — the one the queue popped.
            self.bytes -= bucket.remove(0).bytes();
        }
        if bucket.is_empty() {
            self.buckets.remove(&hash);
        }
    }
}

fn code_cache() -> &'static Mutex<CodeCache> {
    static C: OnceLock<Mutex<CodeCache>> = OnceLock::new();
    C.get_or_init(Default::default)
}

fn code_key_hash(theme: &str, lang: &str, code: &str) -> u64 {
    let mut h = DefaultHasher::new();
    theme.hash(&mut h);
    lang.hash(&mut h);
    code.hash(&mut h);
    h.finish()
}

/// Empty the code memo. A test and bench hook — the app never calls it, since
/// every entry is keyed on everything that decides its html.
pub fn clear_code_cache() {
    *code_cache().lock().unwrap() = CodeCache::default();
}

/// Render markdown source to a sanitized HTML string, with fenced code blocks
/// highlighted using the default syntect theme.
pub fn render(source: &str) -> String {
    render_with(source, CODE_THEME)
}

/// As [`render`], but with the syntect theme named by the active dreamd
/// palette's `--syntax-theme`. An unknown name falls back to [`CODE_THEME`].
pub fn render_with(source: &str, code_theme: &str) -> String {
    with_events(source, code_theme, |events| {
        // Rendered HTML reliably outgrows its source (tags, spans, escapes);
        // starting at double skips most of the doubling reallocs on a large doc.
        let mut html = String::with_capacity(source.len() * 2);
        pulldown_cmark::html::push_html(&mut html, events.into_iter());
        html
    })
}

/// A rendered document: its HTML in one buffer, plus where each top-level
/// block ends in it.
///
/// The block boundaries are what the frontend's save diff compares (see
/// [`render_blocks`]) — but the blocks are also concatenated straight back
/// together on the wire (`frame_blocks` in main.rs), so carrying them as a
/// `Vec<String>` meant ~1300 growing allocations on the way out and a second
/// 4MB copy on the way in, for a document that was one string at both ends.
/// Offsets into one buffer cost neither.
pub struct Rendered {
    html: String,
    /// Byte offset one past the end of each block. Non-decreasing, and the
    /// last entry is `html.len()`.
    ends: Vec<usize>,
}

impl Rendered {
    /// The whole document, which is exactly what [`render_with`] returns for
    /// the same input — the identity a property test below pins.
    pub fn html(&self) -> &str {
        &self.html
    }

    /// The blocks, in document order.
    pub fn blocks(&self) -> impl Iterator<Item = &str> + '_ {
        let mut from = 0;
        self.ends.iter().map(move |&end| {
            let block = &self.html[from..end];
            from = end;
            block
        })
    }

    pub fn block_count(&self) -> usize {
        self.ends.len()
    }

    /// Close the block that ends at the current end of `html`.
    ///
    /// Folding a segment that rendered no element of its own into the block
    /// before it is, in this shape, simply declining to record a boundary —
    /// where the `Vec<String>` version had to concatenate two strings.
    /// Escaped raw HTML (tenet 4 re-emits it as text) is the case that exists:
    /// it renders as bare text starting with `&lt;`, never `<`. The frontend's
    /// splice indexes blocks by element, so one block must be one top-level
    /// element plus whatever trailing text belongs to it. A document that
    /// *opens* with such text keeps it as block zero; the frontend sees a
    /// block that does not start with `<` and declines to patch that document
    /// at all.
    fn close(&mut self, at: usize) {
        if self.ends.is_empty() || self.html.as_bytes().get(at) == Some(&b'<') {
            self.ends.push(self.html.len());
        } else {
            *self.ends.last_mut().unwrap() = self.html.len();
        }
    }
}

/// [`render_with`], with the boundary of every top-level block recorded.
///
/// Those boundaries are the shape the frontend's save-path diff wants:
/// comparing backend strings block by block is a memcmp, where diffing one
/// concatenated document cost a full template parse plus an `outerHTML`
/// re-serialization per save — measured at 130ms of a 209ms save loop before
/// this existed. The guarantee that makes it safe is byte-identity:
/// `render_blocks(s).html() == render_with(s)`, pinned by a property test
/// below, so the two entry points can never disagree about what a document
/// renders to.
///
/// One caveat is structural: pulldown's `push_html` numbers footnotes
/// statefully *across* a single call, so a document that uses them cannot be
/// rendered per-block without renumbering — those fall back to a single
/// segment, which the frontend treats as one big block (a full write per
/// save, exactly the pre-blocks behaviour).
pub fn render_blocks(source: &str, code_theme: &str) -> Rendered {
    with_events(source, code_theme, |events| {
        // Rendered HTML reliably outgrows its source; starting at double skips
        // most of the doubling reallocs, the same bargain `render_with` makes.
        let mut out = Rendered {
            html: String::with_capacity(source.len() * 2),
            ends: Vec::new(),
        };
        // A top-level block is the events from a depth-0 `Start` through its
        // matching `End` — or a single standalone depth-0 event (a `Rule`, or
        // the `Html` a highlighted fence became; `Start(CodeBlock)` never
        // reaches the stream, see the builder above).
        //
        // How many events each block spans is worked out first, in a scan that
        // moves nothing, so that the events themselves travel exactly once —
        // straight into `push_html`. Collecting each block into a scratch
        // `Vec<Event>` on the way cost a push and a drain per event, and there
        // are hundreds of thousands of them in a 2MB document.
        //
        // The footnote question is answered by the same pass, and stops it: a
        // document that uses them cannot be split at all, so there is nothing
        // left to learn once one has been seen.
        let mut runs: Vec<usize> = Vec::new();
        let mut depth = 0usize;
        let mut start = 0usize;
        let mut footnotes = false;
        for (i, ev) in events.iter().enumerate() {
            match ev {
                Event::FootnoteReference(_) => footnotes = true,
                Event::Start(tag) => {
                    footnotes |= matches!(tag, Tag::FootnoteDefinition(_));
                    depth += 1;
                }
                Event::End(_) => depth = depth.saturating_sub(1),
                _ => {}
            }
            if footnotes {
                break;
            }
            if depth == 0 {
                runs.push(i + 1 - start);
                start = i + 1;
            }
        }
        if footnotes {
            pulldown_cmark::html::push_html(&mut out.html, events.into_iter());
            out.ends.push(out.html.len());
            return out;
        }
        if start < events.len() {
            // An unbalanced stream cannot happen out of pulldown, but a
            // truncated segment silently dropped would be a missing block.
            runs.push(events.len() - start);
        }

        let mut rest = events.into_iter();
        for run in runs {
            let at = out.html.len();
            pulldown_cmark::html::push_html(&mut out.html, rest.by_ref().take(run));
            out.close(at);
        }
        out
    })
}

/// `s.encode_utf16().count()`, in one byte scan: every non-continuation byte
/// starts a scalar (one unit), and every 4-byte lead adds the surrogate
/// pair's second unit. The wire framing (`frame_blocks` in main.rs) sends
/// block lengths in these units so the frontend can decode the payload once
/// and `slice` per block — JS string offsets *are* UTF-16 units.
pub fn utf16_units(s: &str) -> usize {
    // The framing runs this over every byte of every render, so the two
    // vectorized counts below beat the one byte-at-a-time match they replace:
    // start from the byte length, drop the continuation bytes (a scalar's
    // trailing bytes are not units of their own) and add one for each 4-byte
    // lead (those scalars need a surrogate pair). `(b as i8) < -64` is the
    // continuation-byte test `str::chars().count()` uses, for the same reason.
    if s.is_ascii() {
        return s.len();
    }
    let bytes = s.as_bytes();
    let continuations = bytes.iter().filter(|&&b| (b as i8) < -64).count();
    let quads = bytes.iter().filter(|&&b| b >= 0xF0).count();
    s.len() - continuations + quads
}

/// Build the event stream — headings slugged, fences highlighted and spliced
/// back in — and hand it to `f`. A closure rather than a returned value
/// because the events borrow `rendered` (the fences' `Arc<str>`s), and the
/// two cannot leave the frame together.
fn with_events<R>(source: &str, code_theme: &str, f: impl FnOnce(Vec<Event>) -> R) -> R {
    let parser = Parser::new_ext(source, options());

    // Reserved rather than grown. The stream is hundreds of thousands of events
    // on a 2MB file and `Event` is wide, so growing from nothing copies the
    // whole buffer again on the way up — 6ms of `render/table/2m`'s 25ms.
    //
    // A fixed fraction of the source, and a compromise on purpose: how many
    // events a byte becomes swings twentyfold between prose and a table, and
    // both directions cost. Sizing it from the rate events actually arrive at
    // was tried and is worse — `into_offset_iter` plus a check per event cost
    // more than the doubling it removed (`render_blocks/mixed/2m` 10.2ms ->
    // 10.5ms). An eighth covers prose, code and mixed outright and leaves a
    // table one more growth; a quarter suits tables and costs everything else
    // more than it saves them.
    let mut events: Vec<Event> = Vec::with_capacity(source.len() / 8);
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

    // Borrowed into the events rather than moved: the rendered blocks are
    // `Arc<str>`s shared with the cache, and `push_html` only needs to read
    // them. `rendered` outlives the `into_iter` below, which is what makes the
    // borrow sound.
    let rendered = highlight_blocks(&blocks, code_theme);
    for (at, html) in &rendered {
        events[*at] = Event::Html(CowStr::Borrowed(html));
    }

    f(events)
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

/// Highlight every block, spreading the ones the memo does not already know
/// across the available cores. Returns `(event index, html)` pairs in arbitrary
/// order.
///
/// The cache lock is taken exactly **twice, on this thread**: once to partition
/// the blocks into hits and misses, once to store what the workers produced.
/// The workers never touch it — 640 blocks contending on one mutex would cost
/// more than the highlighting it saves.
fn highlight_blocks(blocks: &[Block], theme_name: &str) -> Vec<(usize, Arc<str>)> {
    let mut out: Vec<(usize, Arc<str>)> = Vec::with_capacity(blocks.len());

    // Hashes first, outside the lock: SipHash over every fence byte is real
    // work at 640 blocks, and none of it needs the cache.
    let hashes: Vec<u64> = blocks
        .iter()
        .map(|b| code_key_hash(theme_name, &b.lang, &b.text))
        .collect();

    // One entry per *unique* missed fence: `(index of its first block, key
    // hash, every event slot wanting its html)`. A document that repeats a
    // fence must be highlighted once and fanned out, not once per occurrence —
    // on the cold path (first render of a session, a theme switch) the memo
    // has answered nothing yet and cannot dedup for us, and the perf corpus is
    // 32 unique fences in 640. Keyed on the full `(lang, text)` pair, not the
    // hash — the hash only narrows, per the cache's own collision rule.
    let mut misses: Vec<(usize, u64, Vec<usize>)> = Vec::new();
    {
        let mut seen: HashMap<(&str, &str), usize> = HashMap::new();
        let cache = code_cache().lock().unwrap();
        for (i, b) in blocks.iter().enumerate() {
            match cache.get(hashes[i], theme_name, &b.lang, &b.text) {
                Some(html) => out.push((b.at, html)),
                None => match seen.entry((b.lang.as_str(), b.text.as_str())) {
                    std::collections::hash_map::Entry::Occupied(e) => misses[*e.get()].2.push(b.at),
                    std::collections::hash_map::Entry::Vacant(v) => {
                        v.insert(misses.len());
                        misses.push((i, hashes[i], vec![b.at]));
                    }
                },
            }
        }
    }
    if misses.is_empty() {
        return out;
    }

    let theme = code_theme(theme_name);
    // `(index into misses, html)`.
    let done: Vec<(usize, String)> = if misses.len() < 2 {
        misses
            .iter()
            .enumerate()
            .map(|(i, (b, _, _))| {
                let b = &blocks[*b];
                (i, highlight_code(&b.lang, &b.text, theme))
            })
            .collect()
    } else {
        // Force the lazy syntect statics here rather than letting the workers
        // race into `OnceLock::get_or_init`, which would just serialize them
        // again.
        syntaxes();

        let next = AtomicUsize::new(0);
        let fresh = Mutex::new(Vec::with_capacity(misses.len()));
        let workers = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(misses.len());

        // Blocks vary hugely in size, so workers pull the next index rather
        // than taking a fixed slice.
        std::thread::scope(|scope| {
            for _ in 0..workers {
                scope.spawn(|| loop {
                    let i = next.fetch_add(1, Ordering::Relaxed);
                    let Some((b, _, _)) = misses.get(i) else {
                        break;
                    };
                    let b = &blocks[*b];
                    let html = highlight_code(&b.lang, &b.text, theme);
                    fresh.lock().unwrap().push((i, html));
                });
            }
        });

        fresh.into_inner().unwrap()
    };

    {
        let mut cache = code_cache().lock().unwrap();
        for (i, html) in done {
            let (block_idx, hash, ref slots) = misses[i];
            let b = &blocks[block_idx];
            let html: Arc<str> = html.into();
            cache.insert(
                hash,
                CodeEntry {
                    theme: theme_name.to_string(),
                    lang: b.lang.clone(),
                    code: b.text.clone(),
                    html: Arc::clone(&html),
                },
            );
            for &at in slots {
                out.push((at, Arc::clone(&html)));
            }
        }
    }

    out
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
/// Without context this is the first occurrence, which is only right by luck
/// when the quote is repeated. With context, occurrences are
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
/// The decision itself, told where the occurrences are rather than finding
/// them.
///
/// Separating the two is what lets re-anchoring a file find every quote's
/// occurrences in **one** pass (see [`SourceIndex::locate_all`]) and still
/// reach exactly the answer a per-quote scan would: this function sees the
/// same positions in the same order either way, so the two cannot disagree —
/// which `examples/locate_check.rs` asserts over all 611 fixtures.
fn best_match_over(
    hay: &str,
    positions: impl IntoIterator<Item = usize>,
    needle_len: usize,
    before: &str,
    after: &str,
    hint: Option<usize>,
    mut exact: impl FnMut(usize) -> bool,
) -> Option<usize> {
    let mut positions = positions.into_iter();
    if before.is_empty() && after.is_empty() {
        return positions.next();
    }
    let before = tail(before, CONTEXT_WINDOW);
    let after = head(after, CONTEXT_WINDOW);
    let perfect = before.len() + after.len();

    let dist = |pos: usize| hint.map_or(0, |h| h.abs_diff(pos));
    // The nearer of a candidate already seen below the hint and one at or
    // after it. A dead heat goes to `pos`, which is what `pos - h == 0` on an
    // occurrence sitting exactly on the hint makes automatic.
    let nearer = |below: Option<usize>, pos: usize, h: usize| match below {
        Some(b) if h - b < pos - h => b,
        _ => pos,
    };
    let mut best: Option<(usize, usize)> = None; // (score, position)
    let mut perfect_below: Option<usize> = None;
    let mut exact_below: Option<usize> = None;

    for pos in positions {
        let score = context_score(hay, pos, needle_len, before, after);
        if score >= perfect {
            // Tier 1 rides here: an occurrence where the *source* still holds
            // `prefix + quote + suffix` byte for byte outranks one that merely
            // scores perfectly on stripped text. It can only ever be one of
            // these — stripping whitespace out of an exact match leaves the
            // context flush against the quote on both sides — which is what
            // lets that tier cost a few hundred byte comparisons here instead
            // of its own scan of the whole document, once per highlight.
            //
            // Exactness is a rank above a perfect score rather than a
            // first-past-the-post win: within either rank the hint picks, so
            // two exact copies of a block are told apart by where the mark
            // already was. The old tier-1 scan took whichever came first in
            // the file and could not be told otherwise.
            if exact(pos) {
                match hint {
                    None => return Some(pos),
                    Some(h) if pos < h => exact_below = Some(pos),
                    Some(h) => return Some(nearer(exact_below, pos, h)),
                }
            } else {
                match hint {
                    None => return Some(pos),
                    Some(h) if pos < h => perfect_below = Some(pos),
                    // An exact copy already passed outranks this one wherever
                    // it sat.
                    Some(h) => {
                        return Some(exact_below.unwrap_or_else(|| nearer(perfect_below, pos, h)))
                    }
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
    exact_below.or(perfect_below).or(best.map(|(_, pos)| pos))
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
        match self.plan(prefix, quote, suffix, hint_line) {
            Plan::Settled(loc) => loc,
            Plan::Normalized(job) => {
                let source = self.source;
                let span = {
                    let stripped = self.stripped()?;
                    let at =
                        job.pick(source, stripped, occurrences(&stripped.text, &job.quote_ns))?;
                    stripped.span(at, job.quote_ns.len())?
                };
                Some(Location {
                    line_start: self.line_at(span.0),
                    line_end: self.line_at(span.1),
                })
            }
        }
    }

    /// [`SourceIndex::locate_near`] for every anchor in a file at once.
    ///
    /// `Store::reanchor_file` runs on every save of the open document, and the
    /// whitespace-stripped tier is where a rendered selection always lands —
    /// so what it used to do was scan a 1.5MB haystack once *per mark*. Above
    /// [`BATCH_MIN`] anchors the quotes are searched for together, in a single
    /// pass, and each one then makes exactly the decision its own scan would
    /// have made from exactly the same occurrences.
    ///
    /// Results are positional: one entry per anchor, in order.
    pub fn locate_all(&mut self, anchors: &[Anchor<'_>]) -> Vec<Option<Location>> {
        let plans: Vec<Plan<'_>> = anchors
            .iter()
            .map(|a| self.plan(a.prefix, a.quote, a.suffix, a.hint_line))
            .collect();
        let source = self.source;
        // Stripped-text spans first, while the index is borrowed; the line
        // lookup below needs `self` back.
        let spans: Vec<Option<(usize, usize)>> = {
            let jobs: Vec<&Normalized<'_>> = plans
                .iter()
                .filter_map(|p| match p {
                    Plan::Normalized(job) => Some(job),
                    Plan::Settled(_) => None,
                })
                .collect();
            // Built only if something actually needs it. A file whose every
            // quote settled above tier 3 must not pay for a stripped copy of
            // the document nobody is going to search.
            match if jobs.is_empty() {
                None
            } else {
                self.stripped()
            } {
                None => vec![None; plans.len()],
                Some(stripped) => {
                    let hits = Hits::over(&stripped.text, &jobs);
                    let mut nth = 0;
                    plans
                        .iter()
                        .map(|p| match p {
                            Plan::Settled(_) => None,
                            Plan::Normalized(job) => {
                                let at = hits.of(nth, &job.quote_ns, &stripped.text);
                                nth += 1;
                                job.pick(source, stripped, at)
                                    .and_then(|at| stripped.span(at, job.quote_ns.len()))
                            }
                        })
                        .collect()
                }
            }
        };
        plans
            .iter()
            .zip(spans)
            .map(|(plan, span)| match plan {
                Plan::Settled(loc) => loc.clone(),
                Plan::Normalized(_) => span.map(|(start, end)| Location {
                    line_start: self.line_at(start),
                    line_end: self.line_at(end),
                }),
            })
            .collect()
    }

    /// The whitespace-stripped index, built on first use.
    ///
    /// `None` past `u32::MAX` bytes of source: tier 3 is unavailable for a
    /// document that large, not wrong — the slot is left empty so every call
    /// keeps trying rather than caching a permanent miss.
    fn stripped(&mut self) -> Option<&Stripped> {
        if self.stripped.is_none() {
            self.stripped = Stripped::build(self.source);
        }
        self.stripped.as_ref()
    }

    /// Everything about one anchor that can be decided before the stripped
    /// index exists — which for the tiers above tier 3 is the whole answer.
    fn plan<'q>(
        &self,
        prefix: &'q str,
        quote: &'q str,
        suffix: &'q str,
        hint_line: usize,
    ) -> Plan<'q> {
        let quote = quote.trim();
        if quote.is_empty() {
            return Plan::Settled(None);
        }

        let has_context = !prefix.is_empty() || !suffix.is_empty();

        // 1) Exact match with context — but **not from here**. Tier 1 is
        // folded into tier 3's pass below, and this branch is only the
        // fallback for a document too large to have the index that pass runs
        // on.
        //
        // Every tier-1 hit is necessarily one of the perfect-scoring
        // occurrences tier 3 already walks: if the source holds
        // `prefix + quote + suffix` byte for byte, then stripping whitespace
        // leaves `prefix_ns` immediately before the quote and `suffix_ns`
        // immediately after it, so the context scores full marks there. So
        // checking exactness at those occurrences finds every hit this scan
        // would have — and a scan that has to run whether it hits or not, once
        // per highlight, against the whole source, is what re-anchoring a file
        // spent two thirds of its time on. It never hits for a quote the
        // frontend sent: `getSelection().toString()` collapses the line breaks
        // a wrapped source still carries (0 of 611 corpus fixtures reach it).
        // It does hit on documents that are not hard-wrapped, which is why the
        // tier is preserved rather than dropped.
        //
        // One consequence, and it is the intended one: where several
        // occurrences all score perfectly and only a later one is byte-exact,
        // the answer is now the copy nearest `hint` rather than the first in
        // the file. Those copies are indistinguishable by the anchor's own
        // evidence — `examples/locate_check.rs` calls that AMBIGUOUS — and
        // "it was here a moment ago" is the tie-breaker the other tiers
        // already use.
        if has_context && exceeds_stripped_capacity(self.source.len()) {
            let needle = format!("{prefix}{quote}{suffix}");
            if let Some(pos) = self.source.find(&needle) {
                return Plan::Settled(Some(self.span(pos + prefix.len(), quote.len())));
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
                return Plan::Settled(Some(self.span(pos, quote.len())));
            }
        }

        // 3) Whitespace-normalized match (rendered selections collapse
        // whitespace). The context is stripped the same way as the quote —
        // collapsed context no more matches raw source than a collapsed quote
        // does, so tier 3 has to do its own disambiguation or the quote lands
        // on whichever copy comes first in the file.
        let quote_ns = strip_ws(quote);
        if quote_ns.is_empty() {
            return Plan::Settled(None);
        }
        Plan::Normalized(Normalized {
            quote,
            prefix,
            suffix,
            has_context,
            quote_ns,
            prefix_ns: strip_ws(prefix),
            suffix_ns: strip_ws(suffix),
            // Where the hint line begins in the source. Resolved here, against
            // `line_starts`, so the stripped index need never see a line
            // number.
            hint_src: hint_line
                .checked_sub(1)
                .and_then(|i| self.line_starts.get(i).copied()),
        })
    }
}

/// One anchor to locate: what [`SourceIndex::locate_near`] takes, as a value,
/// so a file's worth of them can be handed over together.
pub struct Anchor<'a> {
    pub prefix: &'a str,
    pub quote: &'a str,
    pub suffix: &'a str,
    /// 1-based line the quote was last found at, or 0 for "no idea".
    pub hint_line: usize,
}

/// Below this many anchors a file re-anchors one quote at a time. A single
/// `str::find` stops at the answer and runs at memory speed — 0.13ms against
/// the 2MB corpus document — where the shared pass reads the whole haystack
/// once whatever it is looking for, at about 5.7ms. So it only pays for itself
/// once there are enough quotes to share it between, and measured break-even
/// on `bench.reanchor_with_context` is around sixty.
const BATCH_MIN: usize = 64;

/// How many bytes of a quote the shared automaton is built over. Long enough
/// that a false candidate is rare in prose, short enough that hundreds of
/// quotes still make a small automaton.
const PROBE: usize = 16;

/// Above this many quotes the shared pass uses a contiguous NFA rather than a
/// DFA. The DFA reads the haystack about a third faster, but its transition
/// table is built from scratch on every save: measured 1.2ms and 0.78MB at 100
/// quotes, against 5.9ms and 3.5MB at 500, where the ~4.5ms it saves over the
/// pass no longer covers what it costs to build. Break-even is around four
/// hundred; this keeps a margin and holds the table under a megabyte.
const DFA_MAX: usize = 256;

enum Plan<'a> {
    /// Decided by a tier above the stripped one, or not at all.
    Settled(Option<Location>),
    /// Waiting for the whitespace-stripped pass.
    Normalized(Normalized<'a>),
}

/// One anchor, reduced to what the whitespace-stripped tier needs.
struct Normalized<'a> {
    quote: &'a str,
    prefix: &'a str,
    suffix: &'a str,
    has_context: bool,
    quote_ns: String,
    prefix_ns: String,
    suffix_ns: String,
    /// Where the quote was last found, as a byte offset into the *source*.
    hint_src: Option<usize>,
}

impl Normalized<'_> {
    /// The winning occurrence, given every occurrence of `quote_ns` in the
    /// stripped text.
    fn pick(
        &self,
        source: &str,
        stripped: &Stripped,
        positions: impl IntoIterator<Item = usize>,
    ) -> Option<usize> {
        let hint = self.hint_src.map(|b| stripped.offset_of(b));
        // Tier 1, in source coordinates: the quote starts at `at`, so the
        // needle would start `prefix.len()` bytes earlier. Written as three
        // byte comparisons rather than one slice-and-compare so that no index
        // has to land on a char boundary. The predicate is stated in *source*
        // coordinates — it is about bytes the stripped index has thrown away —
        // so the mapping happens here rather than in `best_match_over`, which
        // knows only about stripped text.
        let exact_src = |at: usize| {
            self.has_context
                && source.as_bytes()[at..].starts_with(self.quote.as_bytes())
                && source.as_bytes()[..at].ends_with(self.prefix.as_bytes())
                && source.as_bytes()[at + self.quote.len()..].starts_with(self.suffix.as_bytes())
        };
        best_match_over(
            &stripped.text,
            positions,
            self.quote_ns.len(),
            &self.prefix_ns,
            &self.suffix_ns,
            hint,
            |p| {
                stripped
                    .source_offsets
                    .get(p)
                    .is_some_and(|&o| exact_src(o as usize))
            },
        )
    }
}

/// Where every quote in a file occurs in the stripped text.
///
/// Two shapes behind one answer. With few enough anchors each quote is found
/// on demand with `str::find`, which is what a single `locate_near` does.
/// Above [`BATCH_MIN`] one Aho-Corasick pass finds all of them together: the
/// per-quote scan is memory-speed but runs once per mark, and a file with
/// hundreds of marks was reading a megabyte and a half of haystack for each.
/// Overlapping matches, because `occurrences` includes them and a quote
/// dragged across a repeated block is routinely one of the copies a
/// resume-after-match search would skip.
enum Hits {
    PerQuote,
    Shared(Vec<Vec<usize>>),
}

impl Hits {
    fn over(hay: &str, jobs: &[&Normalized<'_>]) -> Self {
        if jobs.len() < BATCH_MIN {
            return Hits::PerQuote;
        }
        // The automaton is built over a fixed-length *probe* of each quote
        // rather than the whole thing, and a candidate is confirmed against the
        // full quote when it is reported. A hundred quotes are ten kilobytes of
        // pattern where their probes are three, and the pass runs at the speed
        // the automaton fits in cache; a false candidate costs one comparison
        // and thirty-two stripped bytes of prose are enough that there are
        // almost none.
        let needles: Vec<&str> = jobs.iter().map(|j| head(&j.quote_ns, PROBE)).collect();
        let Ok(ac) = aho_corasick::AhoCorasick::builder()
            // `Standard` is the only kind that supports overlapping search,
            // which is the semantics `occurrences` has.
            .match_kind(aho_corasick::MatchKind::Standard)
            // Chosen rather than left to the builder, because the right
            // answer depends on how many marks the file has — see `DFA_MAX`.
            .kind(Some(if jobs.len() <= DFA_MAX {
                aho_corasick::AhoCorasickKind::DFA
            } else {
                aho_corasick::AhoCorasickKind::ContiguousNFA
            }))
            .build(&needles)
        else {
            // Nothing here is worth failing a re-anchor over — an automaton
            // that would not build simply means every quote scans for itself.
            return Hits::PerQuote;
        };
        let mut found = vec![Vec::new(); needles.len()];
        for m in ac.find_overlapping_iter(hay) {
            found[m.pattern().as_usize()].push(m.start());
        }
        Hits::Shared(found)
    }

    /// The occurrences of the `nth` job's quote, in increasing order — which
    /// is the order `occurrences` produces and the order `best_match_over`
    /// reads them in.
    fn of<'h>(
        &'h self,
        nth: usize,
        needle: &'h str,
        hay: &'h str,
    ) -> Box<dyn Iterator<Item = usize> + 'h> {
        match self {
            Hits::PerQuote => Box::new(occurrences(hay, needle)),
            // Candidates, not matches: the automaton was built over probes, so
            // each one is confirmed against the whole quote here.
            Hits::Shared(found) => Box::new(
                found[nth]
                    .iter()
                    .copied()
                    .filter(move |&p| hay.as_bytes()[p..].starts_with(needle.as_bytes())),
            ),
        }
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
    /// `u32` rather than `usize`: this is one entry per non-whitespace byte of
    /// the document, so on the 2MB perf corpus doc it halves a ~13.7MB table.
    /// Caps `source` at `u32::MAX` bytes (~4.29GB); [`Stripped::build`] returns
    /// `None` past that instead of silently wrapping an offset.
    source_offsets: Vec<u32>,
}

/// Whether a source of `len` bytes is too large for `Stripped::source_offsets`
/// to key by `u32`. Split out from [`Stripped::build`] so the boundary can be
/// tested on a fake length instead of an allocated multi-gigabyte string.
fn exceeds_stripped_capacity(len: usize) -> bool {
    len > u32::MAX as usize
}

/// The ASCII half of `char::is_whitespace`, as a byte test: U+0009..U+000D and
/// U+0020. Deliberately *not* `u8::is_ascii_whitespace`, which omits the
/// vertical tab — the stripped index has to agree with `str::trim` and with
/// `strip_ws` about what a whitespace character is, or a quote and the text it
/// is being matched against are stripped differently.
fn is_ascii_space(b: u8) -> bool {
    matches!(b, 0x09..=0x0D | 0x20)
}

impl Stripped {
    /// Built once per `reanchor_file` and paid on every save, so it walks runs
    /// rather than characters: a run of non-whitespace ASCII is one `push_str`
    /// and one `extend` of a range, where `char_indices` plus a `resize` per
    /// character was a UTF-8 decode, an encode and a fill call for each of the
    /// ~1.5 million of them in the 2MB corpus document.
    fn build(source: &str) -> Option<Self> {
        if exceeds_stripped_capacity(source.len()) {
            return None;
        }
        let bytes = source.as_bytes();
        let mut text = String::with_capacity(source.len());
        // One entry per non-whitespace *byte*, so the only capacity that
        // cannot be wrong is the source's own length. Guessing three quarters
        // of it was under on ordinary prose, and the doubling realloc it
        // triggered copied six megabytes on the 2MB corpus document — the
        // whole of what a `:w` pays to build this. The slack is address space,
        // not memory: only the pages actually written are ever touched.
        let mut source_offsets = Vec::with_capacity(source.len());
        let mut i = 0;
        while i < bytes.len() {
            let b = bytes[i];
            if b < 0x80 {
                if is_ascii_space(b) {
                    i += 1;
                    continue;
                }
                let start = i;
                while i < bytes.len() && bytes[i] < 0x80 && !is_ascii_space(bytes[i]) {
                    i += 1;
                }
                text.push_str(&source[start..i]);
                // One entry per byte, and an ASCII byte is a whole character:
                // the offsets over a run are exactly its byte range.
                // Safe: `i <= source.len() <= u32::MAX` (checked above).
                source_offsets.extend(start as u32..i as u32);
            } else {
                // A multi-byte character, decoded once. Its offset repeats for
                // each of its bytes, which is what makes `offset_of` a plain
                // binary search over a non-decreasing table.
                let ch = source[i..].chars().next().unwrap_or('\u{fffd}');
                let width = ch.len_utf8();
                if !ch.is_whitespace() {
                    text.push(ch);
                    source_offsets.resize(text.len(), i as u32);
                }
                i += width;
            }
        }
        Some(Self {
            text,
            source_offsets,
        })
    }

    /// Offset into `text` of the first char at or after source byte `src`.
    /// `source_offsets` is non-decreasing, so this is a binary search.
    ///
    /// The result needs no rounding to a char boundary: every byte of a char
    /// repeats that char's offset, so the predicate cannot flip partway through
    /// one and the partition point can only land on a boundary.
    fn offset_of(&self, src: usize) -> usize {
        self.source_offsets.partition_point(|&o| (o as usize) < src)
    }

    /// Byte offsets in the *original* source of the first and last char of a
    /// match of `len` stripped bytes starting at `at`.
    fn span(&self, at: usize, len: usize) -> Option<(usize, usize)> {
        let last = at + len.saturating_sub(1);
        let start = *self.source_offsets.get(at)? as usize;
        let end = self
            .source_offsets
            .get(last)
            .copied()
            .map(|o| o as usize)
            .unwrap_or(start);
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

    // ---- the code-block memo ----------------------------------------------
    //
    // Every one of these is a way the cache could be *wrong* rather than slow:
    // a key that drops the lang or the theme is invisible until a reader
    // switches palette and the code does not move.

    const SNIPPET: &str = "fn main() { let x = 1; }\n";

    fn fenced(lang: &str) -> String {
        format!("```{lang}\n{SNIPPET}```\n")
    }

    #[test]
    fn the_same_code_in_two_languages_is_not_one_cache_entry() {
        let rust = render(&fenced("rust"));
        let python = render(&fenced("python"));
        assert_ne!(rust, python, "lang dropped from the key");
        // ...and each is stable when it is the cached one being asked for.
        assert_eq!(rust, render(&fenced("rust")));
        assert_eq!(python, render(&fenced("python")));
    }

    #[test]
    fn the_same_code_under_two_themes_is_not_one_cache_entry() {
        let src = fenced("rust");
        let dark = render_with(&src, CODE_THEME);
        let light = render_with(&src, "InspiredGitHub");
        assert_ne!(dark, light, "theme name dropped from the key");
        assert_eq!(dark, render_with(&src, CODE_THEME));
    }

    #[test]
    fn footnotes_fall_back_to_one_block_and_still_concat_identically() {
        // Footnote numbering is stateful across one push_html call, so a
        // per-block render would renumber; the fallback is one segment, and
        // the byte-identity contract still holds through it.
        let src = "first[^1] paragraph\n\nsecond paragraph\n\n[^1]: the note\n";
        let doc = render_blocks(src, CODE_THEME);
        assert_eq!(
            doc.block_count(),
            1,
            "a footnote document must not be split"
        );
        assert_eq!(doc.html(), render_with(src, CODE_THEME));
    }

    #[test]
    fn a_mixed_document_splits_into_its_top_level_blocks() {
        let src = "# h\n\npara\n\n```rust\nfn x() {}\n```\n\n---\n\n- a\n- b\n";
        let doc = render_blocks(src, CODE_THEME);
        assert_eq!(doc.html(), render_with(src, CODE_THEME));
        let blocks: Vec<&str> = doc.blocks().collect();
        // heading, paragraph, fence, rule, list.
        assert_eq!(blocks.len(), 5, "{blocks:?}");
        assert!(blocks[2].contains("<pre"), "the fence is its own block");
    }

    #[test]
    fn an_element_less_segment_folds_into_the_block_before_it() {
        // An HTML comment is re-emitted as escaped text (tenet 4) — visible
        // bare text with no element of its own. It must ride with the block
        // before it, so the frontend's one-element-per-block splice holds,
        // and the concat identity must survive the regrouping.
        let src = "para one\n\n<!-- a comment -->\n\npara two\n";
        let doc = render_blocks(src, CODE_THEME);
        assert_eq!(doc.html(), render_with(src, CODE_THEME));
        let blocks: Vec<&str> = doc.blocks().collect();
        assert_eq!(blocks.len(), 2, "{blocks:?}");
        assert!(blocks[0].starts_with("<p>"), "{blocks:?}");
        assert!(
            blocks[0].contains("&lt;!--"),
            "the comment rides with block one: {blocks:?}"
        );
        assert!(blocks[1].starts_with("<p>"), "{blocks:?}");
    }

    #[test]
    fn a_warm_cache_renders_byte_identically_to_a_cold_one() {
        // Two blocks so the parallel path is taken, one of them repeated so the
        // partition has to keep hits and misses in the same document straight.
        let src = format!(
            "intro\n\n{}\n{}\n{}",
            fenced("rust"),
            fenced("python"),
            fenced("rust")
        );
        let warm = render(&src);
        clear_code_cache();
        let cold = render(&src);
        assert_eq!(cold, warm);
        assert_eq!(render(&src), cold);
    }

    #[test]
    fn the_cache_stays_under_its_byte_cap_and_its_own_books_balance() {
        // Drives `CodeCache::insert` directly rather than through `render_with`
        // -- exercising eviction through real syntect highlighting would mean
        // megabytes of source for a test that only needs to check bookkeeping.
        // `clear_code_cache` makes this independent of test execution order,
        // since the cache is a process-wide static every test in this module
        // shares.
        clear_code_cache();
        let mut cache = code_cache().lock().unwrap();

        // Each entry is ~100 KiB of html; comfortably more than
        // `CODE_CACHE_MAX_BYTES` worth get inserted, so eviction must run.
        let big: Arc<str> = "x".repeat(100 * 1024).into();
        for i in 0..400 {
            let lang = format!("lang{i}");
            let code = format!("code{i}");
            let hash = code_key_hash("dark", &lang, &code);
            cache.insert(
                hash,
                CodeEntry {
                    theme: "dark".to_string(),
                    lang,
                    code,
                    html: big.clone(),
                },
            );
        }

        assert!(
            cache.bytes <= CODE_CACHE_MAX_BYTES,
            "cache reports {} bytes, over the {} cap",
            cache.bytes,
            CODE_CACHE_MAX_BYTES
        );

        // The two structures must agree on which entries are alive: one hash
        // in `order` per live entry, and `bytes` must equal what the live
        // entries actually total -- not just a number that happens to be
        // under the cap.
        let live: usize = cache.buckets.values().map(|b| b.len()).sum();
        assert_eq!(
            cache.order.len(),
            live,
            "order queue and buckets disagree on how many entries are live"
        );
        let recomputed: usize = cache.buckets.values().flatten().map(CodeEntry::bytes).sum();
        assert_eq!(
            cache.bytes, recomputed,
            "accounted bytes drifted from the entries actually stored"
        );
        assert!(
            !cache.buckets.values().any(Vec::is_empty),
            "empty bucket left behind after eviction"
        );

        drop(cache);
        clear_code_cache();
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
        // Also the coverage for `Stripped::source_offsets` being `u32`-keyed:
        // this exercises `build`, `offset_of` and `find` end to end and would
        // catch a bad cast at any of the three.
        let src = "a paragraph that\nwraps across lines\n";
        let loc = locate(src, "", "paragraph that wraps", "").expect("located");
        assert_eq!((loc.line_start, loc.line_end), (1, 2));
    }

    #[test]
    fn stripped_refuses_a_source_past_u32_capacity() {
        // The real guard in `Stripped::build` is `source.len() > u32::MAX as
        // usize`; a source that large can't be allocated in a test, so this
        // pins the boundary predicate `build` delegates to instead.
        assert!(!exceeds_stripped_capacity(u32::MAX as usize));
        assert!(exceeds_stripped_capacity(u32::MAX as usize + 1));

        // And the wiring at the real boundary: `build` on an ordinary source
        // still succeeds and tier 3 still works — the guard must not fire
        // early.
        assert!(Stripped::build("a paragraph that\nwraps across lines\n").is_some());
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
    fn an_unwrapped_source_still_resolves_through_tier_one() {
        // Tier 1 — the byte-exact `prefix + quote + suffix` match — no longer
        // runs a scan of its own; it is verified at the perfect-scoring
        // occurrences of tier 3's pass. This is the case that reaches it, and
        // the corpus cannot: a document with no hard wrapping, where the
        // rendered text the frontend sends *is* a contiguous source slice.
        //
        // The second copy is the same passage wrapped, so tier 3 scores both
        // perfectly and only exactness separates them.
        let src =
            "Intro. The quoted phrase here. Outro.\n\nIntro. The quoted\nphrase here. Outro.\n";
        let loc = SourceIndex::new(src)
            .locate("Intro. ", "The quoted phrase here.", " Outro.")
            .expect("located");
        assert_eq!((loc.line_start, loc.line_end), (1, 1));
    }

    #[test]
    fn the_hint_separates_two_exact_copies() {
        // What ranking exactness above a perfect score rather than returning
        // the first exact hit buys. Tier 1 used to be `source.find(needle)`,
        // which took copy one whatever the hint said — the same bug the hint
        // was introduced to fix at tier 3.
        let block = "Intro. The quoted phrase here. Outro.\n";
        let src = format!("{block}\n---\n\n{block}");
        let at = |hint| {
            SourceIndex::new(&src)
                .locate_near("Intro. ", "The quoted phrase here.", " Outro.", hint)
                .expect("located")
                .line_start
        };
        assert_eq!(at(1), 1);
        assert_eq!(at(5), 5);
    }

    #[test]
    fn tier_one_reports_the_same_span_from_either_route() {
        // The folded tier 1 returns tier 3's mapping of the match rather than
        // computing a span of its own. The two must agree, including on a
        // quote whose last char is multi-byte and one that spans lines.
        let src = "alpha bravo charlie — dash\n";
        let loc = locate(src, "alpha ", "bravo charlie —", " dash").expect("located");
        assert_eq!((loc.line_start, loc.line_end), (1, 1));
        let src = "head\n\none two\nthree four\n\ntail\n";
        let loc = locate(src, "head\n\n", "one two\nthree four", "\n\ntail").expect("located");
        assert_eq!((loc.line_start, loc.line_end), (3, 4));
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

/// Property sweeps for the two guarantees prose in this file states outright.
#[cfg(test)]
mod properties {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        /// The byte-scan shortcut agrees with the real thing on every
        /// string — the wire's block offsets are only as good as this.
        #[test]
        fn utf16_units_matches_encode_utf16(s in "\\PC{0,64}") {
            prop_assert_eq!(utf16_units(&s), s.encode_utf16().count());
        }

        /// The contract `render_blocks` stands on: block-wise rendering and
        /// whole-document rendering are the same function. Sweeps markdown-ish
        /// text — headings, fences, lists, emphasis, links, tables, rules —
        /// because the failure mode is a pulldown construct whose HTML depends
        /// on its neighbours.
        #[test]
        fn blocks_concat_to_the_whole_document(
            source in "([a-z #>*`|:\\[\\]()\\n-]{0,24}\\n){0,20}",
        ) {
            let doc = render_blocks(&source, CODE_THEME);
            prop_assert_eq!(doc.html(), render_with(&source, CODE_THEME));
        }

        /// "Uniqueness is a guarantee here rather than a near-certainty" —
        /// including the adversarial shape the doc names (`## A` twice beside
        /// a literal `## A 1`), and every shape nobody thought to name.
        #[test]
        fn slugs_are_unique_whatever_the_headings(
            headings in prop::collection::vec("[a-zA-Z0-9 _#*-]{0,16}", 0..24),
        ) {
            let mut slugger = Slugger::default();
            let mut seen = std::collections::HashSet::new();
            for h in &headings {
                prop_assert!(seen.insert(slugger.slug(h)), "two headings got one id");
            }
        }

        /// The exact tier is total on the happy path: any non-blank exact
        /// substring of the source locates, at the first occurrence of its
        /// *trimmed* text — `locate_near` trims the quote before anything
        /// else, which the first draft of this property did not know and
        /// proptest immediately taught it.
        #[test]
        fn an_exact_substring_always_locates(
            source in "[a-z \\n]{1,400}",
            start in any::<prop::sample::Index>(),
            len in 1..40usize,
        ) {
            let start = start.index(source.len());
            let end = (start + len).min(source.len());
            let quote = &source[start..end];
            prop_assume!(!quote.trim().is_empty());

            let loc = locate(&source, "", quote, "");
            prop_assert!(loc.is_some(), "an exact substring failed to locate");
            let loc = loc.unwrap();
            let first = source.find(quote.trim()).unwrap();
            let expect = source[..first].matches('\n').count() + 1;
            prop_assert_eq!(loc.line_start, expect, "not the first occurrence's line");
            prop_assert!(loc.line_end >= loc.line_start);
        }
    }
}
