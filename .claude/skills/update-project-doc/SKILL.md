---
name: update-project-doc
description: Refreshes notes/project.md — the plain-language, entry-level project brief for the human behind dreamd — from the current state of the repo, then commits it to main. Invoke when the user asks to update the project doc / project.md / the team page, when a scheduled job runs this, or when a session materially changed the project story.
---

# Update `notes/project.md`

`notes/project.md` is the daily landing page for the developer on this project.
Someone should be able to be away for a week, read it in five minutes, and know
exactly where dreamd stands. It is written for an **entry-level engineer** — plain
English, jargon explained on first use, no assumed familiarity with Rust, Tauri, or
the codebase.

A scheduled job runs this daily; the user can also invoke it on demand. Both paths
run the same steps.

**The doc is in a different repo from the code it describes.** `notes/` is a clone
of the private `dreamd-notes`, gitignored inside the public `dreamd` tree, so the
reading in step 1 spans both and the commit in step 3 lands in `notes` alone. If
`notes/` does not exist, there is nothing to update: say so and stop — do not
write a copy into the public tree.

## 1. Read the current state

Do this before writing a word — the doc must describe what's *actually* true today,
not what was true when it was last written:

- `git log --since="<date at the top of project.md>" --stat` (or the last ~20
  commits) — what has actually changed since the last refresh.
- `notes/session-log.md` — the top few entries, for the narrative and the decisions
  behind the changes.
- `README.md` — the user-facing feature list and known limits.
- The current `notes/project.md` — what it already says, and its *Last updated*
  date.
- `notes/plan.md` for original intent, and the source tree (`src-tauri/src/`, `ui/`)
  if the architecture section looks stale.

If nothing meaningful changed since the last update, **do not manufacture news**.
Bump the *Last updated* date only if some genuine detail was corrected; otherwise
leave the file untouched, skip the commit, and say so.

## 2. Rewrite the doc

Keep the existing structure — it's the contract:

1. **Header italic line** — the "daily landing spot" framing plus
   `Last updated: YYYY-MM-DD` (get the date with `date +%F`).
2. **What we're building** — the product in plain terms: what it does, who it's
   for, the highlight → annotation → stack → send loop, and the deliberate
   constraints (read-only, nothing persisted).
3. **How it's built** — stack, why Tauri rather than Electron, the backend modules
   and what each one does, and the couple of ideas that come up constantly
   (re-anchoring / stale highlights, CSS theming).
4. **Where things stand right now** — the honest status: what works, what's
   verified and how, what's deliberately not done yet.
5. **Glossary** — terms an entry-level reader might not have. Add entries as new
   concepts enter the project; drop ones no longer relevant.
6. **Recent updates** — reverse-chronological dated bullets, newest first. Add
   today's bullet at the top. Keep roughly the last 8–10 entries; collapse older
   ones into a single `**(earlier)**` line rather than growing forever.

Style rules:

- Explain, don't list. Prose over bullet dumps where a sentence reads better.
- Every piece of jargon gets defined the first time it appears, or lands in the
  glossary.
- Say what's *not* done and what's broken. A status page that only reports wins is
  useless.
- Target 2–3 pages. If it's growing past that, cut history, not explanation.
- No hedging or hype — this is a status page, not a pitch.

## 3. Commit to main — in `notes`, not in `dreamd`

The doc lives in the private `dreamd-notes` repo, so the commit goes there. Like
dreamd it takes commits **straight to main**. Use `git -C notes` rather than `cd`;
the shell's working directory is shared with the user's, and a stray `cd` breaks
whatever they run next.

```sh
git -C notes commit project.md \
  -m "docs: refresh project.md" -m "<one line on what changed>" \
  -m "Co-Authored-By: Claude Opus 5 <noreply@anthropic.com>"
git -C notes push origin main
```

Naming the path on `git commit` rather than staging first is deliberate: it
commits that file alone, so unrelated dirty or staged files in `notes` — a
session log in flight, say — are left exactly as they are.

If the push is rejected because main moved, `git -C notes pull --rebase origin main`
and push again.

## 4. Report

One short paragraph: what changed in the doc, the commit hash, and confirmation of
the push — or "no meaningful change since <date>, left untouched."
