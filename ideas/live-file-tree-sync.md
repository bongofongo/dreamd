# Auto-update on .md add/edit/remove

## ADDENDUM
Not sure why it wasn't working before, but as of right now it seems to be working.

**2026-07-26: closed, no work needed.** Live file-tree sync works for the most
part in day-to-day use — no big issue with it at the moment. Dropped from the
overnight idea pass; the "don't trust it, verify it" section below is a
someday-maybe, not a task.

Have the app stay live as `.md` files are added, edited, or removed under the
app root, without a restart.

## Current state

This already exists, repo-wide, not just at the top level. `watcher.rs`
recursively watches `repo_root` with `notify` and emits `file-added` /
`file-changed` / `file-removed` (debounced 60ms to collapse editor-save
bursts). `ui/app.js:717-747` listens for all three: `file-changed` re-renders
the open document if it's the current file, `file-added`/`file-removed` call
`rebuild_index` and repaint the tree.

## Worth checking rather than building

- Confirm this is actually the gap you're seeing, versus already working —
  if there's a specific case where it *doesn't* update (a particular editor's
  save pattern, a symlinked file, something under a dotfile-ignored dir via
  the `ignore`-crate walker), that's a bug report against the existing
  watcher, not new scope.
- "Root directory (app root)" — current behavior watches the whole resolved
  repo recursively, which is broader than root-only. If you specifically want
  root-only (ignore subdirectories), that would be a narrowing, not the
  addition it reads as.
- Interacts with the single-file-boot-speed idea in `docs/todo.md`: once tree
  building is deferred for a single-file launch, the watcher/tree-rebuild path
  needs to still wake up correctly the first time a tree gets requested, not
  assume it was built at startup.

## Decision: don't trust it, verify it

You suspect the watcher isn't actually doing its job right. Two concrete
next steps, before any new feature work here:

- **Tests.** The repo has no `#[cfg(test)]` unit tests by convention —
  `cargo test` compiles and reports nothing; correctness today comes from
  running the app, the benches, and the two example harnesses
  (`locate_check`, `config_check`, `theme_check`). A watcher test likely
  wants to join that family: a new `cargo run --example watcher_check`
  that drives `watcher.rs`'s `absorb`/`pump`/`emit` logic against a scratch
  directory with synthetic create/modify/remove sequences and asserts the
  right event comes out — including the tricky case the code already calls
  out in comments (a remove-then-create in one debounce window, from an
  editor that saves via temp-file-and-rename, should still net out to
  `file-changed`, not `file-removed`).
- **User testing.** Manually exercise the real dev loop before trusting any
  of this: `nvim :w`, a `git checkout` that touches several files at once,
  and whatever editor/save pattern you actually use day to day. The
  watcher's own comments flag both macOS FSEvents firing ~1.6 events per
  save and `git checkout` firing one event per changed file as the cases
  the debounce exists for — worth confirming those are actually collapsed
  correctly rather than assuming the debounce constant is right.
