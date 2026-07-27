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

```sh
cargo test --all-features                    # unit tests; what CI runs
node --test ui/paths.test.mjs                # ui/paths.js guards, no deps

cargo run --release --example locate_check   # highlight anchoring, 611 corpus fixtures
cargo run --example config_check             # config layering + write-back
cargo run --example theme_check              # bundled palettes: vars, --bg, --syntax-theme
cargo run --example mcp_check                # the MCP socket: mode, lock, wire, retirement
cargo run --example marks_check              # the marks file: modes, caps, crash artifacts, the lock
node perf/harness/ui-check.mjs               # settings panel in Chromium (needs harness setup)
```

```sh
dreamd theme list|set <name>|show [name]|new <name> [--from <base>]
dreamd config path|edit|get <key>|set <key> <value>
dreamd marks path|prune [--stale] [--older-than 30d]   # bare `prune` is a dry run
dreamd mcp                           # the stdio MCP shim; `claude mcp add dreamd -- dreamd mcp`
dreamd --theme nord [path]           # one run, no config write
```

`cargo test` covers the pure core and the security tenets: `guard` (scheme
allowlist, repo-root containment), `markdown` (HTML escaping, slugs, `locate`'s
three tiers), `theme` (`user_path` traversal, the CSS parser), `annotations`
(`Store` semantics), `config` (`deep_merge`), `fs_walk::build_tree`, `is_markdown`,
and `pty` (the base64 round trip, and a real pty driven with `/bin/sh` — never
with `PANE_COMMAND`, so `cargo test` cannot start a Claude Code session).
Nothing there touches `config_dir()` — that reads the real `~/.config/dreamd`,
and sandboxing it is `config_check`'s job — and `mcp_check`'s and
`marks_check`'s, whose socket and marks file live there too. The example
harnesses above still own what a unit test cannot reach (corpus anchoring,
config layering + write-back, the bundled palettes, the MCP transport, the marks
file on disk) and each exits non-zero on failure. **The GUI itself is
verified only by hand** — `ui-check.mjs` asserts what the page knows, not what it
paints; don't claim coverage beyond that.
`perf/run.sh` takes an exclusive lock: one tier at a time, no `cargo` alongside it.

```sh
packaging/set-version.sh 0.2.0       # Cargo.toml + website/src/consts.ts + Cargo.lock
packaging/check-version.sh 0.2.0     # CI assertion; tag must match the tree
packaging/check-signing.sh           # CI preflight; the six APPLE_* secrets, from env
NO_SIGN=1 packaging/build.sh aarch64-apple-darwin   # full release artifact, local
```

## Packaging

`packaging/build.sh <triple>` is the entire pipeline — build, sign, notarize,
staple, zip, checksum. `.github/workflows/release.yml` only wraps it, so a release
is reproducible locally. Everything platform-specific is derived from the triple
inside the script, never from the CI matrix: a Linux target is one matrix entry
plus one `case` arm. Tag `v*` → draft release; **publishing** it bumps the
Homebrew cask from `packaging/cask.rb.tmpl`.

**Releases are signed and notarized.** Turned on 2026-07-26; `packaging/SIGNING.md`
is the runbook, including rotation and back-out. The identity is
`Developer ID Application: OLIVER ONSTOTT FONG (34VGHNCG6J)`, the six `APPLE_*`
secrets are set, and `PUBLISH_CASK` is `true`, so `brew install --cask` and
browser downloads are both live channels again — `com.apple.quarantine` is written
by the downloading application, and a notarized, stapled `.app` passes Gatekeeper
when it is. (curl never writes the flag, which is why the curl channel worked
throughout.)

`Swatinem/rust-cache` carries `~/.cargo/bin`, so the pinned `cargo install
tauri-cli` in the matrix exits in ~1s on a warm cache and needs no cache of its
own. A multi-minute install step means the *rust-cache* missed; don't read it as
a missing tauri-cli cache and add one (tried 2026-07-27, changed nothing).

`NO_SIGN` is the one switch that turns it all off — set it in `release.yml`'s `env`
and `check-signing.sh` stands down while `build.sh` passes `--no-sign`. It is
absent on purpose; don't reintroduce it to work around a signing failure, because
an unsigned artifact served through the cask opens as "dreamd is damaged". Run
`packaging/check-signing.sh` against the secrets instead — it names the wrong one
in ten seconds rather than twenty minutes into the matrix.

Four things that will bite:

- **`bundle.icon` array order is load-bearing.** The first `.png` is baked into
  the binary as raw RGBA (`w × h × 4`). It was `icon.png` at 1024² = 4.19 MB,
  43% of the binary, for an image macOS never reads (`set_window_icon` is a
  no-op there). `128x128.png` is first on purpose; see `src-tauri/icons/README.md`.
- **Archive with `ditto`, never `tar`.** Part of a `.app`'s signature lives in
  xattrs; tar drops them and Gatekeeper rejects the extract. Artifacts are `.zip`.
- **No dmg.** Tauri's dmg bundler AppleScripts Finder and times out (-1712)
  without Automation permission — a coin flip in CI.
- **No entitlements, deliberately.** `trash` is pinned to `NsFileManager` so
  nothing needs Apple Events. Adding an Apple-Events caller re-opens that.

Version lives in `src-tauri/Cargo.toml` (source of truth), `website/src/consts.ts`,
and `Cargo.lock`. `tauri.conf.json` has **no** `version` key — Tauri falls back
to the crate version, verified.

## Tenets

1. **Read-only.** dreamd never writes to the user's markdown, and never writes anything
   inside the repo. Editing stays in Neovim. The one place it writes is
   `~/.config/dreamd/` (see tenet 2).
2. **Session state persists only under `~/.config/dreamd/`, and only as files.**
   Amended in step 4: marks that died with the process made an agent loop
   spanning sessions impossible, so highlights, annotations and the stack are
   written to `marks/<basename>-<16hex>.json` (0600) — debounced, and by the one
   dreamd that holds the repo's socket. *Preferences* are the older exception:
   `config.toml` and saved themes, written by the settings panel and the
   `config`/`theme` subcommands. Still no database, and still nothing written
   anywhere else — a third thing that wants to persist needs its own decision.
3. **No shell interpolation of user content.** Sent queries go through a temp file and
   a fixed `read @<file>` prompt. Highlighted text never enters a command line.
   The pane's `$SHELL -l -c "exec claude"` is the second shell dreamd spawns and
   obeys the same rule: `PANE_COMMAND` is a `const`, not a template, and a test
   pins it. What the user then *types* into that terminal is theirs — they are
   at a prompt, not having content interpolated on their behalf.
4. **Escape, don't execute.** Raw HTML in markdown is escaped. External links are
   restricted to `http`/`https`/`mailto`; relative images must resolve inside the repo
   root.
5. **Themeable to the CSS level.** A theme is `ui/theme.css` (base rules) plus a palette
   from `ui/themes/*.css` or `~/.config/dreamd/themes/*.css`. A palette is a *family*:
   a bare `:root` of shared type metrics plus `:root[data-mode="light"]` and
   `:root[data-mode="dark"]` colour blocks. A palette with no mode blocks is passed
   through untouched and reads the same in both — that is what keeps every pre-family
   user file working. Both surfaces are user-facing and hot-reloaded. `theme_css`
   points at a complete stylesheet instead, replacing the base.
6. **Label what crosses into an agent.** Untrusted content crossing into an agent's
   context is delimited and labelled, never merely passed. Tenet 4 is about a parser
   and has escaping; this reader is an LLM, so there is only labelling —
   `untrusted::delimit` and a per-process random sentinel a document cannot have been
   written to contain.

## Architecture

`src-tauri/src/main.rs` is a thin shell — CLI (clap), `AppState`, the ~27 `#[tauri::command]`
handlers, the builder. All logic lives in the `dreamd` **library** crate (`src/lib.rs` +
modules) *because a `[[bin]]` target cannot be imported*: the split is what makes
`src-tauri/benches/` possible. New logic goes in a module, not in `main.rs`.

State is one `AppState`: `repo_root`, `Mutex<Config>`, `Mutex<Store>` (highlights + stack),
`Mutex<SearchIndex>`. It spans every file opened in the session and dies with the process.
`Config` is behind a lock because the settings panel rewrites it at runtime — it is the
only configuration that changes after startup.

Data flow: `ui/app.js` (plain JS, no build step; `tauri.conf.json` points `frontendDist` at
`../ui`) calls commands over IPC and listens for watcher events. The CSP is
`script-src 'self'` with no `'unsafe-inline'` — an inline `<script>` in `index.html` is
blocked silently, which is why the pre-paint `data-mode` bootstrap lives at the top of
`app.js`. Inline `<style>` is fine (`style-src` carries `'unsafe-inline'`).
`index.html` loads `paths.js` before `app.js` — both classic scripts, `defer`, so
they run in document order and share globals. That is the only split, and it
exists so `node --test` can drive the containment guard without a browser.

`ui/vendor/` is the **one** exception to "no build step" and the only vendored JS
in the repo: xterm.js and its fit addon, the publishers' own prebuilt UMD
bundles, for the pane. Vendoring is forced by the CSP — a CDN is blocked, WASM is
blocked, inline is blocked silently. They are injected by `app.js` on the pane's
**first open**, not declared in `index.html`, so a launch that never opens a
terminal parses none of the 289 KB. `ui/vendor/README.md` is the provenance and
the upgrade procedure.

- `guard` — the two predicates deciding what a document may reach: `allowed_scheme`
  (http/https/mailto, case-folded) and `inside_root` (component-wise, so
  `/w/notes` rejects `/w/notes-private`). They live here rather than in the
  `open_external`/`delete_file` commands *because `main.rs` cannot be imported* —
  in `main.rs` the tenets were enforced by code no test could reach.
  `ui/paths.js` is the frontend twin, for relative links and images.
- `fs_walk` — `ignore` crate (ripgrep's walker) → nested `FileNode` tree, markdown only.
- `search` — `nucleo` fuzzy index over **paths only**; content search is v2.
- `markdown` — pulldown-cmark → HTML, syntect for fenced code. Raw source HTML is
  re-emitted as `Event::Text` (escaped); only syntect's own markup is trusted.
- `annotations::Store` — `Highlight { quote, prefix, suffix, line_start/end, state }` plus an
  ordered `stack` of ids. `set_annotation` is what enqueues a pair.
- `config` — layered TOML: global `~/.config/dreamd/config.toml` under a repo-local
  `.dreamd.toml`. Merging happens on raw `toml::Table`s, *not* on deserialized structs:
  with `#[serde(default)]` an absent key is indistinguishable from a defaulted one, which
  is how a local file that never mentioned `[keymap]` used to wipe the global one. Writes
  patch the global table and rename over the file; unknown keys survive, comments do not.
  `.dreamd.toml` is repo content and therefore untrusted (tenet 4) — it may name a `theme`
  but may not set `theme_css`, which would read an arbitrary file into a `<style>` tag.
- `theme` — the palette registry: `BUNDLED` (`include_str!`'d), user palettes in
  `~/.config/dreamd/themes/`, and the `--bg` / `--syntax-theme` values parsed back out of
  the CSS for the native window and syntect. Those two lookups take a `Scheme`, because
  a family declares both and `custom_property` is a last-wins textual scan: `mode_slice`
  drops the other appearance's blocks **and moves yours to the end** (CSS ranks
  `:root[data-mode=…]` above `:root` regardless of source order, so dropping alone would
  disagree with the webview). `ALIASES` maps pre-family names onto family + scheme; it is
  the *last* resort in `palette()`, after a user file, and its implied appearance loses
  to an explicit `mode` — which is why `Config::mode` is an `Option`. Debug builds read
  bundled palettes off disk so they hot-reload like user ones.
  `readCssVar`/`modeSlice` in `ui/app.js` mirror this; change one, change the other.
- `cli` — the headless `dreamd theme …` / `dreamd config …` / `dreamd marks …`
  subcommands. They run and exit
  before the Tauri builder, sharing the panel's write paths so both produce the same file.
  It also holds `repo_is_claimed`, the read of the socket lock that decides
  whether a process may write marks — in `main.rs` no test could reach it.
- `marks_file` — the persistence half of tenet 2: `admit` (every load-time rule,
  pure) plus `load`/`save` (mode 0600, temp sibling, rename). `load` never fails
  and never panics; a corrupt file costs the marks, not the launch. `main.rs`
  owns the *scheduling* — a load before the walk, a 500ms debounced save thread,
  a flush on `RunEvent::ExitRequested`, and the flush + reload `adopt_root` does
  in the same block that swaps config. Only the primary writes: the second
  dreamd on a repo keeps its marks in memory and says so.
- `mcp` — the agent surface. `jsonrpc`/`schema`/`tools`/`view` are pure and
  Tauri-free; `server` is the Unix socket the GUI listens on
  (`~/.config/dreamd/run/<16hex>.sock`, mode 0600, the same FNV-1a root hash
  `marks_file` uses) and `shim` is `dreamd mcp`, the process Claude Code spawns.
  **The shim answers `initialize`/`tools/list` from the compiled-in `schema`
  const and proxies only `tools/call`** — proxying the list would let a dreamd
  that happened to be closed at client startup cache an empty tool list for the
  whole session. Binding the socket is also how a dreamd claims a repo: an
  `AddrInUse` that *connects* means a live owner and this process runs as a
  secondary; one that refuses is a crash leftover, unlinked and rebound.
  `adopt_root` retires and re-binds it, next to the watcher, because the socket
  is named after the root.
- `notify` — `marks-changed`, the only *store* change dreamd pushes unprompted.
  Emitted **only** from the MCP layer, never from a command: a command's return
  value is already the frontend's truth for its own mutation, and a second
  signal would put two repaint paths in a race. That is also what keeps
  `save_to_paint` out of the agent path entirely. The server takes a `Notifier`
  closure rather than an `AppHandle`, which is what lets `mcp_check` drive the
  transport with no window. (`pty-data`/`pty-exit` are the other unprompted
  events, and the exception that proves the rule: terminal output arrives when
  the child feels like producing it, so there is no command return value to
  carry it.)
- `pty` — the embedded Claude Code pane's pseudo-terminal, one per window,
  created on **first open** and never at boot. Output crosses to the frontend
  **base64-encoded**: a 4 KiB read splits multi-byte characters, and only
  `Terminal.write`'s stateful decoder is in a position to reassemble them.
  Input is base64 for the mirror-image reason — a paste is arbitrary bytes. The
  child is a **login** shell running a fixed `exec claude`, because a `.app`
  launched from Finder inherits launchd's minimal `PATH` and would not find
  `claude` at all. Takes a `Sink` closure rather than an `AppHandle`, the same
  shape and for the same reason as `notify`'s. **A pty needs no entitlement**
  under dreamd's hardened runtime — measured against a signed bundle before the
  module was written; don't add an entitlements file for it.
- `watcher` — `notify` thread emitting `file-changed` / `file-added` / `file-removed` /
  `theme-reloaded`; the frontend responds by re-rendering and calling `reanchor`. It
  watches the repo, the user themes directory, and an explicit `theme_css` path — changing
  `theme_css` needs a restart to re-arm that watch.
- `send` — assembles markdown, writes a temp file, then tmux `send-keys` a fixed
  `read @<file>` prompt (falling back to clipboard). See tenet 3.

**Highlight anchoring is the subtle part.** A quote is located in the *source* by
`markdown::locate` in three fallbacks: exact `prefix+quote+suffix`, exact quote, then
whitespace-stripped match. The frontend sends what `getSelection().toString()` returns —
**rendered DOM text**, never raw source — so the whitespace-normalized path is the hot one
and the only realistic thing to benchmark. `reanchor_file` re-runs this on save; failure
marks the highlight `Stale` rather than dropping it. Marks read off disk are
re-anchored **lazily, once per file, in `get_highlights`** — never all of them at
startup, which would land straight on the cold-start number. `AppState`'s
`pending_reanchor` is what makes the steady state free: a mark created this
session was anchored against the bytes on screen and is never in it.

**Perf instrumentation** is behind the off-by-default `perf` cargo feature. Marks go to
**stderr** as NDJSON because `console.log` in WKWebView never reaches process stdout;
frontend marks route through the `perf_mark` command, `d:`-prefixed phases are durations.
`--bench-startup` runs the pre-window sequence and exits; `DREAMD_PERF_SEED` preloads
highlights from a corpus fixture.

## Working practices

- Commits go **straight to main** — no branches, no PRs.
- `cargo build` must pass before any commit touching `src-tauri/`.
- `.github/workflows/ci.yml` runs fmt + clippy (`-D warnings`) + test + build on
  **macos-14** for every push and PR, then `config_check`, `theme_check`,
  `mcp_check`, `marks_check` and `locate_check` (the last against a cached
  corpus), plus `node --test ui/paths.test.mjs` and `ui-check.mjs` on
  ubuntu. macOS is not a preference: Tauri on Linux needs webkit2gtk, and the
  `#[cfg(target_os = "macos")]` paths compile nowhere else. Run those harnesses
  locally before pushing — CI is the backstop, not the first check.
  Its toolchain is **pinned** (`dtolnay/rust-toolchain@1.97.1`); bumping it is a
  deliberate commit that also clears whatever the new clippy found.
- Repeatable flows become skills in `.claude/skills/`.
- Performance is measured, not guessed. `/perf-quick` (~60s) after an edit,
  `/perf-pass` (~5min) before a large commit touching `src-tauri/` or `ui/` (do `/perf-quick` for smaller commits), `/perf-deep`
  (~20min) to profile or move the baseline only on user request. `perf/baseline.json` changes only via
  `perf-deep`, in the same commit as the change that justified it.
- Numbers from `perf/harness/` are Chromium, **not** WKWebView — relative regression
  signal only. Say so whenever quoting one. `perf/harness/ui-check.mjs` is the exception:
  it lives there for the Playwright install, asserts on DOM and IPC rather than timings,
  and feeds no baseline.

## Docs

- `docs/session-log.md` — running session log, **newest section first**. Written by
  the `/wrap-up` skill at the end of a session.
- `engies/project.md` — the human landing page: a 2–3 page plain-language brief
  written for an entry-level reader, ending with "Recent updates". Refreshed daily by
  a scheduled job and by the `/update-project-doc` skill. If a session materially
  changes the project story, update it in the same session rather than waiting.
- `docs/plan.md` — original design intent. Historical; don't rewrite it.
- `perf/README.md` — what each performance tier measures and how much to trust it.
- `website/CLAUDE.md` — the public site at `fongo.uk/dreamd`. A standalone Astro
  project, deployed separately; source of truth for everything under `website/`.
  Nothing there touches the Rust build.

Keep this CLAUDE.md terse and machine-facing — human-facing guidance belongs in
`engies/`.
