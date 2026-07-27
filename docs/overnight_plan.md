# Overnight idea-implementation run at 03:00 UTC (2026-07-27)

## Context

There are 14 idea files under `ideas/` from a series of planning conversations
(two are out of scope: `file-import-to-markdown.md` is blocked, and
`live-file-tree-sync.md` was closed on 2026-07-26 — the watcher works fine in
practice, nothing to do). The user wants an
unattended, scheduled pass tonight at 3am GMT where Opus 5 works through the
remaining 12, implementing the low-risk/easy ones and writing implementation
plans (not code) for anything that touches a lot of the codebase or carries
real risk — using one subagent per idea to keep the orchestrating context
clean, logging each idea to its own slugged file, then at the end folding all
the per-idea logs into one `docs/session-log.md` entry and deleting the
individual logs. (Per-idea and final perf runs were part of the original ask
but the user has since said to skip perf entirely this round — see below.)
The ask at hand right now is just: does this plan work — so this plan needs
to nail down the mechanics before anything is scheduled.

Research this session did to ground the plan:

- **Scheduling mechanism matters more than it looks.** `CronCreate` is
  explicitly session-only — "gone when Claude exits" — and this remote
  environment reclaims idle containers. It's currently 22:18 UTC; 03:00 UTC on 2026-07-27 is
  ~4h40m away, all of it idle. That's a real risk the job silently never
  fires. The `claude-code-remote` MCP server's `create_trigger` (a
  "Routine") is account-level and durable, and supports a one-shot
  `run_once_at` RFC3339 timestamp plus `create_new_session_on_fire: true`,
  which spins up a **fresh session** in the environment rather than depending
  on this exact session's container surviving the gap. That's the right tool
  here, not `CronCreate`.
- **Model selection.** `create_trigger` has no `model` param, but
  `update_trigger` does, and explicitly: "only fires that create a new
  session pick up the new model." So the flow is `create_trigger` then
  `update_trigger` with `model: claude-opus-5`, targeting the same
  `trigger_id`. Subagents spawned by that session inherit its model
  automatically (per the `Agent` tool's own semantics), so per-idea agents
  don't need the model set individually — though it's cheap to pass
  explicitly for clarity.
- **Sequential, not parallel, and this isn't a compromise — it's required.**
  Two independent facts point the same way (a third — `perf/run.sh`'s
  exclusive lock forcing one-tier-at-a-time — no longer applies now that
  perf isn't run at all, but the other two hold regardless):
  - Several ideas touch the *same* hot spots: `hide-file-tree-keybind`,
    `view-mode-keybind`, `next-prev-file-keybind`, and
    `jump-top-bottom-keybind` all add to the `Keymap` struct in
    `src-tauri/src/config.rs`, the global keydown dispatcher in `ui/app.js`,
    and the settings-panel actions list in `ui/app.js`. Editing these
    concurrently (e.g. via isolated git worktrees merged later) risks
    conflicting/duplicated hunks in the same struct and array. Sequential,
    same-checkout execution avoids this entirely — there's never a moment
    two agents touch `config.rs` at once, because the prior agent's commit
    already landed.
  - `contents-outline-panel.md` and the anchor-link half of
    `file-and-section-links.md` share a real prerequisite: heading `id`
    attributes in `src-tauri/src/markdown.rs`'s render pass don't exist yet,
    and both need them. Doing `contents-outline-panel` first means the
    second idea's agent can build on committed work instead of redoing it.
  - Net: worktree-based parallelism would cost more (redundant `cargo build`
    per worktree, plus a merge step) than it would save here. So: **one idea
    at a time, foreground, fully committed before the next starts** —
    implemented as the top-level
    fired session calling the `Agent` tool once per idea and waiting for
    each to finish, not the `Workflow` tool's fan-out helpers (which are for
    independent, non-conflicting work — not this batch).
- **No perf runs at all, per explicit user instruction.** `perf/run.sh` isn't
  yet confirmed working on Linux and hasn't been tested off the user's own
  machine — this remote container is a different environment than where the
  perf harness has actually been validated. Originally this plan called for
  `perf-quick` per idea and a final `perf-pass`; both are dropped entirely.
  The user will run perf themselves, on their own machine, when reviewing
  the PR. (This also sidesteps a real headless-environment wrinkle this
  session found — `DISPLAY` is unset here and `perf-pass`/`perf-deep` open
  the real Tauri window unless given `--no-window` — moot now that no perf
  tier runs in this environment at all.)
- **The final aggregation step already has a home, minus the perf step.**
  The `.claude/skills/wrap-up/SKILL.md` skill already does almost exactly
  what's needed for the finish line: gate on `cargo build`, prepend one
  dated section to `docs/session-log.md` (newest-first, with `### What
  happened` / `### Mistakes & deviations` / `### State` subsections),
  refresh `engies/project.md` if the story changed, commit, push. Follow
  that shape for the aggregation step, but skip its perf-pass gate — note
  in the `### State` section that perf was deliberately not run in this
  environment and is left for the user's own machine.
- **Risk classification is the user's own stated criteria, applied by each
  idea's own agent** — not pre-baked into the orchestrator. The user's rule
  ("doesn't pose great risk / easier ones → implement; touches a lot of the
  codebase / much higher risk → plan only") is given to every per-idea agent
  verbatim, and it makes its own call, since several ideas are genuinely
  mixed (e.g. `file-and-section-links.md` bundles a small, low-risk path-
  containment fix with a cross-cutting "jump back" navigation stack that
  touches every navigation entry point — one file, two risk levels).

## Recommended approach

**1. Schedule it now, in this session, using `create_trigger` + `update_trigger`:**

- `create_trigger(name: "dreamd overnight idea pass", run_once_at:
  "2026-07-27T03:00:00Z", create_new_session_on_fire: true, prompt:
  <the full standalone instructions below>, notifications: {push: true})` —
  a fresh session has no memory of this conversation, so the prompt must be
  fully self-contained (repo location, the ideas/ directory's purpose, the
  exact per-idea process, the skip list (`file-import-to-markdown.md`, `live-file-tree-sync.md`),
  the ordering note for the heading-id dependency, the no-perf instruction,
  the branch-not-main override below, and the final
  aggregation-into-wrap-up-shape instructions). Push notification only, per
  user's answer — no email.
- `update_trigger(trigger_id: <from above>, model: "claude-opus-5")`
  immediately after, so the fired session runs on Opus 5.

**Explicit deviation from repo convention, for this run only:** `CLAUDE.md`
and the `wrap-up` skill both say dreamd commits straight to main, no
branches, no PRs. The user chose a review branch instead for this
unsupervised overnight batch, given its scale (~12 sequential commits with
no one watching). The fired session's prompt must say this explicitly and
prominently — otherwise a fresh session reading `CLAUDE.md` naturally
defaults back to "straight to main," which is exactly the opposite of what
was asked here. Branch name: `claude/overnight-ideas-2026-07-27`, created off **`origin/main` after a `git fetch`** — the branch must start
from the newest commit pushed to `main`, not from whatever the container's
checkout happens to hold. Every commit in this
run (per-idea and the final aggregation) goes to that branch, not main.
After the aggregation commit, push the branch and open a PR against `main`
summarizing what was implemented vs. plan-only per idea, so there's a single
review surface in the morning rather than 12+ commits to wade through
individually.

**2. The fired session's standalone prompt instructs it to:**

0. `git fetch origin` first, then branch off `origin/main` — start from the
   newest commit pushed to `main`, never from a stale local checkout.
1. Read every file in `ideas/` except two: `file-import-to-markdown.md`
   (blocked) and `live-file-tree-sync.md` (closed 2026-07-26 — sync works in
   practice). Skip both entirely — don't even plan them. Whatever else is
   there at fire time is the scope; don't work from a hardcoded count.
2. Order the rest so `contents-outline-panel.md` precedes the
   `file-and-section-links.md` anchor-link work (shared heading-id
   prerequisite).
3. For each idea, in order, foreground (wait for completion before starting
   the next): spawn one `Agent` (general-purpose, model `opus`) whose prompt
   contains the idea file's full content, the user's risk criteria verbatim,
   and instructions to: implement the low-risk slice and/or write a
   plan-only doc for the high-risk slice (an idea can be split, per the
   `file-and-section-links.md` example above); **do not run any perf
   tier** — explicitly told not to, since the harness isn't confirmed
   working on Linux/off the user's machine yet; write a short
   `docs/idea-logs/<slug>.md` (what was done, files touched, anything left
   open, and "perf not run — pending manual check on the author's machine");
   commit its own change plus that log file with a clear message; report
   back 3-5 sentences.
4. After each subagent returns, sanity-check it actually left a clean commit
   (`git status`/`git log`) and that `cargo build` still passes if it
   touched `src-tauri/`. If an idea's agent leaves the tree broken or fails
   to commit, reset to the last good commit, record the failure in a fallback
   log entry, and move on — one bad idea must not sink the rest of the run.
5. Once all 12 are processed (no perf-pass — skipped per the same
   instruction), read every `docs/idea-logs/*.md` and synthesize them into
   **one** dated `docs/session-log.md` entry in the wrap-up skill's format
   (title, what happened per idea, mistakes/deviations if any, and a
   `### State` note that perf was intentionally not run in this environment
   and is left for the user to check on their own machine), delete the
   `docs/idea-logs/` files, refresh `engies/project.md` if warranted, commit
   (`Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>`) to the
   `claude/overnight-ideas-2026-07-27` branch — **not main**, per the
   explicit override above.
6. Push the branch and open a PR against `main` (title + summary listing
   each idea, implemented or plan-only, and a note that perf validation is
   still needed from the user).
7. Report a final summary (ideas implemented, ideas plan-only, any
   failures, the PR URL).

## Critical files

- `ideas/*.md` — the 14 source idea files (12 in scope).
- `src-tauri/src/config.rs` (`Keymap` struct/`Default`), `ui/app.js` (global
  keydown dispatcher, settings-panel actions array, `refreshStack`,
  `interceptLinks`), `src-tauri/src/markdown.rs` (heading rendering) — the
  shared hot spots multiple ideas touch, in sequence.
- `.claude/skills/wrap-up/SKILL.md` — the shape the final aggregation/commit
  step should follow.
- `docs/session-log.md`, `docs/idea-logs/<slug>.md` (new, transient),
  `engies/project.md`.

## Verification

- After scheduling: `CronList`/checking the trigger isn't the right call
  here (that's for `CronCreate` jobs) — instead confirm via the
  `create_trigger`/`update_trigger` responses that the trigger id,
  `run_once_at`, `create_new_session_on_fire`, and model all landed as
  expected.
- The real verification happens after the fire: confirm `main` itself is
  untouched (no direct pushes), check `git log --oneline` on
  `claude/overnight-ideas-2026-07-27` for the sequence of per-idea commits
  plus the final aggregation commit, confirm the opened PR lists each idea's
  outcome, confirm `docs/session-log.md` (on that branch) has exactly one
  new dated section — not 12 fragments — and that `docs/idea-logs/` is gone,
  and confirm `cargo build` still passes at the branch HEAD. No perf output
  to check — that's explicitly left for the user to run themselves when
  reviewing. Push notification should also have landed once the run
  completes.
