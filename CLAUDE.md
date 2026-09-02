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
./perf/run.sh deep --update-baseline # the ONLY way the perf baseline changes (it lives in notes/)
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
cargo run --example agent_check              # the permission gate: the real `dreamd approve` over a real socket
node perf/harness/ui-check.mjs               # settings panel in Chromium (needs harness setup)
```

```sh
dreamd theme list|set <name>|show [name]|new <name> [--from <base>]
dreamd config path|edit|get <key>|set <key> <value>
dreamd marks path|prune [--stale] [--older-than 30d]   # bare `prune` is a dry run
dreamd mcp                           # the stdio MCP shim; `claude mcp add dreamd -- dreamd mcp`
dreamd approve --socket <path>       # the PreToolUse permission hook. dreamd installs it; not for typing
dreamd --theme nord [path]           # one run, no config write
```

`cargo test` covers the pure core and the security tenets: `guard` (scheme
allowlist, repo-root containment), `untrusted` (the envelope, and that a body
which has learned the sentinel still cannot break out of it), `markdown` (HTML
escaping, slugs, `locate`'s three tiers), `theme` (`user_path` traversal, the
CSS parser), `annotations`
(`Store` semantics), `config` (`deep_merge`), `fs_walk::build_tree`, `is_markdown`,
`prompt` (that nothing of dreamd's lands inside an envelope, that a forged
delimiter cannot break out, and that nothing a reader wrote reaches the typed
line), `rootfield` (the absolute-path rule, names-not-paths, the cap),
and `pty` (the base64 round trip, and a real pty driven with `/bin/sh` — never
with `PANE_COMMAND`, so `cargo test` cannot start a Claude Code session).
Nothing there touches `config_dir()` — that reads the real `~/.config/dreamd`,
and sandboxing it is `config_check`'s job — and `mcp_check`'s and
`marks_check`'s, whose socket and marks file live there too. The example
harnesses above still own what a unit test cannot reach (corpus anchoring,
config layering + write-back, the bundled palettes, the MCP transport, the marks
file on disk) and each exits non-zero on failure. **The GUI itself is
verified only by hand** — `ui-check.mjs` asserts what the page knows, not what it
paints; don't claim coverage beyond that. `testdocs/images.md` is what that hand
check reads: open dreamd on this repo and work down it, at 100% and zoomed.

`packaging/smoke.sh` is the one check that *launches* the program. Under Xvfb,
in its own fixture repo with `XDG_CONFIG_HOME` pointed at a scratch directory,
in one of two modes. `SMOKE_EXPECT=paint` waits for the `first_paint` mark,
which `ui/app.js` cannot emit unless GTK initialised, wry built a real
WebKitGTK webview, `frontendDist` loaded, the CSP admitted both classic scripts
and IPC completed in both directions — so it needs `--features perf`, and it is
the stronger check. `SMOKE_EXPECT=window` is for a release artifact, which
carries no instrumentation and must be smoked as shipped: the MCP socket
appearing (bound in `.setup`, i.e. after the window exists), a `WebKit*`
descendant process, and survival. It proves a window, not a page. Both are
Linux-only and need nothing but bash, coreutils and `/proc`, which is what lets
them run inside a bare container next to a downloaded artifact.
`perf/run.sh` takes an exclusive lock: one tier at a time, no `cargo` alongside it.

```sh
packaging/set-version.sh 0.2.0       # Cargo.toml + website/src/consts.ts + Cargo.lock
packaging/check-version.sh 0.2.0     # CI assertion; tag must match the tree
packaging/check-signing.sh           # CI preflight; the six APPLE_* secrets, from env
NO_SIGN=1 packaging/build.sh aarch64-apple-darwin      # full release artifact, local
NO_SIGN=1 packaging/build.sh x86_64-unknown-linux-gnu  # AppImage + .deb + .tar.gz

SMOKE_EXPECT=paint packaging/smoke.sh ./target/debug/dreamd   # needs --features perf
SMOKE_EXPECT=window packaging/smoke.sh ./dist/dreamd-*.AppImage
```

## Packaging

`packaging/build.sh <triple>` is the entire pipeline — build, sign, notarize,
staple, zip, checksum. `.github/workflows/release.yml` only wraps it, so a release
is reproducible locally. Everything platform-specific is derived from the triple
inside the script, never from the CI matrix. Tag `v*` → draft release;
**publishing** it bumps the Homebrew cask from `packaging/cask.rb.tmpl`, and (if
`vars.PUBLISH_AUR` is true) renders `packaging/PKGBUILD.tmpl` as an artifact.

**Linux** is `x86_64-unknown-linux-gnu`, built on `ubuntu-22.04` — the oldest
supported image, because the ELF links the runner's glibc and 2.35 runs on
everything newer. Three artifacts: `.AppImage` and `.deb` from the bundler, plus
a `.tar.gz` of the bundler's *staged deb tree* (`usr/bin/dreamd` + the generated
`.desktop` + hicolor icons) — so the tarball, the deb and the AppImage carry
byte-identical desktop integration, and `install.sh` and the PKGBUILD both just
copy it. `tauri-bundler` has **no pacman backend**, which is why Arch goes
through a hand-written `PKGBUILD.tmpl` instead of a fourth `--bundles` entry.
Nothing on Linux is signed; the `.sha256` files are the integrity story.

The desktop entry is **not** the bundler's default — `bundle.linux.deb.desktopTemplate`
points at `packaging/dreamd.desktop`, which adds `MimeType` and the `%f` field
code (without both, dreamd never appears under "Open With" for a `.md` file).
It reaches all three artifacts because the AppImage's AppDir comes from
`debian::generate_data` and the tarball is that staged tree. `bundle.icon` is
likewise the literal hicolor size list on Linux; see `src-tauri/icons/README.md`.

**Building the Linux artifacts locally needs `NO_STRIP=1`** on any distro new
enough to emit `SHT_RELR`. linuxdeploy's bundled `strip` cannot parse
`.relr.dyn` and fails on every system library it copies in, and Tauri swallows
its stderr, so the only symptom is `failed to run linuxdeploy`. `ubuntu-22.04`
predates it, so `release.yml` is unaffected and must not set it. A rerun after a
failure needs the `bundle/` directory deleted first. Still true on Arch as of
2026-07-28: `NO_SIGN=1 NO_STRIP=1 packaging/build.sh x86_64-unknown-linux-gnu`
produces all three artifacts on the development box.

**`failed to run linuxdeploy` is the whole error, not a summary of one.**
tauri-bundler runs linuxdeploy through `cmd.output()` — stderr captured and
discarded — whenever its own log level is Error, and only switches to the
variant that surfaces the child's output above that. `VERBOSE=1` on `build.sh`
appends `--verbose` and is the only way to turn that string into a sentence;
`canary.yml` sets it unconditionally, because a scheduled job nobody is
watching gets one chance to explain itself. Reach for it before theorising.

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
   **There is no longer any write outside it into another program's
   configuration.** `mcp::register` used to run `claude mcp add` from a button on
   the pane's MCP strip; that button is gone, because `agent_spawn` hands the
   session `--mcp-config` and the surface dreamd launches no longer needs a
   registration to exist. What the module still owns is the *text* of that
   command, for the surfaces dreamd does not launch. The query file `send` puts
   in the system temp directory is transport rather than state — tenet 3 is why
   it exists, and a later day's first send removes it.
3. **No shell interpolation of user content.** Sent queries go through a temp file and
   a fixed `read @<file>` prompt. Highlighted text never enters a command line.
   The pane's `$SHELL -l -i -c "exec claude"` is the second shell dreamd spawns and
   obeys the same rule: `PANE_COMMAND` is a `const`, not a template, and a test
   pins it. `concat!` of two literals in the same file is still a `const`; the
   `--allowed-tools` grant and the `/model` lines are both written that way.
   What the user then *types* into that terminal is theirs — they are
   at a prompt, not having content interpolated on their behalf.
   The native surface has **no shell at all**: `agent::claude` resolves where
   `claude` is once, through a login shell, then spawns that path with one
   `Command::arg` per argument, and a turn crosses as a `serde_json`-built
   string rather than as typed keystrokes. The one place a shell survives is
   Claude Code's own hook runner, which is why `gate_server::settings_json`
   single-quotes the two dreamd-minted paths it embeds and **refuses to launch**
   rather than escaping if either could break out.
4. **Escape, don't execute.** Raw HTML in markdown is escaped. External links are
   restricted to `http`/`https`/`mailto`; relative images must resolve inside the repo
   root. That last one is enforced twice, in two layers that cannot cover for
   each other: `ui/paths.js` decides which `<img src>` becomes a URL at all, and
   the `asset://` protocol's scope decides what the process will open. The CSP
   carries no `file:` — an image reaches the page through the asset protocol or
   not at all.
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

`src-tauri/src/main.rs` is a thin shell — CLI (clap), `AppState`, the ~54 `#[tauri::command]`
handlers, the builder. All logic lives in the `dreamd` **library** crate (`src/lib.rs` +
modules) *because a `[[bin]]` target cannot be imported*: the split is what makes
`src-tauri/benches/` possible. New logic goes in a module, not in `main.rs`.

State is one `AppState`: `RwLock<PathBuf>` for the root (every command reads it, only
File ▸ Open writes), `Mutex<Config>`, `Arc<Mutex<Store>>` (highlights + stack) and
`Arc<Catalog>` — the tree and the search index, from one walk behind one readiness gate.
It spans every file opened in the session, and the store is the one part that outlives
the process, through `marks_file` (tenet 2). Every `Arc` in it is state shared with a
thread that outlives a command, and there are four: the MCP socket thread holds the
store and a reader over `open_doc`, a deferred walk fills the catalog, and the
debounced marks-save thread reads `dirty`. `Config` is behind a lock because the settings panel
rewrites it at runtime — it is the only configuration that changes after startup.

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
  `ui/paths.js` is the frontend twin, for relative links and images. The
  *image* half of that twin has a second enforcement point with no code in
  common: `main.rs`'s `allow_asset_root` opens the `asset://` scope on the repo
  root and on nothing else, at `.setup()` and again in `adopt_root`. It never
  *forbids* the old root — a forbid outranks every later allow for the life of
  the process, so returning to a repo would find its images dead — and nothing
  can reach the leftover permission anyway, because the only paths that ever
  become `asset://` are the ones `interceptLinks` resolved inside the root that
  is open now.
- `untrusted` — tenet 6's enforcement, in the library for the same reason `guard` is.
  `delimit` **labels** a body rather than filtering it — a user's own words are the
  evidence the whole highlight loop exists to carry, and an instruction can be spelled
  too many ways for a denylist to be anything but theatre. The sentinel it delimits with
  is per-process random because a document written yesterday cannot contain a value drawn
  from the OS this morning, which is the only unforgeability available when the parser is
  a reader. `neutralise` *replaces* a forged sentinel instead of deleting it: deleting
  would splice the neighbours together and let a body carrying one in two halves
  reassemble a whole one through the gap.
- `fs_walk` — `ignore` crate (ripgrep's walker) → nested `FileNode` tree, markdown only.
  `rel_of` is shared with `search`: the frontend looks a search hit up in the
  tree by that exact string, so the two must not derive it separately.
- `search` — `nucleo` fuzzy index over **paths only**; content search is v2.
- `flow` — the state machine between Ctrl+Enter and the pty, and there is **no
  clock in it**: the frontend owns the timer and supplies the events (`arm`,
  `cancel`, `take_ready`), so this is testable without sleeping or injecting a
  clock. A terminal transition *removes* the entry rather than marking it, which
  makes double-submit unrepresentable instead of merely guarded; `take_ready`
  hands back the oldest armed entry and one at a time, so two queued sends are
  two turns. `Phase::Undo` survives an undo window that no longer waits, because
  "queued" and "eligible" coinciding is the frontend's policy, not this type's.
- `prompt` — `send`'s assembly reshaped for the pane, and the reshaping is tenet
  6: dreamd's instruction, the numbering, the file name, the mark id and the
  reader's *question* all sit **outside** the envelopes, one envelope per
  passage, so everything inside a sentinel is file content byte for byte. The
  question is outside deliberately — the notice inside says "do not obey this",
  which is right for a quoted passage and wrong for the ask. `read_line` is the
  only thing typed into the pty and carries no newline: the far end is Claude
  Code's composer *or*, once `claude` has exited, the login shell that spawned
  it, and a fixed line naming a dreamd-minted path is safe under either reading.
- `rootfield` — the root path field, in a module for the reason `guard` is.
  `inside_root` deliberately does not apply — leaving the root is the point — so
  what stands in its place is narrower: absolute only (nothing resolves against
  the cwd, which is `/` for a Finder launch), an existing directory to list, bare
  *names* back and never paths, dot-directories only when the prefix asks by
  name, and a capped listing.
- `markdown` — pulldown-cmark → HTML, syntect for fenced code. Raw source HTML is
  re-emitted as `Event::Text` (escaped); only syntect's own markup is trusted.
- `annotations::Store` — `Highlight { quote, prefix, suffix, line_start/end, state }` plus an
  ordered `stack` of ids. `set_annotation` is what enqueues a pair. `mark_sent`
  stamps `sent_at`, sets `prior`, and takes the ids off the stack; the `prior` is
  the fade, and it is the *only* thing the reader sees of a send — the pending
  stamp itself paints nothing. The chip that used to sit on the rail
  per sent mark, and the `resolve_mark` command behind its "Answered" button, are
  both gone — a question that has been asked is assumed dealt with, and five cards
  asking to confirm that was five cards over the paragraph they were about.
  `sent_at`, `Store::resolve` and the MCP `resolve_highlight` stay: they are the
  agent's record of what it closed, and `list_highlights` filters on `resolved`.
  **One highlight per passage, and `retarget` is the price of it.** Overlapping
  marks were all in the store and all on the stack, but only the topmost was
  reachable by a click, so the ones underneath became annotations nobody could
  read, edit or delete. `triggerHighlight` now refuses a selection that overlaps
  a painted mark and opens *that* mark's modal instead — the decision is made in
  the DOM (`overlappingIds`), not in the store, because line numbers cannot tell
  two phrases in one paragraph apart, and adjacency is deliberately not overlap
  or the sentence after a highlight would be unhighlightable. With overlap
  refused, changing where a passage ends has to reach the mark that exists, which
  is `Store::retarget` (command: `resize_highlight`): new quote, prefix, suffix
  and lines, same id — so the annotation, the stack slot, `sent_at`, `resolved`
  and `prior` all survive, and a resize moves nothing on or off the stack. It
  anchors against the *mark's own* file, not the open document, because the stack
  spans files. Anchoring is `add_anchored`'s, `(0, 0)` fallback included.
- `config` — layered TOML: global `~/.config/dreamd/config.toml` under a repo-local
  `.dreamd.toml`. Merging happens on raw `toml::Table`s, *not* on deserialized structs:
  with `#[serde(default)]` an absent key is indistinguishable from a defaulted one, which
  is how a local file that never mentioned `[keymap]` used to wipe the global one. Writes
  patch the global table and rename over the file; unknown keys survive, comments do not.
  `.dreamd.toml` is repo content and therefore untrusted (tenet 4) — it may name a `theme`
  but may not set `theme_css`, which would read an arbitrary file into a `<style>` tag,
  nor `agent.permission_mode`, which would let a repo choose what your agent may do
  unasked, nor `ui.menubar`/`ui.titlebar`, which are the user's window frame and
  not furniture inside it — `titlebar = false` from a cloned repo would take away
  the close button. All four refusals live in `strip_untrusted`, a pure function
  over the local table, so a new denied key is a test rather than a code path only
  `config_check` reaches. `ui.titlebar` and `ui.titlebar_fade` are the two
  preferences whose *defaults* are per-platform (`TITLEBAR_DEFAULT` and
  `TITLEBAR_FADE_DEFAULT`, both true only on macOS, and in the same direction
  for opposite reasons: the bar there is an overlay, so there is none to reclaim
  and dreamd's own row is the only edge worth dissolving); both are values
  rather than `cfg` arms precisely so `apply_chrome` stays one code path on both.
  The four panel sizes — `ui.tree_width` (140–600), `stack_width` (200–720),
  `pane_width` (240–1200) and `pane_height` (120–1200) — are *clamped* on
  deserialize rather than
  validated — a drag persists without a round trip through a validator, and a stale
  number costs a nearest-usable panel, not the rest of the file. The pane keeps a
  width *and* a height because its drag handle changes axis with `agent.position`:
  one shared key would reinterpret a tall bottom pane as a wide right one.
  `ui.zoom` (50–300, percent) is clamped the same way and is the fifth size,
  but the only one about the *document*: it multiplies `--font-size` **and**
  `--content-width`, so the measure in characters is what stays constant and
  the prose reflows instead of growing a scrollbar. Deliberately not
  `WebviewWindow::set_zoom` — that scales the chrome too and moves every rect
  the placement code measures. A repo may set it, next to the four sizes and
  `titlebar_fade`, and unlike them the frontend re-reads it on `repo-changed`:
  it is a claim about the *material*, where a panel width is a claim about the
  reader's window.
  `agent.position` defaults to `right`, and the reason is the stack: both panels
  dock to the same edge, so sending from the stack panel is a *substitution* —
  the queue closes, the pane opens where it was — rather than a jump across the
  window. `bottom` stays a supported layout and is the one value a test should
  use when it means "not the default". `agent.popout` (`never` / `send` /
  `always`) is the other axis and outranks `position` when it fires: the
  conversation goes into a centred card instead of a dock. Three values rather
  than a bool because the reason to want one is usually the *send* — a stack
  hand-off produces an answer to read, not a session to sit in — so `send`
  leaves the pane's own toggle on the dock and `always` makes the card the only
  agent surface there is. Like `surface` and unlike `permission_mode` it is a
  repo's to set: where a conversation is drawn is not what an agent may do.
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
  **What the OS asks for is a second, separately remembered fact.** `scheme_for`
  takes it as an argument, and `AppState` holds it in its own atom beside the
  appearance on screen: while an explicit light or dark is pinned it cannot be
  *asked* for — `Window::theme()` returns tao's cache of the pin on both
  platforms, and the webview's `prefers-color-scheme` follows the pin too — so
  the pinned value used to stand in for it, and one trip through Light rewrote
  what `system` meant for the rest of the session. `os_scheme` is the only read:
  it clears the pin first, and `set_config` calls it only on the way *into*
  `system`, which is a mode that ends unpinned anyway.
- `cli` — the headless `dreamd theme …` / `dreamd config …` / `dreamd marks …`
  subcommands. They run and exit
  before the Tauri builder, sharing the panel's write paths so both produce the same file.
  It also holds `repo_is_claimed`, the read of the socket lock that decides
  whether a process may write marks — in `main.rs` no test could reach it.
- `marks_file` — the persistence half of tenet 2: `admit` (every load-time rule,
  pure) plus `load`/`save` (mode 0600, temp sibling, rename). `load` never fails
  and never panics; a corrupt file costs the marks, not the launch. `main.rs`
  owns the *scheduling* — a load before the walk, a 500ms debounced save thread,
  a flush on `RunEvent::ExitRequested` *and* on `Exit` (the first can be
  cancelled by a listener, and a second flush costs one atomic read), and the
  flush + reload `adopt_root` does
  in the same block that swaps config. Only the primary writes: the second
  dreamd on a repo keeps its marks in memory and says so.
- `mcp` — the agent surface. Six tools: four read (`get_stack`,
  `get_open_document`, `get_highlight`, `list_highlights`) and two write
  (`resolve_highlight`, `mark_passage`), none of which writes a file byte.
  **`schema::TOOLS`'s descriptions are product surface, not boilerplate** — they
  are the only thing steering an agent to the queue rather than a sweep of the
  repo, and getting one wrong fails no test. `get_open_document` is the one tool
  answering about the *window* rather than the store: it reports the path the
  human has on screen and nothing else, for a question that says "this file" or
  "here". `main.rs` records that path in `render_markdown` rather than through a
  command of its own — dreamd renders one document at a time, so the render *is*
  the open document, and a `set_open_document` would be a third IPC on a path
  that already costs two and is measured. The slot is **never cleared**; the
  tool re-checks containment and existence on every call, so a File → Open, a
  deletion and a repo swap each answer "nothing open" without anything having to
  remember to blank it. It reaches the tool as an `OpenDoc` *closure*, the same
  shape and for the same reason as `notify::Notifier`: `AppState` is a name this
  module may not speak, so `main.rs` supplies a reader and `mcp_check` supplies
  its own. `jsonrpc`/`schema`/`tools`/`view` are pure and
  Tauri-free; `jsonrpc` owns the NDJSON framing for **both** transports, the
  `MAX_LINE` cap included — an unbounded `read_line` on either end is a memory
  DoS, so the reader enforcing it is one function rather than one per stream.
  `server` is the Unix socket the GUI listens on
  (`~/.config/dreamd/run/<16hex>.sock`, mode 0600, the same FNV-1a root hash
  `marks_file` uses) and `shim` is `dreamd mcp`, the process Claude Code spawns.
  **The shim answers `initialize`/`tools/list` from the compiled-in `schema`
  const and proxies only `tools/call`** — proxying the list would let a dreamd
  that happened to be closed at client startup cache an empty tool list for the
  whole session. Binding the socket is also how a dreamd claims a repo: an
  `AddrInUse` that *connects* means a live owner and this process runs as a
  secondary; one that refuses is a crash leftover, unlinked and rebound.
  `adopt_root` retires and re-binds it, next to the watcher, because the socket
  is named after the root. `server::Status` is what the pane's status strip
  reads: `serving` (this process won the bind) and `clients` (connections
  accepted since it). Each `spawn` gets its own, so a retiring server on the
  previous root cannot write `serving = false` over its replacement. `clients`
  is **not** liveness — the shim connects per call — and it is deliberately
  **not on the wire**: a count of zero is equally true of a correctly-wired
  agent that has not needed dreamd yet, so a strip keyed on it spent every
  session accusing a healthy window and offered a Register button whose success
  it could not observe, which is how pressing it led to a Restart that
  repainted Register. It stays as a diagnostic `mcp_check` asserts on, never as
  a verdict. `register` no longer *writes* anything — `registered` below still
  runs a `claude` — and what replaced the write is two documents: `config_json` is the
  `--mcp-config` document `agent_spawn` hands the session, and `add_command` is
  `claude mcp add dreamd --scope user -- <launcher> mcp` as *text*, for the
  surfaces dreamd does not launch (a Claude Code in tmux, or the terminal pane,
  whose command is four fixed literals with nowhere to put a path). Both come
  off `add_args`, and a test pins the printed one to it — the last hand-written
  copy dropped `--scope user`, named a bare `dreamd`, and put a second
  registration on the development machine. **`--scope user`, because `shim`
  derives the root from its own cwd** — one registration serves every repo,
  where the CLI's default `local` scope would put the strip back on screen in
  the next one. The launcher is `$APPIMAGE` before `current_exe`: a
  registration outlives the process, and an AppImage's `current_exe` is a
  `/tmp` mount that dies with the window. `registered` is `claude mcp get`'s
  **exit status**, three-valued — a `claude` that could not be run at all is
  not the same answer as one that ran and said no — cached per process, and
  read only by the terminal surface. **And now asked only by it**: that probe is
  a whole Claude Code startup (measured 0.8–1.5s), `mcp_status` fires on the
  pane's first open, and on the native surface it was a second `claude` racing
  the session that open had just spawned for an answer that surface cannot use —
  it is handed `--mcp-config` and has no registration to be missing.
- `notify` — `marks-changed`, the only *store* change dreamd pushes unprompted.
  Emitted **only** from the MCP layer, never from a command: a command's return
  value is already the frontend's truth for its own mutation, and a second
  signal would put two repaint paths in a race. That is also what keeps
  `save_to_paint` out of the agent path entirely. The server takes a `Notifier`
  closure rather than an `AppHandle`, which is what lets `mcp_check` drive the
  transport with no window. (The other unprompted events are all the same
  exception proving the same rule — the answer is not ready when the command
  returns, so no return value can carry it. `pty-data`/`pty-exit` and
  `agent-event`/`agent-ask` arrive when the child feels like producing them;
  `repo-changed` is `set_root`'s, emitted from the thread `adopt_root` hands the
  re-walk to, which is the one command that emits.)
- `agent` — the **native** agent surface, and the default one: `claude -p
  --output-format stream-json --input-format stream-json`, whose output is
  structure rather than pixels, so the conversation is drawn by dreamd. Laid
  out like `mcp/` and Tauri-free for the same reason — `Sink` is a closure, not
  an `AppHandle`. `wire::digest` is the whole of dreamd's knowledge of another
  program's schema and is **lenient by contract**: it returns a `Vec`, never a
  `Result`, so an unknown message kind costs a ticker row and never the pane
  (`fixtures/edges.ndjson` carries invented kinds and a line of prose to keep
  that true). Text is read twice — deltas paint plain, the closing `assistant`
  block is re-rendered through `markdown::render_with` — which is what makes an
  answer about typeset prose itself typeset, and what makes tenet 4 cover it
  for free. `gate` is the permission policy: **`--permission-prompt-tool` no
  longer exists**, so the gate is a `PreToolUse` hook, which measured against
  2.1.220 fires *even under `bypassPermissions`* and outranks the mode. That
  makes the seven pre-granted tools a fast path rather than the whole policy, and
  **deny is the answer to every kind of silence** — closed pane, retired
  server, unparseable payload, elapsed wait. `gate_server` is a **per-session**
  socket, not the per-repo MCP one: a secondary window's agent must not raise
  its cards in the primary's. Its name is `g<12hex>.sock`, deliberately shorter
  than MCP's `<16hex>.sock`, because that one already spends the ~104-byte
  `sun_path` budget down to 102 under `$TMPDIR` — a `gate-` prefix did not bind
  at all, and a test pins the inequality. `hook` is `dreamd approve`, shaped
  like `mcp::shim` and inverted where it counts: that one answers locally so a
  closed dreamd cannot blank an agent's tool list, this one has no local
  answers and fails closed. The launch carries **two** inline JSON documents,
  one `Command::arg` each: `--settings` is the hook, and `--mcp-config` is
  `mcp::register::config_json` — which is what makes the six pre-granted
  `mcp__dreamd__*` tools refer to anything, and why the native surface needs no
  `claude mcp add` and cannot inherit a registration naming a binary that has
  moved. `spawn_args` is the whole vector, pure and separate, because a flag
  that must never appear is a claim about the assembled argv rather than about
  any one const: **never pass `--bare`** — it skips hooks, and the gate is a
  hook — and **never pass `--strict-mcp-config`**, which would make dreamd's
  document the only MCP config the session sees and drop every other server the
  reader has, inside dreamd's pane and nowhere else. `--verbose` is not
  decoration; `-p` will not stream without
  it. A slash command sent as ordinary user text *is* honoured, which is why
  the model chips still cost no restart. `resolve` asks a **login *and*
  interactive** shell where `claude` is, once per process — and `warm` is that
  once, moved to a thread in `.setup()`. It used to be `agent_spawn`'s first act,
  so a whole `$SHELL -l -i` startup (~300ms here, and however long the reader's
  `.zshrc` takes elsewhere) was paid on the pane's first open, which is the one
  place it is visible. It starts no agent: the session is still created on first
  open, and a launch that never opens the pane still spawns no `claude`.
- `pty` — the **fallback** surface, kept undocumented behind `agent.surface =
  "terminal"` and removed when nobody reports needing it. The embedded Claude
  Code pane's pseudo-terminal, one per window,
  created on **first open** and never at boot. Output crosses to the frontend
  **base64-encoded**: a 4 KiB read splits multi-byte characters, and only
  `Terminal.write`'s stateful decoder is in a position to reassemble them.
  Input is base64 for the mirror-image reason — a paste is arbitrary bytes. The
  child is a **login *and* interactive** shell (`-l -i -c`) running a fixed
  `exec claude`, because a `.app` launched from Finder inherits launchd's minimal
  `PATH` and would not find `claude` at all. `-l` alone was not enough: it reads
  `.zprofile`, never `.zshrc`, and `.zshrc` is where the PATH carrying
  `~/.local/bin` — the official installer's target — lives. `cargo tauri dev`
  inherits the terminal's PATH, so the failure existed only in a bundled build
  and only from Finder; `SHELL_FLAGS` is a pinned const with a test for that
  reason. Takes a `Sink` closure rather than an `AppHandle`, the same
  shape and for the same reason as `notify`'s. **A pty needs no entitlement**
  under dreamd's hardened runtime — measured against a signed bundle before the
  module was written; don't add an entitlements file for it.
  Every launch carries `--allowed-tools Read` plus dreamd's six MCP tools and
  **nothing that writes** — highlighting a passage and attaching a question to
  it is already the consent, and a permission prompt for the stack lands in a
  terminal nobody is looking at. The list is spelled out rather than
  wildcarded, and a test pins it at exactly seven; a seventh MCP tool is a
  deliberate line, not a silent grant. `Model` is the second closed enum here:
  three chips in the pane header become one of three fixed `/model` lines typed
  into the *running* child, so switching model costs no restart (the permission
  mode still does — that one is a launch flag).
- `watcher` — `notify` thread emitting `file-changed` / `file-added` / `file-removed` /
  `theme-reloaded`; the frontend responds by re-rendering and calling `reanchor`. It
  watches the repo, the user themes directory, and an explicit `theme_css` path — changing
  `theme_css` needs a restart to re-arm that watch. `Recursive` is one FSEvents
  stream on macOS but one inotify watch **per directory** on Linux, against a
  machine-wide `fs.inotify.max_user_watches` budget — the one place the same call
  has a materially different cost per platform.
- `send` — assembles markdown, writes a temp file, then tmux `send-keys` a fixed
  `read @<file>` prompt (falling back to clipboard). See tenet 3. The session's
  first send also deletes *earlier days'* query files from the temp directory —
  today's are kept because the path inside a prompt has to stay readable for as
  long as the agent might act on it, and a second dreamd is pointing at its own.
- `chrome` — the other half of `apply_chrome`: `ui.titlebar`, the window
  manager's bar, `set_decorations`. **A no-op on macOS, and that is the whole
  module.** tao rebuilds the style mask from scratch on every
  `set_decorations` call — `Closable | Miniaturizable | Resizable | Titled` for
  `true` — dropping the `FullSizeContentView` and transparent-titlebar bits
  `tauri.conf.json` created the window with. So toggling the bar off and back on
  produced an *opaque* native bar on top of a page laid out for none,
  unrecoverable short of hand-editing `config.toml`, and there is no second call
  that puts those bits back. macOS has no bar to reclaim anyway — only the
  traffic lights, which stay in every mode — so the settings panel hides the row
  there (`WINDOW_TOGGLES`, `mac: false`) rather than offering a dead switch.
  `ui.titlebar_fade` is the macOS-only preference that replaced it and never
  reaches this module: it is how dreamd paints a row of its *own page*, so it is
  CSS (`body.chrome-fade`) and the native window knows nothing about it. It is
  also therefore the one window setting a repo may set — it takes no button away
  and walking to another repo undoes it.
- `menu` — the native menubar, and the only module with two whole `build`
  implementations. muda exposes every `PredefinedMenuItem` on every platform but
  its GTK backend silently *drops* all but Separator/Cut/Copy/Paste/SelectAll/About
  and custom items, so the macOS bar would render two empty submenus on Linux;
  Linux gets File/Edit/Help instead. Only custom items register a real GTK
  accelerator, which is why Open Folder is `Ctrl+Shift+O` there — `CmdOrCtrl+O`
  would be consumed before the webview saw it and `keymap.toggle_stack` would
  simply stop working. Whether the bar exists at all is `ui.menubar`, and
  `main.rs`'s `apply_chrome` owns the mechanism: **attached and detached, never
  shown and hidden.** tao turns `Window::show` into `gtk_widget_show_all`, which
  re-shows a hidden menubar, and `show` is queued through tao's request channel
  while `hide_menu` runs inline — so no ordering inside `.setup()` wins. Hence
  `menubar_at_launch` deciding whether `.menu()` is called at all, and
  `set_menu`/`remove_menu` for the live toggle. Detaching takes the accel group
  with it; that is the honest cost and the root field is the way to open a
  folder without the bar.
- `webkit` — one env var, set at the very top of `main` before any thread
  exists. WebKitGTK's DMA-BUF renderer allocates through GBM, which fails on the
  NVIDIA proprietary driver; on Wayland the malformed `wl_buffer` is a *protocol*
  error, so the compositor drops the connection and GDK aborts with `Error 71
  (Protocol error) dispatching to Wayland display` before the window exists.
  Nothing can catch it — the failure is inside GTK init, so the fix has to be in
  the environment beforehand. Detection is a probe of `/proc/driver/nvidia/version`
  rather than a `#[cfg]`, so the module builds and runs the same everywhere and
  answers "no" on macOS and on Mesa, leaving the accelerated path alone. An
  existing `WEBKIT_DISABLE_DMABUF_RENDERER` always wins, including `=0`.

**Platform surface.** After Linux became a shipping target the whole
`#[cfg(target_os = "macos")]` surface is five things: `menu::build`'s two arms,
`chrome::set_titlebar`'s two arms (one of which is empty on purpose),
`trash_context`'s `DeleteMethod::NsFileManager`, and `DEFAULT_SHELL` in both
`pty` and `agent::claude` — the two modules that spawn one. `config`'s
`TITLEBAR_DEFAULT` and `TITLEBAR_FADE_DEFAULT` are cfg'd too but are *values*,
which is the point of them: no code path forks on either.
Everything else — including `adopt_root`, which carries the config reload,
re-walk, watcher re-arm, marks flush and socket retirement — compiles on both, on
purpose: gating it was what kept it from ever being built off macOS.
`config_dir()` already resolved to `~/.config/dreamd` on both platforms
(`dirs::config_dir()` is deliberately its last resort), so the socket, the marks
file and the themes directory need no per-platform anything. `webkit` is the
pattern to copy for the next such quirk: a runtime probe of something only the
affected system has, not a `cfg` arm nobody else compiles.

**`#workspace` is a 2×2 grid and the sidebar spans both rows.** `#titlebar` is
its child, in column 2 — not a row above it — so the tree reaches the *top* of
the window and the bar spans only the document beside it. That is what puts the
macOS traffic lights inside the tree when it is open: `#sidebar-lights` is 38px
of the sidebar's own background holding the corner, and `#titlebar` takes the
78px gutter back only under `body.nav-collapsed`. Exactly one of the two owes
the lights room at any moment, and the pair of rules is written so they cannot
disagree. All three children are placed by explicit `grid-area`, which is what
turned `body.view-mode #workspace` from a measurement into a rule — auto-placement
used to drop `#main-wrap` into the zero-width track and lay the document out at
width 0.

**The fading bar moves the scroller, not the wrapper.** `ui.titlebar_fade` makes
`#titlebar` transparent and puts a scrim on a `::before` that hangs 26px below
it, masked along a gradient; `#content-scroll` is pulled up under it with
`margin-top: -38px` and given the same number back as padding. Only the scroller
moves — `#main-wrap` stays in row 2, which is what keeps the agent pane, the find
bar, the stack panel and the outline card out from under a bar none of them
should be under (moving `#main-wrap` was the obvious version and put the pane's
header behind the scrim). The scrim is a pseudo-element because a mask applies to
an element's *children*: masking `#titlebar` would fade the buttons with it. The
mask carries the translucency as well as the fade, so no `color-mix` is needed to
derive a see-through `--bg` — that matters, `color-mix` is Safari 16.2 and
`minimumSystemVersion` is 10.15. A consequence: the scroller is 38px *taller*
than `#main-wrap` in this mode, so a geometry assertion meaning "full height"
must measure against the wrapper.

**Zoom is one custom property, and the chrome is not in it.** `--zoom` on
`<html>` (inline, so it outranks any palette's `:root`) times two inline
`calc()`s on `#content` — `font-size` and `max-width` — plus one rule for
images. Inline on `#content` because theme.css sets `font` there as a shorthand
and is injected *after* index.html's stylesheet, so a rule there loses to every
palette; and written as calcs over the variable so a pinch is one property write
rather than a walk of the document. Images are the one thing in a rendered
document sized in device pixels rather than `em`, which is why `measureImage`
records `--img-w` per image and the `[data-w]` rule multiplies it — without that
the diagram the reader zoomed in for is the only thing that would not have
grown. The three ways in (trackpad, `Cmd`/`Ctrl` `+`/`−`/`0`, the pill and the
settings field) all land in `applyZoom`; the keys are claimed **above** Escape
and above the overlay guard, because a panel you cannot read is the case for
zooming rather than a reason to be denied it. The wheel and gesture handlers are
on `window` and preventDefault unconditionally: a pinch WebKit handles itself
zooms the whole webview, chrome included, and dreamd has no control that undoes
that. The image viewer (`#lightbox`) claims the same keys while it is open and
zooms the image instead; `@media print` sets `--zoom: 1 !important`, an author
`!important` being the one thing that outranks the inline style.

**Do not add `backdrop-filter` to that bar.** Preview and the Claude desktop app
blur what passes under theirs, and it was tried here: **+38% renderer main-thread
time per scroll** (282ms -> 392ms, 30 wheel ticks, 2MB mixed corpus), arms
interleaved as a `body.chrome-fade` toggle inside one page so the machine could
not favour either, and the two sets did not overlap. `blur(6px)` cost +37.5% —
the price is the per-frame backdrop readback, not the radius. The masked scrim
alone measured -3.2%, i.e. free. Chromium-relative, like everything from
`perf/harness`. Note the method: `perf/run.sh quick` could not answer this,
because a loaded machine moved the *Rust* benches 10–15% on a CSS-only change and
contaminated every row — a one-page interleaved A/B is what a Chromium row is
worth trusting on.

**The pop-out is one body in two containers, not two views.** `#agent-body` is
*moved* between `#pty-pane` and `#agent-card` — `raisePopout` / `dockAgentBody`
in app.js — so a mid-stream turn keeps streaming across the move and an
unanswered permission card is still answerable on the other side. Duplicating
the subtree would mean two logs to keep in step and two ids for every element in
them. Consequences a change here has to keep: every rule styling the
conversation is scoped by *container* (`#pty-pane.native …` / `#agent-popout …`)
rather than written as its own class; `#pty-mcp` travels with the body, so
`paintMcpStatus` toggles `mcp-warn` on **both**, without which the one mode that
never opens the dock (`always`) could never show the warning; and exactly one
container may hold it, which is why `openPane` lowers the card and `raisePopout`
closes the dock rather than either leaving the other up. The card has **no
header at all** — the dock's status text has nowhere to go there, so
`setPaneStatus` also keeps the string on `pty.status` for the hint line to read,
and a session that cannot start says why instead of saying "starting" forever.

**Highlight anchoring is the subtle part.** A quote is located in the *source* by
`markdown::locate` in three tiers, the first two alternatives rather than a chain: with
context, exact `prefix+quote+suffix`; without it, the exact quote alone. A quote carrying
context that misses tier 1 skips tier 2 and drops straight to tier 3, the
whitespace-stripped match — an exact search would take the earliest occurrence while
ignoring the very context saying the quote came from a later copy. The frontend sends what
`getSelection().toString()` returns — **rendered DOM text**, never raw source — so the
whitespace-normalized path is the hot one and the only realistic thing to benchmark.
`reanchor_file` re-runs this on save; failure
marks the highlight `Stale` rather than dropping it — **but only if it ever
anchored.** A quote spanning inline markdown (`**bold**`, a link) is DOM text
`locate` cannot find in the source at any tier, so `add_anchored` keeps it at
`(0, 0)`; those marks used to go `Stale` on the first re-anchor of an *untouched*
file, which since marks read off disk are all re-anchored on first sight meant a
red "? still pertinent" every session. Hence the `line_start > 0` guard, which
makes `Stale` exact rather than approximate: past it, `locate_near` is
deterministic in its inputs, so a quote that resolved against these bytes
resolves again. The frontend twin of the same rule: a placement failure
**claims nothing** — `reanchor_file` is the only thing entitled to say a mark is
stale, and the rail has no other tenant.

**Placement is a third thing, separate from anchoring, and it spans nodes.**
`wrapByWalk` and `locateInNodes` both look inside a *single* text node, so the
same cross-markdown quote they cannot anchor they also could not draw: it painted
once at creation (`wrapRange` on the live selection, which spans elements
happily) and then vanished on the next repaint — a mark still in the store, still
counted by the stack badge, invisible. `placeAcrossNodes` is the fallback both
placers hand their misses to, and it wraps **one `<mark>` per text-node slice,
sharing the id**, rather than one `<mark>` around the range: `surroundContents`
throws across an element boundary and `extractContents` would re-parent the
`<strong>`'s contents to draw on them. `data-run` (`start`/`mid`/`end`) squares
the interior edges so `mark.hl`'s radius and padding do not make one phrase read
as three. Anything consuming a mark must therefore tolerate several per id —
`clearHighlights` and `deleteHighlight` both use `querySelectorAll`.

**Resize is a mode, not a modal**, and it is the only reason `#resize-hint`
exists. What it waits for is a selection in the document, which is exactly the
gesture a modal cannot be open for — so the annotation modal's Resize button
closes it, `armResize` marks the `<mark>`s and the hint bar takes the modal's
place until Enter (or the highlight key) commits or Escape cancels. Escape ranks
below every overlay and above view mode; `clearHighlights` ends the mode, because
a repaint pulls the marks out from under it. The commit re-checks overlap
excluding the mark itself — a resize that swallowed a neighbour would recreate
the unreachable stacking the refusal exists to prevent. The stack panel's `⤢`
opens the pair's file, scrolls the mark into view and arms the same mode, which
is what makes a queued pair resizable without hunting for the passage. All of it
is asserted in `ui-check.mjs`, against a stub store — the DOM decides overlap, so
a harness that only counted IPC calls would assert nothing.

**`prior` means "done with", not "read off disk".** `marks_file::admit`,
`Store::mark_sent` and `Store::remove_from_stack` all set it; `set_annotation`
alone clears it. So the fade tracks the stack — bright on it, faded off it — and
annotating a faded mark brightens it and re-enqueues the pair in one step. The
frontend has to repaint for the reader to see either direction, which is why the
pop handler and `saveAnnot` call `repaintHighlights` beside `refreshStack`.
`doc_from` still strips the flag on the way to disk: `admit` makes everything
prior on the way back in, so writing it would only let a hand-edited file assert
a fade the reader's history does not support.

Marks read off disk are re-anchored
**lazily, once per file, in `get_highlights`** — never all of them at
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
  a **macos-14 / ubuntu-22.04 matrix** for every push and PR, then `config_check`,
  `theme_check`, `mcp_check`, `marks_check`, `agent_check` and `locate_check` (the last against a
  cached corpus) on both, plus `node --test ui/paths.test.mjs` and `ui-check.mjs`
  in a separate ubuntu job, plus a `launch` job that runs `packaging/smoke.sh`
  against a `--features perf` build under Xvfb — the only thing in CI that starts
  the program, and the answer to "it compiled, tested green and aborted inside
  GTK init". Two arms, with and without `WEBKIT_DISABLE_DMABUF_RENDERER`; the
  accelerated one is `continue-on-error` because a hosted runner has no GPU and
  a failure there is a statement about llvmpipe. The Linux arm is what keeps the
  untaken `cfg` arm from rotting — that is exactly what happened to the Linux one while CI was macOS-only.
  Run those harnesses locally before pushing — CI is the backstop, not the first
  check. The toolchain is **pinned** (`dtolnay/rust-toolchain@1.97.1`); bumping it
  is a deliberate commit that also clears whatever the new clippy found.
- **The perf workflow is out of tree.** `perf.yml` lives in the private notes
  repo at `notes/workflows/perf.yml` and runs nowhere by default; copy it to
  `.github/workflows/` to use it. What it does, when it runs: the quick tier and
  an unsigned `packaging/build.sh` on **both** platforms. Its *numbers* gate
  nothing and move no baseline — a shared runner is not a quiet machine — but its
  packaging jobs do fail the workflow. Dispatch it with
  `compare_ref` set for a same-runner A/B; the `package-smoke` job is the real
  value, because a tagged release is frozen and a broken Linux arm found at tag
  time cannot be re-run. `install-smoke` is the half package-smoke cannot do: it
  takes the Linux bundles built on ubuntu-22.04 and *installs and launches* them
  in `ubuntu:24.04`, `debian:12`, `archlinux:latest` and `fedora:latest`
  containers — the deb through `apt-get install ./`, so the bundler's `Depends`
  is checked by being resolved; the tarball at `usr/bin/dreamd`, the path
  `install.sh` and the PKGBUILD both take; and the AppImage on every arm. That
  is what turns the glibc 2.35 floor from an argument into an assertion. It does
  **not** check the AppImage's self-containment: every arm has webkit installed
  by the time it runs, so a bundle that quietly stopped carrying its libraries
  would still pass.
- `.github/workflows/canary.yml` builds, tests and bundles on an
  **`archlinux:latest` container**, weekly. Every other Linux runner is
  `ubuntu-22.04` — pinned for the glibc 2.35 floor, and therefore the exact
  inverse of a rolling development box: 22.04 emits no `SHT_RELR` so the
  `NO_STRIP` failure cannot happen there, and its webkit2gtk is generations
  behind. This job is the only thing that asserts `NO_STRIP=1` is still
  sufficient and that a newer webkit still compiles. It needs
  `APPIMAGE_EXTRACT_AND_RUN=1` on top, because linuxdeploy is an AppImage and a
  container has no FUSE. **Scheduled, not per-push, and it gates nothing** — the
  failure it watches for is "the distro moved", not "this commit broke it", so a
  red canary is a message about Arch before it is one about the tree. There is a
  third reading, and its first two runs (2026-07-28) were it: not "Arch moved"
  but "the *container* lacks something a real Arch install has". linuxdeploy's
  gtk plugin `cp -r`s `/usr/lib/gdk-pixbuf-2.0/2.10.0` and exits 1 when it is
  absent — and no package declares that directory, so a container built from the
  README's dependency line does not have it while every desktop does. It failed
  with the same bare `failed to run linuxdeploy` the `SHT_RELR` problem produces,
  which is exactly why `VERBOSE=1` is set: the two are indistinguishable without
  it. Rust is
  pinned to 1.97.1 like `ci.yml`, leaving the system libraries as the only
  moving part. It also *launches* twice — `smoke.sh` against a `--features perf`
  build, which is the only place a rolling webkit2gtk is asked to bring a window
  up rather than merely to link, and against the bundled AppImage as shipped. It
  does *not* cover the NVIDIA/Wayland crash `webkit` works around: no hosted
  runner has the driver or a compositor, and that stays a hand-check.
- Repeatable flows become skills in `.claude/skills/`.
- Performance is measured, not guessed. `/perf-quick` (~60s) after an edit,
  `/perf-pass` (~5min) before a large commit touching `src-tauri/` or `ui/` (do `/perf-quick` for smaller commits), `/perf-deep`
  (~20min) to profile or move the baseline only on user request. The baseline
  changes only via `perf-deep`, alongside the change that justified it.
  **The baseline is not in this repo.** It is one machine's numbers, so it is
  working material, and it lives at `notes/perf-baseline.json`. `run.sh` resolves
  `$DREAMD_PERF_BASELINE`, then `notes/perf-baseline.json`, then a gitignored
  local `perf/baseline.json`, and **a missing one is not an error** — the tier
  runs, `perf/results/` is written, the comparison is skipped with a line saying
  so, and the exit status is the tier's own. That is what a clone without `notes/`
  gets, and nothing tracked here may assume otherwise.
  **The baseline is also macOS-only and `run.sh` enforces it**: off Darwin no
  comparison is made and `--update-baseline` is refused. To detect a regression on
  Linux, A/B two trees on the same machine — a diff against one arm64 Mac's numbers
  is noise wearing a regression's clothes.
- Numbers from `perf/harness/` are Chromium, **not** WKWebView — relative regression
  signal only. Say so whenever quoting one. `perf/harness/ui-check.mjs` is the exception:
  it lives there for the Playwright install, asserts on DOM and IPC rather than timings,
  and feeds no baseline.
- **A nightly job sweeps one area of the repo per night** — `/upkeep`, driven by a
  Routine at 02:17 UTC. Fifteen areas on rotation, so each is swept about
  fortnightly; the area due next is the stalest row in `.claude/upkeep/ledger.md`.
  Each sweep does three passes over its area — verify CLAUDE.md's claims against
  the code, simplify, then fix the drift and tighten that section — and the first
  is the valuable one: a false claim here misleads every session that reads it.
  Four things make it safe to run unwatched. The *code* lands on a
  `claude/upkeep-<date>` branch as one PR and **never on `main`**, which is the
  one place this repo's straight-to-main rule doesn't hold, because nobody is
  watching. `ui/app.js` is **propose-only** — 5,700 lines the harnesses can't
  prove the paint of, so those areas write one file to `.claude/upkeep/findings/`
  on that same branch and change no code. An empty night is a **success**: a job
  that must produce a diff churns code to justify itself. And the ledger commit
  lands on `main` on its own even when the PR doesn't, so an unmerged review
  can't stall the rotation into re-sweeping the same area nightly — one file, and
  the only exception to the sentence above. **The job runs in a cloud checkout of
  this repo alone**, which is why both its outputs are tracked here rather than
  in the private notes clone the local sessions have: `notes/` is not there, and
  a job that stopped on its absence would never sweep. It never runs a perf tier
  — the baseline is in `notes/` and `run.sh` refuses comparison off
  Darwin anyway — and flags in the PR when a measured path needs `/perf-quick` on
  your own machine.

## Docs

**Two repos.** `bongofongo/dreamd` is public and holds the product and the code
that proves it works. The working material — the session log, the plans and idea
backlog, the human-facing brief, the perf baseline — is *about working on* the
project rather than part of it, and lives in the private
`bongofongo/dreamd-notes`, cloned inside this tree at `notes/` and gitignored
here. Deliberately an independent clone, **not a submodule**: a submodule would
publish a pointer to a private repo in a public tree.

The hard rule that follows: **nothing tracked in dreamd may require `notes/` to
exist.** A fresh clone has no `notes/`, and `cargo build`, `cargo test
--all-features`, `node --test ui/paths.test.mjs`, `perf/corpus/gen.mjs +
locate_check` and every tier of `perf/run.sh` all have to work without it. A file
under `notes/` may be *referenced* in prose; it may not be *read* by a build, a
test or CI.

In this repo:

- `CLAUDE.md` — this file. The architecture doc, and the best one here.
- `CONTRIBUTING.md` — the straight-to-main convention and what to run before a PR.
- `README.md` — the user-facing feature list, install and known limits, and the
  one doc that is also a *claim about the code*: every keybind default, config
  key, CLI flag and theme name it prints is checkable, and several had drifted
  before anyone checked. Change it in the same commit as the default it documents.
- `perf/README.md` — what each performance tier measures and how much to trust it.
- `website/CLAUDE.md` — the public site at `fongo.uk/dreamd`. A standalone Astro
  project, deployed separately; source of truth for everything under `website/`.
  Nothing there touches the Rust build.
- `src-tauri/icons/README.md`, `ui/vendor/README.md`, `packaging/SIGNING.md` —
  the three narrow runbooks, each next to what it describes.
- `.claude/upkeep/ledger.md` and `.claude/upkeep/findings/` — the nightly
  Routine's two outputs; see Working practices. Tracked here because that job's
  checkout is this repo and nothing else.
- `testdocs/images.md` — the hand-verification document for image rendering and
  document zoom, with `testdocs/images/` beside it: one generated source at
  seven sizes, aspects and JPEG qualities, plus four references that must
  *fail*. Tracked, and the only committed images outside `src-tauri/icons/`,
  because a fixture a fresh clone does not have is a check nobody runs. Not
  shipped — the bundle is `frontendDist`, which is `ui/`.

In `notes/`:

- `notes/session-log.md` — running session log, **newest section first**. Written
  by the `/wrap-up` skill at the end of a session.
- `notes/project.md` — the human landing page: a 2–3 page plain-language brief
  written for an entry-level reader, ending with "Recent updates". Refreshed by
  the `/update-project-doc` skill, **by hand**: the daily Routine that used to do
  it is disabled, because a cloud checkout has no `notes/` and the job's own
  target moved out of the public tree with the split. If a session materially
  changes the project story, update it in that session rather than waiting.
- `notes/plan.md` — original design intent. Historical; don't rewrite it.
  `notes/agentic-direction.md` and `notes/overnight_plan.md` read the same way:
  the reasoning behind work that has since shipped.
- `notes/plans/`, `notes/ideas/{done,hold}/`, `notes/bugs/` — per-feature design
  notes and the idea backlog, by state. `notes/bugs/` is where raw notes are
  filed before triage, and is not either log below.
- `notes/bugs.md` and `notes/patch-log.md` — two newest-first logs with a real
  split: `bugs.md` is for a bug that reached a *release artifact* (symptom,
  cause, what would have caught it), `patch-log.md` for a between-release
  one-change repair that would otherwise leave no trace but a commit subject.
- `notes/todo.md`, `notes/todo2.md` — queues, not logs. An item is **deleted**
  when it lands; the history is `session-log.md`'s job. A queue nobody clears
  reads as a list of things that don't work.
- `notes/perf-baseline.json` — the perf reference numbers; see Working practices.
- `notes/workflows/perf.yml` — the perf workflow, parked rather than run.

Keep this CLAUDE.md terse and machine-facing — human-facing guidance belongs in
`notes/project.md`.
