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
            match index.locate(&h.prefix, &h.quote, &h.suffix) {
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
