---
name: wrap-up
description: Closes out a work session on the dreamd repo — reviews and commits the session's changes straight to main, pushes, prepends a dated entry to docs/session-log.md, refreshes engies/project.md if the project story changed, and saves any durable insight to memory. Invoke when the user asks to wrap up, close out, or finish a session, or runs /wrap-up.
---

# Wrap-up

End-of-session ritual for the `dreamd` repo (root = the directory containing this
`.claude/` folder, currently `/Users/oliverfong/toadmountain/dreamd/`). One atomic
commit lands the session's work **and** its log. Do the steps in order.

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
Frontend-only (`ui/`) or docs/skills-only changes need no gate; say so rather than
running a pointless build.

## 3. Write the session log

Prepend a new section to `docs/session-log.md`, directly under the `# Session log`
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

## 4. Refresh `engies/project.md` if the story changed

`engies/project.md` is the human landing page. A scheduled job refreshes it daily,
but if this session changed something a teammate would need to know — new feature,
changed architecture, shipped milestone, new known limitation — update it now
rather than waiting. Invoke the `update-project-doc` skill for the rules; at
minimum, add a dated bullet at the top of its **Recent updates** list and bump the
*Last updated* date. Small internal refactors don't warrant an entry.

## 5. Commit and push to main

dreamd commits **directly to main** — no branches, no PRs. Stage the relevant paths
explicitly (session log and any `engies/` update included, so it's one atomic
wrap-up commit), then commit with a message that explains *why*, not just *what*:

- Subject line: imperative, under 72 chars.
- Body: key decisions, what was verified, notable tradeoffs.
- End with:

```
Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>
```

Always a new commit — never `--amend`. Then `git push origin main`.

If a heredoc commit message gets blocked by the environment's command classifier,
retry with multiple `-m` flags (one per paragraph).

## 6. Memory (lean)

Only save to memory if this session produced something **non-obvious and durable** —
a user preference, a gotcha that would waste time if rediscovered, a project
constraint not visible in the code. Do not duplicate the session log, file lists,
or anything derivable from git history. Check for an existing memory file on the
topic and update it in place rather than creating a near-duplicate.

## 7. Report

Short report: the commit hash, confirmation it pushed to `main`, the session-log
section title you added, whether `engies/project.md` was touched, and any memory
file written. Don't paste the log content back — it's in the file.
