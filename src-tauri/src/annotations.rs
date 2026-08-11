//! In-memory highlight + annotation store and the send stack.
//!
//! The store lives in process memory and spans every file opened during the
//! session; switching files does NOT clear it. It no longer dies with the
//! process: [`crate::marks_file`] projects it to disk and reads it back, which
//! is the one place tenet 2 lets session state outlive a run.
//!
//! The shape below was frozen wider than its first caller needed, and every
//! consumer it was widened for has since landed: [`Origin`] and [`Resolution`]
//! are read by `mcp::view`, [`Store::from_parts`] by `marks_file::admit` and
//! `cli`'s prune, the `Deserialize` derives by the marks file, and the
//! `reanchored` gate by `get_highlights`. Freezing it in one go was deliberate —
//! this is the worst file in the repo to merge, and several threads of work
//! needed to build against the same struct at once.

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
    /// Unix seconds of the send that put this mark in front of an agent, and
    /// `None` once it is answered. It sits beside `state` rather than inside it
    /// because a passage can go stale *while* it is out with the agent, and a
    /// single enum would have to pick one of the two and would pick wrong.
    ///
    /// Persisted, so pending survives a restart. It is the container-wide
    /// `#[serde(default)]` that lets a marks file written before this field
    /// existed load unchanged.
    pub sent_at: Option<u64>,
    /// Is this mark **done with**? The frontend fades it if so.
    ///
    /// It used to mean strictly "was read off disk", and the fade was therefore
    /// "highlighted in a previous session" and nothing else. That was too narrow
    /// to describe what a reader is actually looking at. A question you have
    /// handed to an agent, or taken off the stack by hand, is finished business
    /// in exactly the way a mark from last week is: still worth keeping, no
    /// longer the thing you are working on. So three things set it now, and the
    /// meaning is the union of them:
    ///
    /// - [`crate::marks_file::admit`], on everything it reads — a previous
    ///   session is over by definition.
    /// - [`Store::mark_sent`], on a send. The question has gone.
    /// - [`Store::remove_from_stack`], on a pop. The question was withdrawn.
    ///
    /// [`Store::set_annotation`] is the one thing that clears it, and it is the
    /// other half of the same idea: annotating is picking the passage back up, so
    /// it returns to full strength and — because annotating also re-enqueues the
    /// pair — the fade tracks the stack rather than drifting from it. Annotate a
    /// faded mark and it brightens; send it and it fades again.
    ///
    /// Still no clock, no session id and no schema field: it is derived from what
    /// has happened to the mark, computed in the three places above.
    ///
    /// The serde attributes are not `skip`, and the difference matters, because
    /// **one derive serves two boundaries**: the marks file and the IPC reply the
    /// frontend paints from. A plain `skip` would keep it off disk and out of
    /// the frontend's hands in the same stroke, leaving the fade with nothing to
    /// read. So instead:
    ///
    /// `skip_deserializing` — a marks file, hand-edited or copied, can never
    /// assert that a mark is prior; only this session's own reasoning decides it.
    ///
    /// `skip_serializing_if` — `false` is the overwhelming majority and stays
    /// off the wire entirely, which is also why the frontend must treat an
    /// absent key as false rather than testing for its presence.
    ///
    /// Off disk it is kept by [`crate::marks_file::doc_from`], which clears the
    /// flag on the copy it writes. That stays right under the wider meaning, and
    /// deliberately: `admit` makes *everything* prior on the way back in, so a
    /// sent mark is faded in the next session regardless, and writing the flag
    /// would only let a hand-edited file assert a fade the reader's own history
    /// does not support.
    #[serde(skip_deserializing, skip_serializing_if = "is_false")]
    pub prior: bool,
}

/// `skip_serializing_if` hands its predicate a reference; `bool`'s own `Not`
/// takes it by value.
#[allow(clippy::trivially_copy_pass_by_ref)]
fn is_false(b: &bool) -> bool {
    !*b
}

/// Locate `quote` in `source`, falling back to `(0, 0)` when no tier of
/// [`markdown::locate`] finds it.
///
/// One function rather than a copy per caller, because
/// [`Store::add_anchored`] and [`Store::retarget`] have to agree about what an
/// unlocatable quote costs. Losing the mark because the DOM's whitespace
/// collapsing — or its stripping of `**bold**` — beat `locate` is worse than an
/// imprecise line number, and the next save re-anchors it anyway. Past that, the
/// `line_start > 0` guard in [`Store::reanchor_file`] is what keeps `Stale` from
/// accusing a file nobody touched.
fn anchor(source: &str, prefix: &str, quote: &str, suffix: &str) -> (usize, usize) {
    markdown::locate(source, prefix, quote, suffix)
        .map_or((0, 0), |loc| (loc.line_start, loc.line_end))
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
    /// Unlocatable text still becomes a highlight, at `(0, 0)` — see [`anchor`]
    /// for why that beats losing the mark.
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
        let (line_start, line_end) = anchor(source, &prefix, &quote, &suffix);
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
            sent_at: None,
            prior: false,
        });
        id
    }

    /// Replace a mark's extent — the reader shrinking or growing a highlight
    /// they already made — keeping everything else about it.
    ///
    /// This is the counterpart to the frontend's refusal to highlight text that
    /// is already highlighted: with overlap denied, changing your mind about
    /// where a passage starts or ends has to be *this*, or it would mean
    /// deleting the mark and losing the question attached to it. So the id
    /// survives, and with it the annotation, the stack slot, `sent_at`,
    /// `resolved` and `prior`. Only the quote, its context and the lines change.
    ///
    /// `prior` in particular is left alone: the fade tracks the stack (see
    /// [`Highlight::prior`]), resizing moves nothing on or off it, and
    /// brightening a mark here would put the two surfaces back in disagreement.
    ///
    /// Anchoring is [`anchor`], the same function
    /// [`add_anchored`](Store::add_anchored) uses and for the same reason — a
    /// new extent can span inline markdown just as a first one can.
    ///
    /// Returns false if the id names nothing.
    pub fn retarget(
        &mut self,
        id: &str,
        source: &str,
        quote: String,
        prefix: String,
        suffix: String,
    ) -> bool {
        let (line_start, line_end) = anchor(source, &prefix, &quote, &suffix);
        let Some(h) = self.highlights.iter_mut().find(|h| h.id == id) else {
            return false;
        };
        h.quote = quote;
        h.prefix = prefix;
        h.suffix = suffix;
        h.line_start = line_start;
        h.line_end = line_end;
        // The new extent was just measured against these bytes, so whatever the
        // old one had come to say about the source no longer applies.
        h.state = HighlightState::Active;
        true
    }

    /// Attach/replace an annotation and enqueue the pair on the stack.
    ///
    /// Two rules ride along, and both are about what annotating *means*:
    ///
    /// It clears `prior`. The fade reads as "done with", and annotating is
    /// picking the passage back up, so the mark returns to full saturation. This
    /// is the exact inverse of what [`mark_sent`](Store::mark_sent) and
    /// [`remove_from_stack`](Store::remove_from_stack) do, and between the three
    /// of them the fade tracks the stack: on it is bright, off it is faded.
    ///
    /// It does **not** clear `sent_at`. Re-annotating a mark that is out with an
    /// agent does not retract the question already asked — it stacks a second
    /// one about the same passage, which is what the re-push below produces
    /// once [`mark_sent`](Store::mark_sent) has taken the id off the stack.
    pub fn set_annotation(&mut self, id: &str, text: String) -> bool {
        let Some(h) = self.highlights.iter_mut().find(|h| h.id == id) else {
            return false;
        };
        h.annotation = Some(text);
        h.prior = false;
        if !self.stack.iter().any(|x| x == id) {
            self.stack.push(id.to_string());
        }
        true
    }

    /// Record that `ids` have gone to an agent: stamp `sent_at`, fade them, and
    /// take exactly those ids off the stack.
    ///
    /// Exactly those — not the whole stack. `Send selected` sends a subset, and
    /// clearing everything would silently drop questions that were never asked.
    /// The highlights themselves all survive; only the queue shrinks.
    ///
    /// It sets `prior`, which is the fade. A sent question is done with from the
    /// reader's side — it is the agent's turn — and the passage should stop
    /// competing with the paragraph it sits in the moment the send happens rather
    /// than at the next restart. `releaseSend` in `app.js` already calls
    /// `repaintHighlights` after a send, so this is what the reader sees change.
    pub fn mark_sent(&mut self, ids: &[Id]) {
        let at = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        for h in self.highlights.iter_mut() {
            if ids.iter().any(|id| id == &h.id) {
                h.sent_at = Some(at);
                h.prior = true;
            }
        }
        self.stack.retain(|x| !ids.iter().any(|id| id == x));
    }

    pub fn remove(&mut self, id: &str) {
        self.highlights.retain(|h| h.id != id);
        self.stack.retain(|x| x != id);
    }

    /// Take a pair off the stack, keeping the highlight — and fade it.
    ///
    /// The fade is the same statement `mark_sent` makes: this passage is no
    /// longer a question waiting to be asked. Withdrawing it by hand and handing
    /// it to an agent differ in what happened, not in what is left, so they read
    /// the same. Annotating it again brightens it and puts it back on the stack,
    /// which is [`set_annotation`](Store::set_annotation)'s half of the rule.
    ///
    /// [`resolve`](Store::resolve) reaches this too, and should: an agent closing
    /// a question is the strongest form of "done with" there is.
    pub fn remove_from_stack(&mut self, id: &str) {
        self.stack.retain(|x| x != id);
        if let Some(h) = self.highlights.iter_mut().find(|h| h.id == id) {
            h.prior = true;
        }
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
        // The question has been answered, so it is no longer out with anyone —
        // this is what makes the pending glyph go away.
        h.sent_at = None;
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
    /// those whose quote no longer resolves become Stale — **but only if they
    /// resolved once**, which is what makes Stale mean what it says.
    ///
    /// A mark at `(0, 0)` was never located: see [`Store::add_anchored`], which
    /// keeps an unlocatable selection rather than losing it. The realistic way to
    /// get one is a quote spanning inline markdown — the frontend sends rendered
    /// DOM text, and `**bold**text` contains `boldtext` at no tier of
    /// [`markdown::locate`], not even the whitespace-stripped one. Those marks
    /// used to flip to Stale the *first* time this ran, and since every file with
    /// marks read off disk is re-anchored on first sight (`ensure_reanchored`),
    /// that was a red "? still pertinent" on an untouched file every session.
    /// Failing to find a quote that was never found says nothing new about
    /// whether anyone edited it, so the state is left alone.
    ///
    /// Past that guard the implication is exact rather than approximate:
    /// `locate_near` is deterministic in `(source, prefix, quote, suffix, hint)`,
    /// so a quote that resolved against these bytes resolves again. Stale
    /// therefore happens if and only if the source changed such that the quote is
    /// gone — which is the only claim the margin chip is entitled to make.
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
                // Only a mark that was anchored can have come unanchored. See
                // the doc comment: `line_start == 0` is "never located", and
                // demoting it here accused a file nobody had touched.
                None if h.line_start > 0 => {
                    h.state = HighlightState::Stale;
                }
                None => {}
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

    // ---- retargeting ------------------------------------------------------

    #[test]
    fn retarget_moves_the_extent_and_re_anchors_it() {
        let source = "alpha\nbeta gamma delta\nepsilon\n";
        let mut store = Store::default();
        let id = store.add_anchored(
            FILE.to_string(),
            source,
            "gamma".into(),
            "beta ".into(),
            " delta".into(),
            Origin::Human,
        );
        assert!(store.retarget(
            &id,
            source,
            "beta gamma delta".into(),
            String::new(),
            "\nepsilon".into()
        ));
        let h = store.get(&id).expect("present");
        assert_eq!(h.quote, "beta gamma delta");
        assert_eq!(h.prefix, "");
        assert_eq!(h.suffix, "\nepsilon");
        assert_eq!((h.line_start, h.line_end), (2, 2));
    }

    #[test]
    fn retarget_keeps_the_id_the_annotation_and_the_stack_slot() {
        // The whole reason resizing is not delete-and-re-add: the question
        // attached to the passage is the expensive part, and a reader adjusting
        // where the passage starts has not withdrawn it.
        let source = "alpha\nbeta gamma\n";
        let (mut store, ids) = store_with(source, &["alpha", "beta"]);
        store.set_annotation(&ids[0], "first".into());
        store.set_annotation(&ids[1], "second".into());
        store.mark_sent(&[ids[1].clone()]);
        store.set_annotation(&ids[1], "second again".into());

        let before = store.get(&ids[1]).expect("present");
        assert!(store.retarget(
            &ids[1],
            source,
            "beta gamma".into(),
            String::new(),
            String::new()
        ));
        let after = store.get(&ids[1]).expect("present");
        assert_eq!(after.id, before.id);
        assert_eq!(after.annotation.as_deref(), Some("second again"));
        assert_eq!(after.sent_at, before.sent_at);
        assert_eq!(after.prior, before.prior);
        // Still second on the stack, still behind the pair that was queued first.
        assert_eq!(
            store
                .stack_pairs()
                .iter()
                .map(|p| p.highlight.id.clone())
                .collect::<Vec<_>>(),
            ids
        );
    }

    #[test]
    fn retarget_clears_a_stale_state_because_the_new_extent_was_just_measured() {
        let mut store = Store::default();
        let id = store.add_anchored(
            FILE.to_string(),
            "alpha\nbeta\n",
            "beta".into(),
            String::new(),
            String::new(),
            Origin::Human,
        );
        // The passage itself is edited: `beta` is gone, so the mark goes Stale.
        store.reanchor_file(FILE, "alpha\ndelta\n");
        assert_eq!(
            store.get(&id).expect("present").state,
            HighlightState::Stale
        );
        assert!(store.retarget(
            &id,
            "alpha\ndelta\n",
            "delta".into(),
            String::new(),
            String::new()
        ));
        let h = store.get(&id).expect("present");
        assert_eq!(h.state, HighlightState::Active);
        assert_eq!((h.line_start, h.line_end), (2, 2));
    }

    #[test]
    fn retarget_of_an_unlocatable_extent_keeps_the_mark_at_line_zero() {
        // `add_anchored`'s fallback, and it has to be the same one: a new extent
        // spanning inline markdown is DOM text `locate` cannot find at any tier,
        // and losing the mark over it would be worse than an imprecise line.
        let source = "alpha **beta** gamma\n";
        let mut store = Store::default();
        let id = store.add_anchored(
            FILE.to_string(),
            source,
            "alpha".into(),
            String::new(),
            String::new(),
            Origin::Human,
        );
        assert!(store.retarget(
            &id,
            source,
            "alpha beta".into(),
            String::new(),
            String::new()
        ));
        let h = store.get(&id).expect("the highlight is kept");
        assert_eq!((h.line_start, h.line_end), (0, 0));
        assert_eq!(h.quote, "alpha beta");
        // And the `line_start > 0` guard now covers it, so the next re-anchor of
        // an untouched file does not accuse it.
        store.reanchor_file(FILE, source);
        assert_eq!(
            store.get(&id).expect("present").state,
            HighlightState::Active
        );
    }

    #[test]
    fn retarget_of_an_unknown_id_changes_nothing() {
        let (mut store, ids) = store_with("alpha\nbeta\n", &["alpha"]);
        assert!(!store.retarget(
            "hdeadbeefdeadbeef",
            "alpha\nbeta\n",
            "beta".into(),
            String::new(),
            String::new()
        ));
        assert_eq!(store.get(&ids[0]).expect("present").quote, "alpha");
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

    // ---- pending, and prior ------------------------------------------------

    #[test]
    fn a_mark_made_this_session_is_neither_prior_nor_pending() {
        // The baseline the rest of this section is measured against. `prior` means
        // "done with", and a mark that was just annotated onto the stack is the
        // opposite of that — it is the thing the reader is working on.
        let (mut store, ids) = store_with("alpha\n", &["alpha"]);
        store.set_annotation(&ids[0], "why?".into());
        let h = store.get(&ids[0]).expect("present");
        assert!(!h.prior);
        assert_eq!(h.sent_at, None);
    }

    #[test]
    fn mark_sent_stamps_the_marks_and_clears_only_what_it_sent() {
        // D17: `Send selected` sends a subset. Clearing the whole stack would
        // silently discard questions that were never asked — and the order of
        // what is left is the order they were queued in, which is the order the
        // agent should eventually see them in.
        let (mut store, ids) = store_with("alpha\nbeta\ngamma\n", &["alpha", "beta", "gamma"]);
        for (i, text) in ["a", "b", "c"].iter().enumerate() {
            store.set_annotation(&ids[i], (*text).to_string());
        }

        store.mark_sent(&[ids[1].clone()]);

        assert_eq!(
            store
                .stack_pairs()
                .iter()
                .map(|p| p.highlight.id.clone())
                .collect::<Vec<_>>(),
            vec![ids[0].clone(), ids[2].clone()],
            "the unsent entries stand, in order"
        );
        assert!(
            store.get(&ids[1]).expect("present").sent_at.is_some(),
            "the sent mark is stamped"
        );
        for i in [0, 2] {
            assert_eq!(
                store.get(&ids[i]).expect("present").sent_at,
                None,
                "an unsent mark must not be stamped"
            );
        }
    }

    #[test]
    fn a_sent_mark_survives_the_send() {
        // The stack shrinks; the highlight list does not. A send is not a
        // delete — the passage stays anchored so the answer has something to
        // land on.
        let (mut store, ids) = store_with("alpha\n", &["alpha"]);
        store.set_annotation(&ids[0], "why?".into());
        store.mark_sent(&ids);
        assert!(store.stack_pairs().is_empty());
        let h = store.get(&ids[0]).expect("the highlight survives");
        assert_eq!(h.quote, "alpha");
        assert_eq!(h.annotation.as_deref(), Some("why?"));
        assert!(h.sent_at.expect("stamped") > 0);
    }

    #[test]
    fn mark_sent_ignores_an_id_it_does_not_know() {
        let (mut store, ids) = store_with("alpha\n", &["alpha"]);
        store.set_annotation(&ids[0], "why?".into());
        store.mark_sent(&["h0000000000000000".to_string()]);
        assert_eq!(store.stack_pairs().len(), 1, "the queue is untouched");
        assert_eq!(store.get(&ids[0]).expect("present").sent_at, None);
    }

    #[test]
    fn resolve_clears_the_pending_stamp() {
        // What makes the pending glyph go away. `resolve` is the only thing
        // that clears `sent_at`; nothing times it out.
        let (mut store, ids) = store_with("alpha\n", &["alpha"]);
        store.set_annotation(&ids[0], "why?".into());
        store.mark_sent(&ids);
        assert!(store.get(&ids[0]).expect("present").sent_at.is_some());

        assert!(store.resolve(&ids[0], Some("because".into())));
        let h = store.get(&ids[0]).expect("the highlight survives");
        assert_eq!(h.sent_at, None, "answered is not pending");
        assert_eq!(h.quote, "alpha", "the evidence is untouched");
    }

    #[test]
    fn mark_sent_fades_what_it_sent_and_only_that() {
        // A sent question is done with from the reader's side, so the passage
        // stops competing with the paragraph immediately rather than at the next
        // restart. `Send selected` sends a subset, so the fade has to be as
        // selective as the stamp beside it.
        let (mut store, ids) = store_with("alpha\nbeta\ngamma\n", &["alpha", "beta", "gamma"]);
        for (i, text) in ["a", "b", "c"].iter().enumerate() {
            store.set_annotation(&ids[i], (*text).to_string());
        }

        store.mark_sent(&[ids[1].clone()]);

        assert!(
            store.get(&ids[1]).expect("present").prior,
            "the sent mark reads as done with"
        );
        for i in [0, 2] {
            assert!(
                !store.get(&ids[i]).expect("present").prior,
                "a question still waiting to be asked stays at full strength"
            );
        }
    }

    #[test]
    fn a_pop_fades_the_passage_it_takes_off_the_stack() {
        // Withdrawing a question by hand and handing it to an agent differ in
        // what happened, not in what is left behind, so they read the same.
        let (mut store, ids) = store_with("alpha\nbeta\n", &["alpha", "beta"]);
        store.set_annotation(&ids[0], "why?".into());
        store.set_annotation(&ids[1], "and this?".into());

        store.remove_from_stack(&ids[0]);

        assert!(store.get(&ids[0]).expect("present").prior);
        assert!(
            !store.get(&ids[1]).expect("present").prior,
            "the pair still queued is untouched"
        );
        assert_eq!(store.stack_pairs().len(), 1);
    }

    #[test]
    fn resolving_fades_the_mark_as_well() {
        // `resolve` goes through `remove_from_stack`, and an agent closing a
        // question is the strongest form of "done with" there is. Asserted rather
        // than left to the call graph, because routing `resolve` around
        // `remove_from_stack` would silently drop the fade.
        let (mut store, ids) = store_with("alpha\n", &["alpha"]);
        store.set_annotation(&ids[0], "why?".into());
        assert!(store.resolve(&ids[0], Some("because".into())));
        assert!(store.get(&ids[0]).expect("present").prior);
    }

    #[test]
    fn the_fade_tracks_the_stack_in_both_directions() {
        // The whole rule in one pass, because the three writers only make sense
        // against each other: bright on the stack, faded off it, and annotating
        // is what puts a passage back on.
        let (mut store, ids) = store_with("alpha\n", &["alpha"]);
        store.set_annotation(&ids[0], "why?".into());
        assert!(
            !store.get(&ids[0]).expect("present").prior,
            "queued: bright"
        );

        store.mark_sent(&ids);
        assert!(store.get(&ids[0]).expect("present").prior, "sent: faded");

        store.set_annotation(&ids[0], "and also why?".into());
        let h = store.get(&ids[0]).expect("present");
        assert!(!h.prior, "re-annotated: bright again");
        assert_eq!(store.stack_pairs().len(), 1, "and back on the stack");
        assert!(
            h.sent_at.is_some(),
            "re-annotating does not retract the question already asked — the \
             fade and the pending stamp answer different questions"
        );

        store.remove_from_stack(&ids[0]);
        assert!(store.get(&ids[0]).expect("present").prior, "popped: faded");
    }

    #[test]
    fn re_annotating_revives_a_prior_mark() {
        // D13, widened. The fade means "done with", so annotating — which is
        // picking the passage back up — returns it to full saturation.
        let (mut store, ids) = store_with("alpha\n", &["alpha"]);
        store.highlights.iter_mut().for_each(|h| h.prior = true);
        store.set_annotation(&ids[0], "why?".into());
        assert!(!store.get(&ids[0]).expect("present").prior);
    }

    #[test]
    fn re_annotating_a_pending_mark_mints_a_second_question() {
        // D14. The question already out with the agent is not retracted — the
        // stamp stays — and the passage is queued again, so the send that
        // follows asks a second thing about the same lines.
        let (mut store, ids) = store_with("alpha\nbeta\n", &["alpha", "beta"]);
        store.set_annotation(&ids[0], "why?".into());
        store.mark_sent(&[ids[0].clone()]);

        store.set_annotation(&ids[1], "unrelated".into());
        store.set_annotation(&ids[0], "and also why?".into());

        let h = store.get(&ids[0]).expect("present");
        assert!(
            h.sent_at.is_some(),
            "re-annotating must not retract the question already asked"
        );
        assert_eq!(h.annotation.as_deref(), Some("and also why?"));
        assert_eq!(
            store
                .stack_pairs()
                .iter()
                .map(|p| p.highlight.id.clone())
                .collect::<Vec<_>>(),
            vec![ids[1].clone(), ids[0].clone()],
            "the fresh entry goes to the back of the queue, behind what was \
             already waiting"
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

    /// The quote here is what `getSelection().toString()` returns over
    /// `the **bold** word`: no asterisks, because the DOM has thrown them away.
    /// No tier of `locate` can find it — tier 3 strips whitespace, not syntax —
    /// so the mark is stored at `(0, 0)`, and re-anchoring against the *same*
    /// bytes must not read that as an edit.
    #[test]
    fn a_mark_that_never_anchored_does_not_go_stale_on_an_untouched_file() {
        const SOURCE: &str = "the **bold** word\n";
        let mut store = Store::default();
        let id = store.add_anchored(
            FILE.to_string(),
            SOURCE,
            "bold word".to_string(),
            String::new(),
            String::new(),
            Origin::Human,
        );
        assert_eq!(
            store.get(&id).expect("present").line_start,
            0,
            "the fixture is only interesting if the quote really is unlocatable"
        );

        let after = store.reanchor_file(FILE, SOURCE);
        assert_eq!(after.len(), 1);
        assert_eq!(
            after[0].state,
            HighlightState::Active,
            "nothing was edited, so nothing went stale"
        );
    }

    /// The other side of that guard: it keys on `line_start`, not on which
    /// constructor made the mark. An `add_anchored` highlight that *did* locate
    /// still goes stale when its text is taken away.
    #[test]
    fn a_mark_that_anchored_still_goes_stale_when_its_text_goes() {
        let mut store = Store::default();
        let id = store.add_anchored(
            FILE.to_string(),
            "alpha\nbeta\n",
            "beta".to_string(),
            String::new(),
            String::new(),
            Origin::Human,
        );
        assert_eq!(store.get(&id).expect("present").line_start, 2);

        let after = store.reanchor_file(FILE, "alpha\nBETA IS GONE\n");
        assert_eq!(after[0].state, HighlightState::Stale);
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
