---
name: upkeep
description: Sweeps one area of the dreamd repo — verifying CLAUDE.md's claims about it against the code, simplifying the code, then bringing that section of CLAUDE.md back into line — and opens one PR. Rotation state lives in .claude/upkeep/ledger.md. Invoke when the nightly upkeep job runs this, when the user asks to sweep, debloat, or simplify an area, or runs /upkeep.
---

# Nightly upkeep sweep

One area per night, on rotation. Nothing else in the toolchain pushes back on
growth: CI proves correctness, the perf tiers prove speed, `wrap-up` and
`update-project-doc` keep the *narrative* current — but nothing asks "is this
still the simplest shape, and does CLAUDE.md still describe what the code
actually does?"

Args: `/upkeep` picks the stalest area. `/upkeep <area>` forces one.
`/upkeep --propose` runs any area in propose-only mode.

**An empty night is a success.** If the area is clean, update the ledger, report
"nothing to do", and land no code change. This is the most important rule here —
a nightly job that must produce a diff will churn code to justify itself. Never
manufacture work.

## 1. Pick the area

Read `.claude/upkeep/ledger.md`. Take the row with the oldest *last swept*;
while that column is empty, use the `#` column for the first cycle's order. Break
ties by churn — `git log --oneline --since=<last swept> -- <paths>` — and sweep
where the code actually moved.

Before starting, check the backlog: if **three or more `claude/upkeep-*` PRs are
already open**, skip the sweep entirely. Note the backlog in the ledger row,
report it, and stop. Review debt is not worth adding to.

## 2. The areas

Stable config — this table, not the ledger. Every area excludes `ui/vendor/`
(vendored publisher bundles, upgraded by procedure, never edited here) and
`target/`. `website/` is deliberately out of scope: a standalone Astro project
with its own `CLAUDE.md`, deployed separately.

| # | Area | Paths | Extra gate | Effort |
|---|---|---|---|---|
| 1 | `core-text` | `markdown.rs`, `guard.rs`, `untrusted.rs` | `locate_check` | high |
| 2 | `store` | `annotations.rs`, `marks_file.rs` | `marks_check` | high |
| 3 | `config-theme` | `config.rs`, `theme.rs` | `config_check`, `theme_check` | high |
| 4 | `mcp` | `src-tauri/src/mcp/**` | `mcp_check` | high |
| 5 | `agent` | `src-tauri/src/agent/**` | `agent_check` | high |
| 6 | `shell` | `main.rs`, `lib.rs`, `cli.rs` | smoke | high |
| 7 | `os-edges` | `pty.rs`, `send.rs`, `menu.rs`, `webkit.rs` | smoke | high |
| 8 | `index-io` | `fs_walk.rs`, `search.rs`, `watcher.rs`, `notify.rs`, `catalog.rs`, `flow.rs`, `rootfield.rs`, `prompt.rs`, `perf.rs` | — | high |
| 9 | `ui-reading` | `ui/app.js`: render, highlights, placement, anchoring, scroll | **propose-only** | medium |
| 10 | `ui-panels` | `ui/app.js`: tree, stack, settings, keys, palette; `ui/index.html` | **propose-only** | medium |
| 11 | `ui-agent` | `ui/app.js`: pane, pop-out, pty, MCP strip, gate cards | **propose-only** | medium |
| 12 | `ui-style` | `ui/theme.css`, `ui/themes/*.css`, `ui/paths.js` | `paths.test.mjs`, `ui-check.mjs` | medium |
| 13 | `build-release` | `packaging/`, `.github/workflows/` | none run — read and reason | medium |
| 14 | `perf-harness` | `perf/` (never `baseline.json` or `results/`), `src-tauri/benches/` | `cargo bench --no-run` | medium |
| 15 | `repo-docs` | `README.md`, `docs/` (minus two), `ideas/`, `bugs/` | — | medium |

Rust paths are under `src-tauri/src/`.

**`ui/app.js` is propose-only.** It is 5,700 lines and the GUI is verified only
by hand — `ui-check.mjs` asserts what the page knows, not what it paints. Areas
9–11 produce a written proposal at `docs/upkeep/<date>-<area>.md` (findings, a
diff sketch, what would need checking by eye) and change **no** code. That is the
user's call, and it is the rule most likely to erode in practice: if a change
there looks obviously safe, it still goes in the proposal.

## 3. The three passes

In order. The first is the most valuable.

**Verify.** Read the CLAUDE.md paragraphs covering this area and check every
factual claim against the code. Claims worth checking are the countable and the
pinned ones — "~48 `#[tauri::command]` handlers", "a test pins it at exactly
six", the four panel size ranges, "`ALIASES` maps pre-family names", "the first
`.png` is baked into the binary". A false claim in CLAUDE.md misleads every
future session that reads it, so drift outranks any refactor you might find.

**Simplify.** Dead code, unreachable branches, duplicated constants, redundant
tests, over-abstraction, functions that have outgrown their shape. Behaviour-
preserving only.

**Document.** Fix the drift found in pass 1. Tighten that section's prose —
redundant restatement only. **Never delete a reason.** The "because", "so that"
and "otherwise" clauses are the earned knowledge and the whole justification for
the file's size; a sentence explaining *why* something looks wrong but isn't is
the single most valuable kind of line in it. Cutting one costs a future session
the rediscovery.

## 4. Invariants

A sweep may never:

- **Change public surface.** CLI flags and subcommands, config keys,
  `#[tauri::command]` names or signatures, MCP tool names or schemas, event
  names, theme CSS custom properties, keymap action ids.
- **Weaken a tenet.** `guard`, `untrusted`, the HTML escaping in `markdown`, the
  no-shell-interpolation rule, and "nothing written outside `~/.config/dreamd/`"
  are law, not code to be simplified. A sweep that finds one awkward writes a
  proposal and changes nothing.
- **Weaken a test to make a gate pass.** A red gate means revert the hunk. If the
  test is genuinely wrong, that is a finding for the PR body, not an edit.
- **Run a perf tier or touch `perf/baseline.json`.** The harness is unvalidated
  off the author's Mac and `run.sh` refuses comparison off Darwin anyway.
- **Touch** `docs/session-log.md`, `docs/plan.md`, `engies/project.md` (its own
  job), `ui/vendor/`, or any version string.
- **Edit `ui/app.js`** — areas 9–11 are propose-only.
- **Exceed 400 changed lines.** Over cap: land the best slice, propose the rest.
  Net line count should be negative or neutral unless fixing a real defect.

## 5. Gates

Green before any commit. These mirror `.github/workflows/ci.yml`; run the whole
Rust block for any area touching `src-tauri/`, plus that area's own harness.

```sh
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
cargo build
cargo run --example <area harness>          # config_check | theme_check | mcp_check | marks_check | agent_check
node perf/corpus/gen.mjs                    # core-text only, before locate_check
cargo run --release --example locate_check  # core-text only
```

For `shell`, `os-edges`, and anything else that could break launch — the
"compiled, tested green, aborted inside GTK init" failure:

```sh
cargo build --features perf
SMOKE_EXPECT=paint SMOKE_TIMEOUT=120 packaging/smoke.sh ./target/debug/dreamd
```

`smoke.sh` re-execs itself under `xvfb-run`; do not wrap it.

For `ui-style`:

```sh
node --test ui/paths.test.mjs
cd perf/harness && npm ci && npx playwright install --with-deps chromium && node ui-check.mjs
```

Chromium is pre-installed in this environment (`PLAYWRIGHT_BROWSERS_PATH=/opt/pw-browsers`).

**Measured-path handoff.** If the diff touches `markdown::render*`, `locate*`,
`search`, `get_highlights`, `fs_walk::build_tree`, or an app.js render/scroll
path, say so in the PR body and flag that `/perf-quick` is needed on the author's
machine before merge. The nightly never measures; it hands off.

## 6. Land it

Two targets, and the split is deliberate.

**The work** goes to a branch off the freshly fetched remote tip:

```sh
git fetch origin
git checkout -B claude/upkeep-$(date +%F) origin/main
```

Commit with a message that says what was simplified and why, ending with the
`Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>` trailer, then push and
open one PR against `main`. The body names the area, the drift found (even when
the fix was one word), what was simplified, the net line delta, and any measured
path touched.

**The ledger** goes straight to `main`, in its own commit, whether or not the
sweep produced code:

```sh
git checkout main && git pull --rebase origin main
# edit .claude/upkeep/ledger.md
git commit .claude/upkeep/ledger.md -m "chore(upkeep): <area> swept $(date +%F)"
git push origin main
```

The ledger is a *scheduling* record, not a work record. If a PR sits unmerged for
a week the rotation still has to advance — otherwise the same area is re-swept
every night and duplicate PRs pile up behind the first one.

**Never push code to `main`.** CLAUDE.md says dreamd commits straight to main
with no branches; that is for supervised work. This job runs with nobody
watching, so the code half goes to a reviewable PR. The ledger commit is the only
thing this job puts on `main` directly.

## 7. How to work

Four things about this task specifically:

- **Stay in the area.** Sweep what the ledger gave you and stop. Noticing that an
  adjacent module also needs work is a line in the PR body, not a second diff.
- **Keep the output short.** The PR body and the CLAUDE.md section are both
  things this job exists to keep small. Write them that way.
- **One agent, no delegation.** A one-area sweep has nothing worth fanning out,
  and a subagent would re-establish the context you already hold.
- **Do not add a verification pass.** Run the gates and read the diff; that is
  the verification. A separate re-check step produces motion, not confidence.

## 8. Report

Short: the area swept, the drift found, what was simplified, net line delta, gate
results, the PR URL (or "nothing to do"), and the ledger commit. Do not paste the
diff back — it is in the PR.
