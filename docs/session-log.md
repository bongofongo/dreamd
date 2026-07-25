# Session log

## 2026-07-25 — kill the white flash on boot

Single-bug thread. The reading pane painted white from window creation until the
frontend finished booting — longer on a slower start, which is exactly backwards.
Fixed in four layers, outermost first.

### What happened

1. **The cause is where `--bg` lives.** `body { background: var(--bg) }` is in
   `ui/theme.css`, and theme.css is injected into `#user-theme` from JS. So for
   the first stretch of boot the variable is simply unset and the webview paints
   its own default white. Nothing was "flashing" — the app had no background yet.

2. **`src-tauri/src/theme.rs` (new).** `resolve()` (user's theme file, else the
   bundled `DEFAULT_THEME`) and `background()`, which parses `--bg` out of the CSS
   as `(r, g, b)`. Hex only — `#rgb`, `#rrggbb`, and the 4/8-digit alpha forms, with
   `/* … */` blocks stripped first so a commented-out declaration cannot win.
   Anything else returns `None` and the caller keeps its default.

3. **The native window is painted from the *user's* theme, not a constant.**
   `main.rs` computes the colour before the builder and `setup` calls
   `set_background_color`. Hardcoding the default dark would have made a custom
   light theme flash dark — a worse bug for the person who bothered to write one
   (tenet 5). `backgroundColor` in `tauri.conf.json` is the static default covering
   the frame before `setup` runs; `get_theme_css` now just calls `theme::resolve`.

4. **`ui/index.html` carries fallbacks.** `html`/`body` get
   `var(--bg, #1b1f27)` / `var(--text, #c7cedb)`, matching the `var(--x, fallback)`
   idiom the rest of the structural CSS already uses. This is what covers webview
   first paint onward.

5. **`loadTheme()` moved to the front of `init()`** in `ui/app.js`, ahead of
   `repo_info` and `get_keymap`. Every IPC before it was time a user with a custom
   theme spent looking at the default one. Consequence for perf: the `ipc_theme`,
   `ipc_repo_info` and `ipc_keymap` marks are cumulative timestamps, so their order
   in the baseline changed — a future diff on those three rows is the reorder, not
   a cost.

### Mistakes & deviations

Ran clean. The one judgement call worth recording is that `#1b1f27` now appears in
three places (theme.css, the index.html fallback, tauri.conf.json); only theme.css
is authoritative at runtime, the other two are pre-boot defaults, and both say so
in a comment.

### State

`cargo build` clean. `theme::background` verified differentially through a throwaway
example — default theme, `#fff`, 8-digit hex, a commented-out decoy, `rgb()`, and a
`--sidebar-bg` near-miss all parse as expected; example deleted after.

`perf/run.sh pass` — the run the previous session deferred — **195 metrics compared:
0 regressed, 1 slower, 54 improved**. The single slower row is
`chromium.palette.repo5000.keystroke_ms.p95` at 0.4 → 0.5ms, sub-millisecond and
Chromium-relative, not a real signal. The 54 improvements are almost all
`chromium.highlight.*.apply_ms` (up to −92%) and belong to the anchoring commit
below, not to this change; boot ordering cannot move highlight apply. Full table:
`perf/results/pass-4138e62-20260725-012944.json`.

Not verified visually — the fix was reasoned from where `--bg` is defined and the
build, not from watching a boot. Worth one look on the next real launch.

## 2026-07-25 — highlight anchoring: 31.6% of selections landed on the wrong copy

Bug-hunting thread. The brief was a suspected off-by-one plus a "quote appears
twice" case in `markdown::locate`, estimated at 6.5% of selections. Both were
real; the estimate was low by a factor of five, because the ground truth the
estimate was measured against was itself wrong.

### What happened

1. **Built the harness first.** `src-tauri/examples/locate_check.rs` runs all 611
   corpus highlight fixtures through `locate` and exits non-zero on a wrong
   anchor. The repo had no `#[cfg(test)]` tests and the session log had named
   `locate()` as the obvious first target for one; this is it. It reimplements the
   whitespace-stripping and line lookup independently rather than importing them —
   a checker that reuses the code under test cannot catch it being wrong.

2. **The original ground truth was wrong, which hid most of the bug.** The brief
   defined truth as `locate(source, "", exact_source_quote, "")`, i.e.
   `source.find`. But the generated corpus repeats whole blocks — one sampled code
   block occurs **195 times** — so `find` returns the first copy, which is usually
   not the one the fixture was sampled from. `perf/corpus/gen.mjs` now records
   `lineStart`/`lineEnd` at sample time (GENERATOR bumped to 3; quote, rendered,
   prefix and suffix bytes are unchanged, so bench inputs are identical). Against
   the recorded origin the real pre-fix failure rate is **193/611 = 31.6%**, not
   6.5%.

3. **Class (b), the off-by-one, was tier 2's bug, not tier 3's.** Tier 3 maps to
   the first and last *non-whitespace* character; tier 2's `span()` used the raw
   byte range, so a selection ending on a line break claimed the following line.
   Tier 3's answer is the right one. `locate` now trims the quote up front and all
   three tiers agree by construction.

4. **Class (a), the wrong copy, needed four separate fixes.** `ui/app.js` sent
   `prefix: ""`, so nothing could disambiguate:
   - `selectionContext()` in `ui/app.js` walks out node by node from the selection
     for 96 chars each side. Deliberately not a Range back to document start —
     `toString()` on that would build a copy of the whole file.
   - Tier 3 became context-aware. Rendered context has lost the markdown syntax
     the source still carries, so occurrences are *scored* by shared bytes either
     side rather than required to match exactly; a full match short-circuits, so
     the common case still stops at the first hit.
   - Tier 2 now runs **only** when there is no context. With context it was
     actively harmful — it took the first exact hit while ignoring the context
     saying otherwise. Worth 10 of the residual failures.
   - `str::match_indices` was replaced by an overlapping-aware `occurrences()`.
     Non-overlapping iteration cannot see a match that begins inside the previous
     one, and periodic text (a repeated config block) is exactly where the right
     copy hides. The last 10 failures.

5. **A position hint settles what context cannot.** Two byte-identical copies of a
   block have byte-identical context; no anchor can choose between them. But a
   re-anchor knows where the highlight was a moment ago. `SourceIndex::locate_near`
   takes the previous line, `Store::reanchor_file` passes it, and among full
   context matches the nearest to the hint wins. One pass, early-exit once it has
   bracketed the hint.

6. **Kept the bench row names, corrected the story.** `benches/locate.rs` led with
   "context is the obvious-looking fix that **does not work**" — true about
   *speed*, and it reads as an argument against the correctness fix. Rewritten.
   The row ids (`today`, `with_context`, …) were left alone so the committed
   baseline still lines up, with a note that `today` is now the pre-fix reference.
   `seed_highlights` in `main.rs` now seeds collapsed context, since that is what
   the app sends.

### Mistakes & deviations

- **Trusted the brief's ground truth.** The first harness reproduced 40/611 as
  advertised, then the fix appeared to make things *worse* (96 mismatches). It
  hadn't: the "correct" answers were wrong. Caught by dumping every occurrence of
  one failing quote and finding 195 of them. Cost a rebuild of the fixtures and
  the harness, and was the single most useful thing in the session — the real
  failure rate is five times what was reported.
- **First hint implementation started the scan at `hint − 4096` bytes and took the
  first full match.** Two bugs: it returned the first match rather than the
  *nearest*, and its "don't rescan from the top" shortcut silently discarded the
  hint for any highlight near the top of a file. It also needed a second pass when
  no full match existed, which is the common case in real markdown. Replaced with
  a single hint-aware pass. 74 highlights still moved on re-anchor before this;
  0 after.
- **`is_none_or` broke MSRV.** Stable since 1.82, `rust-version` is 1.77. Caught by
  `cargo clippy`, not by the build.

### State

`cargo build` clean, `cargo clippy --all-targets` clean apart from one pre-existing
`while_let_loop` in `render`. `locate_check`: 611 fixtures, **0 wrong** on first
anchor, **0 moved** on re-anchor, 0 batched-vs-one-shot disagreements. 146 remain
*ambiguous* — another copy exists where quote and context both match in full, so no
anchor built from that evidence can choose; that is a synthetic-corpus artifact and
re-anchoring resolves all of them via the hint.

`perf/run.sh quick` after the fix: 49 metrics, **0 regressed, 11 improved**;
`locate_single/with_context`, the path the app is now on, 8.23 → 6.66ms. Caveat: the
committed baseline predates the perf commit already in the tree, so that run confirms
no regression rather than isolating this change. **`perf-pass` was skipped at the
user's request** — it should run before the next commit touching `src-tauri/` or
`ui/`.

## 2026-07-25 — the first optimization pass, measured end to end

The measurement framework built last session finally got used for what it was for.
Worked the ranked fix list plus a fresh audit of the hot paths; landed six changes
across Rust and the frontend. Final `perf/run.sh pass`: **0 regressed, 53 improved**.

### What happened

1. **`reanchor` no longer rebuilds its index per highlight.** `markdown::locate`'s
   third tier built a whitespace-stripped copy of the entire source plus an offset
   table, and threw it away again — *once per highlight*, against documents that can
   be megabytes. New `markdown::SourceIndex` builds it once per `reanchor_file` call
   and lends it to every highlight in the file. Two further scans went with it: line
   numbers now come from a prefix-summed line table (`partition_point`) instead of
   counting newlines from byte 0 twice per highlight, and the stripped index is keyed
   by *byte* rather than char index, which removed a `chars().count()` over the whole
   document per lookup. `reanchor_today/100` 702.7ms → 107.8ms; `locate_single/today`
   improved too (8.08 → 6.85ms), so nothing was traded away to get it.

2. **Fenced code blocks are highlighted in parallel.** Syntect is essentially all of
   render cost — `render/code/2m` was 3033ms against `render/prose/2m` at 4ms — and
   the blocks are independent of each other. They're now collected during the parse
   pass, highlighted across the cores with `std::thread::scope` and an atomic index
   (work-stealing, because block sizes vary hugely), then spliced back into their
   event slots. No new dependency. `render/code/512k` 779.6 → 176.3ms,
   `render/mixed/512k` 130.0 → 33.8ms. Output verified **byte-identical** on all four
   corpus variants before the benches were believed.

3. **The watcher debounces and coalesces (fix B2).** 60ms window, per path. The
   subtlety: kinds are *accumulated*, not overwritten, and resolved against the
   filesystem at flush — an editor that saves by writing a temp file and renaming it
   over the original emits a remove *and* a create for a file that still exists, so
   collapsing to "removed" would have blanked the open document. `events_per_save`
   1.583 → **1.00**.

4. **The startup double walk is gone (fix B4).** `main` builds the tree from the
   paths it already walked for the search index and caches it in `AppState`;
   `list_markdown_files` returns the cache, and `rebuild_index` returns the fresh
   tree so the frontend stops asking for a second walk on every add/remove. Also
   dropped a `stat` per repo entry: the walker filtered on `into_path().is_file()`,
   which discarded the `DirEntry`'s already-cached `file_type()` and issued a fresh
   syscall for every entry — directories included — *before* the cheap extension test
   could reject it. `walk_startup_pair/5000` 118.7 → 57.4ms.

5. **`applyHighlights` stopped re-walking the document per highlight.** It built a
   fresh `TreeWalker` from the top of a 105k-node document for each one. Now one pass
   flattens the document to a string plus an offset table, quotes are found with a
   native string search, and the wraps are applied back-to-front so splitting a text
   node can't invalidate an offset computed after it. Kept a walk-and-stop path below
   five highlights — flattening costs ~4ms whether there is one quote or five hundred,
   and the measured crossover is around five. Chromium apply at 500: 284.9 → 23.9ms.
   Applied-counts match the baseline exactly, so behaviour is unchanged.

6. **Frontend odds and ends.** The active tree item is tracked by reference instead of
   toggling a class on all 5000 file nodes per file open; palette arrow keys move a
   class instead of rebuilding 200 rows (2.10 → 0.60ms); `runPalette` got a monotonic
   sequence guard so a slow query can't paint over a newer one; `escapeHtml` does one
   regex pass instead of four; the capture-phase scroll listener is passive and
   returns immediately when no tooltip is showing.

### Mistakes & deviations

- **Two ideas were measured and rejected, not shipped.** The parallel repo walker
  (`build_parallel`) is ~45% faster on 5000 files but pays ~1.2ms of thread spawn that
  a small repo eats in full — 3.5x slower on `walk/markdown_paths/10`, which is exactly
  what a cold start on a single file hits. Kept the sequential walker with only the
  `stat` fix, which is a strict win at every size. `content-visibility: auto` cuts
  forced layout after a render by 97% but raises scroll main-thread time by 81%;
  scoping it to `> pre` was worse on both axes, and an A/B with the rule removed
  confirmed the cost was entirely its own. Scrolling is the dominant interaction in a
  reader, so it was left out — deliberately, with the numbers recorded in a comment in
  `index.html` so nobody re-litigates it blind. Worth re-testing under WKWebView.
- **A first cut of the stripped index used two offset tables and a binary search.** It
  made batch re-anchoring fast but left single `locate` calls ~14% *slower* than
  before. Rewriting it as one byte-keyed table made the lookup O(1) and turned that
  regression into an improvement. Caught because the single-call bench was read
  alongside the batch one, not instead of it.
- **Three rows were flagged as regressions and turned out to be noise.**
  `render/table/8k` (+54%), `render/table/512k`, and `locate_single/with_context`
  (+16%) all moved on a loaded machine, had no plausible mechanism in the diff — the
  table variant contains no code blocks at all — and came back flat or improved on a
  quiet re-run. The repo's own rule held: a flagged row needs a mechanism or an A/B
  before it gets called a regression.
- **Started this thread on the assumption the tree was dirty and non-building** (a
  rustc diagnostic reported `copy_clipboard` as private). It was stale; a parallel
  thread had already committed that work. Confirmed against `git log` before doing
  anything, rather than "fixing" a file that was already correct.

### State

`cargo build` passes. `perf/run.sh pass` on the shipped tree: 195 metrics compared,
**0 regressed, 1 slower, 53 improved** — the one slower row is Chromium `composite_ms`
moving 3.3 → 3.7ms across 30 wheel ticks, well inside the harness's own threshold and
relative-only anyway. Rendered HTML byte-identical on four corpus variants; highlight
applied-counts identical to baseline.

`perf/baseline.json` is **not** updated — that needs a deliberate `perf-deep
--update-baseline` run on a quiet machine, in its own commit. Until then every `real.*`
row will keep reporting as new.

**A pre-existing correctness bug was found and deliberately left alone.** Verifying
`locate` against all 611 corpus highlight fixtures showed **40 cases where a rendered
quote anchors to the wrong lines** — one resolved to lines 108–113 instead of 362–368.
Confirmed identical before and after this session's changes, so it is not a regression.
Root cause lead: `ui/app.js:419` sends `prefix: ""` and `suffix: ""`, so tier 1 of
`locate` can never fire and both remaining tiers take the *first* match in the
document. A handoff prompt for the fix was written this session. Note that populating
prefix/suffix was previously rejected as a *performance* fix — that finding stands and
does not argue against it here, since disambiguation is exactly what it is for.

Left on the table, ranked: a render cache keyed on (path, mtime, hash) to kill
re-renders of unchanged content; not holding the store mutex across `reanchor`;
raw-bytes IPC instead of a JSON-escaped 4MB string.

## 2026-07-25 — dreamd gets a public face at fongo.uk/dreamd

Built and shipped the project's first website: a single dark, picture-free landing
page live at `https://fongo.uk/dreamd`, in a new `website/` directory that is its own
Astro project and its own deploy. Nothing in `src-tauri/` or `ui/` was touched. Also
fixed a canonical-URL defect that turned out to affect autorota too, and wrote
`website/CLAUDE.md` as the source of truth for the directory.

### What happened

1. **Copied autorota's hosting pattern, and corrected the premise while doing it.**
   `website/` deploys as an assets-only Cloudflare Worker (`dreamd-web`) on the zone
   route `fongo.uk/dreamd*`, which intercepts that one path. The rest of fongo.uk is
   **paper_web on Vercel** — autorota's `wrangler.jsonc` claims Cloudflare Pages and
   is stale. There is no build-time wiring between the repos at all: no submodule, no
   symlink, no sync script, just the zone route plus a hardcoded `link` string in
   paper_web's `project_list.json`. The load-bearing trick is `base: "/dreamd"` with
   `outDir: "./dist/dreamd"` while wrangler's `assets.directory` is `./dist` — Astro's
   `base` only prefixes URLs, not output paths, so this is what lines the Worker's 1:1
   path→asset mapping up with the route prefix.

2. **Design decisions, all deliberate and recorded in `website/CLAUDE.md`:** dark only
   (no toggle, no `data-theme` — a divergence from paper_web and autorota, which are
   both dual-theme); tokens lifted verbatim from `ui/theme.css` so the site is the
   app's own colours, pushed toward near-black; Spectral **500 only** for display with
   the app's system-sans stack for prose; no images; and the app's highlight yellow as
   the page's only visual device, used three times total. Copy says "Source on GitHub"
   and never "open source" — the repo has no `LICENSE` and no `license` field in either
   `Cargo.toml`, so it is legally all-rights-reserved.

3. **The landing went through three shapes.** First a hero-scoped drifting gradient;
   then, on a brief for a Sandman-like dream aesthetic, a darker page-level field
   (starfield, indigo and violet veils on unrelated clocks, a breathing teal aurora,
   sand grains in the highlight yellow); finally scoped back to a **sticky one-screen
   landing** that the opaque `.page` scrolls up over like a curtain, so the site reads
   as two pages joined by a scroll. Star density was cut ~73% on request. All animation
   is transform/opacity only, and everything freezes under `prefers-reduced-motion`.

4. **The name became the title.** `dreamd` set large in Spectral italic, the former
   headline demoted to subhead. The corner wordmark is hidden on the landing — the name
   is already the headline there — and arrives past 70% of a viewport as the way home.

5. **`website/CLAUDE.md`** documents the wiring, the invariants, the zero-JS-bundle
   contract, the verification checklist, and every trap below. A pointer line was added
   to the root `CLAUDE.md` under Docs so the directory is discoverable from the root.

6. **Canonical URLs, in both repos.** `/dreamd` 307'd to `/dreamd/` while the page's own
   `<link rel="canonical">` was the slashless form — the canonical pointed at a
   redirect. The obvious one-word fix (`html_handling: "drop-trailing-slash"`) would
   have fixed dreamd's index and **broken autorota's subpages**, because Astro reports
   `/autorota` for the index but `/autorota/support/` for a nested route, so the
   canonicals already disagreed with each other. The real fix is three settings in
   agreement: `drop-trailing-slash` in `wrangler.jsonc`, `trailingSlash: "never"` in
   `astro.config.mjs`, and a canonical normalisation in `SiteLayout.astro`. Applied to
   both sites and both deployed; autorota's stale Pages comment corrected in passing.

### Mistakes & deviations

- **`overflow-x: hidden` on `body` silently killed the landing's `position: sticky`.**
  It makes `body` a scroll container, so the landing scrolled away instead of pinning
  and the curtain never happened. Caught by asserting
  `.landing.getBoundingClientRect().top === 0` at several scroll offsets rather than by
  looking at a screenshot. `html` does the horizontal clamp now; `body` is left alone.
- **`fullPage` screenshots flatten sticky and fixed layers**, rendering them once at the
  top. This made the whole lower page look flat and dead when it was fine, and I nearly
  "fixed" a non-problem. Switched to viewport-sized shots at explicit `scrollTo`
  offsets, which is the only way to judge these states.
- **The curtain sliced the subhead mid-glyph** — it read as a clipping bug, not a
  reveal. Added a `--wake` variable, written by the existing scroll handler, that fades
  and lifts the landing text to completion by 40% of a viewport, so the rising edge
  arrives at empty sky.
- **Shipped Spectral 400 and then cut it.** Headings are 500 and body prose is system
  sans, so 400 was two woff2 files of dead weight; noticed on the first build's preload
  list and removed, halving the font payload.
- **Trimmed a sentence of hero copy during the restructure without flagging it**, and
  the user wanted it back. Restored minus its leading "dreamd" (the title now says the
  name directly above it), and widened the measure 30em → 34em because at 30em the line
  broke with "action." orphaned and `text-wrap: pretty` did not rescue it.
- **My `cd` in tool calls left the user's shell in `perf/harness`,** so their
  `npm run deploy` failed twice with a confusing "Missing script" error. The shell's cwd
  is shared with the Bash tool.
- **The root `.gitignore` ignores `*.svg` wholesale**, which would have silently dropped
  `public/favicon.svg` and left a fresh clone unable to rebuild the site. Caught while
  staging, because the file count was one short; re-included with a negation in
  `website/.gitignore`.
- **A redirect loop that wasn't.** Immediately after the `html_handling` deploy,
  `/dreamd` and `/dreamd/` both 307'd at each other. It was Cloudflare still serving the
  old cached redirect — a cache-buster query returned 200 straight away.

### State

No Rust and no `ui/` changes this session, so **no `cargo build` gate and no perf tier
were run** — the binary is untouched and measuring it would have proved nothing.
`perf/baseline.json` not touched.

Site verified in Chromium via the existing `perf/harness` Playwright install (a plain
static page, so unlike the app's perf numbers these results are the real thing, not a
proxy): sticky pinned at `top = 0` across offsets; wordmark opacity 0 → 1 past 70% of a
viewport and clicking it returns `scrollY` to 0; landing text fits inside one screen at
1440×900, 1280×700, 375×812 and 375×667 with no clipping; no horizontal overflow at 375
or 1440; under `reducedMotion: "reduce"` zero animations running; no console errors. On
the built output: no `.js` emitted, Spectral `@font-face` and preloads present. HTML
4.47 KB gzip, CSS 1.86 KB, **0 bytes of JS bundle** (one inline scroll listener), fonts
2 woff2 / 32 KB — all well inside paper_web's budgets, which this directory follows as
design rules since it has no perf harness of its own.

Live and checked after deploy: `/dreamd`, `/autorota`, `/autorota/support`,
`/autorota/privacy`, `/autorota/tutorials` all 200 at the slashless form; slashed forms
are a single 307 hop; unknown paths 404; `/` and `/projects` still 200, so the zone route
shadows only its own prefix. Served HTML was byte-identical to the local build. The
cross-site link from paper_web's `/projects` hard-navigates cleanly despite that repo's
`<ClientRouter />` — no paper_web DOM survives, `data-landing` is set, no errors — so no
`data-astro-reload` is needed.

Pushed: `b7c4e19` (the site), `3a51203` (`website/CLAUDE.md`), `e49b101` (canonical fix),
plus the earlier `1e8cd8f` perf commit they carried along. autorota pushed separately as
`6d96125`. paper_web's card was already pointing at `/dreamd` — committed by the user as
`1159ea4` during the session.

Open, and **not** from this session: `src-tauri/` and `ui/` have uncommitted
modifications from parallel work (`benches/walk.rs`, `annotations.rs`, `fs_walk.rs`,
`main.rs`, `markdown.rs`, `watcher.rs`, `ui/app.js`, `ui/index.html`). Deliberately left
alone — staging was explicit throughout. Still open on the site itself: no `LICENSE`, so
the copy cannot claim a licence; a gallery/media tab is scaffolded for but not built (the
`nav` array in `Header.astro` is empty and wired).

## 2026-07-24 — Rust debloat pass

A pure simplification sweep over `src-tauri/`: no behavior changes, no new
features, just deleting duplication. Net −32 lines across `src/`, and one of the
deletions turned out to be worth 16–22% on the whole search path.

### What happened

1. **`fs_walk::scan` and `markdown_paths` were two copies of the same walker.**
   Each built its own `ignore::WalkBuilder`, applied the same `extra_ignores`
   overrides, filtered to markdown and sorted. `scan` is now
   `build_tree(root, &markdown_paths(root, ignores))`. The explicit
   `.git_global(true).git_ignore(true).git_exclude(true)` on the old `scan` were
   deleted because they are `WalkBuilder`'s defaults — they read as meaningful
   configuration and were not. `build_tree`'s nested `Dir`/`to_node` helpers were
   lifted to module level, `rel_of` extracted (it was inlined twice), and the
   manual `comps[..len-1]` + `.last().unwrap()` replaced with `split_last`.
   58 lines gone from that file alone.

2. **`SearchIndex` lost its `by_rel: HashMap<String, usize>`.** `Pattern::match_list`
   is generic over `T: AsRef<str>`, so `impl AsRef<str> for Entry` makes nucleo
   hand back the entry itself instead of a `&str` we then looked up again — the map
   existed only to undo that. This also fixed a latent bug: two files with the same
   `rel` collided in the map and one was silently unreachable. `node()` became
   `impl From<&Entry> for FileNode`. **This is the one change with a measured
   payoff** — see State.

3. **`is_markdown` existed three times**, verbatim, in `lib.rs`, `fs_walk.rs` and
   `watcher.rs`. One copy now, in `lib.rs`.

4. **`Store::stack_pairs` was a character-for-character copy of `selected_pairs`.**
   It's now `self.selected_pairs(&self.stack)`. Added a private `Store::find` for
   the id lookup repeated in `get` and `selected_pairs`.

5. **`markdown::locate` tiers 1 and 2 built the same `Location` twice** with
   slightly different arithmetic; extracted `span(source, start, len)`. Both
   `.unwrap()`s in `render`'s code-block buffer are gone — the guard-then-unwrap
   pattern (`Event::Text(t) if code_buf.is_some() => code_buf.as_mut().unwrap()`)
   became a match on `&mut code_buf` / `code_buf.take()`, which is the same
   semantics with the impossible case expressed as a branch instead of a panic.

6. **`send.rs`**: four `push_str(&format!(...))` calls became `write!` (no
   throwaway `String` per line); two identical tmux exit-status checks became
   `tmux_run(args, what)`; `detect_claude_pane`'s manual `splitn(3)` plus three
   `unwrap_or("")` plus a `format!` became one `split_once` inside a `find_map`.
   `copy_clipboard` was made `pub` because `main.rs` carried a second copy of it.

7. Smaller: `resolve_repo_root`'s hand-rolled parent loop → `ancestors()`;
   `map_or`, `is_ok_and`, nested or-patterns (`Some("md" | "markdown" | ...)`), and
   a dead `cfg.clone()` in `main`.

8. **Ran `cargo fmt` on the repo.** It was already failing `--check` on `main`
   before this session — four bench files and several `src/` files — which is why
   `benches/` appears in the diff of a session that changed no bench logic.

### Mistakes & deviations

The refactor itself ran clean, but the *verification* did not, twice:

- **`perf-quick` reported `reanchor_today/1` and `/10` up 6–9%, reproducibly**,
  across two runs. Reproducible is supposed to mean real. It wasn't: a direct
  criterion A/B of the stashed old code against the new gave −3.3%/−4.2%/+1.8%,
  all `p > 0.05`. The tell was mechanism — `reanchor_file` is untouched, and
  `locate_single/today` hadn't moved — plus `meta.load1` of 4.6 on 8 cores.
- **`perf-pass` then reported `render/code/512k` +14.7%, `render/code/128k` +13.1%,
  `render/table/512k` +15.2%, `walk_scan/500` +7.6%** — past the 15% regression
  line in one case. Same story: re-running the *identical new code* on a quieter
  machine gave 764ms against a 779ms baseline, i.e. −2%, and an old-vs-new A/B put
  every render and walk figure inside ±3%. Machine load, not the edit.

The rule this reinforces: on a loaded machine the tier diff is a screening test,
not a verdict. A flagged row needs either a plausible mechanism in the diff or a
back-to-back A/B against the stashed old code before it gets called a regression.

Because there are no unit tests, correctness was established by differential
testing rather than by the build: a throwaway `examples/_abcheck.rs` dumped
`render` over six real documents, `scan` (with and without `extra_ignores`),
`markdown_paths`, seven `query` cases and seven `locate` cases covering all three
tiers, and `assemble_query` over stale / multi-line / out-of-root / unannotated
highlights. Every byte identical between stashed-old and new. The harness was
deleted afterwards rather than committed — it asserts nothing, it only diffs.

Also corrected a comment my own change had made stale: `benches/walk.rs` said
`markdown_paths` and `scan` "are the two duplicated halves" and that fix B4 is
about merging them. B4 is actually about the *startup pair* — `main()` and
`list_markdown_files` each walking the repo — which this session did not touch.

### State

`cargo build` clean with and without `--features perf`;
`cargo clippy --all-targets --all-features -- -D warnings` clean; `cargo fmt
--check` clean (for the first time). `cargo test` runs **0 tests** — there are
still no `#[cfg(test)]` blocks, so it proves compilation and nothing else.

`perf-pass` measured, results in `perf/results/pass-cdf24d2-20260724-233207.json`:

- **Improved, real, consistent across every size**: `index_build` −18%/−20%/−18%
  (10/500/5000 files), `query/500` −17%, `keystrokes/10` −22%, `keystrokes/500`
  −16%. All of it the `by_rel` HashMap removal.
- **Nothing regressed.** Every slower row was disproven by A/B (above).
- The four `XX` rows were all `chromium.highlight.*.apply_ms`, which is
  `ui/app.js` — zero JS changed this session, the Chromium noise floor is 27%, and
  one of them was a 190% "move" on a single-highlight sample. Chromium-relative
  numbers, not WKWebView.

Open, and **not** caused by this session: the `pass` tier's entire `real.*` group
now has nothing to compare against. Commit `cdf24d2` renamed those metric keys
(`real.loop.h10` → `real.loop.debug-h10`, `real.startup.*` →
`real.startup.debug.*`) so the pass tier emits `debug`-prefixed paths while
`baseline.json` still holds the release-tier names — so they print as `*` (no
baseline) and their old names print under `not measured this run`. That includes
`events_per_save` and `save_to_paint_ms`, which `perf-pass` itself calls the most
product-relevant numbers it has. Needs a `perf-deep --update-baseline`, or keys
that don't encode the profile, before those rows mean anything again.

`perf/baseline.json` untouched, as required.

## 2026-07-24 — performance measurement framework

Built the three-tier performance harness (`perf/`) and captured a first baseline.
No optimizations yet — this session was about being able to prove them. It got
there, but only after the harness was caught lying five separate times.

### What happened

1. **`src-tauri` split into `lib.rs` + a thin `main.rs`.** `main.rs` keeps the CLI,
   `AppState`, the 21 commands and the builder; everything they do moved to the
   library. Code moved verbatim — no logic changed. This is what makes
   `src-tauri/benches/` and future unit tests possible at all, since a `[[bin]]`
   target can't be imported.

2. **Deterministic corpus** (`perf/corpus/gen.mjs`, node, no deps). 5,536 files /
   21MB: four document variants at 8KB–2MB, synthetic repos of 10/500/5000 files,
   highlight sets of 1/10/100/500. Seeded PRNG, byte-identical across runs; a
   3.5KB summary manifest with an aggregate digest is committed rather than 5,500
   individual hashes.

   Highlight fixtures carry both `quote` (exact source) and `rendered`
   (whitespace-collapsed). That distinction turned out to be load-bearing — see
   the findings below.

3. **Four criterion suites** — `render`, `locate`, `search`, `walk` — with sample
   counts capped per group so the deep sweep stays inside its budget.

4. **A `perf` cargo feature** (off by default, verified to compile out) emitting
   NDJSON timing marks on **stderr**, because `console.log` inside WKWebView never
   reaches the process's stdout. Rust marks plus frontend marks forwarded through a
   `perf_mark` command land in one ordered stream. Also `--bench-startup` and a
   `DREAMD_PERF_SEED` hook so the save loop can be measured at a realistic
   highlight count without driving the UI.

5. **Playwright/Chromium harness** (`perf/harness/`, test-only, never referenced by
   `tauri.conf.json`) with a stubbed `window.__TAURI__`. Scroll cost is taken from
   CDP trace events, not `performance.now()` — style, paint, raster and composite
   happen outside JavaScript, so timing a `scrollTop` assignment from inside the
   page reports approximately zero no matter how expensive the scroll is.

6. **Runner, three tiers, one entry point.** quick ~80s, pass ~6min, deep ~20min.
   Metrics are flattened to dot-paths and diffed against `perf/baseline.json`;
   adding a measurement anywhere shows up in the table with no registry to update.

7. **Three skills** — `perf-quick`, `perf-pass`, `perf-deep` — plus a perf clause in
   `wrap-up`'s gate and a line in CLAUDE.md.

### Findings (all measured, release profile)

- `reanchor` costs **~7ms per highlight** on a 2MB document — and a *live* highlight
  costs exactly what a stale one does (7.08ms vs 7.10ms), because a quote taken from
  the rendered DOM never matches `locate`'s cheap tiers and always rebuilds the
  whitespace-stripped index. 500 highlights = 3.9s.
- The watcher emits **1.6 events per save**, and they compound: `reanchor` measured
  at 74ms in isolation becomes 2.5s in the live app because the re-renders serialize
  on the main thread. Save→repaint is **5.4s** at 100 highlights.
- First paint **1,417ms** on a 5000-file repo, 1,189ms on a trivial one. The entire
  pre-window Rust sequence is only 59ms of that.
- 512KB of markdown becomes **1.06MB of HTML** with ~12,000 inline-styled spans.
- Selections spanning inline markup **fail to anchor 76–100%** of the time and are
  silently demoted to stale chips. That is a correctness bug, found incidentally.

### Mistakes & deviations

This thread did not run clean; the harness produced confident, plausible, wrong
numbers five times, and each one had to be caught by disbelieving a result.

- **`grep -q` under `set -o pipefail`** reported every symbolicated binary as
  stripped — `grep -q` exits early, `nm` dies of SIGPIPE, the pipeline reports
  failure despite matching.
- **`$!` on a subshell** meant the kill hit the subshell, not the app, leaking a
  live `dreamd` per run. Fixed with `exec`.
- **A relative binary path after `cd`** made first-paint measurement return empty
  rather than wrong — silence, not an error.
- **Workload mismatches keyed to the same metric path**, three times over: quick
  rendered 512KB while deep rendered 2MB; quick cut `--sample-size` to 10 while deep
  used full sampling; `pass` measured the debug binary while the baseline held
  release. Each produced double-digit phantom deltas — the 250% "regression" in the
  final `perf-pass` was the last of them.
- **Thresholds set below the noise floor.** Two runs on identical code disagreed by
  up to 27% on Chromium raster. 5%/15% guaranteed false positives.
- I also **corrupted three of my own baseline runs** by running `cargo clippy` and
  `gen.mjs --force` alongside them, and by killing leaked processes mid-measurement.

The corrections, and the rule they produced: **tiers differ in how much they run,
never in how they run it**, and anything that differs about a workload belongs in
the metric's key, not silently in its value. Plus a lockfile, load recorded in
`meta`, min-of-3 for launches (first-paint spread went from ~3x to 59ms), and
thresholds measured rather than guessed.

Two items from the plan were corrected by the measurements: adding prefix/suffix
context does **not** fix `reanchor` (rendered context misses tier 1 identically —
7.22ms vs 7.21ms; the fix is memoizing the stripped index once per call), and
deduplicating the double repo walk is worth ~57ms, not the headline it was ranked
as. The debounce fix is the top item, above where it was ranked.

### State

`cargo build` clean with and without `--features perf`; `cargo clippy --all-targets`
clean; all shell and JS syntax-checked. Verified the harness detects the known
missing-debounce bug positively (1.6 events/save where >1.0 is the signal), that
injected +40% and +50% regressions are caught and exit non-zero, and that two runs
on identical code flag nothing.

The final `perf-pass` reported 33 regressions; **all are harness artifacts, not code
regressions** — 21 from the debug-vs-release profile mismatch fixed in this commit,
the rest sub-5ms benchmarks within noise. No measured Rust logic changed this
session, so bench movement is noise by construction.

`perf/baseline.json` is committed from a clean deep run. Its `real.*` entries
predate the profile-keying fix and will realign on the next
`./perf/run.sh deep --update-baseline`; `bench.*` and `chromium.*` are current. The
baseline was deliberately not hand-edited to paper over this.

Nothing optimized yet. The ranked fix list stands, led by watcher debounce,
`reanchor` index memoization, and syntect warm-up.

## 2026-07-24 — icon-button tooltips with keybinds

Every icon-only button in the GUI now shows a hover popup naming the button and,
where one exists, the keybind that triggers it. Frontend-only; landed.

### What happened

1. **Inventoried the icon-only buttons.** Seven: `#btn-hl-mode`, `#btn-stack`,
   `#btn-send` (titlebar), `#btn-collapse` / `#btn-expand` (file tree),
   `#stack-close`, and the per-file `⋯` (`.file-opts`, built in `app.js`).

2. **Dropped native `title` in favour of `data-tip`.** The browser tooltip has a
   ~1s delay, can't be styled, and can't render the keybind chip. Each button now
   carries `data-tip="<label>"` plus an optional `data-tip-key="<keymap field>"`
   (`toggle_stack`, `send_stack`). Storing the *keymap field name* rather than a
   literal combo means a user-configured bind from `get_keymap` renders correctly
   — the tooltip reads the live `keymap` object at show time.

3. **`wireTooltips()` in `ui/app.js`.** One `#tooltip` div, delegated
   `mouseover`/`mouseout` off `document` so tree rows rendered later are covered
   without rewiring. 350ms delay, positioned below the button and flipped above
   when it would clip the viewport bottom, clamped horizontally. Also fires on
   `focusin` (keyboard parity) and hides on click, scroll, and blur — a click
   means the user already knows what the button does.

4. **Styling in `index.html`'s structural `<style>`, not `theme.css`.** The popup
   is chrome, not reading surface; it inherits the existing `--sidebar-bg` /
   `--border` vars so a theme still recolours it.

### Mistakes & deviations

- First cut called `scheduleTip()` without claiming `tipTarget` up front, so every
  `mouseover` bubbling from inside the button restarted the 350ms timer and the
  tooltip never appeared while the mouse was moving. Fixed by setting `tipTarget`
  at schedule time, not at show time.
- The `innerHTML` write tripped a security hook warning; both interpolations go
  through the existing `escapeHtml()`, so it was left as-is.

### State

`node --check ui/app.js` passes. No Rust touched, so no `cargo build` gate. Not
exercised in a running window — the hover behaviour is unverified visually.

## 2026-07-24 — session rituals: wrap-up skill + daily project doc

Ported the blogregator docs setup into dreamd: a `/wrap-up` skill, a
`/update-project-doc` skill, the `engies/project.md` landing page, and a cloud
routine that refreshes that page daily. All landed.

### What happened

1. **Surveyed the source pattern.** Read blogregator's `CLAUDE.md`,
   `engies/project.md`, and `engies/ai-practices.md`, plus the existing wrap-up
   skills in `tree/` and `spotify_interview/`. Found blogregator has no
   `.claude/` of its own — the wrap-up ritual lives in those other repos, and
   what blogregator contributes is the `engies/` convention plus the daily job.
   Also found the blogregator routine creation had **failed** with a 403
   ("You don't have access to a repository this routine uses") — the daily job
   the user believed was running never existed.

2. **`.claude/skills/wrap-up/SKILL.md`.** Review diff → gate (`cargo build`
   only when `src-tauri/` is touched) → prepend a dated section to
   `docs/session-log.md` → refresh `engies/project.md` if the project story
   changed → one atomic commit **straight to main** + push → lean memory →
   report. Log layout decision: keep dreamd's existing single running file and
   prepend newest-first, rather than adopting the `session-logs/` directory the
   other two repos use.

3. **`.claude/skills/update-project-doc/SKILL.md`.** Regenerates
   `engies/project.md` from `git log` + `docs/session-log.md` + `README.md` +
   the source tree. Pins the section contract, the entry-level voice, and an
   explicit *"if nothing meaningful changed, do not manufacture news — leave the
   file untouched and make no commit"* rule, so a daily unattended job can't
   invent progress. Commits only that one path.

4. **`engies/project.md`.** The human landing page for dreamd: product loop,
   module-by-module architecture, honest known limits, glossary, reverse-chron
   "Recent updates".

5. **`CLAUDE.md`.** Terse machine-facing tenets (read-only, nothing persists, no
   shell interpolation of user content, escape-don't-execute, CSS-themeable) plus
   the docs conventions. Human-facing guidance deliberately stays in `engies/`.

6. **Cloud routine.** `trig_01GLUNmetTpUmT5ptfLzrMLM`, cron `3 7 * * *` UTC
   (≈08:03 UK in BST), sonnet-5, tools limited to Bash/Read/Write/Edit/Glob/Grep.
   Its prompt tells the agent to read `.claude/skills/update-project-doc/SKILL.md`
   from the checkout and follow it — so editing the skill changes the job, no
   routine edit needed.

### Mistakes & deviations

- **First routine creation 403'd**, same as blogregator's: claude.ai had no
  GitHub access to `bongofongo/dreamd`. Saved the exact create body to the
  scratchpad, reported the blocker with the fix (connect GitHub at
  claude.ai/code). User updated the Claude GitHub app; the retry returned 200.
- **Test run was inconclusive.** Fired the routine manually and polled
  `git ls-remote origin main` for ~5 min — no new commit. That is the expected
  no-op path (project.md was written the same day from the same git log), but
  the cloud session's transcript isn't readable from the CLI, so *correct no-op*
  and *failed run* look identical from here. Reported it as unproven rather than
  claiming success. Real verification comes at the next scheduled run.

### State

Docs/skills only — no Rust touched, no build gate needed. Skills committed and
pushed to main (`b78c9fb`). Routine created and enabled, next run
2026-07-25 07:03 UTC. `engies/project.md` left as written earlier this session;
its top "Recent updates" bullet already covers this work.

## 2026-07-24 — v1 build

Went from an empty scaffold to a working v1 of dreamd in one session.

### What happened

1. **Plan review.** Attacked the original `docs/plan.md` for gaps: the tmux
   send-to-Claude design (injection/escaping), raw-HTML XSS in the webview,
   scroll loss on live reload, missing link/image handling, no launch CLI, and
   the "Telescope reuse" assumption. Reworked the design around a
   highlight → annotation → **stack** → send loop (annotations promoted from v2
   to v1 core; nothing persisted — in-memory, dies with the process).

2. **Backend (`src-tauri/src/`).**
   - `fs_walk` — `ignore`-crate markdown scan → `FileNode` tree.
   - `markdown` — `pulldown-cmark` + `syntect`; raw HTML **escaped** (XSS closed);
     `locate()` powers both anchoring and evidence `file:line`.
   - `annotations` — in-memory highlights/annotations/stack + re-anchoring
     (Active → Stale when the highlighted text itself is edited).
   - `search` — `nucleo` fuzzy over file paths (Telescope lookalike).
   - `send` — assemble a temp query file; auto-detect a `claude` tmux pane and
     type a fixed `read @file` prompt (no shell interpolation), else clipboard.
   - `watcher` — `notify` → `file-added/changed/removed` + `theme-reloaded`.
   - `config` — TOML global + repo-local `.dreamd.toml` override.

3. **Frontend (`ui/`).** Tree, fuzzy palette, stack panel, annotation modal,
   live highlight wrap, stale margin rail, scroll-preserving reload, link/image
   resolution, embedded hot-reloadable theme.

4. **Security fix.** Restricted `open_external` to `http`/`https`/`mailto`;
   stopped routing bare local paths to the OS opener; gated relative images to
   inside the repo root.

### UI iterations (same session)

- Fixed viewer scrolling (grid row was unbounded).
- Highlight mode: highlighter icon toggles auto-highlight-on-select; `h`
  highlights the current selection and prompts for an annotation.
- Collapsible panes; edit existing highlights by clicking them (re-add / edit /
  delete), which is also how a removed stack pair gets re-added.
- Overlay titlebar (macOS) so **highlight · stack · send** icons sit on the
  traffic-light row; file path removed from the top bar; repo root shown
  home-relative (`~/…`) in the tree header.
- Collapse arrow: `◀` in the tree header when expanded, floating `▶` when
  collapsed; preview always full-width.
- Per-file `⋯` menu: Copy path / Delete (moves to OS Trash, repo-scoped, with a
  confirm dialog).
- `Ctrl+Y` submits the annotation from the textarea (keyboard-only flow).
- Vim-style keybinds: palette on `Ctrl+F`, `Ctrl+P`/`Ctrl+N` prev/next in the
  palette, `Ctrl+O` toggles the stack, `Ctrl+C` copies the stack (defers to the
  OS copy when text is selected), `Ctrl+Enter` sends.
- nvim-style CLI: `dreamd file.md` opens the file on load while the tree stays
  rooted at the current directory's repo.

### State

Compiles clean (`cargo build`); launches and passes startup smoke tests. The
send loop was verified end-to-end (a real stack landed as a formatted query).

### Not yet verified / known limits

- Full GUI interactions (traffic-light alignment, drag, ⋯ menu, Trash
  round-trip) checked only by launch smoke tests, not interactively.
- Highlight DOM re-wrap uses single-node text search; heavily formatted
  selections may read as stale.
- tmux `claude`-pane detection is heuristic (may run as `node`); pin
  `tmux_target` in config for reliability.
- No unit tests yet — `locate()`/`reanchor` are the obvious first targets.
- Fuzzy search covers paths only; content/`live_grep` is a v2 item.
- Placeholder app icon (blue square).
