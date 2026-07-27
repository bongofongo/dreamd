# dreamd — agentic direction: persistence, MCP, and the shape of the last mile

*Written 2026-07-27. A design analysis, not a decision record — nothing here is
committed to. Supersedes nothing; `docs/plan.md` remains the historical design
intent.*

## The question

dreamd today is a read-only markdown reader whose product is "the reading
experience plus the highlight → annotation → stack → send loop." Tenet 2 says no
session state persists: highlights, annotations, and the stack live in
`Mutex<Store>` and die with the process. `send` assembles markdown, writes a temp
file, and tmux `send-keys` a fixed `read @<file>` prompt into the user's already
running Claude Code session.

Two directions have been floated for where that goes next:

1. **No persistence, an in-app agentic window (Zed-style), no MCP.** The stack is
   sent to an agent pane inside dreamd rather than out to tmux. dreamd becomes
   self-contained.
2. **Full persistence, an MCP server, plus whatever UX follows from it.** dreamd
   stays a reader and becomes a tool provider to whatever agent the user already
   runs. The agent can query what the human marked.

This document works through both, plays out scenarios, identifies the user bases,
and answers whether they should ship as one product or two.

---

## Prerequisite findings

Three things established before the comparison, because the recommendation
depends on them.

**Highlight ids are ephemeral.** `Highlight.id` is a `u64` from a `next_id`
counter that restarts at 1 every process (`src-tauri/src/annotations.rs:45`). Any
external consumer holding id `7` across a dreamd restart is silently pointed at a
different highlight — no error, no mismatch signal. Stable ids (content-derived
or UUID) are a prerequisite for any API that takes an id as a parameter, not a
follow-up to one.

**MCP subsumes tenet 3 rather than straining it.** Tenet 3 exists because sent
queries must not be interpolated into a shell; the answer was a temp file and a
fixed prompt, which is what most of `send.rs`'s 298 lines are for. MCP is
structured JSON over a transport — no shell, no temp file, no clipboard fallback.
It is strictly safer than `send-keys`.

**The tenet under pressure is 1, not 2.** Agentic UX pulls toward "the agent
applies the edit my annotation describes." The split that preserves read-only:
dreamd's agent-facing surface exposes read and annotation-state operations only;
file mutation stays with the agent's own tools, which already exist. dreamd never
gains a write path.

---

## Option 1 splits in two, and the split matters more than the option

"Agentic window like Zed" is ambiguous, because Zed ships both shapes:

- **1a — own LLM client.** dreamd builds a chat pane: API keys, streaming, model
  selection, token accounting, conversation state.
- **1b — embed an external agent.** dreamd hosts Claude Code in a pane (PTY, or an
  ACP-style protocol). The agent stays Claude Code; dreamd gives it a home.

These share almost nothing in cost or risk. What follows treats them separately
wherever they diverge.

### Pros

- **Zero setup.** Download, open, works. No MCP wiring, no tmux, no separate agent
  install. The single biggest advantage, and not a small one.
- **Zero context switch.** Highlight → ask → answer appears beside the passage
  you highlighted. The eye never leaves the text. tmux `send-keys` cannot match
  this.
- **Demoable in ten seconds.** Matters for adoption, for the website, for showing
  someone what the thing is.
- **No trust boundary problem.** Nothing exposes repo content to an agent with
  tools, so the prompt-injection surface never opens.
- **Works outside a repo.** A downloaded spec, a paper, a loose `.md` in
  `~/Downloads`. Option 2's repo-root keying is awkward there; this doesn't care.
- **`send.rs` simplifies.** The tmux/clipboard transport becomes an in-process
  call.
- **(1b only) No LLM client to maintain.** Claude Code brings its own tools, repo
  access, `CLAUDE.md`, and auth.

### Cons

- **(1a) A chat box that cannot read the rest of the repo is a toy for a
  repo-reading tool.** The user highlights a function reference in `plan.md` and
  asks "is this actually implemented?" — 1a cannot grep, cannot open
  `src-tauri/`, cannot run anything. It answers from the passage alone. Fixing
  that means tool-use, a permission model, a guarded file walker: reimplementing
  Claude Code. This is the option's central problem.
- **(1a) dreamd becomes an LLM vendor.** API keys, billing surface, model churn,
  rate limits, streaming edge cases, provider deprecations. A permanent
  maintenance tax on a project whose current strength is "no build step, plain
  JS, ~23 commands."
- **"No persistence" is unstable here.** A chat window that loses the entire
  conversation on quit is far worse than losing highlights. The agentic window is
  precisely the feature that makes no-persistence untenable — option 1 as stated
  argues against itself.
- **It abandons the stated thesis.** `CLAUDE.md` opens with "GUI markdown reader
  for a tmux + Neovim + Claude Code workflow." If dreamd has its own agent
  window, the user has no reason to be in tmux. That is a pivot, and worth
  choosing deliberately rather than sliding into.
- **No inbound channel.** The agent can never see what the human marked.
  Structurally impossible, not merely unbuilt.
- **Crowded market.** Obsidian plus plugins, Zed, Cursor, Notion AI, paste-into-
  ChatGPT. Low switching cost in, low switching cost out.
- **Tenet 1 pressure.** An in-app agent that says "this line is wrong" invites a
  fix button, and the fix button writes to the repo.

---

## Option 2 — full persistence, MCP server, agentic UX addons

### Pros

- **The one thing nothing else does.** "Human marks up a doc in a good reader;
  the agent that wrote it can see the marks" is an empty niche. Option 1 competes
  with five products; this competes with roughly none.
- **Complements rather than replaces.** Claude Code keeps full repo access, its
  own tools, the user's `CLAUDE.md`, the user's auth. dreamd contributes the half
  Claude Code is worst at: a human reading surface.
- **Bidirectional.** Agent annotates → GUI repaints through the existing watcher
  event path. The document becomes a shared workspace rather than a one-way
  outbox.
- **Persistence pays for itself immediately.** `list_highlights` needs to mean "in
  this repo," not "in whichever files happened to be opened during this process."
  Cross-session marks stop being a survival nicety and become the API contract.
- **Reanchoring finally does its real job.** `locate`'s whitespace-stripped tier
  was built for exactly the case where a file changed while dreamd wasn't
  looking.
- **Useful addons get cheap once the store is real:** human-vs-agent highlight
  provenance in distinct colours; a stale queue ("3 marks need re-anchoring");
  annotation search; grouping marks by task; an "agent is reading this file"
  indicator.

### Cons

- **Setup friction is the whole ballgame.** MCP config, an agent already
  installed, a terminal. Each is a drop-off point. Option 1's user is running in
  ten seconds; this one may never get there.
- **It requires the user to already run an agentic loop.** Addressable market =
  Claude Code users who read markdown in a GUI. That is a small number.
- **Prompt injection needs a real answer.** Any tool returning repo markdown makes
  every file in the repo an instruction channel into the agent's context —
  including `.dreamd.toml`, which tenet 4 already classifies as untrusted. Tenet 4
  escapes HTML for the webview and does nothing here. This needs delimiting,
  untrusted-labelling at the tool-result boundary, and a rule that no tool result
  may name another tool. New tenet territory, not a code detail.
- **Persistence machinery, none of which exists today:** schema, versioning,
  migration, corruption recovery; a stale-mark GC policy (a UX call, not a code
  one); `git mv` orphaning path-keyed records; startup reanchor cost landing
  directly on the hyperfine cold-start number in `perf-pass`; `annotations` tests
  losing their purity and needing an example harness like `config_check`.
- **Invisible-value risk.** If the agent does the reading, why is there a GUI? The
  human surface has to stay genuinely good, or the MCP server becomes the only
  part anyone uses.

### Architecture note

stdio MCP spawns one server process per client, owned by that client and dying
with it. Such a process cannot see the GUI's `AppState`, so it would have to
mediate through a file on disk — making persistence the IPC channel between GUI
and agent. **That is the shape to avoid.** It reintroduces multi-writer clobber,
now as one GUI plus N concurrent agent sessions.

In-process MCP — localhost HTTP/SSE served from the Tauri process, hitting the
live `Mutex<Store>` — keeps exactly one writer, serialises through mutexes that
already exist, and gets GUI repaint free via the Tauri event path the watcher
already uses. A thin stdio shim can proxy to it for clients that need stdio.

---

## Scenarios

The same four situations under each option.

**A — Reading a design doc, spotting something suspect, wanting to ask.**
1a: highlight → ask → answer inline. Fast, but scoped to the passage. "Is this
actually implemented?" is unanswerable.
1b: same speed, and the embedded Claude Code can go check.
2: highlight → annotate → stack → the agent in the adjacent pane picks it up.
Full repo access. One eye-movement of context switch if the panes are tiled.

**B — Reviewing output after a long agent run.** *(the thesis scenario)*
1a/1b: you mark confusions and ask — but the agent you are asking is not the
agent that did the work. No shared context. You are asking a stranger about
someone else's code.
2: your marks are visible to the same session that wrote it. "I flagged four
things, address them." This scenario is the entire argument for option 2.

**C — Multi-day read of a large spec.**
1: day two starts blank. Highlights gone, and the conversation with them.
2: resumes, with marks in changed sections flagged Stale.

**D — Agent wants to know what the human cared about before editing.**
1: structurally impossible.
2: `list_highlights(file)` before it touches anything.

Two of four are impossible or bad under option 1. One (A) favours option 1 on
latency and option 2 on capability.

---

## User bases

| | Option 1 | Option 2 |
|---|---|---|
| Who | Reads markdown, wants answers about it. May not use tmux, Neovim, or any agent. | Already runs an agentic coding loop; wants the human-review half to be good. |
| Shape | Broad, shallow, high churn | Narrow, deep, high retention |
| Competitors | Obsidian, Zed, Cursor, Notion AI, paste-into-ChatGPT | Effectively none |
| Switching cost | Low in, low out | High in, high out |
| Is it the author? | No | Yes |

Option 1's audience is larger and likelier to leave. Option 2's is small enough to
name individually and unlikely to find a substitute.

---

## Should both ship, as separate apps?

No.

**What differs:** the last mile — where the stack goes. That is the whole of it.

**What's shared:** markdown render, syntect, the theme family system, watcher,
`fs_walk`, search, anchoring, `guard`, config layering, packaging. Roughly 90% of
the tree.

Two apps means two release pipelines, two notarization runs, two casks, two
websites, two issue trackers, and version sync across `Cargo.toml` +
`website/src/consts.ts` + `Cargo.lock` doubled — for a solo project that commits
straight to main with no branches. That overhead is the likeliest thing to kill
both.

The tenets do not actually diverge either. Both options want persistence; option 1
simply hasn't admitted it yet.

**Reframe: one reader, pluggable last mile.** Two send targets already ship (tmux,
clipboard fallback). MCP is a third. An embedded agent pane is a fourth. They
belong behind configuration, not behind separate binaries. Courting option 1's
audience is then two landing pages and two onboarding paths — cheap — rather than
two products.

---

## Recommendation

**1b and 2 are not alternatives. They compose.**

Drop 1a. Building an LLM client is the expensive, replaceable, unmaintainable
path, and it puts dreamd in competition with the tool it was designed to serve.

Then an embedded Claude Code pane (1b) plus a dreamd MCP server (2) yields an
embedded agent that queries dreamd's own highlights: self-contained for the
zero-setup user, agentic for the tmux user, and no LLM client to maintain. The
in-pane agent and the tmux-pane agent hit the same surface, so it is one
implementation serving both audiences.

Suggested order:

1. **Stable ids.** Blocks everything downstream. Cheap now, painful later.
2. **In-process MCP over live `AppState`.** No storage yet, so no multi-writer
   problem.
3. **The untrusted-content boundary**, before any tool returns repo markdown.
4. **Persistence keyed to repo root**, once `list_highlights` must span sessions.
5. **Embedded agent pane**, reusing (2).

Steps 1–3 are worth doing even if 4 and 5 never happen.

### Open questions

- Does an embedded agent pane violate the spirit of "editing stays in Neovim,"
  even if dreamd itself never writes?
- What is the stale-mark GC policy? Never dropping them means unbounded growth;
  dropping them silently loses work.
- Does persistence key on repo root alone, or on repo root plus git remote, so a
  clone in a second location inherits marks?
- Is a fifth tenet warranted for untrusted content crossing into an agent's
  context, or does tenet 4 stretch to cover it?
