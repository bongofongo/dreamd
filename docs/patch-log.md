# Patch log

Small fixes made between releases — what broke, why, and what now stops it
recurring. Newest first. Larger session narratives live in
`docs/session-log.md`; this file is for the one-change repairs that would
otherwise leave no trace but a commit subject.

## 2026-07-31 — the keys tab's first row stopped being a shortcut

**Symptom.** The `ui` job of `ci.yml` failed on every push from `78f93c4`
onward, including the one that tagged 0.2.1. It was the only red job; rust,
launch and the release, canary and perf workflows were all green, which is why
the draft release exists and is fine.

```
locator.textContent: Timeout 30000ms exceeded.
Call log:
  - waiting for locator('#st-keys .st-row').first().locator('button.combo')
    at perf/harness/ui-check.mjs:210
```

**Cause.** `78f93c4` ("three ways to spell a shortcut") added the key-mode
picker — Ctrl / Cmd / None — to the top of the settings panel's keys tab. It is
a `.st-row` like every binding below it, but it holds a `<select>`, not a
`button.combo`. `ui-check.mjs` reached for the palette binding as *the first
row*, so it started waiting on a button that row will never have. Nothing was
wrong with the panel; the harness was addressing a row by position, and the
position had changed.

Two things made this worse than it looks. A Playwright locator timeout is an
uncaught exception, so the run died at that line and printed no summary — the
25 checks before it and the 300 after it were never reported. And one of those
25 was already failing quietly: `every action gets a row` asserted a literal
`25` against a list that has grown to 30 entries.

**Fix.**

- `ui/app.js` — `renderKeys` now stamps `row.dataset.action = action.id` on each
  binding row. The mode picker above and the quick-highlight checkbox below are
  `.st-row` too and carry no combo, so an action's identity has to be on the
  row rather than implied by where it sits.
- `perf/harness/ui-check.mjs` — selects `.st-row[data-action="palette"]`, and
  counts `.st-row[data-action]` against `KEY_ACTIONS.length` read out of the
  page instead of a hardcoded number. `KEY_ACTIONS` is a top-level `const` in a
  classic script, so it is in the page's global lexical scope and
  `page.evaluate` can read it by name. Adding a shortcut no longer needs a
  matching edit here.

**Verified.** Reproduced locally first (same timeout, same line). After the fix,
`node perf/harness/ui-check.mjs` is 325 passed, 0 failed, and
`node --test ui/paths.test.mjs` is green. The guard was proved to have teeth by
removing the `data-action` line and watching the harness go red again, then
restoring it.

**Known and not fixed here.** A locator timeout still takes the whole harness
down with a stack trace and no summary, which is what turned one stale selector
into an unreadable CI log. Making every locator fail as a `check` rather than as
an exception is a larger change to the harness's shape than this repair wanted
to be.

No Rust changed, and nothing on a measured path — `renderKeys` runs when the
settings panel opens, and no perf scenario opens it.
