# Auto-update on .md add/edit/remove

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
