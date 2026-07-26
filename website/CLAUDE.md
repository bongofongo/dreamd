# CLAUDE.md — website/

Source of truth for this directory. The root `../CLAUDE.md` governs the app; it does
not govern anything here. Nothing in this directory affects the Rust build — no
`cargo build`, no perf tier is implicated by a change to the site.

# dreamd website

The public face of dreamd, live at **https://fongo.uk/dreamd**. A standalone Astro 5
static site: no framework, no Tailwind, no build step beyond Astro itself.

## Commands

```sh
npm install
npm run dev        # http://localhost:4321/dreamd  — note the base path
npm run build      # → dist/dreamd/
npm run preview    # serves the built output on 4321
npm run deploy     # rm -rf dist && astro build && wrangler deploy  ← publishes live
npx wrangler deploy --dry-run   # validate the route config without publishing
```

`npm run deploy` is the entire pipeline; there is no CI. It needs an authenticated
wrangler (`wrangler login`). **It publishes to the live zone — only run it when
asked.**

## How this reaches fongo.uk

There is no build-time wiring between repos. No submodule, no symlink, no sync script.

- `fongo.uk` is **paper_web on Vercel**, deployed on push to its `main`.
- This directory deploys as its own **assets-only Cloudflare Worker** (`dreamd-web`)
  on the zone route `fongo.uk/dreamd*`, which intercepts that one path before Vercel
  ever sees it. Two independent deploys, zero coupling.
- Discovery is a hardcoded `link` string in
  `paper_web/src/data/project_list.json` (slug 4), pointing at `/dreamd`. Changing it
  is a separate commit in that repo, and pushing that repo's `main` rebuilds the whole
  personal site on Vercel.
- paper_web mounts `<ClientRouter />`, so a same-origin `/dreamd` link is in principle
  a candidate for a view-transition partial swap, which would glue two sites together.
  **Checked against production: it hard-navigates cleanly** — no paper_web DOM
  survives, `data-theme` is gone, `data-landing` is set, no console errors. No
  `data-astro-reload` needed. Re-check if paper_web's router config changes.

**The `outDir` trick is load-bearing.** `astro.config.mjs` sets `base: "/dreamd"` and
`outDir: "./dist/dreamd"`, while `wrangler.jsonc` points `assets.directory` at
`./dist` — *not* `./dist/dreamd`. That makes the Worker's 1:1 path→asset mapping line
up with the route prefix, because Astro's `base` only prefixes URLs, not output paths.
Changing either half in isolation breaks every URL.

Every internal link goes through `href()` in `src/consts.ts`, which normalises
`import.meta.env.BASE_URL`. Use it; never hardcode `/dreamd/...`.

## Design invariants

Break these only on an explicit decision.

1. **Dark only.** No light theme, no toggle, no `data-theme`, no pre-paint script.
   One token block and `color-scheme: dark`. This is a deliberate divergence from
   paper_web and autorota, which are both dual-theme.
2. **The palette is the app's.** Tokens in `src/styles/global.css` are lifted from
   `../ui/theme.css`, pushed toward near-black for the landing. If the app's theme
   moves, this should follow. Never hardcode a hex outside the `:root` block.
3. **Serif display, sans body.** Spectral **500 only** (normal + italic) via Astro's
   font API — the wordmark and the landing title are italic. Body prose is the app's
   own system-sans stack; code is the app's mono stack. Do not add a weight without
   using it: 400 was shipped once and cut as dead weight.
4. **The highlight is the only visual device.** `mark` renders the app's yellow
   (`--hl`, `#f6d365`) on dark ink. Used two or three times on the whole page. The
   moment it becomes decoration it stops meaning anything.
5. **No images.** The landing earns its identity from type, colour, and CSS. If a
   gallery arrives later it goes on its own page, not here.
6. **The licence is Apache 2.0, and copy must match the repo.** `LICENSE` at the repo
   root plus `license = "Apache-2.0"` in `src-tauri/Cargo.toml` are the source of
   truth; the site names it via `LICENSE_NAME` / `LICENSE_URL` in `src/consts.ts` and
   nowhere else. Never name a licence the repo does not actually carry.

## The landing, and why it works

`src/components/Landing.astro` is one screen tall and `position: sticky; top: 0`.
`.page` (in `index.astro`, styled in `global.css`) is opaque and `z-index: 1`, so
scrolling raises it over the landing like a curtain — the two halves read as two
pages joined by a scroll. The transition takes exactly one viewport.

`src/components/DreamField.astro` is the dream: void gradient, two drifting veils on
unrelated clocks, a breathing teal aurora, a sparse two-scale starfield with a few
grains in the highlight yellow, and a vignette. It is scoped to the landing — it is
**not** a page-level background. Every animation is transform or opacity only, so it
stays compositor work.

Three traps, all of which have already bitten:

- **`overflow-x` on `body` silently kills the sticky.** It makes `body` a scroll
  container, and the landing then scrolls away instead of pinning. `html` does the
  horizontal clamp; leave `body` alone. Regression check:
  `document.querySelector('.landing').getBoundingClientRect().top` must stay `0` at
  every scroll offset.
- **The curtain will slice a line of type in half** unless the landing text is gone
  before the edge reaches it. `--wake` (written by the scroll handler in
  `SiteLayout.astro`, complete at 40% of a viewport) fades and lifts `.inner`. If you
  add or lengthen landing copy, re-check that nothing gets cut mid-glyph.
- **`fullPage` screenshots flatten sticky and fixed layers**, rendering them once at
  the top. They will make the lower page look wrong when it is fine. Judge scroll
  states with viewport-sized shots at explicit `scrollTo` offsets.

`.cue`'s bob animates transform only — opacity belongs to `--wake` and the two fight
if both animate it.

## JavaScript contract

**No JS bundle. Zero.** The only script is inline in `SiteLayout.astro`: one passive,
rAF-throttled scroll listener that writes `--wake` and toggles `html.scrolled`.

That drives two things: the landing's fade, and the corner wordmark, which is hidden
on the landing (the name is already the headline there) and arrives past 70% of a
viewport as the way home. Both are **progressive enhancement** — the `html.js` class
gates them, so without JS the wordmark is simply always visible and nothing fades.
Keep that property. Any new behaviour extends the existing listener rather than adding
another.

`html[data-landing]` is set by `SiteLayout`'s `landing` prop and marks the one page
that opens with the dream screen. Pages without it (404) get a filled header from the
start, since nothing scrolls over them.

## Budgets and floors

Not enforced by a harness here — there is no test suite and no perf gate in this
directory. They are design rules, because paper_web's `perf/budgets.json` is what
"in line with the family" means: HTML ≤ 15 KB gzip, CSS ≤ 5 KB, JS ≤ 5 KB.

Current, measured: HTML **4.68 KB** gzip, CSS **2.07 KB** (index; the 404's own chunk is
1.86 KB), JS **0**, fonts 2 woff2 (32 KB). Plenty of headroom; don't spend it on a
framework.

Accessibility floor: WCAG AA (`--text` on `--bg` ≈ 10:1, `--muted` ≈ 5.2:1 — both
pass), a visible focus ring on every interactive element, `aria-hidden` on decorative
SVG and on the whole dream field, and a skip link. All motion is suppressed under
`prefers-reduced-motion`; the field stays present, just frozen.

## Verifying a change

There is nothing to `npm test`. Build, serve, and drive a real browser. Playwright and
Chromium are already installed for the app's perf harness — run scripts from
`../perf/harness` so `import { chromium } from "playwright"` resolves:

```sh
npm run build && npm run preview &   # or npx astro preview --port 4399
cd ../perf/harness && node /path/to/script.mjs
```

Worth asserting, in roughly this order:

1. `.landing` `getBoundingClientRect().top === 0` at several scroll offsets (sticky).
2. `.brand` computed opacity: `0` at scroll 0, `1` past 70% of a viewport. Note the
   class flips at `y > 0.7h` but `.brand` has a **0.35 s opacity transition**, so a
   check that samples sooner than ~400 ms after the scroll reads a fraction and looks
   like a regression when it isn't. Drive scrolls with
   `window.scrollTo({top, behavior: "instant"})` too — `html` has
   `scroll-behavior: smooth`, and a plain `scrollTo` animates, so an immediate read
   sees the old offset.
   `.brand` links to `#top`, which is `.landing` — a `position: sticky` element whose
   rect is already pinned at the viewport top, so a native anchor-scroll to it only
   applies `scroll-padding-top` and moves up ~74 px rather than reaching `scrollY: 0`.
   `SiteLayout.astro`'s scroll script intercepts clicks on `a[href="#top"]` and calls
   `scrollTo(0, 0)` directly instead. It inherits `scroll-behavior` from `html`, so the
   settle takes the usual smooth-scroll duration — sample after it, not immediately.
3. Landing text fits inside one screen — it is `overflow: hidden`, so check
   `.inner`'s top/bottom against `innerHeight` at 1440×900, 1280×700, 375×812, 375×667.
4. `document.documentElement.scrollWidth > innerWidth` is false at 375 and 1440.
5. Under `reducedMotion: "reduce"`, `getAnimations().filter(a => a.playState === "running")`
   is empty.
6. No console errors; the built HTML still emits the Spectral `@font-face` and preloads
   and no `.js` file.

Chromium here is not WKWebView and not the app — but the site is a plain static page,
so unlike the app's perf numbers these results are the real thing, not a proxy.

## Gotchas

- **The repo root ignores `*.svg` wholesale.** `public/favicon.svg` is re-included by a
  negation in `website/.gitignore`. Any new SVG needs the same, or it silently never
  gets committed and a fresh clone cannot rebuild the site. It is also the source of
  truth for the *app's* icon set (`cargo tauri icon` reads it), so that negation is
  load-bearing well outside this directory.
- **There is no download button, and the Homebrew line is gone — on purpose.**
  Releases are unsigned while there is no Developer ID certificate (see the root
  `CLAUDE.md`), and both of those channels hand the user a quarantined artifact that
  opens as "dreamd is damaged". `curl … | sh` does not, because curl never writes
  `com.apple.quarantine`. So the install section offers exactly one command and says
  why. `RELEASES_URL` is still exported from `consts.ts`, unused, for when signing
  returns; if you restore a button it points at `/releases/latest` rather than a
  pinned asset, because this site deploys by a manual `npm run deploy` independent of
  the release workflow and a pinned href would 404 between tag and deploy. `VERSION`
  appears as prose only, where being stale is harmless.
- **`install.sh` is deliberately *not* served from `public/`,** even though it would
  work (`html_handling: drop-trailing-slash` only affects HTML, and curl ignores
  content-type) and `fongo.uk/dreamd/install.sh` would be the nicer URL. It would
  duplicate a file that has to stay in lockstep with the release artifact format, and
  a fix to it would sit unpublished until someone ran a deploy. Canonical copy lives at
  `packaging/install.sh` in the repo root and is served from raw.githubusercontent.
  `INSTALL_URL` in `src/consts.ts` is the only place that URL is written.
- `wrangler.jsonc` and `astro.config.mjs` both carry comments explaining the route and
  the `outDir` nesting. Keep them accurate — autorota's equivalent comment went stale
  (it still claims Cloudflare Pages) and is the single most likely source of confusion
  about how any of this is hosted.
- Assets are content-hashed and `dist/` is gitignored, so a deploy uploads only what
  changed. "N already uploaded" in the output is normal.
- **Slashless URLs are canonical**, and three settings have to agree or you get a
  redirect loop: `html_handling: "drop-trailing-slash"` in `wrangler.jsonc`,
  `trailingSlash: "never"` in `astro.config.mjs`, and the canonical normalisation in
  `SiteLayout.astro`. That last one is needed because Astro reports `/dreamd` for the
  index but `/dreamd/404/` for a nested route, so canonicals disagree with each other
  otherwise. autorota is configured identically.
- **Cloudflare caches redirects.** Right after a deploy that changes
  `html_handling`, the old redirect can still be served — briefly making `/dreamd` and
  `/dreamd/` 307 at each other, which looks exactly like a loop you just shipped.
  Re-check with a cache-buster query before debugging it.

## Adding a page

`src/pages/*.astro` → `SiteLayout` with `title` and `description` (omit `landing`).
Add the route to the `nav` array in `Header.astro` — it is empty and already wired, so
a Gallery tab is one entry plus one file. Use `href()` for the link.
