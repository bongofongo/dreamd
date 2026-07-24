# Session log

## 2026-07-24 — session rituals: wrap-up skill + daily project doc

Ported the blogregator docs setup into dreamd: a `/wrap-up` skill, a
`/update-project-doc` skill, the `engies/project.md` landing page, and a cloud
routine that refreshes that page daily. All landed.

### What happened

1. **Surveyed the source pattern.** Read blogregator's `CLAUDE.md`,
   `engies/project.md`, and `engies/ai-practices.md`, plus the existing wrap-up
   skills in `tree/` and `spotify_interview/`. Found blogregator has no
   `.claude/` of its own — the wrap-up ritual lives in those other repos, and
   what blogregator contributes is the `engies/` convention plus the daily job.
   Also found the blogregator routine creation had **failed** with a 403
   ("You don't have access to a repository this routine uses") — the daily job
   the user believed was running never existed.

2. **`.claude/skills/wrap-up/SKILL.md`.** Review diff → gate (`cargo build`
   only when `src-tauri/` is touched) → prepend a dated section to
   `docs/session-log.md` → refresh `engies/project.md` if the project story
   changed → one atomic commit **straight to main** + push → lean memory →
   report. Log layout decision: keep dreamd's existing single running file and
   prepend newest-first, rather than adopting the `session-logs/` directory the
   other two repos use.

3. **`.claude/skills/update-project-doc/SKILL.md`.** Regenerates
   `engies/project.md` from `git log` + `docs/session-log.md` + `README.md` +
   the source tree. Pins the section contract, the entry-level voice, and an
   explicit *"if nothing meaningful changed, do not manufacture news — leave the
   file untouched and make no commit"* rule, so a daily unattended job can't
   invent progress. Commits only that one path.

4. **`engies/project.md`.** The human landing page for dreamd: product loop,
   module-by-module architecture, honest known limits, glossary, reverse-chron
   "Recent updates".

5. **`CLAUDE.md`.** Terse machine-facing tenets (read-only, nothing persists, no
   shell interpolation of user content, escape-don't-execute, CSS-themeable) plus
   the docs conventions. Human-facing guidance deliberately stays in `engies/`.

6. **Cloud routine.** `trig_01GLUNmetTpUmT5ptfLzrMLM`, cron `3 7 * * *` UTC
   (≈08:03 UK in BST), sonnet-5, tools limited to Bash/Read/Write/Edit/Glob/Grep.
   Its prompt tells the agent to read `.claude/skills/update-project-doc/SKILL.md`
   from the checkout and follow it — so editing the skill changes the job, no
   routine edit needed.

### Mistakes & deviations

- **First routine creation 403'd**, same as blogregator's: claude.ai had no
  GitHub access to `bongofongo/dreamd`. Saved the exact create body to the
  scratchpad, reported the blocker with the fix (connect GitHub at
  claude.ai/code). User updated the Claude GitHub app; the retry returned 200.
- **Test run was inconclusive.** Fired the routine manually and polled
  `git ls-remote origin main` for ~5 min — no new commit. That is the expected
  no-op path (project.md was written the same day from the same git log), but
  the cloud session's transcript isn't readable from the CLI, so *correct no-op*
  and *failed run* look identical from here. Reported it as unproven rather than
  claiming success. Real verification comes at the next scheduled run.

### State

Docs/skills only — no Rust touched, no build gate needed. Skills committed and
pushed to main (`b78c9fb`). Routine created and enabled, next run
2026-07-25 07:03 UTC. `engies/project.md` left as written earlier this session;
its top "Recent updates" bullet already covers this work.

## 2026-07-24 — v1 build

Went from an empty scaffold to a working v1 of dreamd in one session.

### What happened

1. **Plan review.** Attacked the original `docs/plan.md` for gaps: the tmux
   send-to-Claude design (injection/escaping), raw-HTML XSS in the webview,
   scroll loss on live reload, missing link/image handling, no launch CLI, and
   the "Telescope reuse" assumption. Reworked the design around a
   highlight → annotation → **stack** → send loop (annotations promoted from v2
   to v1 core; nothing persisted — in-memory, dies with the process).

2. **Backend (`src-tauri/src/`).**
   - `fs_walk` — `ignore`-crate markdown scan → `FileNode` tree.
   - `markdown` — `pulldown-cmark` + `syntect`; raw HTML **escaped** (XSS closed);
     `locate()` powers both anchoring and evidence `file:line`.
   - `annotations` — in-memory highlights/annotations/stack + re-anchoring
     (Active → Stale when the highlighted text itself is edited).
   - `search` — `nucleo` fuzzy over file paths (Telescope lookalike).
   - `send` — assemble a temp query file; auto-detect a `claude` tmux pane and
     type a fixed `read @file` prompt (no shell interpolation), else clipboard.
   - `watcher` — `notify` → `file-added/changed/removed` + `theme-reloaded`.
   - `config` — TOML global + repo-local `.dreamd.toml` override.

3. **Frontend (`ui/`).** Tree, fuzzy palette, stack panel, annotation modal,
   live highlight wrap, stale margin rail, scroll-preserving reload, link/image
   resolution, embedded hot-reloadable theme.

4. **Security fix.** Restricted `open_external` to `http`/`https`/`mailto`;
   stopped routing bare local paths to the OS opener; gated relative images to
   inside the repo root.

### UI iterations (same session)

- Fixed viewer scrolling (grid row was unbounded).
- Highlight mode: highlighter icon toggles auto-highlight-on-select; `h`
  highlights the current selection and prompts for an annotation.
- Collapsible panes; edit existing highlights by clicking them (re-add / edit /
  delete), which is also how a removed stack pair gets re-added.
- Overlay titlebar (macOS) so **highlight · stack · send** icons sit on the
  traffic-light row; file path removed from the top bar; repo root shown
  home-relative (`~/…`) in the tree header.
- Collapse arrow: `◀` in the tree header when expanded, floating `▶` when
  collapsed; preview always full-width.
- Per-file `⋯` menu: Copy path / Delete (moves to OS Trash, repo-scoped, with a
  confirm dialog).
- `Ctrl+Y` submits the annotation from the textarea (keyboard-only flow).
- Vim-style keybinds: palette on `Ctrl+F`, `Ctrl+P`/`Ctrl+N` prev/next in the
  palette, `Ctrl+O` toggles the stack, `Ctrl+C` copies the stack (defers to the
  OS copy when text is selected), `Ctrl+Enter` sends.
- nvim-style CLI: `dreamd file.md` opens the file on load while the tree stays
  rooted at the current directory's repo.

### State

Compiles clean (`cargo build`); launches and passes startup smoke tests. The
send loop was verified end-to-end (a real stack landed as a formatted query).

### Not yet verified / known limits

- Full GUI interactions (traffic-light alignment, drag, ⋯ menu, Trash
  round-trip) checked only by launch smoke tests, not interactively.
- Highlight DOM re-wrap uses single-node text search; heavily formatted
  selections may read as stale.
- tmux `claude`-pane detection is heuristic (may run as `node`); pin
  `tmux_target` in config for reliability.
- No unit tests yet — `locate()`/`reanchor` are the obvious first targets.
- Fuzzy search covers paths only; content/`live_grep` is a v2 item.
- Placeholder app icon (blue square).
