//! In-memory highlight + annotation store and the send stack.
//!
//! The store lives in process memory, spans every file opened during the
//! session, and is destroyed when dreamd exits. Switching files does NOT clear
//! it.
//!
//! The shape below is deliberately wider than what today's callers read.
//! [`Origin`], [`Resolution`], the `Deserialize` derives, [`Store::from_parts`]
//! and the `reanchored` gate all exist for consumers that arrive later (the MCP
//! tools and the marks file). They landed here in one go because this is the
//! worst file in the repo to merge, and freezing the shape once lets several
//! threads work against it in parallel. A field with no reader yet is not an
//! oversight.

use crate::markdown;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::hash::{BuildHasher, Hasher};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

/// A highlight id: `h` followed by 16 lowercase hex digits.
///
/// Opaque on purpose. It used to be a `u64` from a counter that restarted at 1
/// every process, so an id written down in one session pointed at a *different*
/// highlight in the next — silently, which is the worst way to be wrong. It is
/// also not derived from the content: two identical quotes in one file would
/// collide, and re-highlighting deleted-then-restored text should mint a new
/// identity rather than resurrect an old one.
pub type Id = String;

const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

fn fold(mut hash: u64, value: u64) -> u64 {
    for byte in value.to_le_bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    hash
}

/// One OS-seeded value, minted once per process.
///
/// `RandomState`'s keys are seeded by the OS, so hashing nothing with a fresh
/// one yields a per-process random `u64` without a dependency. Also the entropy
/// source for the untrusted-content sentinel, which needs a value a document
/// cannot have been written to contain.
pub(crate) fn process_seed() -> u64 {
    static SEED: OnceLock<u64> = OnceLock::new();
    *SEED.get_or_init(|| {
        std::collections::hash_map::RandomState::new()
            .build_hasher()
            .finish()
    })
}

/// Mint a fresh id: wall-clock nanos, pid, a per-process counter and the
/// process seed, folded with FNV-1a.
///
/// The counter is what makes two ids minted inside the same nanosecond differ;
/// the seed and the pid are what make two *processes* differ even if their
/// clocks agree.
fn mint_id() -> Id {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos() as u64);
    let mut hash = FNV_OFFSET;
    hash = fold(hash, nanos);
    hash = fold(hash, u64::from(std::process::id()));
    hash = fold(hash, COUNTER.fetch_add(1, Ordering::Relaxed));
    hash = fold(hash, process_seed());
    format!("h{hash:016x}")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HighlightState {
    /// Anchor still resolves; shown in the default color.
    #[default]
    Active,
    /// The highlighted text itself was edited; shown red with a margin "?".
    Stale,
}

/// Who made the mark. An agent's marks are the agent's own annotations on a
/// document, and the frontend is free to paint them differently from a human's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Origin {
    #[default]
    Human,
    Agent,
}

/// What closed a question. Set when an agent answers a queued pair: the
/// highlight stays, the stack entry goes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution {
    /// What the agent reported back. `None` when it closed without a word.
    pub note: Option<String>,
    /// Unix seconds.
    pub at: u64,
}

/// A stored mark.
///
/// `#[serde(default)]` is container-wide, and there is no `deny_unknown_fields`:
/// a marks file written by a later version must load in an earlier one, keeping
/// what it understands, rather than being rejected wholesale.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Highlight {
    pub id: Id,
    /// Absolute file path this highlight belongs to.
    pub file_path: String,
    pub line_start: usize,
    pub line_end: usize,
    /// The highlighted source text (the evidence).
    pub quote: String,
    /// A short slice of text immediately before/after the quote, used to
    /// re-anchor unambiguously after edits elsewhere in the file.
    pub prefix: String,
    pub suffix: String,
    pub state: HighlightState,
    /// The question/comment for the LLM. `None` until the user annotates.
    pub annotation: Option<String>,
    pub origin: Origin,
    /// `Some` once an agent has answered. The evidence survives resolution.
    pub resolved: Option<Resolution>,
}

/// A queued (highlight + annotation) pair, as sent to the frontend stack panel.
#[derive(Debug, Clone, Serialize)]
pub struct Pair {
    pub highlight: Highlight,
    pub annotation: String,
}

#[derive(Default)]
pub struct Store {
    highlights: Vec<Highlight>,
    /// Ordered highlight ids queued for the next send.
    stack: Vec<Id>,
    /// Files re-anchored at least once this session. See
    /// [`Store::ensure_reanchored`].
    reanchored: HashSet<String>,
}

impl Store {
    /// Rebuild a store from its parts, as a loader does. `reanchored` starts
    /// empty on purpose: marks that came off disk have not been checked against
    /// the file on this run's bytes yet.
    pub fn from_parts(highlights: Vec<Highlight>, stack: Vec<Id>) -> Self {
        Self {
            highlights,
            stack,
            reanchored: HashSet::new(),
        }
    }

    /// Borrow the parts, for a writer that only has a `&Store`.
    pub fn parts(&self) -> (&[Highlight], &[Id]) {
        (&self.highlights, &self.stack)
    }

    pub fn into_parts(self) -> (Vec<Highlight>, Vec<Id>) {
        (self.highlights, self.stack)
    }

    pub fn add_highlight(
        &mut self,
        file_path: String,
        line_start: usize,
        line_end: usize,
        quote: String,
        prefix: String,
        suffix: String,
    ) -> Id {
        self.push(
            file_path,
            line_start,
            line_end,
            quote,
            prefix,
            suffix,
            Origin::Human,
        )
    }

    /// Anchor `quote` in `source` and add it.
    ///
    /// Unlocatable text still becomes a highlight, at `(0, 0)` — losing the
    /// mark because the DOM's whitespace collapsing beat `locate` is worse than
    /// an imprecise line number, and the next save re-anchors it anyway.
    ///
    /// This lives here rather than at the command layer because `main.rs` is a
    /// `[[bin]]` and cannot be imported, so the fallback above had no test at
    /// all. It takes `origin` because the agent's `mark_passage` is the second
    /// caller and must not have to reimplement the fallback to set one field.
    pub fn add_anchored(
        &mut self,
        file_path: String,
        source: &str,
        quote: String,
        prefix: String,
        suffix: String,
        origin: Origin,
    ) -> Id {
        let (line_start, line_end) = markdown::locate(source, &prefix, &quote, &suffix)
            .map_or((0, 0), |loc| (loc.line_start, loc.line_end));
        self.push(
            file_path, line_start, line_end, quote, prefix, suffix, origin,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn push(
        &mut self,
        file_path: String,
        line_start: usize,
        line_end: usize,
        quote: String,
        prefix: String,
        suffix: String,
        origin: Origin,
    ) -> Id {
        let id = mint_id();
        self.highlights.push(Highlight {
            id: id.clone(),
            file_path,
            line_start,
            line_end,
            quote,
            prefix,
            suffix,
            state: HighlightState::Active,
            annotation: None,
            origin,
            resolved: None,
        });
        id
    }

    /// Attach/replace an annotation and enqueue the pair on the stack.
    pub fn set_annotation(&mut self, id: &str, text: String) -> bool {
        let Some(h) = self.highlights.iter_mut().find(|h| h.id == id) else {
            return false;
        };
        h.annotation = Some(text);
        if !self.stack.iter().any(|x| x == id) {
            self.stack.push(id.to_string());
        }
        true
    }

    pub fn remove(&mut self, id: &str) {
        self.highlights.retain(|h| h.id != id);
        self.stack.retain(|x| x != id);
    }

    pub fn remove_from_stack(&mut self, id: &str) {
        self.stack.retain(|x| x != id);
    }

    /// Close a question: record the answer and drop the stack entry. Returns
    /// false if the id names nothing.
    ///
    /// The highlight survives — the evidence stays anchored to the passage,
    /// which is the whole reason this is not [`remove`](Store::remove).
    ///
    /// It deliberately leaves `reanchored` alone. Resolving changes no line
    /// numbers, so a file already checked against this run's bytes is still
    /// checked. The MCP layer used to reach `resolved` through
    /// `into_parts`/`from_parts`, which resets the gate, and so paid one wasted
    /// `SourceIndex` rebuild per resolved mark — in the primary loop, where an
    /// agent closes marks in bursts. That is what this method exists to avoid,
    /// and `resolving_a_mark_does_not_invalidate_the_reanchor_cache` pins it.
    pub fn resolve(&mut self, id: &str, note: Option<String>) -> bool {
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let Some(h) = self.highlights.iter_mut().find(|h| h.id == id) else {
            return false;
        };
        h.resolved = Some(Resolution { note, at });
        self.remove_from_stack(id);
        true
    }

    fn find(&self, id: &str) -> Option<&Highlight> {
        self.highlights.iter().find(|h| h.id == id)
    }

    pub fn get(&self, id: &str) -> Option<Highlight> {
        self.find(id).cloned()
    }

    /// All highlights for a given file (for rendering the overlay).
    pub fn for_file(&self, file_path: &str) -> Vec<Highlight> {
        self.highlights
            .iter()
            .filter(|h| h.file_path == file_path)
            .cloned()
            .collect()
    }

    /// The current send stack as (highlight, annotation) pairs, in order.
    pub fn stack_pairs(&self) -> Vec<Pair> {
        self.selected_pairs(&self.stack)
    }

    /// Pairs for an explicit id selection, in the order given. Ids with no
    /// highlight, or a highlight with no annotation, are skipped.
    pub fn selected_pairs(&self, ids: &[Id]) -> Vec<Pair> {
        ids.iter()
            .filter_map(|id| self.find(id))
            .filter_map(|h| {
                h.annotation.clone().map(|a| Pair {
                    highlight: h.clone(),
                    annotation: a,
                })
            })
            .collect()
    }

    /// Re-anchor every highlight in `file_path` against fresh source. Returns
    /// the updated highlights for that file. Highlights whose quote still
    /// resolves are re-anchored (and stay Active even if lines shifted);
    /// those whose quote no longer resolves become Stale.
    pub fn reanchor_file(&mut self, file_path: &str, source: &str) -> Vec<Highlight> {
        self.reanchored.insert(file_path.to_string());
        // One index for the whole file, not one per highlight — see
        // [`markdown::SourceIndex`].
        let mut index = markdown::SourceIndex::new(source);
        for h in self
            .highlights
            .iter_mut()
            .filter(|h| h.file_path == file_path)
        {
            // The previous line is passed as a hint: when a block appears twice
            // verbatim, the quote and its context are identical in both copies
            // and only "it was here a moment ago" can tell them apart.
            match index.locate_near(&h.prefix, &h.quote, &h.suffix, h.line_start) {
                Some(loc) => {
                    h.line_start = loc.line_start;
                    h.line_end = loc.line_end;
                    h.state = HighlightState::Active;
                }
                None => {
                    h.state = HighlightState::Stale;
                }
            }
        }
        self.for_file(file_path)
    }

    /// Re-anchor `file_path` the *first* time this session looks at it, and
    /// never again.
    ///
    /// Marks loaded from a previous session were anchored against bytes that
    /// may have changed since, so their line numbers are guesses until checked.
    /// Doing that check for every file at startup would land straight on the
    /// cold-start number, so it happens lazily, on first sight of each path —
    /// inside work that was already scanning that file.
    pub fn ensure_reanchored(&mut self, file_path: &str, source: &str) {
        if !self.reanchored.contains(file_path) {
            self.reanchor_file(file_path, source);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "/repo/a.md";

    fn store_with(source: &str, quotes: &[&str]) -> (Store, Vec<Id>) {
        let mut store = Store::default();
        let ids = quotes
            .iter()
            .map(|q| {
                let at = source.find(q).expect("fixture quote is in the source");
                let line = source[..at].lines().count().max(1);
                store.add_highlight(
                    FILE.to_string(),
                    line,
                    line,
                    (*q).to_string(),
                    String::new(),
                    String::new(),
                )
            })
            .collect();
        (store, ids)
    }

    // ---- ids --------------------------------------------------------------

    #[test]
    fn ids_carry_no_ordering() {
        // This used to assert `[1, 2, 3]`. Ordering is now false by design: an
        // id is an opaque handle, and an agent holding two of them must not be
        // able to infer which mark came first, or do arithmetic to reach a
        // third. Order lives in the stack, which is where it means something.
        let (store, ids) = store_with("alpha\nbeta\ngamma\n", &["alpha", "beta", "gamma"]);
        let unique: HashSet<&Id> = ids.iter().collect();
        assert_eq!(unique.len(), 3, "ids collided: {ids:?}");
        // The only ordering dreamd exposes is insertion order, and it comes
        // from the vectors, not from the ids.
        assert_eq!(
            store
                .for_file(FILE)
                .iter()
                .map(|h| h.id.clone())
                .collect::<Vec<_>>(),
            ids
        );
    }

    #[test]
    fn two_ids_minted_back_to_back_are_different() {
        // The per-process counter is what this pins. Wall-clock nanos alone can
        // repeat: two `add_highlight` calls in a row can land inside the same
        // nanosecond on a fast machine, and a coarse clock makes it likely.
        let mut store = Store::default();
        let mut seen = HashSet::new();
        for _ in 0..1000 {
            let id = store.add_highlight(
                FILE.to_string(),
                1,
                1,
                "alpha".into(),
                String::new(),
                String::new(),
            );
            assert!(seen.insert(id.clone()), "minted {id} twice");
        }
    }

    #[test]
    fn an_id_is_opaque_and_url_safe() {
        // `^h[0-9a-f]{16}$`. Nothing downstream should have to escape an id
        // into a CSS selector, a JSON key, a file name or a URL.
        let (_, ids) = store_with("alpha\n", &["alpha"]);
        let id = &ids[0];
        assert_eq!(id.len(), 17, "{id}");
        assert!(id.starts_with('h'), "{id}");
        assert!(
            id[1..]
                .chars()
                .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)),
            "{id} is not lowercase hex"
        );
    }

    #[test]
    fn an_id_from_a_previous_session_resolves_to_nothing_rather_than_the_wrong_highlight() {
        // The test this whole change exists for. Under the old `next_id`
        // counter both stores mint 1, 2, 3, so an id written down before a
        // restart silently addressed a *different* highlight afterwards —
        // and `set_annotation` would have queued the wrong evidence.
        //
        // Both stores live in one process on purpose: same pid, same process
        // seed. That is the hardest case for the minter, not the easiest.
        let (previous, remembered) =
            store_with("alpha\nbeta\ngamma\n", &["alpha", "beta", "gamma"]);
        drop(previous);

        let (mut fresh, current) = store_with("alpha\nbeta\ngamma\n", &["alpha", "beta", "gamma"]);
        for id in &remembered {
            assert!(
                fresh.get(id).is_none(),
                "{id} from a dead session resolved in a fresh one"
            );
            assert!(
                !fresh.set_annotation(id, "why?".into()),
                "{id} annotated a highlight it does not own"
            );
        }
        assert!(fresh.stack_pairs().is_empty());
        assert!(remembered.iter().all(|old| !current.contains(old)));
    }

    #[test]
    fn ids_are_not_reused_after_a_removal() {
        // A removed id must stay dead: the frontend holds ids in the DOM, and
        // reuse would silently re-point a stale element at a new highlight.
        let (mut store, ids) = store_with("alpha\nbeta\n", &["alpha", "beta"]);
        store.remove(&ids[1]);
        let next = store.add_highlight(
            FILE.to_string(),
            1,
            1,
            "alpha".into(),
            String::new(),
            String::new(),
        );
        assert!(!ids.contains(&next), "reused {next}");
        assert!(store.get(&ids[1]).is_none());
    }

    #[test]
    fn a_fresh_highlight_is_active_and_unannotated() {
        let (store, ids) = store_with("alpha\n", &["alpha"]);
        let h = store.get(&ids[0]).expect("present");
        assert_eq!(h.state, HighlightState::Active);
        assert_eq!(h.annotation, None);
        assert_eq!(h.origin, Origin::Human);
        assert_eq!(h.resolved, None);
        // ...and therefore not on the stack.
        assert!(store.stack_pairs().is_empty());
    }

    // ---- anchoring --------------------------------------------------------

    #[test]
    fn add_anchored_finds_the_line_the_quote_is_on() {
        let source = "alpha\nbeta\ngamma\n";
        let mut store = Store::default();
        let id = store.add_anchored(
            FILE.to_string(),
            source,
            "gamma".into(),
            String::new(),
            String::new(),
            Origin::Human,
        );
        let h = store.get(&id).expect("present");
        assert_eq!((h.line_start, h.line_end), (3, 3));
    }

    #[test]
    fn an_unlocatable_quote_still_becomes_a_highlight_at_line_zero() {
        // The fallback that used to sit in `main.rs`, where no test could reach
        // it. Dropping the mark is worse than an imprecise line number: the
        // quote survives, so the next save re-anchors it.
        let mut store = Store::default();
        let id = store.add_anchored(
            FILE.to_string(),
            "alpha\nbeta\n",
            "nowhere in this file".into(),
            String::new(),
            String::new(),
            Origin::Human,
        );
        let h = store.get(&id).expect("the highlight is kept");
        assert_eq!((h.line_start, h.line_end), (0, 0));
        assert_eq!(h.quote, "nowhere in this file");
    }

    #[test]
    fn an_agent_mark_records_its_origin() {
        let mut store = Store::default();
        let id = store.add_anchored(
            FILE.to_string(),
            "alpha\n",
            "alpha".into(),
            String::new(),
            String::new(),
            Origin::Agent,
        );
        assert_eq!(store.get(&id).expect("present").origin, Origin::Agent);
    }

    // ---- stack ------------------------------------------------------------

    #[test]
    fn set_annotation_is_what_enqueues() {
        let (mut store, ids) = store_with("alpha\nbeta\n", &["alpha", "beta"]);
        assert!(store.set_annotation(&ids[1], "why?".into()));
        let pairs = store.stack_pairs();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].highlight.id, ids[1]);
        assert_eq!(pairs[0].annotation, "why?");
    }

    #[test]
    fn re_annotating_replaces_the_text_without_reordering_the_stack() {
        let (mut store, ids) = store_with("alpha\nbeta\n", &["alpha", "beta"]);
        store.set_annotation(&ids[0], "first".into());
        store.set_annotation(&ids[1], "second".into());
        store.set_annotation(&ids[0], "revised".into());
        let pairs = store.stack_pairs();
        assert_eq!(
            pairs
                .iter()
                .map(|p| p.highlight.id.clone())
                .collect::<Vec<_>>(),
            ids,
            "re-annotating must not move a pair to the back"
        );
        assert_eq!(pairs[0].annotation, "revised");
    }

    #[test]
    fn annotating_an_unknown_id_is_a_no_op() {
        let mut store = Store::default();
        assert!(!store.set_annotation("h0000000000000000", "why?".into()));
        assert!(store.stack_pairs().is_empty());
    }

    #[test]
    fn remove_clears_the_highlight_and_the_stack_entry() {
        let (mut store, ids) = store_with("alpha\n", &["alpha"]);
        store.set_annotation(&ids[0], "why?".into());
        store.remove(&ids[0]);
        assert!(store.get(&ids[0]).is_none());
        assert!(store.stack_pairs().is_empty());
        assert!(store.for_file(FILE).is_empty());
    }

    #[test]
    fn remove_from_stack_keeps_the_highlight() {
        let (mut store, ids) = store_with("alpha\n", &["alpha"]);
        store.set_annotation(&ids[0], "why?".into());
        store.remove_from_stack(&ids[0]);
        assert!(store.stack_pairs().is_empty());
        assert!(
            store.get(&ids[0]).is_some(),
            "the highlight itself survives"
        );
    }

    #[test]
    fn selected_pairs_honours_the_order_given_and_skips_the_unusable() {
        let (mut store, ids) = store_with("alpha\nbeta\ngamma\n", &["alpha", "beta", "gamma"]);
        store.set_annotation(&ids[0], "a".into());
        store.set_annotation(&ids[2], "c".into());
        // ids[1] has no annotation; the invented id has no highlight. Both are
        // skipped rather than erroring or producing an empty pair.
        let picked = store.selected_pairs(&[
            ids[2].clone(),
            "h0000000000000000".to_string(),
            ids[1].clone(),
            ids[0].clone(),
        ]);
        assert_eq!(
            picked
                .iter()
                .map(|p| p.highlight.id.clone())
                .collect::<Vec<_>>(),
            vec![ids[2].clone(), ids[0].clone()]
        );
    }

    #[test]
    fn for_file_does_not_leak_other_files() {
        let (mut store, _) = store_with("alpha\n", &["alpha"]);
        store.add_highlight(
            "/repo/b.md".into(),
            1,
            1,
            "alpha".into(),
            String::new(),
            String::new(),
        );
        assert_eq!(store.for_file(FILE).len(), 1);
        assert_eq!(store.for_file("/repo/b.md").len(), 1);
        assert!(store.for_file("/repo/missing.md").is_empty());
    }

    // ---- resolve ----------------------------------------------------------

    #[test]
    fn resolve_closes_the_question_and_keeps_the_evidence() {
        let (mut store, ids) = store_with("alpha\nbeta\n", &["alpha", "beta"]);
        store.set_annotation(&ids[0], "why?".into());
        store.set_annotation(&ids[1], "and?".into());

        assert!(store.resolve(&ids[0], Some("because".into())));
        let h = store.get(&ids[0]).expect("the highlight survives");
        assert_eq!(
            h.resolved.as_ref().and_then(|r| r.note.as_deref()),
            Some("because")
        );
        assert!(h.resolved.as_ref().expect("resolved").at > 0);
        assert_eq!(h.quote, "alpha", "the evidence is untouched");
        assert_eq!(
            store
                .stack_pairs()
                .iter()
                .map(|p| p.highlight.id.clone())
                .collect::<Vec<_>>(),
            vec![ids[1].clone()],
            "only the resolved entry leaves the queue"
        );
    }

    #[test]
    fn resolving_an_unknown_id_changes_nothing() {
        let (mut store, ids) = store_with("alpha\n", &["alpha"]);
        store.set_annotation(&ids[0], "why?".into());
        assert!(!store.resolve("h0000000000000000", None));
        assert_eq!(store.stack_pairs().len(), 1);
    }

    #[test]
    fn resolving_a_mark_does_not_invalidate_the_reanchor_cache() {
        // Resolving changes no line numbers, so a file already checked against
        // this run's bytes must stay checked. The MCP layer's old helper
        // round-tripped the store through `into_parts`/`from_parts`, which
        // starts `reanchored` empty — correct, but it made the next
        // `ensure_reanchored` rebuild a `SourceIndex` for nothing, once per
        // resolved mark, in the primary loop. Under that helper this test goes
        // red on the last assertion, which is how it has teeth.
        let (mut store, ids) = store_with("alpha\nbeta\n", &["beta"]);
        store.set_annotation(&ids[0], "why?".into());
        store.reanchor_file(FILE, "alpha\nbeta\n");
        assert_eq!(store.get(&ids[0]).expect("present").line_start, 2);

        assert!(store.resolve(&ids[0], Some("done".into())));

        // A source that would move the mark if the gate had been cleared.
        store.ensure_reanchored(FILE, "x\ny\nz\nbeta\n");
        assert_eq!(
            store.get(&ids[0]).expect("present").line_start,
            2,
            "resolving cleared the reanchor gate and paid for a second scan"
        );
    }

    // ---- reanchor ---------------------------------------------------------

    #[test]
    fn reanchor_follows_a_quote_that_moved() {
        let (mut store, ids) = store_with("alpha\nbeta\n", &["beta"]);
        let after = store.reanchor_file(FILE, "inserted\nlines\nalpha\nbeta\n");
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].id, ids[0], "re-anchoring must not re-identify");
        assert_eq!(after[0].line_start, 4);
        assert_eq!(after[0].state, HighlightState::Active);
    }

    #[test]
    fn an_edited_quote_goes_stale_rather_than_disappearing() {
        let (mut store, ids) = store_with("alpha\nbeta\n", &["beta"]);
        let after = store.reanchor_file(FILE, "alpha\nBETA IS GONE\n");
        assert_eq!(after.len(), 1, "a stale highlight is kept, not dropped");
        assert_eq!(after[0].state, HighlightState::Stale);
        // The quote is preserved so it can come back if the edit is undone.
        assert_eq!(after[0].quote, "beta");
        let revived = store.reanchor_file(FILE, "alpha\nbeta\n");
        assert_eq!(revived[0].state, HighlightState::Active);
        assert_eq!(revived[0].id, ids[0]);
    }

    #[test]
    fn reanchor_leaves_other_files_alone() {
        let (mut store, _) = store_with("alpha\n", &["alpha"]);
        let other = store.add_highlight(
            "/repo/b.md".into(),
            7,
            7,
            "untouched".into(),
            String::new(),
            String::new(),
        );
        store.reanchor_file(FILE, "nothing matches here\n");
        let h = store.get(&other).expect("present");
        assert_eq!(h.line_start, 7);
        assert_eq!(h.state, HighlightState::Active);
    }

    #[test]
    fn ensure_reanchored_runs_once_per_file_and_then_stands_down() {
        let (mut store, ids) = store_with("alpha\nbeta\n", &["beta"]);

        // First sight of the path: the marks came from somewhere else (a
        // previous session, in the real case) and are checked against these
        // bytes.
        store.ensure_reanchored(FILE, "inserted\nalpha\nbeta\n");
        assert_eq!(store.get(&ids[0]).expect("present").line_start, 3);

        // Second sight: no work, even though the source now says otherwise.
        // Anything that genuinely needs re-anchoring calls `reanchor_file`,
        // which is what the watcher does on every save.
        store.ensure_reanchored(FILE, "beta\n");
        assert_eq!(store.get(&ids[0]).expect("present").line_start, 3);
        store.reanchor_file(FILE, "beta\n");
        assert_eq!(store.get(&ids[0]).expect("present").line_start, 1);
    }

    #[test]
    fn a_file_already_reanchored_this_session_is_not_reanchored_again() {
        let (mut store, ids) = store_with("alpha\nbeta\n", &["beta"]);
        store.reanchor_file(FILE, "alpha\nbeta\n");
        // `reanchor_file` counts as having seen the path, so a later
        // `get_highlights` doesn't pay for a second scan of the same file.
        store.ensure_reanchored(FILE, "x\ny\nz\nbeta\n");
        assert_eq!(store.get(&ids[0]).expect("present").line_start, 2);
    }

    // ---- shape ------------------------------------------------------------

    #[test]
    fn parts_round_trip_through_from_parts() {
        // The seam step 4's loader and writer use, so the private fields stay
        // private.
        let (mut store, ids) = store_with("alpha\nbeta\n", &["alpha", "beta"]);
        store.set_annotation(&ids[1], "why?".into());
        store.set_annotation(&ids[0], "and?".into());

        let (highlights, stack) = store.into_parts();
        assert_eq!(stack, vec![ids[1].clone(), ids[0].clone()]);

        let rebuilt = Store::from_parts(highlights, stack);
        assert_eq!(
            rebuilt
                .stack_pairs()
                .iter()
                .map(|p| p.highlight.id.clone())
                .collect::<Vec<_>>(),
            vec![ids[1].clone(), ids[0].clone()],
            "stack order is the human's asking order and must survive a reload"
        );
        let (h, s) = rebuilt.parts();
        assert_eq!(h.len(), 2);
        assert_eq!(s.len(), 2);
    }

    #[test]
    fn a_highlight_round_trips_through_json() {
        let (mut store, ids) = store_with("alpha\n", &["alpha"]);
        store.set_annotation(&ids[0], "why?".into());
        let h = store.get(&ids[0]).expect("present");
        let json = serde_json::to_string(&h).expect("serialize");
        let back: Highlight = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.id, h.id);
        assert_eq!(back.quote, h.quote);
        assert_eq!(back.annotation, h.annotation);
        assert_eq!(back.origin, Origin::Human);
        assert_eq!(back.resolved, None);
    }

    #[test]
    fn a_highlight_from_a_future_version_loads_what_it_understands() {
        // No `deny_unknown_fields` and `#[serde(default)]` throughout: a marks
        // file written by a later dreamd must not be rejected wholesale by an
        // earlier one. This is the guard that lets step 4's format grow.
        let json = r#"{"id":"habcdef0123456789","file_path":"/repo/a.md","quote":"alpha",
                       "state":"stale","invented_by_a_later_version":42}"#;
        let h: Highlight = serde_json::from_str(json).expect("deserialize");
        assert_eq!(h.id, "habcdef0123456789");
        assert_eq!(h.state, HighlightState::Stale);
        assert_eq!(h.line_start, 0, "an absent field defaults");
        assert_eq!(h.origin, Origin::Human);
        assert_eq!(h.suffix, "");
    }
}
