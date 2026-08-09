# Upkeep ledger

Rotation state for the nightly sweep. The area *definitions* — paths, gates,
effort — live in `.claude/skills/upkeep/SKILL.md`; this file holds only which
area is due next and what the last pass found.

The sweep takes the row with the oldest **last swept**, breaking ties by churn
since that date. While **last swept** is empty the `#` column is the order, so
the first cycle runs highest-value-first rather than arbitrarily.

Written by the sweep itself, straight to `main`, whether or not that night
produced code — a PR left unmerged must not stall the rotation. It lives here,
in the public tree, because the job runs in a cloud checkout that has only this
repo: the private `notes/` clone is a local convenience and is not there. This
one-line scheduling record is the **only** thing the nightly job may commit to
`main`; the code half always goes to a PR.

| # | Area | Last swept | Outcome | Open PR |
|---|---|---|---|---|
| 1 | `repo-docs` | 2026-08-05 | swept — README named a keybind that does something else, the wrong `agent.position` default, and the tmux path as the send path | #15 (merged) |
| 2 | `shell` | 2026-08-07 | swept — `pin_native_theme`'s doc paragraph was sitting on `native_pin`; CLAUDE.md's `AppState` named a `Mutex<SearchIndex>` field that does not exist; three duplicated messages/branches folded | #16 (merged) |
| 3 | `core-text` | 2026-08-08 | swept — CLAUDE.md called `locate`'s tiers a three-step chain when tiers 1 and 2 are alternatives gated on context; `untrusted` was missing from both the test-coverage sentence and the module list; one unreachable char-boundary loop dropped from `Stripped::offset_of` | #17 (merged) |
| 4 | `store` | 2026-08-09 | swept — `annotations.rs`'s module doc still said the store dies with the process, and froze five items "for consumers that arrive later" that have all landed; CLAUDE.md's `AppState` named two `Arc`s where there are four; the `(0, 0)` anchoring fallback deduplicated into one `anchor` fn | #18 |
| 5 | `config-theme` | — | — | — |
| 6 | `ui-reading` | — | — | — |
| 7 | `agent` | — | — | — |
| 8 | `mcp` | — | — | — |
| 9 | `index-io` | — | — | — |
| 10 | `ui-panels` | — | — | — |
| 11 | `os-edges` | — | — | — |
| 12 | `build-release` | — | — | — |
| 13 | `ui-agent` | — | — | — |
| 14 | `perf-harness` | — | — | — |
| 15 | `ui-style` | — | — | — |

Outcome is one word plus a clause: `clean` (nothing to do), `swept` (a PR
opened), `skipped` (backlog too deep, or blocked — say why), `failed` (a gate
went red and could not be resolved).
