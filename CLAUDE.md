# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

# dreamd

GUI markdown **reader** for a tmux + Neovim + Claude Code workflow. Rust + Tauri 2 +
plain HTML/CSS/JS. The product is the reading experience plus the
highlight → annotation → stack → send loop.

## Commands

```sh
cargo tauri dev                      # run (needs `cargo install tauri-cli --version "^2"`)
cargo tauri dev -- -- /path/to/repo  # run against another repo
cargo build                          # must pass before any commit touching src-tauri/
cargo build --features perf          # timing-instrumented binary (NDJSON marks on stderr)
cargo build --profile profiling --features perf   # release + symbols, for Instruments/samply
cargo tauri build                    # release binary

cargo bench --bench render           # one bench target: render | locate | search | walk
cargo bench --bench locate -- locate_single/today  # a single case (filter is a regex)

./perf/run.sh quick|pass|deep [--verbose] [--no-window]
./perf/run.sh deep --update-baseline # the ONLY way perf/baseline.json changes
node perf/corpus/gen.mjs             # rebuild fixtures (run.sh does this itself)
cd perf/harness && npm run setup     # Playwright + Chromium, test-only, never ships
```

There are no `#[cfg(test)]` unit tests — `cargo test` compiles and reports nothing.
Correctness is checked by running the app and by the benches; don't claim test coverage.
`perf/run.sh` takes an exclusive lock: one tier at a time, no `cargo` alongside it.

## Tenets

1. **Read-only.** dreamd never writes to the user's markdown. Editing stays in Neovim.
2. **Nothing persists.** Highlights, annotations, and the stack are in-memory and die
   with the process. Don't add a database without an explicit decision to.
3. **No shell interpolation of user content.** Sent queries go through a temp file and
   a fixed `read @<file>` prompt. Highlighted text never enters a command line.
4. **Escape, don't execute.** Raw HTML in markdown is escaped. External links are
   restricted to `http`/`https`/`mailto`; relative images must resolve inside the repo
   root.
5. **Themeable to the CSS level.** `ui/theme.css` is a user-facing surface, hot-reloaded.

## Architecture

`src-tauri/src/main.rs` is a thin shell — CLI (clap), `AppState`, the ~23 `#[tauri::command]`
handlers, the builder. All logic lives in the `dreamd` **library** crate (`src/lib.rs` +
modules) *because a `[[bin]]` target cannot be imported*: the split is what makes
`src-tauri/benches/` possible. New logic goes in a module, not in `main.rs`.

State is one `AppState`: `repo_root`, `Mutex<Store>` (highlights + stack), `Mutex<SearchIndex>`.
It spans every file opened in the session and dies with the process.

Data flow: `ui/app.js` (plain JS, no build step; `tauri.conf.json` points `frontendDist` at
`../ui`) calls commands over IPC and listens for watcher events.

- `fs_walk` — `ignore` crate (ripgrep's walker) → nested `FileNode` tree, markdown only.
- `search` — `nucleo` fuzzy index over **paths only**; content search is v2.
- `markdown` — pulldown-cmark → HTML, syntect for fenced code. Raw source HTML is
  re-emitted as `Event::Text` (escaped); only syntect's own markup is trusted.
- `annotations::Store` — `Highlight { quote, prefix, suffix, line_start/end, state }` plus an
  ordered `stack` of ids. `set_annotation` is what enqueues a pair.
- `watcher` — `notify` thread emitting `file-changed` / `file-added` / `file-removed` /
  `theme-reloaded`; the frontend responds by re-rendering and calling `reanchor`.
- `send` — assembles markdown, writes a temp file, then tmux `send-keys` a fixed
  `read @<file>` prompt (falling back to clipboard). See tenet 3.

**Highlight anchoring is the subtle part.** A quote is located in the *source* by
`markdown::locate` in three fallbacks: exact `prefix+quote+suffix`, exact quote, then
whitespace-stripped match. The frontend sends what `getSelection().toString()` returns —
**rendered DOM text**, never raw source — so the whitespace-normalized path is the hot one
and the only realistic thing to benchmark. `reanchor_file` re-runs this on save; failure
marks the highlight `Stale` rather than dropping it.

**Perf instrumentation** is behind the off-by-default `perf` cargo feature. Marks go to
**stderr** as NDJSON because `console.log` in WKWebView never reaches process stdout;
frontend marks route through the `perf_mark` command, `d:`-prefixed phases are durations.
`--bench-startup` runs the pre-window sequence and exits; `DREAMD_PERF_SEED` preloads
highlights from a corpus fixture.

## Working practices

- Commits go **straight to main** — no branches, no PRs.
- `cargo build` must pass before any commit touching `src-tauri/`.
- Repeatable flows become skills in `.claude/skills/`.
- Performance is measured, not guessed. `/perf-quick` (~60s) after an edit,
  `/perf-pass` (~5min) before a commit touching `src-tauri/` or `ui/`, `/perf-deep`
  (~20min) to profile or move the baseline. `perf/baseline.json` changes only via
  `perf-deep`, in the same commit as the change that justified it.
- Numbers from `perf/harness/` are Chromium, **not** WKWebView — relative regression
  signal only. Say so whenever quoting one.

## Docs

- `docs/session-log.md` — running session log, **newest section first**. Written by
  the `/wrap-up` skill at the end of a session.
- `engies/project.md` — the human landing page: a 2–3 page plain-language brief
  written for an entry-level reader, ending with "Recent updates". Refreshed daily by
  a scheduled job and by the `/update-project-doc` skill. If a session materially
  changes the project story, update it in the same session rather than waiting.
- `docs/plan.md` — original design intent. Historical; don't rewrite it.
- `perf/README.md` — what each performance tier measures and how much to trust it.

Keep this CLAUDE.md terse and machine-facing — human-facing guidance belongs in
`engies/`.
