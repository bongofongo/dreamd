# todo 2 — platform divergence (Mac / Linux)

Follow-up planning pass, right after the settings/theming work landed
(PR #1, merged). Same rules as `docs/todo.md`: a queue, not a log — clear
items out as they land, see `docs/session-log.md` for history.

## What I want to build

- [x] Start letting the Mac and Linux builds diverge a little — not a fork,
      just: same engine and almost all functionality shared, but each target
      gets its own small UI tweaks and its own performance tuning. Baseline
      feel should stay close to identical on both; the divergence is additive
      polish, not a different app.
      *Landed 2026-07-27, as the base rather than the polish: the binary now
      compiles and packages on Linux, CI is a two-platform matrix, and the
      first real divergence is `menu::build`'s two arms — forced by what muda's
      GTK backend will actually draw, not chosen.*
- [ ] Mac: a settings option to remove the traffic-light window controls
      (close/minimize/fullscreen) entirely.
- [x] Mac: make the top bar semi-transparent, Preview-style — content shows
      through/behind it rather than sitting on a flat panel.
      *Landed as `ui.titlebar_fade` (`body.chrome-fade`), on by default on macOS.
      Not Preview's blur, deliberately: `backdrop-filter` was measured at +38%
      renderer main-thread time per scroll and dropped. What ships is a scrim of
      the page background masked out along a gradient, which measured free — see
      the note in `CLAUDE.md`.*
- [ ] Mac: once the top bar is the semi-transparent style, let it be toggled
      off completely, but only as a package with the traffic lights also
      being off — no title bar and no window buttons, content edge-to-edge.
      Another settings-panel toggle.
- [x] Whatever this turns into on the Mac side, keep day-to-day development
      streamlined across both platforms — a contributor working on the
      "engine" (fs walk, search, markdown, annotations, send) shouldn't have
      to think about which OS they're on at all.
      *Landed 2026-07-27. Same commands on both, and the two-platform CI matrix
      is what enforces it — see the note below on how this quietly goes wrong.*

## Deliberations — how this should actually work

**Don't fork the codebase or the build.** `cargo build`/`cargo tauri build`
already produces a Mac binary when built on Mac and a Linux binary when built
on Linux from the *same* source tree — that's what `cfg(target_os = "...")`
is for. "Diverging binaries" doesn't need two crates, two CI pipelines, or a
second `[[bin]]` in `src-tauri/Cargo.toml`; it needs a handful of code paths
that only compile/run on one target. The risk to actually manage is the
opposite one: keeping those paths from spreading through `main.rs` and
`app.js` until every change requires "did I remember both platforms?"
Isolate, don't sprinkle:

- Rust side: a `src-tauri/src/platform/` module with `mac.rs`, `linux.rs`,
  and a `mod.rs` that re-exports the right one behind `#[cfg(target_os =
  "macos")]` / `#[cfg(not(target_os = "macos"))]`. Anything Mac-only (traffic
  light visibility, vibrancy setup) lives there, called from one or two spots
  in `main.rs` — not conditionally compiled inline all over the file.
- JS/CSS side: the codebase already has exactly the right seam for this —
  `ui/app.js:69` sets `document.body.classList.add("mac")` off a UA sniff,
  and `ui/index.html`'s `body.mac #titlebar` rule is the one existing example
  of a platform-conditional style. Keep using that pattern (maybe promote the
  sniff into a tiny `ui/platform.js` once there's more than one or two rules)
  rather than branching logic throughout `app.js`.

**Window chrome (traffic lights, title bar) is mostly a config problem, not
a code problem.** Tauri 2 supports per-platform config overlays —
`tauri.macos.conf.json` / `tauri.linux.conf.json` next to `tauri.conf.json`,
merged in automatically for that build target. `tauri.conf.json` today sets
`titleBarStyle: "Overlay"` and `hiddenTitle: true` globally for every
platform, but `titleBarStyle` is a macOS-only concept to begin with — on
Linux it's presumably a no-op already. Splitting the window config so
Mac-specific chrome lives in `tauri.macos.conf.json` (and Linux gets its own,
plainer decorations) is lower-risk than adding runtime branching, and is the
"native" way Tauri expects this kind of divergence to be expressed.

**The runtime toggles are the genuinely new work, and they're Mac-only
engineering:**

- *Hiding the traffic lights* isn't a window-config flag — Tauri 2 doesn't
  expose "hide the standard buttons" as JSON config. It means calling
  `NSWindow.standardWindowButton(_:)` and setting `.isHidden` via `cocoa`/
  `objc2` from Rust, behind `#[cfg(target_os = "macos")]`, driven by a new
  `#[tauri::command]` the settings panel calls when the toggle flips. This
  is real, narrow platform code — a good first candidate for
  `platform/mac.rs`.
- *Semi-transparent title bar* (the Preview look) needs `transparent: true`
  on the window plus an `NSVisualEffectView` behind it for the frosted-glass
  effect — the `window-vibrancy` crate is the standard way to do this in a
  Tauri app and supports macOS (and separately, differently, Windows).
  There's no real Linux equivalent — window transparency/blur on Linux is
  compositor-dependent and unreliable across window managers, so Linux
  staying opaque isn't a missing feature, it's the correct baseline. This is
  the clearest example of "same engine, different polish per target."
- *Fully hiding the bar* (title bar + traffic lights together, content
  edge-to-edge) composes the two above: `decorations: false` /
  `transparent` window plus the traffic-light-hiding call, gated together by
  one settings option rather than two independent ones, since the todo above
  frames it as a single packaged toggle.

**Performance divergence** likely doesn't need special build machinery
either — `[target.'cfg(target_os = "macos")'.dependencies]` in
`Cargo.toml` (same section syntax already implicit in how `tauri` itself
pulls platform backends) covers OS-specific deps, and the existing `perf`
feature (`src-tauri/Cargo.toml`) is the existing precedent for opt-in,
feature-gated code if a given optimization needs to be switchable rather than
always-on for one OS. Measure any claimed optimization the same way as
everything else in this repo — `perf-quick`/`perf-pass`, not guesswork —
before believing it actually helped, per the perf tenet in `CLAUDE.md`.

**Where this can quietly go wrong:** a Linux-only dev environment will never
compile-check the `#[cfg(target_os = "macos")]` code paths day to day, so a
break there can sit unnoticed until someone builds on Mac. Worth deciding
before this work starts, not after: either a macOS CI job that builds (even
if it can't run a full GUI test), or a habit of explicitly building on both
platforms before merging anything that touches `platform/`. Keeping the
platform-specific surface small (a couple of files, not scattered `cfg`s) is
what makes that check cheap enough to actually happen.

*Settled 2026-07-27, and the prediction was right in the mirror image: it was
the **Linux** arm that rotted, because CI was macOS-only. `cargo build` had been
failing on Linux with five errors since `docs/session-log.md:1189` recorded it.
The answer is a `matrix: [macos-14, ubuntu-22.04]` on ci.yml's `rust` job running
identical steps, plus `perf.yml`'s `package-smoke` for the release path. Note
also that the `src-tauri/src/platform/` module this file proposes was not needed
for the port — the whole cfg surface came to three items (`menu::build`'s arms,
`trash_context`, `pty::DEFAULT_SHELL`), each already isolated in the module it
belongs to. Introduce `platform/` when the Mac-only window chrome above lands
and there is actually something to put in it.*
