//! In-memory highlight + annotation store and the send stack.
//!
//! Nothing here is ever persisted: the whole store lives in process memory,
//! spans every file opened during the session, and is destroyed when dreamd
//! exits. Switching files does NOT clear it.

use crate::markdown;
use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HighlightState {
    /// Anchor still resolves; shown in the default color.
    Active,
    /// The highlighted text itself was edited; shown red with a margin "?".
    Stale,
}

#[derive(Debug, Clone, Serialize)]
pub struct Highlight {
    pub id: u64,
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
}

/// A queued (highlight + annotation) pair, as sent to the frontend stack panel.
#[derive(Debug, Clone, Serialize)]
pub struct Pair {
    pub highlight: Highlight,
    pub annotation: String,
}

#[derive(Default)]
pub struct Store {
    next_id: u64,
    highlights: Vec<Highlight>,
    /// Ordered highlight ids queued for the next send.
    stack: Vec<u64>,
}

impl Store {
    pub fn add_highlight(
        &mut self,
        file_path: String,
        line_start: usize,
        line_end: usize,
        quote: String,
        prefix: String,
        suffix: String,
    ) -> u64 {
        self.next_id += 1;
        let id = self.next_id;
        self.highlights.push(Highlight {
            id,
            file_path,
            line_start,
            line_end,
            quote,
            prefix,
            suffix,
            state: HighlightState::Active,
            annotation: None,
        });
        id
    }

    /// Attach/replace an annotation and enqueue the pair on the stack.
    pub fn set_annotation(&mut self, id: u64, text: String) -> bool {
        let Some(h) = self.highlights.iter_mut().find(|h| h.id == id) else {
            return false;
        };
        h.annotation = Some(text);
        if !self.stack.contains(&id) {
            self.stack.push(id);
        }
        true
    }

    pub fn remove(&mut self, id: u64) {
        self.highlights.retain(|h| h.id != id);
        self.stack.retain(|x| *x != id);
    }

    pub fn remove_from_stack(&mut self, id: u64) {
        self.stack.retain(|x| *x != id);
    }

    fn find(&self, id: u64) -> Option<&Highlight> {
        self.highlights.iter().find(|h| h.id == id)
    }

    pub fn get(&self, id: u64) -> Option<Highlight> {
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
    pub fn selected_pairs(&self, ids: &[u64]) -> Vec<Pair> {
        ids.iter()
            .filter_map(|id| self.find(*id))
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
}

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "/repo/a.md";

    fn store_with(source: &str, quotes: &[&str]) -> (Store, Vec<u64>) {
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

    #[test]
    fn ids_are_monotonic_from_one() {
        let (_, ids) = store_with("alpha\nbeta\ngamma\n", &["alpha", "beta", "gamma"]);
        assert_eq!(ids, vec![1, 2, 3]);
    }

    #[test]
    fn ids_are_not_reused_after_a_removal() {
        // A removed id must stay dead: the frontend holds ids in the DOM, and
        // reuse would silently re-point a stale element at a new highlight.
        let (mut store, ids) = store_with("alpha\nbeta\n", &["alpha", "beta"]);
        store.remove(ids[1]);
        let next = store.add_highlight(
            FILE.to_string(),
            1,
            1,
            "alpha".into(),
            String::new(),
            String::new(),
        );
        assert_eq!(next, 3);
    }

    #[test]
    fn a_fresh_highlight_is_active_and_unannotated() {
        let (store, ids) = store_with("alpha\n", &["alpha"]);
        let h = store.get(ids[0]).expect("present");
        assert_eq!(h.state, HighlightState::Active);
        assert_eq!(h.annotation, None);
        // ...and therefore not on the stack.
        assert!(store.stack_pairs().is_empty());
    }

    #[test]
    fn set_annotation_is_what_enqueues() {
        let (mut store, ids) = store_with("alpha\nbeta\n", &["alpha", "beta"]);
        assert!(store.set_annotation(ids[1], "why?".into()));
        let pairs = store.stack_pairs();
        assert_eq!(pairs.len(), 1);
        assert_eq!(pairs[0].highlight.id, ids[1]);
        assert_eq!(pairs[0].annotation, "why?");
    }

    #[test]
    fn re_annotating_replaces_the_text_without_reordering_the_stack() {
        let (mut store, ids) = store_with("alpha\nbeta\n", &["alpha", "beta"]);
        store.set_annotation(ids[0], "first".into());
        store.set_annotation(ids[1], "second".into());
        store.set_annotation(ids[0], "revised".into());
        let pairs = store.stack_pairs();
        assert_eq!(
            pairs.iter().map(|p| p.highlight.id).collect::<Vec<_>>(),
            ids,
            "re-annotating must not move a pair to the back"
        );
        assert_eq!(pairs[0].annotation, "revised");
    }

    #[test]
    fn annotating_an_unknown_id_is_a_no_op() {
        let mut store = Store::default();
        assert!(!store.set_annotation(99, "why?".into()));
        assert!(store.stack_pairs().is_empty());
    }

    #[test]
    fn remove_clears_the_highlight_and_the_stack_entry() {
        let (mut store, ids) = store_with("alpha\n", &["alpha"]);
        store.set_annotation(ids[0], "why?".into());
        store.remove(ids[0]);
        assert!(store.get(ids[0]).is_none());
        assert!(store.stack_pairs().is_empty());
        assert!(store.for_file(FILE).is_empty());
    }

    #[test]
    fn remove_from_stack_keeps_the_highlight() {
        let (mut store, ids) = store_with("alpha\n", &["alpha"]);
        store.set_annotation(ids[0], "why?".into());
        store.remove_from_stack(ids[0]);
        assert!(store.stack_pairs().is_empty());
        assert!(store.get(ids[0]).is_some(), "the highlight itself survives");
    }

    #[test]
    fn selected_pairs_honours_the_order_given_and_skips_the_unusable() {
        let (mut store, ids) = store_with("alpha\nbeta\ngamma\n", &["alpha", "beta", "gamma"]);
        store.set_annotation(ids[0], "a".into());
        store.set_annotation(ids[2], "c".into());
        // ids[1] has no annotation; 99 has no highlight. Both are skipped
        // rather than erroring or producing an empty pair.
        let picked = store.selected_pairs(&[ids[2], 99, ids[1], ids[0]]);
        assert_eq!(
            picked.iter().map(|p| p.highlight.id).collect::<Vec<_>>(),
            vec![ids[2], ids[0]]
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

    // ---- reanchor ---------------------------------------------------------

    #[test]
    fn reanchor_follows_a_quote_that_moved() {
        let (mut store, ids) = store_with("alpha\nbeta\n", &["beta"]);
        let after = store.reanchor_file(FILE, "inserted\nlines\nalpha\nbeta\n");
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].id, ids[0]);
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
        let h = store.get(other).expect("present");
        assert_eq!(h.line_start, 7);
        assert_eq!(h.state, HighlightState::Active);
    }
}
