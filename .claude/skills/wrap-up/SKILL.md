---
name: wrap-up
description: Closes out a work session on the dreamd repo — reviews and commits the session's changes straight to main, pushes, prepends a dated entry to notes/session-log.md, refreshes notes/project.md if the project story changed, and saves any durable insight to memory. Invoke when the user asks to wrap up, close out, or finish a session, or runs /wrap-up.
---

# Wrap-up

End-of-session ritual for the `dreamd` repo (root = the directory containing this
`.claude/` folder, currently `/Users/oliverfong/toadmountain/dreamd/`). Do the
steps in order.

**Two repos, two commits.** `dreamd` is public and holds the product; the
session log and the project brief live in the private `dreamd-notes`, cloned at
`notes/` and gitignored on this side. So a wrap-up is no longer one atomic
commit — it is the code to `dreamd` and the log to `notes`, each pushed to its
own `main`. Keep them adjacent in time and cross-reference by content, not by
sha.

If `notes/` does not exist, the session log and the project doc cannot be
written. Do steps 1, 2, 5 and 6, and say plainly in the report that the log was
skipped for a missing `notes/` clone — do not invent a substitute location
inside the public tree.

Proactively suggest `/wrap-up` when a session looks like it's concluding — the
invocation is manual on purpose (no Stop hook, no surprise auto-commits), so the
nudge has to come from you noticing.

## 1. Review state

Run `git status` and `git diff` (plus `git diff --staged` if anything is already
staged) from the repo root. Summarize what changed this thread. Do **not** blindly
`git add -A`. If you see files you don't recognize from this session, build
artifacts (`target/`, `ui/dist`), or anything that could hold a secret, stop and
look at it before staging.

## 2. Gate

If any change touches Rust (`src-tauri/`), run `cargo build` and it must succeed
before you commit. Never commit over a failing build — fix it, or stop and report.
Frontend-only (`ui/`) or notes/skills-only changes need no gate; say so rather than
running a pointless build.

If the session touched `src-tauri/` or `ui/`, also run the `perf-pass` skill. A red
regression is **not** an automatic block — performance is the user's call, not
yours — but it must be stated before the commit lands, and named in the session log
under `### State`. Never update `notes/perf-baseline.json` as part of wrapping up:
that takes a deliberate `perf-deep` run, and it belongs in the commit that
justified it.
Docs- or skills-only sessions skip this; say so rather than spending five minutes
measuring an unchanged binary.

## 3. Write the session log

Prepend a new section to `notes/session-log.md`, directly under the `# Session log`
heading — **newest first**, so the top of the file is always the most recent
session. Get the date with `date +%F`. Match the existing entries' voice: concrete,
past tense, names the actual modules and decisions.

```markdown
## YYYY-MM-DD — <short title>

<One or two lines: what this thread set out to do and whether it got there.>

### What happened

<The work, in numbered or bulleted form. Name files and modules. Record the
*decisions*, not just the edits — a future reader wants to know why.>

### Mistakes & deviations

<Wrong turns, dead ends, plan changes — and how each was caught and corrected.
If the thread ran clean, say so explicitly in one line.>

### State

<Build/test status, what was verified and how, anything left open.>
```

If the session was discussion or planning only with no code changes, still write
the entry — a decision made is worth logging.

## 4. Refresh `notes/project.md` if the story changed

`notes/project.md` is the human landing page. A scheduled job refreshes it daily,
but if this session changed something a teammate would need to know — new feature,
changed architecture, shipped milestone, new known limitation — update it now
rather than waiting. Invoke the `update-project-doc` skill for the rules; at
minimum, add a dated bullet at the top of its **Recent updates** list and bump the
*Last updated* date. Small internal refactors don't warrant an entry.

## 5. Commit and push, both repos

Both repos commit **directly to main** — no branches, no PRs.

**`dreamd` first**, since that is the work the log describes. Stage the relevant
paths explicitly — never `git add -A`, and note that `notes/` is gitignored here
so it cannot be swept in by accident. Commit with a message that explains *why*,
not just *what*:

- Subject line: imperative, under 72 chars.
- Body: key decisions, what was verified, notable tradeoffs.
- End with:

```
Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
```

Always a new commit — never `--amend`. Then `git push origin main`.

**Then `notes`**, from inside `notes/`: stage the session log and any
`project.md` change, commit with the session's title as the subject, and push.
Use `git -C notes` rather than `cd` — the shell's working directory is shared
with the user's.

If a heredoc commit message gets blocked by the environment's command classifier,
retry with multiple `-m` flags (one per paragraph).

## 6. Memory (lean)

Only save to memory if this session produced something **non-obvious and durable** —
a user preference, a gotcha that would waste time if rediscovered, a project
constraint not visible in the code. Do not duplicate the session log, file lists,
or anything derivable from git history. Check for an existing memory file on the
topic and update it in place rather than creating a near-duplicate.

## 7. Report

Short report: both commit hashes and which repo each is in, confirmation each
pushed to its `main`, the session-log section title you added, whether
`notes/project.md` was touched, and any memory file written. Don't paste the log
content back — it's in the file.
