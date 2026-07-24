# Session log

## 2026-07-24 — performance measurement framework

Built the three-tier performance harness (`perf/`) and captured a first baseline.
No optimizations yet — this session was about being able to prove them. It got
there, but only after the harness was caught lying five separate times.

### What happened

1. **`src-tauri` split into `lib.rs` + a thin `main.rs`.** `main.rs` keeps the CLI,
   `AppState`, the 21 commands and the builder; everything they do moved to the
   library. Code moved verbatim — no logic changed. This is what makes
   `src-tauri/benches/` and future unit tests possible at all, since a `[[bin]]`
   target can't be imported.

2. **Deterministic corpus** (`perf/corpus/gen.mjs`, node, no deps). 5,536 files /
   21MB: four document variants at 8KB–2MB, synthetic repos of 10/500/5000 files,
   highlight sets of 1/10/100/500. Seeded PRNG, byte-identical across runs; a
   3.5KB summary manifest with an aggregate digest is committed rather than 5,500
   individual hashes.

   Highlight fixtures carry both `quote` (exact source) and `rendered`
   (whitespace-collapsed). That distinction turned out to be load-bearing — see
   the findings below.

3. **Four criterion suites** — `render`, `locate`, `search`, `walk` — with sample
   counts capped per group so the deep sweep stays inside its budget.

4. **A `perf` cargo feature** (off by default, verified to compile out) emitting
   NDJSON timing marks on **stderr**, because `console.log` inside WKWebView never
   reaches the process's stdout. Rust marks plus frontend marks forwarded through a
   `perf_mark` command land in one ordered stream. Also `--bench-startup` and a
   `DREAMD_PERF_SEED` hook so the save loop can be measured at a realistic
   highlight count without driving the UI.

5. **Playwright/Chromium harness** (`perf/harness/`, test-only, never referenced by
   `tauri.conf.json`) with a stubbed `window.__TAURI__`. Scroll cost is taken from
   CDP trace events, not `performance.now()` — style, paint, raster and composite
   happen outside JavaScript, so timing a `scrollTop` assignment from inside the
   page reports approximately zero no matter how expensive the scroll is.

6. **Runner, three tiers, one entry point.** quick ~80s, pass ~6min, deep ~20min.
   Metrics are flattened to dot-paths and diffed against `perf/baseline.json`;
   adding a measurement anywhere shows up in the table with no registry to update.

7. **Three skills** — `perf-quick`, `perf-pass`, `perf-deep` — plus a perf clause in
   `wrap-up`'s gate and a line in CLAUDE.md.

### Findings (all measured, release profile)

- `reanchor` costs **~7ms per highlight** on a 2MB document — and a *live* highlight
  costs exactly what a stale one does (7.08ms vs 7.10ms), because a quote taken from
  the rendered DOM never matches `locate`'s cheap tiers and always rebuilds the
  whitespace-stripped index. 500 highlights = 3.9s.
- The watcher emits **1.6 events per save**, and they compound: `reanchor` measured
  at 74ms in isolation becomes 2.5s in the live app because the re-renders serialize
  on the main thread. Save→repaint is **5.4s** at 100 highlights.
- First paint **1,417ms** on a 5000-file repo, 1,189ms on a trivial one. The entire
  pre-window Rust sequence is only 59ms of that.
- 512KB of markdown becomes **1.06MB of HTML** with ~12,000 inline-styled spans.
- Selections spanning inline markup **fail to anchor 76–100%** of the time and are
  silently demoted to stale chips. That is a correctness bug, found incidentally.

### Mistakes & deviations

This thread did not run clean; the harness produced confident, plausible, wrong
numbers five times, and each one had to be caught by disbelieving a result.

- **`grep -q` under `set -o pipefail`** reported every symbolicated binary as
  stripped — `grep -q` exits early, `nm` dies of SIGPIPE, the pipeline reports
  failure despite matching.
- **`$!` on a subshell** meant the kill hit the subshell, not the app, leaking a
  live `dreamd` per run. Fixed with `exec`.
- **A relative binary path after `cd`** made first-paint measurement return empty
  rather than wrong — silence, not an error.
- **Workload mismatches keyed to the same metric path**, three times over: quick
  rendered 512KB while deep rendered 2MB; quick cut `--sample-size` to 10 while deep
  used full sampling; `pass` measured the debug binary while the baseline held
  release. Each produced double-digit phantom deltas — the 250% "regression" in the
  final `perf-pass` was the last of them.
- **Thresholds set below the noise floor.** Two runs on identical code disagreed by
  up to 27% on Chromium raster. 5%/15% guaranteed false positives.
- I also **corrupted three of my own baseline runs** by running `cargo clippy` and
  `gen.mjs --force` alongside them, and by killing leaked processes mid-measurement.

The corrections, and the rule they produced: **tiers differ in how much they run,
never in how they run it**, and anything that differs about a workload belongs in
the metric's key, not silently in its value. Plus a lockfile, load recorded in
`meta`, min-of-3 for launches (first-paint spread went from ~3x to 59ms), and
thresholds measured rather than guessed.

Two items from the plan were corrected by the measurements: adding prefix/suffix
context does **not** fix `reanchor` (rendered context misses tier 1 identically —
7.22ms vs 7.21ms; the fix is memoizing the stripped index once per call), and
deduplicating the double repo walk is worth ~57ms, not the headline it was ranked
as. The debounce fix is the top item, above where it was ranked.

### State

`cargo build` clean with and without `--features perf`; `cargo clippy --all-targets`
clean; all shell and JS syntax-checked. Verified the harness detects the known
missing-debounce bug positively (1.6 events/save where >1.0 is the signal), that
injected +40% and +50% regressions are caught and exit non-zero, and that two runs
on identical code flag nothing.

The final `perf-pass` reported 33 regressions; **all are harness artifacts, not code
regressions** — 21 from the debug-vs-release profile mismatch fixed in this commit,
the rest sub-5ms benchmarks within noise. No measured Rust logic changed this
session, so bench movement is noise by construction.

`perf/baseline.json` is committed from a clean deep run. Its `real.*` entries
predate the profile-keying fix and will realign on the next
`./perf/run.sh deep --update-baseline`; `bench.*` and `chromium.*` are current. The
baseline was deliberately not hand-edited to paper over this.

Nothing optimized yet. The ranked fix list stands, led by watcher debounce,
`reanchor` index memoization, and syntect warm-up.

## 2026-07-24 — icon-button tooltips with keybinds

Every icon-only button in the GUI now shows a hover popup naming the button and,
where one exists, the keybind that triggers it. Frontend-only; landed.

### What happened

1. **Inventoried the icon-only buttons.** Seven: `#btn-hl-mode`, `#btn-stack`,
   `#btn-send` (titlebar), `#btn-collapse` / `#btn-expand` (file tree),
   `#stack-close`, and the per-file `⋯` (`.file-opts`, built in `app.js`).

2. **Dropped native `title` in favour of `data-tip`.** The browser tooltip has a
   ~1s delay, can't be styled, and can't render the keybind chip. Each button now
   carries `data-tip="<label>"` plus an optional `data-tip-key="<keymap field>"`
   (`toggle_stack`, `send_stack`). Storing the *keymap field name* rather than a
   literal combo means a user-configured bind from `get_keymap` renders correctly
   — the tooltip reads the live `keymap` object at show time.

3. **`wireTooltips()` in `ui/app.js`.** One `#tooltip` div, delegated
   `mouseover`/`mouseout` off `document` so tree rows rendered later are covered
   without rewiring. 350ms delay, positioned below the button and flipped above
   when it would clip the viewport bottom, clamped horizontally. Also fires on
   `focusin` (keyboard parity) and hides on click, scroll, and blur — a click
   means the user already knows what the button does.

4. **Styling in `index.html`'s structural `<style>`, not `theme.css`.** The popup
   is chrome, not reading surface; it inherits the existing `--sidebar-bg` /
   `--border` vars so a theme still recolours it.

### Mistakes & deviations

- First cut called `scheduleTip()` without claiming `tipTarget` up front, so every
  `mouseover` bubbling from inside the button restarted the 350ms timer and the
  tooltip never appeared while the mouse was moving. Fixed by setting `tipTarget`
  at schedule time, not at show time.
- The `innerHTML` write tripped a security hook warning; both interpolations go
  through the existing `escapeHtml()`, so it was left as-is.

### State

`node --check ui/app.js` passes. No Rust touched, so no `cargo build` gate. Not
exercised in a running window — the hover behaviour is unverified visually.

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
