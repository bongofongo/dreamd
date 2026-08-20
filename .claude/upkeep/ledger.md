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
| 4 | `store` | 2026-08-09 | swept — `annotations.rs`'s module doc still said the store dies with the process, and froze five items "for consumers that arrive later" that have all landed; CLAUDE.md's `AppState` named two `Arc`s where there are four; the `(0, 0)` anchoring fallback deduplicated into one `anchor` fn | #18 (merged) |
| 5 | `config-theme` | 2026-08-10 | swept — `parse_hex` sliced `--bg` by byte offset after matching on byte length, so a non-ASCII byte there panicked inside `.setup()` and cost the launch; CLAUDE.md called `ui.titlebar` the only per-platform default when `titlebar_fade` is one too; `config_check`'s header still claimed the crate has no unit tests; four clamp deserializers folded into one | #19 (merged) |
| 6 | `ui-reading` | 2026-08-11 | swept (propose-only) — CLAUDE.md's placement, resize and `prior` claims all hold; the line count and the multi-mark rule's named consumers drifted. Proposed: `wrapRange`'s `stale` param is dead with unreachable CSS behind it, `applyHighlights` flattens twice for one job, and the delete path skips `refreshOutline`/`resetFind` because it nulls `currentFile` before the watcher sees it | #20 (merged) |
| 7 | `agent` | 2026-08-12 | swept — `GRANTS` grew 6→7 in 6c56e74 and four prose sites still said six (`gate`'s module doc twice, a stale test cross-reference in `claude`, `agent_spawn`'s doc, and CLAUDE.md); `gate_server::serve_connection` wrapped a `BufReader` it then read straight past with `get_mut()`; a test counter that was never wired to the gate's asker asserted nothing | #21 (merged) |
| 8 | `mcp` | 2026-08-14 | swept — the 1 MiB `MAX_LINE` cap was implemented twice, once per transport, so the reader moved into `jsonrpc` beside the constant; `mcp::fixed_open_doc` had no caller anywhere; `get_open_document` canonicalised the root twice. Drift: a test named for a three-read split that has asserted four since `get_open_document` landed, three references to shipped plan steps, CLAUDE.md saying `register` runs nothing seven lines above describing it running `claude mcp get`, and two different measurements of the same probe in CLAUDE.md and `main.rs` | #22 (merged) |
| 9 | `index-io` | 2026-08-14 | swept — `notify.rs` said "~33 commands" against 54 and "none of them emit" when `set_root` does, via `adopt_root`'s `repo-changed`; CLAUDE.md's list of other unprompted events predated `agent-event`/`agent-ask`; `flow`, `prompt` and `rootfield` had no architecture bullet at all, and the last two pin tenets the coverage sentence omitted. `MarksChanged::anywhere` was dead; the `rel` rule was written twice and now lives once in `fs_walk::rel_of` | #23 (merged) |
| 10 | `ui-panels` | 2026-08-15 | swept (propose-only) — CLAUDE.md's grid, fade, panel-clamp and `WINDOW_TOGGLES` claims all hold exactly. Drift: `#outline-panel`'s markup comment claims it docks left "rather than fighting" the stack panel when it is `right: 8px` and overlaps on purpose; `index.html` names an `installFindCss` that is `findCssOn`/`findCssOff` and calls it a once-per-session install; the settings panel says three tabs where there are four; the print sheet still calls highlights "session state that dies with the process". Proposed: `mark.hl.stale` is unreachable but is `--stale-text`'s only reader, so deleting it is public surface; `WINDOW_TOGGLES` carries two platform flags where one does; `renderBlockPicker` parses the palette twice | #24 |
| 11 | `os-edges` | 2026-08-16 | swept — CLAUDE.md's tenet 3 quoted the pane's shell as `$SHELL -l -c` when `-i` is the half that finds `claude`; the platform-surface count missed `agent::claude`'s `DEFAULT_SHELL`; tenet 2's "no write outside it" has been false since v1 (`send`'s temp query file, which tenet 3 depends on); `menu.rs` named the wrong submenu as GTK-empty, three lines under its own list saying About is admitted; `main.rs` said four pty commands where `pty_model` made five. `send::tmux_available` spawned a tmux the detector spawned again one line later; the base64 alphabet was written out twice | #25 |
| 12 | `build-release` | 2026-08-17 | swept — `packaging/SIGNING.md` still opened with "Why this is currently off" and a state table reading `NO_SIGN` set, `TAP_GITHUB_TOKEN` missing and `PUBLISH_CASK` unset, all false since 2026-07-26; its own step 8c is the warning it fell to. `ci.yml` kept a second inventory of the `#[cfg(target_os = "macos")]` sites and named two of five, and called `pull_request` unused when this job's every diff arrives through it; `release.yml` said "both arches" for three targets and omitted the `aur` job; `canary.yml`'s "first five are README's Arch line" were in positions 4–8, and it ran four correctness harnesses where `ci.yml` runs five. `set-version.sh` verified its own write with a grep that also passes when the substitution matched nothing | #26 |
| 13 | `ui-agent` | — | skipped 2026-08-18, 2026-08-19 and 2026-08-20 — the same three upkeep PRs are open all three nights (#24, #25, #26), at the backlog cap, and none has been touched since it was opened. No sweep run any night, so **last swept** stays empty and `ui-agent` is still the area due. The rotation is stalled until one of the three is reviewed | — |
| 14 | `perf-harness` | — | — | — |
| 15 | `ui-style` | — | — | — |

Outcome is one word plus a clause: `clean` (nothing to do), `swept` (a PR
opened), `skipped` (backlog too deep, or blocked — say why), `failed` (a gate
went red and could not be resolved).
