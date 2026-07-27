# dreamd: the 1b + 2 agentic architecture

## Context

`docs/agentic-direction.md` worked through two directions for dreamd and the
market evaluation decided it. An in-app LLM client competes with Obsidian, Zed,
Cursor and Notion AI for a broad, low-retention audience, and costs a permanent
maintenance tax on API keys, model churn and streaming. An MCP server competes
with essentially nothing, complements Claude Code instead of replacing it, and
serves the workflow `CLAUDE.md` names in its first sentence.

The two are not alternatives. An embedded pane hosting the *real* Claude Code
(1b), plus a dreamd MCP server (2), means the in-pane agent and the tmux-pane
agent hit the same surface. One implementation, both audiences, no LLM client to
maintain.

This plan implements that.

**The flow it builds is queue-first.** The human reads, highlights, annotates —
and those annotated marks form an ordered queue. The agent's entry point is that
queue: "here is what I asked, in the order I asked it." It works through them
with its own tools and closes each one, live, in a GUI the human never has to
touch. dreamd is not a document server the agent browses; it is the human's
outbox, and now the outbox answers back.

That framing decides the tool surface (`get_stack` is the entry point, not
`list_highlights`), the persistence shape (a queue that survives a restart is the
whole point), and what gets cut (`read_marked_document`, which only serves a
file-first flow).

## Locked decisions

| | |
|---|---|
| **Transport** | Unix domain socket + a `dreamd mcp` stdio shim. Not localhost HTTP. |
| **Store** | JSON, one file per repo under `~/.config/dreamd/`, hashed filename, `0600`, `guard::inside_root` filter on load, size cap, lazy per-file reanchor. No SQLite. |
| **Multi-instance** | The socket bind *is* the lock. Secondary runs in-memory and says so. |
| **Stale-mark GC** | Cap at save (`marks.max_per_repo`, default 2000), dropping oldest stale-and-unannotated first. Never drops an annotated mark. Plus `dreamd marks prune`. |
| **Client target** | Claude Code only. No generic-client conformance work. |
| **Read-only** | Tenet 1 holds. dreamd exposes read + annotation-state operations. File mutation stays with the agent's own `Edit`/`Write`. |
| **Pane** | Last, PTY + xterm.js, and droppable without a scar. |

## Architecture

```
Claude Code (tmux pane, or the embedded pane in step 5)
    │  stdio, newline-delimited JSON-RPC
    ▼
`dreamd mcp`  ← same binary, a cli.rs subcommand
    │  owns initialize + tools/list locally from a compiled-in schema
    │  proxies only tools/call
    ▼
~/.config/dreamd/run/<16hex>.sock          0600
    ▼
dreamd GUI process — server thread spawned in .setup()
    ▼
Arc<Mutex<Store>>  ←  the live store the GUI is using
    │
    └─→ marks-changed event ──→ webview repaints
```

Repo routing solves itself: `cli::run` already computes
`resolve_repo_root(None)` (`cli.rs:60`), and Claude Code's cwd is inside the
repo, so the shim derives the same root — and the same socket path — the GUI
used. No registry, no discovery, no config.

The shim owning `tools/list` is load-bearing: if it proxied that call and dreamd
were down at Claude Code's startup, Claude Code would cache an empty tool list
for the whole session. One `const` schema, used by both halves of one binary,
means zero drift.

## Three corrections to `docs/agentic-direction.md`

1. **The stdio-vs-in-process framing was a false dichotomy.** It rejected stdio
   because a stdio *server* can't see `AppState`. A stdio *shim* proxying to an
   in-process server is neither, and is strictly better for a Claude-Code-only
   target: filesystem auth, no port, no token, no firewall question.
2. **The untrusted boundary cannot come after the MCP server.** `list_highlights`
   returns `quote`, which is verbatim repo markdown, so the injection surface
   opens on the first successful tool call regardless of which tools ship. It
   moves *before* MCP, as step 2 — a pure module with no consumer yet, so MCP is
   built on top of it and cannot forget it.
3. **"Pluggable last mile" should not be built.** Nothing in steps 0–4 needs
   `send.rs` to become a trait or registry, and step 5 needs one `if`. Building
   the abstraction now makes step 5 *harder* to cut, which is the opposite of the
   constraint.

---

# Step 0 — CI hygiene

Land this first so every later step arrives on a CI that actually checks things.
Zero product risk, nothing on the critical path.

**Files**
- `perf/harness/ui-check.mjs:19` — `const UI = "/Users/oliverfong/..."` is a
  hardcoded absolute path to one machine, and it is the sole blocker to running
  the frontend harness anywhere else. Derive from `import.meta.url` exactly as
  `ui/paths.test.mjs` already does.
- `.github/workflows/ci.yml` — add `cargo run --example config_check` and
  `cargo run --example theme_check` to the `rust` job after `Test`. Both run
  against an already-compiled crate; seconds on a warm `Swatinem/rust-cache`.
- `src-tauri/src/send.rs:71-77` — `write_temp` creates
  `dreamd-query-{pid}-{n}.md` in `env::temp_dir()` and nothing ever removes them.
  Sweep files older than 24h at startup. **Not** delete-on-send: `SendResult.temp_path`
  is handed to the user and the tmux prompt references `@<path>`, so the file must
  outlive the call.

**Tests** — `stale_query_files_are_swept_but_todays_are_kept` in `send.rs`, using
a self-created scratch dir under `env::temp_dir()` (permitted by the repo's rule;
it is not `config_dir()`).

**Abort point** — shippable alone, worth having regardless.

---

# Step 1 — Stable ids

The blocker for everything downstream. `Highlight.id` is a `u64` from `next_id`
restarting at 1 every process (`annotations.rs:46,62-63`), so an agent holding an
id across a restart is silently pointed at a different highlight.

**Decision: opaque `String` ids**, minted as `h` + 16 lowercase hex — wall-clock
nanos, pid, a per-process counter and one OS-seeded value, folded with FNV-1a.
Hand-rolled (~12 lines) rather than `uuid`, matching the manifest's dep-frugality;
`uuid` is already in the lockfile transitively if you'd rather take it — the plan
is unchanged either way.

Not content-derived: two identical quotes in one file would collide, and
re-highlighting deleted-then-restored text should mint a *new* identity. Opaque
rather than numeric so an agent can't do arithmetic on ids or infer ordering.

**Lift the anchoring logic out of `main.rs`.** `main.rs:198-214` reads the file
and calls `markdown::locate` at the command layer, defaulting to `(0,0)` — and
`main.rs` is a `[[bin]]` that cannot be imported, so that logic is untestable.
This is exactly the situation `guard.rs`'s header says the repo already fixed
once. Move it to:

```rust
impl Store {
    /// Anchor `quote` in `source` and add it. Unlocatable text still becomes a
    /// highlight, at (0,0) — losing the mark because the DOM's whitespace
    /// collapsing beat `locate` is worse than an imprecise line number.
    pub fn add_anchored(&mut self, file_path: String, source: &str,
                        quote: String, prefix: String, suffix: String) -> Id;
}
```

Step 3's `mark_passage` tool calls the same function, which is why this moves now
rather than duplicating the `(0,0)` fallback in two places.

## This step freezes the whole `Store` shape, not just ids

Every structural change to `annotations.rs` lands here, at once, even where the
consumer arrives two steps later. The reason is parallelism: threads C and G would
otherwise both edit `annotations.rs` during what is meant to be the plan's
parallel window, and `annotations.rs` is the second-worst file in the repo to
merge. Freeze the shape once and four threads run against a frozen struct.

So step 1 also lands:

- `origin: Origin` (`Human` | `Agent`) — consumer is `mark_passage`, in step 3.
- `resolved: Option<Resolution>` — consumer is `resolve_highlight`, in step 3.
- `Deserialize` on `Highlight` and `HighlightState`, with `#[serde(default)]` on
  every field. **Not** on `Pair` — it's a projection, not a stored shape, and
  deriving it invites someone to persist it. Consumer is step 4.
- `from_parts` / `into_parts`, so step 4's loader can build a `Store` without
  making the private fields public.
- `reanchored: HashSet<String>` and `ensure_reanchored(path, source)` — the lazy
  reanchor gate. Consumer is step 4.

Fields with no consumer yet are fine and won't trip clippy; they serialize as
defaults and the frontend ignores what it doesn't read. Getting them in now costs
one extra hour here and saves two merges later.

**Files** — `annotations.rs` (the shape freeze above), `main.rs` (7 command
signatures + `seed_highlights` at :660), `send.rs` (test fixture `pair()` at
:182), `perf/harness/ui-check.mjs` (invoke stub), and **exactly two lines of
`ui/app.js`**: drop the `Number(` at `:1750` and `:2002`. Everything else already
round-trips ids as strings.

**`annotations.rs` is closed for the duration of the parallel window only** —
threads B, C, E and G. It reopens the moment those land, because the freeze
exists to prevent a merge, not because the shape is sacred.

State the stop condition to those threads as **"a capability you need isn't
there,"** not "a field you need isn't there." The distinction is not academic: on
the first run, thread C needed to write `resolved`, found the field present and
public, and so never tripped a field-shaped stop condition — it reached for the
`from_parts`/`into_parts` seam instead. Correct behaviour given the rule, and its
helper was commented and tested, but `from_parts` resets `reanchored`, so every
resolved mark cost one wasted `SourceIndex` rebuild in the primary loop.

The lesson for step 1: **ship the write paths, not just the fields.** A public
field with no method to set it is not a frozen shape, it is an invitation to
work around the freeze.

**Tests** (`annotations.rs`, pure)
- `two_ids_minted_back_to_back_are_different` — pins the counter; nanos alone can
  repeat inside one nanosecond.
- `an_id_is_opaque_and_url_safe` — `^h[0-9a-f]{16}$`, so nothing downstream must
  escape it into a CSS selector or JSON key.
- `an_id_from_a_previous_session_resolves_to_nothing_rather_than_the_wrong_highlight`
  — **the test this whole step exists for.** Under the old counter it fails.
- `an_unlocatable_quote_still_becomes_a_highlight_at_line_zero` — pins the
  fallback that just moved out of `main.rs` and had no test at all.
- Rewrite `ids_are_monotonic_from_one` (:193) as `ids_carry_no_ordering`; the old
  assertion is now false by design and the comment should say why.

**CI** — `locate_check` joins here, because this step changes the only caller of
`markdown::locate` outside the module and 611 corpus fixtures with an independent
oracle are the only thing that would catch the move breaking anchoring:

```yaml
- uses: actions/cache@v4
  with:
    path: perf/corpus/generated
    key: corpus-${{ hashFiles('perf/corpus/manifest.json', 'perf/corpus/gen.mjs') }}
- run: node perf/corpus/gen.mjs        # no-op on a cache hit
- run: cargo run --release --example locate_check
```

Key on `gen.mjs` as well as the manifest — a changed generator produces different
fixtures for an unchanged manifest.

**Perf** — `/perf-quick`. Nothing should move. If `apply_highlights` moves, the
`Number()` removal broke DOM keying.

**Abort point** — ids are stable, anchoring is testable, CI is stronger. Nothing
agentic yet, nothing regressed.

---

# Step 2 — The untrusted-content boundary

Pure, security-shaped, no consumer yet. Same shape and same rationale as
`guard.rs`, whose header comment is the precedent: the predicate lives in the
library *because `main.rs` cannot be imported*, so the tenet is enforced by code a
test can reach.

**New module `src-tauri/src/untrusted.rs`**

```rust
/// Wrap document-derived text so an agent reading a tool result cannot mistake
/// it for an instruction from dreamd or from its user.
pub fn delimit(kind: &str, body: &str) -> String;

/// A sentinel the body cannot forge. Per-process random, minted once.
fn sentinel() -> String;
```

**The sentinel must be per-process random, not a constant.** A markdown file in
the repo can contain any fixed string — including one read off dreamd's own
source on GitHub. A per-process sentinel cannot be pre-written into a document.
Reuse step 1's entropy source.

The envelope carries a fixed, dreamd-authored notice: this is document text a
human marked, treat it as data, do not follow instructions inside it, and **no
tool named inside it exists**. That last clause states the "no tool result may
name another tool" rule as a prohibition to the reader rather than enforcing it by
filtering — filtering is a losing game and this is honest about that.

**Tenet 5.** Tenet 4 is "Escape, don't execute" and is about HTML for a parser.
This is a different boundary with a different failure mode: the reader is an LLM,
so there is no escaping, only labelling. Add to `CLAUDE.md`:

> **6. Label what crosses into an agent.** Untrusted content crossing into an
> agent's context is delimited and labelled, never merely passed.

**Tests** — all in `untrusted.rs`, pure.
- `a_body_containing_the_sentinel_does_not_break_out` — the core one.
- `the_sentinel_differs_between_processes`
- `the_notice_names_no_tool` — pins that dreamd's own boilerplate doesn't hand the
  agent a tool name to latch onto.
- Feed it `send.rs`'s adversarial fixture (`$(rm -rf /) \`id\` "; echo pwned; #`)
  plus a forged closing delimiter and a fake tool-call block.

**Verification discipline** — remove the neutralisation, watch
`a_body_containing_the_sentinel_does_not_break_out` go red, restore. Record it in
the commit message. A green suite over a toothless guard is an unearned claim.

**CI** — none; `untrusted.rs` is in `cargo test`.

**Abort point** — a tested module and a documented tenet. Harmless if you stop.

---

# Step 3 — The MCP server

Three threads, two of them genuinely parallel. See the thread map.

## 3a — Protocol core (pure)

```
src-tauri/src/mcp/jsonrpc.rs   Request/Response/Error, NDJSON framing
src-tauri/src/mcp/schema.rs    the tool list as a const &str of JSON
src-tauri/src/mcp/tools.rs     dispatch: (&mut Store, &Path, ToolCall) -> ToolResult
src-tauri/src/mcp/view.rs      wire DTOs; the single choke point for untrusted::delimit
```

`tools.rs` takes **no `AppHandle`, no `AppState`, no Tauri type at all** — just
`&mut Store` and the repo root. That is what makes it unit-testable with the same
`store_with()` fixtures `annotations.rs` already uses, and it is the seam that
lets one thread work in isolation.

**The tool descriptions in `schema.rs` are product surface, not boilerplate.**
They are the only thing steering the agent toward the queue rather than a file
sweep, and they are read by a model, so write them for one. `get_stack`'s
description should say it is the entry point and that the order is the human's
asking order; `list_highlights`'s should say it is for a file you are already
working in, not for finding work. Getting this wrong doesn't fail a test — it
produces an agent that technically works and behaves wrongly.

`view.rs` must **not** serialize `Highlight` directly. `Highlight` serializes
snake_case and the frontend depends on that; the MCP wire contract should be
camelCase, agent-shaped, repo-relative, and owned by the MCP module so the
internal struct can evolve without breaking a published schema. One
`From<&Highlight> for view::Mark`, and that impl is where `untrusted::delimit`
is applied to `quote`/`prefix`/`suffix`.

### Tools — the queue-first loop

**The primary flow is queue-first, not file-first.** The agent works through the
human's queue of open questions; it does not read marked-up documents looking for
things to do. The stack is what the whole highlight → annotate → send loop exists
to produce, and this is the surface that consumes it.

```
get_stack()                  →  the human's open questions, in order
     │
     ├─ get_highlight(id)    →  fuller context when the quote isn't enough
     │
     ├─ … the agent works, using its OWN tools: Read, Grep, Edit …
     │
     └─ resolve_highlight(id, note)
                             →  closes it. Badge decrements, GUI repaints,
                                evidence survives.
```

| Tool | Args | Returns | Kind |
|---|---|---|---|
| **`get_stack`** | `{}` | ordered `{mark, annotation}[]` | read |
| **`get_highlight`** | `{id}` | one `Mark` + `prefix`/`suffix` | read |
| **`resolve_highlight`** | `{id, note?}` | `{ok, remainingOnStack}` | **write** |
| `list_highlights` | `{file?, state?: active\|stale, resolved?}` | `Mark[]` | read |
| `mark_passage` | `{file, quote, note}` | `{id}` | **write** |

Three read, two write, none touching file bytes.

- **`get_stack` is the entry point.** Ordered by first annotation
  (`annotations.rs:84-86`), which is the order the human asked in. An agent that
  calls nothing else still delivers the thesis scenario.
- **`resolve_highlight` is the loop-closer** and the reason any of this beats a
  one-way outbox. It sets a `resolved: Option<Resolution>` field and calls the
  existing `Store::remove_from_stack` (`annotations.rs:95`). The highlight
  survives — evidence preserved, badge decrements, GUI repaints. This is what
  turns "I flagged four things, address them" → "addressed" into a cycle.
- `list_highlights` is the **secondary** entry, not the primary one: "what's
  marked in the file I'm already editing," and auditing stale marks. Scoping the
  agent's attention is the stack's job.
- `mark_passage` calls step 1's `Store::add_anchored` with `origin: Agent`, so the
  frontend can paint agent marks differently. `file` resolves against the repo
  root and **must** pass `guard::inside_root` before anything happens.

Deliberately absent: anything that **reorders the stack** (there is no reorder API
and `annotations.rs:236` asserts re-annotating doesn't reorder — an agent must not
break that), anything that **deletes a highlight** (destroying the human's marks
isn't the agent's call; `resolve_highlight` covers the legitimate case), and
anything that **writes a file**.

**`read_marked_document` is cut**, not deferred-with-a-question. It returns a
file's source with marks inlined, which saves the agent correlating eight line
numbers into a 400-line file — a real benefit, but only in a file-first flow
("read this doc, tell me about everything I flagged"). Queue-first never needs a
document laid out: `get_stack` already hands over each question with its quote
attached, and the agent reads whatever else it needs with its own `Read`.

For the record, the argument against it is **redundancy, not danger**. An earlier
draft said it "doubles the untrusted surface"; that overstated things, since the
agent can `Read` the same bytes with its own tool regardless, so no content
becomes newly reachable. If the file-first flow turns out to matter after real
use, add it then — it is additive and nothing here forecloses it.

`origin` and `resolved` already exist on `Highlight` — step 1 froze the shape, so
this thread **consumes** them and must not edit `annotations.rs`. If it needs a
field that isn't there, that is a signal step 1 got the shape wrong; stop rather
than editing a file another thread may be in.

## 3b — Frontend push path (`ui/app.js` only)

Today **no Store mutation emits anything**. An MCP handler mutating from the
socket thread would be invisible until the user pressed a key.

**`src-tauri/src/notify.rs`** — one event, `marks-changed`, payload
`{file_path: Option<String>, stack: bool}`.

**Emit only from the MCP layer, never from a Tauri command.** A command's return
value is already the GUI's source of truth for its own mutation — that's how all
33 work today. A second signal would mean two repaint paths racing, and
`refreshStack`'s `stackSeq` guard (`app.js:953`) exists precisely because two
in-flight refreshes is a known hazard. Keeping user-origin mutations silent also
means **`save_to_paint` is untouched by this entire step**, which is what you want
going into a perf run.

Three fixes, all here:

**(a) `applyHighlights` is not idempotent** (`app.js:673`) — it never removes
existing `mark.hl` nodes, assuming a virgin DOM because its only caller runs
immediately after `contentEl.innerHTML = html`. Fix with the `unwrap()` that
already exists at `:840`:

```js
// `normalize()` is not optional: unwrap leaves adjacent text nodes behind, and a
// quote split across two of them is silently skipped by locateInNodes — so
// without this, repeated repaints progressively stop finding their own marks.
function clearHighlights() {
  for (const m of [...contentEl.querySelectorAll("mark.hl")]) unwrap(m);
  contentEl.normalize();
  staleRail.innerHTML = "";
}
```

**(b) A cheap incremental repaint.** The correct external-repaint call today is
`renderCurrent({preserveScroll: true, reanchor: false})` — the full path
`save_to_paint` measures, far too expensive for an agent resolving six marks in a
row. Instead re-place only the overlay, with the mandatory find bracket
`renderCurrent` uses at `:449`/`:490`:

```js
async function repaintHighlights() {
  if (!currentFile) return;
  invalidateFind();      // every stored find Range points into the text nodes
  clearHighlights();     // clearHighlights is about to split
  applyHighlights(await invoke("get_highlights", { path: currentFile }));
  if (findQuery) findRecompute(false);
}
```

**(c) `init()` never calls `refreshStack()`** (`app.js:98-141`) — invisible today
because the store starts empty, a visible bug the moment an agent or a loaded file
populates it. Add it **unawaited**, alongside the `loadTree().catch()` at `:135`,
so the measured `first_paint` at `:140` is unaffected.

Listener coalesces on ~80ms, mirroring `watcher.rs:22`'s 60ms debounce.

## 3c — Socket and wiring

- `store: Mutex<Store>` → `Arc<Mutex<Store>>`. This is the `catalog: Arc<Catalog>`
  precedent (`main.rs:50`), and command bodies don't change — `Arc<Mutex<T>>`
  derefs.
- `mcp::spawn(app.handle().clone(), state.store.clone(), repo_root)` in `.setup()`,
  the `watcher::spawn` shape. Cancel token in a new `AppState.mcp_cancel`,
  mirroring `watcher_cancel`.
- **`adopt_root` (`main.rs:571-621`) must retire and re-bind the socket**, in the
  same block that retires the watcher at `:599-607`. `repo_root` mutating at
  runtime is the single most common way this design breaks, and the fix is eight
  lines copied from code that already exists.
- `cli.rs` — a `Cmd::Mcp` variant.

**Tests**

`mcp/tools.rs`, pure:
- `every_tool_in_the_schema_has_a_dispatch_arm` and its converse — parses the
  schema const and cross-checks the `match`. The drift guard, and the single most
  valuable test in the module.
- `mark_passage_refuses_a_path_outside_the_repo_root` — **security-shaped**;
  break-the-guard verification applies.
- `mark_passage_refuses_a_path_that_escapes_by_dot_dot` — distinct, pins that
  canonicalisation precedes containment.
- `paths_leaving_dreamd_are_repo_relative` — the invariant `send.rs:211` already
  pins for the query assembler. An absolute path leaks `$HOME` into agent context.
- `resolve_highlight_leaves_the_highlight_and_clears_the_stack_entry`
- `an_unknown_tool_name_is_an_error_not_a_panic`
- `get_stack_returns_questions_in_the_order_the_human_asked_them` — the queue-first
  loop's core promise, and it crosses the wire boundary that
  `annotations.rs:236`'s no-reorder test only pins internally.
- `get_stack_skips_a_highlight_that_was_never_annotated` — an unannotated mark is
  not a question and must not reach the agent as one. Mirrors `selected_pairs`'
  existing skip (`annotations.rs:123`).
- `a_resolved_mark_leaves_the_stack_but_stays_in_list_highlights` — pins that
  resolving closes the question without destroying the evidence.

`mcp/jsonrpc.rs`:
- `a_request_with_no_id_is_a_notification_and_gets_no_reply`
- `a_malformed_line_produces_a_parse_error_not_a_disconnect` — keeps a bad byte
  from killing a Claude Code session.
- `a_line_longer_than_the_cap_is_refused` — an unbounded `read_line` on a socket
  is a memory DoS. Cap at 1 MiB.

**New example: `src-tauri/examples/mcp_check.rs`.** Required because the socket
lives under `config_dir()`, which reads the real `~/.config/dreamd` — precisely
what `config.rs:438` and `CLAUDE.md:50` ban from `cargo test`. Sandbox
`XDG_CONFIG_HOME` process-wide as `config_check.rs` does; reuse its `Checks`
struct. ~25 checks: bind, connect, `initialize`, `tools/list` matches the const,
each tool once, an error case, socket mode is 0600, cancel retires the thread and
unlinks.

**`ui-check.mjs`** — add `marks-changed` to the event stub, then assert:
`querySelectorAll("mark.hl").length` is stable across three consecutive events
(the direct regression test for hazard (a)); the badge updates; an event for a
*different* file does not repaint the open one.

**CI** — `cargo run --example mcp_check`.

**Perf** — `/perf-pass`. `save_to_paint` must not move; if it does, the emit
filter leaked into the command path.

**The real acceptance test is manual** and no harness covers it: with the GUI
open, `claude mcp add dreamd -- dreamd mcp`, resolve a mark from the tmux pane,
watch the badge decrement without touching the window.

**Abort point** — **steps 0–3 are a complete product.** Stable ids, a working MCP
server, a live GUI that repaints when the agent acts, a labelled trust boundary.
Marks are still per-session, which is today's contract, not a regression. This
ships `docs/agentic-direction.md`'s scenario B for single-session use.

---

# Step 4 — Persistence

## Layout

```
~/.config/dreamd/marks/<basename>-<16hex>.json    0600   (dir 0700)
~/.config/dreamd/run/<16hex>.sock                 0600   (from step 3)
```

`<16hex>` is **FNV-1a over the canonicalised repo root**. Not
`DefaultHasher` — its output is explicitly undefined across Rust releases, so a
toolchain bump would silently orphan every user's marks. The socket deliberately
skips the basename prefix: macOS caps `sun_path` at ~104 bytes.

## Format

```json
{ "version": 1, "root": "/Users/me/toadmountain/dreamd", "saved_at": 1753617600,
  "highlights": [ { "id": "h…", "file_path": "…", "line_start": 1, "line_end": 3,
                    "quote": "…", "prefix": "…", "suffix": "…", "state": "active",
                    "annotation": null, "origin": "human", "resolved": null } ],
  "stack": ["h…"] }
```

`root` makes a hash collision or a copied file detectable — **load refuses if it
doesn't match the current root** rather than adopting another repo's marks.
`next_id` is gone; step 1 made ids independent of any counter, which is that
step's quiet payoff. **No `deny_unknown_fields`**, and `#[serde(default)]` on
every field: a v0.3 file must not be rejected by a v0.2 binary.

Add `Deserialize` to `Highlight` and `HighlightState`. **Not** to `Pair` — it's a
projection, not a stored shape, and deriving it invites someone to persist it.

## `src-tauri/src/marks_file.rs` — pure

```rust
pub fn path_for(root: &Path) -> PathBuf;
pub fn load(root: &Path) -> Store;              // never fails, never panics
pub fn save(root: &Path, store: &Store) -> io::Result<()>;
pub fn admit(root: &Path, doc: MarksDoc) -> Store;   // every load-time rule
```

`admit` is where the hardening lives, so all of it is testable without a real
directory: `doc.root` must equal the current root; every `file_path` must pass
`guard::inside_root`; `quote`/`prefix`/`suffix` truncated at 8 KiB each; a mark
with an empty quote is dropped (it can never re-anchor); stack entries with no
matching highlight are dropped; the per-repo cap applies.

Size cap checked *before* reading: `metadata().len() > 4 MiB` → warn and return
`Store::default()`. On any read or parse failure: `eprintln!` and fall back to
empty, never panic — the convention `config.rs` establishes throughout.

**Writing** copies `config::write_global` (`config.rs:324-336`) exactly —
`create_dir_all` → serialise → `.tmp` sibling → `rename` — plus mode `0600` set on
the temp file *before* writing, via `OpenOptionsExt::mode`. `write_global` doesn't
do this today; that's arguably a bug there too, but fix it separately rather than
entangling it here.

**Saves are debounced off the command thread** — a dirty flag and a ~500ms
coalescing timer on a background thread holding the `Arc<Mutex<Store>>`.
Serialising on every `set_annotation` would put a file write on a UI-latency path.
Plus an explicit flush on `RunEvent::ExitRequested`, so a quit inside the debounce
window doesn't lose the last annotation.

## Multi-instance: the bind is the lock

1. Try to bind `~/.config/dreamd/run/<hash>.sock`.
2. `AddrInUse` → `connect()`. **Connects** → another live dreamd owns this repo;
   we're a **secondary**. **Refused** → stale socket from a crash; unlink, rebind.
3. Primary owns persistence *and* serves MCP. Secondary keeps marks in memory,
   persists nothing, shows an indicator saying so.
4. Any other bind error → log, run as primary-with-no-socket. Losing MCP beats
   losing persistence.

Note the coupling in the module doc: if the transport ever moves off UDS, the lock
must become an explicit lockfile.

## Lazy per-file reanchor

Never reanchor-everything at startup — that lands straight on the hyperfine
cold-start number. `Store` gains a private `reanchored: HashSet<String>`;
`get_highlights(path)` — already the first thing `renderCurrent` calls after
`innerHTML` (`app.js:476`) — reanchors on first sight of a path, then behaves as
today. Same for the MCP read tools, or an agent gets line numbers from a previous
session.

**`get_stack` is the one that needs care here**, because it is the primary tool
*and* it spans files — so the first call after a cold start reanchors every
distinct file the stack touches, in one burst, on the socket thread. That is the
correct cost (the alternative is handing the agent stale line numbers), but it
means the first `get_stack` of a session is the slowest, proportional to the
number of *files* the queue spans rather than the number of marks. Reanchor once
per file and reuse the `SourceIndex`, as `reanchor_file` (`annotations.rs:139`)
already does. If a queue spanning twenty files ever feels slow, the fix is to
reanchor lazily per returned mark, not to reanchor eagerly at boot.

Cost is paid once per file, on first open, inside a path already doing a
`for_file` scan. Invisible on a repo where you open two documents; enormous on one
where you open two hundred — the right trade, since nobody does the latter.

## Stale-mark GC

`marks.max_per_repo` (default 2000), enforced **at save**, dropping oldest
stale-and-unannotated marks first and **never** an annotated one — stale marks are
exactly the ones a `git checkout` is about to revive. Plus
`dreamd marks prune [--stale] [--older-than 30d]` and `dreamd marks path`, shaped
like the existing `ConfigCmd`, because `cli.rs:1-7` states inspectability as
policy.

## Files

`marks_file.rs` (new) · **`annotations.rs` is untouched** — the derives,
`from_parts`/`into_parts`, and the `reanchored` gate all landed in step 1's shape
freeze · `lib.rs` · `cli.rs` ·
`main.rs` — load after `Config::load` (`:725`) and before `catalog.build`
(`:754`), guarded by `has_repo`; `perf::mark("marks_loaded")`; save thread in
`.setup()`; **`adopt_root` (`:571`) must flush the old root's marks and load the
new root's**, in the same block that swaps config.

## Tests

`marks_file.rs`, pure:
- `a_file_whose_root_disagrees_is_refused_wholesale`
- `a_highlight_outside_the_repo_root_is_dropped_on_load` — **security-shaped**,
  break-the-guard applies
- `a_highlight_that_escapes_by_dot_dot_is_dropped_on_load`
- `an_overlong_quote_is_truncated_rather_than_rejected`
- `a_file_from_a_future_version_loads_what_it_understands` — unknown top-level key
  and unknown entry field; the test that stops step 5 breaking step 4's files
- `a_round_trip_preserves_stack_order`
- `the_path_for_two_roots_differs_and_is_stable` — hardcoded expected hex. **This
  is what catches someone swapping FNV for `DefaultHasher`.**
- `the_cap_drops_stale_unannotated_marks_before_anything_else`

`annotations.rs`: `a_file_is_reanchored_once_per_process_not_once_per_read`.

**New example: `src-tauri/examples/marks_check.rs`** — non-negotiable, everything
here reaches `config_dir()`. Sandbox `XDG_CONFIG_HOME` as `config_check.rs` does.
~30 checks: round trip; file mode 0600; dir mode 0700; a corrupt file loads empty
without panicking; a 5 MiB file trips the cap; a `.tmp` from a simulated crash
never becomes the real file; the lock produces exactly one writer.

`ui-check.mjs` — one check that the badge is non-zero at boot when the stub's
initial state is non-empty. The direct regression test for the `init()` bug.

**CI** — `cargo run --example marks_check`. That's four examples plus the
corpus-cached `locate_check`, all in the existing `rust` job.

**Perf** — `/perf-pass`, then `/perf-deep --update-baseline`. The baseline is
already stale (sha `312ac8b`, 2026-07-25) and reporting phantom rows; this is the
natural moment to reset it. `perf::mark("marks_loaded")` needs no registry work —
`startup.sh` folds the NDJSON and `report.mjs` flattens any dot-path ending in a
number. Expect well under 1ms for a few hundred marks; if not, `admit` is
canonicalising per entry instead of once.

**Abort point** — **this is "a fully working agentic dreamd."** Marks span
sessions, the agent reads and closes them, the GUI repaints live, everything is
persisted safely. The intended terminus if step 5 doesn't earn its keep.

---

# Step 5 — Embedded Claude Code pane (droppable)

## What steps 0–4 avoided, deliberately

No PTY dependency, no vendored JS, no `#main-wrap` layout change, no `send.rs`
refactor, and **zero MCP-side work** — the embedded Claude Code spawns
`dreamd mcp` exactly as the tmux one does and reaches the same socket. That is the
entire payoff of step 3's transport choice, and the reason this step is small.

## Shape

PTY over Tauri IPC. The webview never opens a socket, so the CSP
(`connect-src 'self' ipc: http://ipc.localhost`) is untouched.

`src-tauri/src/pty.rs` — `spawn`/`write`/`resize`/`kill`, plus a reader thread
emitting `pty-data` chunks **base64-encoded**, so a partial UTF-8 sequence at a
chunk boundary isn't corrupted. Four commands in `main.rs`, all under
`core:default` — no plugin, no capability entry, which matters because
`dynamic-acl` is deliberately dropped and `Manager::add_capability` is
unavailable.

`portable-pty` is the one genuinely new dependency tree in this plan.

**Layout** — a `flex: 0 0 <h>` child of `#main-wrap` after `#content-scroll`, the
same shape as `#find-bar`. It just works. Adding a third child to `#workspace` is
the landmine and `index.html:351` says so in as many words.

**xterm.js vendored** to `ui/vendor/` and loaded by `<script src>` — CSP-clean
under `script-src 'self'`. CDN blocked, WASM blocked, inline `<script>` blocked
silently. Vendoring is the only option and it works. First vendored JS in a repo
whose frontend story is "no build step" — note it in `CLAUDE.md`.

**The ~30-line panel checklist**, per the repo's own pattern: the div, a CSS
block, `body.view-mode` hide list (`index.html:343`), print hide list (`:623`),
`togglePane()`, `wireUi()` onclick, `wireKeys()` matchCombo, Escape claim list
(`app.js:2081`), a keymap default (`app.js:67`) **plus its Rust twin in
`config::Keymap`**, and a `KEY_ACTIONS` entry (`app.js:2248`).

**Terminal-specific hazard**: bare-letter keybinds must not fire while the
terminal has focus. Extend the existing `isEditable` (`app.js:2056`) rather than
adding a second guard.

**Spike the signing question first.** `hardenedRuntime: true` with no entitlements
file, and `CLAUDE.md` says "No entitlements, deliberately." I believe
`posix_openpt`/`forkpty` need none and dreamd isn't App-Sandboxed, but **confirm
against a real notarized build before writing any UI** — a signing surprise is the
one thing that makes this step expensive rather than cheap.

**Tests** — `a_chunk_boundary_inside_a_multibyte_character_survives_reassembly`,
`resizing_a_dead_pty_is_an_error_not_a_panic`. `ui-check.mjs`: pane toggles,
Escape claims it before view mode, keystrokes reach `pty_write` rather than the
global keymap. Everything visual is hand-verified — `CLAUDE.md:54` is explicit
that `ui-check.mjs` asserts what the page knows, not what it paints. Don't claim
otherwise in the commit message.

**Perf** — `/perf-pass`. `portable-pty` must not initialise at boot, only on first
pane open. A cold-start regression means the pane is being constructed eagerly.

**Abort point** — cuttable before it merges, cheap to revert after: one dep, one
module, four commands, one div, one CSS block.

## As built (2026-07-27)

Four things the plan got wrong, all found by measuring:

- **The signing spike passed.** `flags=0x10000(runtime)`, entitlements empty, and
  a pty works from the signed `.app` — including from a session with no
  controlling terminal, which is the Finder-launch shape and the one the plan
  did not think to check. The notarized half was skipped for want of local
  `APPLE_ID`/`APPLE_PASSWORD`; the notary ticket cannot change runtime
  capability, only Gatekeeper admission.
- **`resizing_a_dead_pty_is_an_error_not_a_panic` names the wrong outcome.** It
  is not an error: the master fd is still open and the ioctl succeeds. Shipped
  as `resizing_a_pty_whose_child_exited_is_not_a_panic`.
- **The pane was a keyboard trap**, and the plan's guard would not have caught
  it. xterm calls `stopPropagation` on every key it handles — measured with a
  capture-phase probe — so `wireKeys` never sees a keystroke from the terminal
  at all. Extending `isEditable` is therefore *unreachable* for those keys, and
  `toggle_pane` pressed inside the pane did nothing. The fix is xterm's own
  `attachCustomKeyEventHandler`; the `isEditable` clause stays as belt to its
  braces.
- **xterm.js is injected on first open, not declared in `index.html`.** A
  `<script defer>` costs its parse on every launch, including the ones that
  never open a terminal — which is the same argument the plan makes for not
  constructing `portable-pty` at boot, applied to the 289 KB that would have
  landed on first paint.

Also unplanned: input is base64 too (a paste is arbitrary bytes), and the pane
runs a **login** shell, because a `.app` from Finder inherits launchd's minimal
`PATH` and would never find `claude`.

---

# Thread map

| Thread | Scope | Files | Needs | Collides with |
|---|---|---|---|---|
| **J** | Step 0 — CI hygiene | `ci.yml`, `ui-check.mjs`, `send.rs` | — | nothing |
| **A** | Step 1 — stable ids | `annotations.rs`, `main.rs`, `send.rs`, `app.js` (2 lines) | J | **everything** — must be alone, must land first |
| **B** | Step 2 — untrusted boundary | `untrusted.rs` (new), `CLAUDE.md` | — | nothing |
| **C** | Step 3a — protocol core | `mcp/*.rs` (new) | A, B | — |
| **E** | Step 3b — frontend push | `app.js` only | A | none, *if* the payload is fixed up front |
| **D** | Step 3c — socket + wiring | `main.rs`, `cli.rs`, `notify.rs`, `examples/mcp_check.rs` | C, E | C on `mcp/mod.rs` |
| **G** | Step 4a — file format | `marks_file.rs` (new) | A | A on `annotations.rs` |
| **H** | Step 4b — persistence wiring | `main.rs`, `cli.rs`, `examples/marks_check.rs` | G, D | D on `main.rs` |
| **I** | Step 5 — pane | `pty.rs`, `main.rs`, `index.html`, `app.js`, `ui/vendor/` | H | H on `main.rs`, E on `app.js` |

**Order:** `J → A → (B ∥ C ∥ E) → D → G → H → I`

**The parallel window is B ∥ C ∥ E.** B is a pure new module touching nothing. C
is pure Rust in a new directory. E is pure JS in `app.js`. Zero file overlap
between any pair. The only shared artefact is the `marks-changed` payload shape —
write it into both C's and E's briefs verbatim and they never need to talk. Note
C depends on B's `delimit` signature, so hand C that signature in its brief rather
than making it wait.

**`main.rs` is the contention point** — threads A, D, H and I all touch it. Never
run two concurrently. Commits go straight to main with no branch discipline, so a
conflict in `AppState` or the `generate_handler!` list is a nasty merge for no
gain.

**`annotations.rs` is the second** — A adds ids, G adds derives. Sequential, A
first, because G's file format bakes in A's id type.

**The two cleanest hand-offs are C and G.** Either can be briefed with almost no
context: "here is `annotations.rs`, here is the tool list, here are the test
names, produce a pure module and its tests." Neither needs to know about Tauri,
the frontend, or the socket. D, H and I are integration threads and want the full
picture.

---

# Hazard register

| Hazard | Handled | How |
|---|---|---|
| No Store→GUI push path | 3b | `marks-changed`, emitted **only** from the MCP layer, so `save_to_paint` is untouched |
| `applyHighlights` non-idempotent (`app.js:673`) | 3b | `clearHighlights()` on the existing `unwrap()` + mandatory `normalize()` |
| `init()` never calls `refreshStack()` | 3b | Added unawaited, so `first_paint` is unaffected |
| `repo_root` mutates via `adopt_root` (`main.rs:571`) | 3c, 4 | Socket retire+rebind and marks flush+reload, both in the block that already retires the watcher |
| `Highlight` lacks `Deserialize` | 4 | Derived on `Highlight`/`HighlightState`, deliberately **not** `Pair` |
| Anchor logic in un-importable `main.rs` | 1 | Lifted to `Store::add_anchored`; `mark_passage` reuses it |
| `send.rs` temp-file leak | 0 | Age-based sweep, not delete-on-send |
| Multi-instance clobber | 4 | The bind is the lock; connect-probe distinguishes live from stale |
| Persistence on cold start | 4 | Small read + parse; **lazy** per-file reanchor; `marks_loaded` mark auto-appears in the perf table |
| Agent repaints hitting `save_to_paint` | 3b | `repaintHighlights()` skips render/`innerHTML`/`decorateCodeBlocks`; ~80ms coalesce |
| Find ranges invalidated by repaint | 3b | `invalidateFind()`/`findRecompute(false)` bracket, same pairing `renderCurrent` uses |
| Hash instability orphaning marks | 4 | FNV-1a, never `DefaultHasher`; pinned by a hardcoded-hex test |
| Injection surface | 2 | Boundary lands **before** any tool ships |

---

# Orchestrating this

**One step per session.** The context that makes step N good is noise for
step N+1. Don't let a session span two.

**Brief each thread with its own section of this plan, not the whole plan.**
`CLAUDE.md` loads automatically and carries the tenets and the testing rules. C
and G additionally need `annotations.rs` read into context; E needs the `app.js`
sections named in 3b; D, H and I want the architecture diagram and the hazard
register.

**Fix the perf baseline before step 3.** It's stale (sha `312ac8b`) and reports
phantom rows, so any `pass` run during this work is unreadable. One `/perf-deep
--update-baseline` on current main, committed on its own, and every later
comparison means something.

**Run an independent verification pass on steps 2, 3 and 4** in a *fresh* thread —
these are the security-shaped ones, and a context that didn't write the code is
meaningfully better at breaking it. The brief is short: "break the guard, watch
the named test go red, restore, report." A green suite over a toothless guard is
an unearned claim, and the author is the worst person to check.

**Per-session close-out:** `/wrap-up` commits, pushes, and prepends the session
log. Run `/update-project-doc` after step 3 and again after step 4 — both
materially change the project story.

**The five pre-commit commands** (`CLAUDE.md`: CI is the backstop, not the first
check): `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features
-- -D warnings`, `cargo test --all-features`, `cargo build`, `node --test
ui/paths.test.mjs`.

**Perf cadence:** `/perf-quick` after steps 0, 1, 2. `/perf-pass` before
committing 3, 4 and 5 — those touch the repaint path, startup, and boot
respectively.

**What Claude can drive unattended:** J, B, C and G are near-fully autonomous —
self-contained briefs, pure modules, named tests, no product judgement required.
A is mechanical but touches everything, so watch it land. D, H and I want you
reviewing, because they're the integration points where a wrong call is expensive.
The verification passes are ideal subagent work.

---

# Verification

**End to end, after step 3 — this is the queue-first loop, and it is the
acceptance test for the whole architecture:**
```sh
cargo tauri dev                              # GUI, this repo
claude mcp add dreamd -- dreamd mcp          # in a tmux pane, cwd inside the repo

# In the GUI: highlight three passages across two files, annotate each.
#   Badge reads 3.
#
# In Claude Code, one prompt and nothing else:
#   "Work through my dreamd queue."
#
# Expect, without touching the GUI:
#   get_stack           → three questions, in the order you asked them
#   (its own Read/Grep/Edit as needed)
#   resolve_highlight   ×3
#   badge 3 → 2 → 1 → 0, live, no keypress
#   marks still visible in the document, evidence intact
```
The failure that matters here is the agent reaching for `list_highlights` and a
file sweep instead of `get_stack`. If it does, the tool descriptions in
`mcp/schema.rs` are not making the queue the obvious entry point — fix the
descriptions, not the agent.

**After step 4:** quit dreamd, reopen, confirm the marks and the stack are still
there and the badge is right at boot. Then open a second dreamd on the same repo
and confirm it announces itself as a secondary rather than clobbering the file.

**Automated, at every step:**
```sh
cargo test --all-features
cargo run --example config_check
cargo run --example theme_check
cargo run --example mcp_check         # step 3+
cargo run --example marks_check       # step 4+
cargo run --release --example locate_check
node --test ui/paths.test.mjs
node perf/harness/ui-check.mjs        # local only; needs `cd perf/harness && npm run setup`
```

# Still open

- **Keying on repo root alone vs root + git remote**, so a second clone inherits
  marks. Root alone for now; the file already stores `root`, so adding `remote`
  later is purely additive and `admit` grows one arm.
- **Whether a file-first flow ever earns a tool.** `read_marked_document` is cut
  on the grounds that queue-first is the primary loop. If real use turns up a
  case — reviewing one large doc with a dozen scattered marks — it is additive and
  nothing here forecloses it.
- **Whether agent-origin marks should paint differently.** `origin` is recorded
  from step 3; the CSS is a one-liner in `wrapRange` (`app.js:829`) whenever you
  want it.
