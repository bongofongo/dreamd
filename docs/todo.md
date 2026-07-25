# todo

Working list from the 2026-07-25 planning pass on settings/theming and startup.
Not committed to timing or ordering — pull items into session work as they come up.
Delete items here once they land; this is a queue, not a log (see `docs/session-log.md`
for history).

## Settings panel & theme framework

- [ ] Design a settings panel (currently none exists — no UI surface in `ui/index.html`/
      `ui/app.js` lets a user pick or preview a theme today). Needs at least: theme picker,
      dark/light toggle, font override.
- [ ] Generalize `theme_css` (`src-tauri/src/config.rs`) from a single user override path
      into a small theme registry: a set of named, bundled CSS files plus the existing
      "point at your own file" escape hatch. Keep the hot-reload behavior from `watcher.rs`
      (`theme-reloaded` event) working per-theme, not just for the one override path.
- [ ] Decide where bundled themes live on disk (e.g. `ui/themes/*.css`) and how the frontend
      lists/loads them without a build step (tenet: plain HTML/CSS/JS, no bundler).
- [ ] Settings need to persist somewhere across launches — check this against tenet 2
      ("nothing persists... don't add a database without an explicit decision"). Likely
      answer: a small config file (already have `Config` in `config.rs`), not the in-memory
      `Store`. Make that boundary explicit before writing code.

## Reading/literary theme pack

- [ ] Today there is exactly one shipped UI theme (`ui/theme.css`, dark, coder-toned) plus
      syntect's built-in *code-block* highlighting themes (`ThemeSet::load_defaults()` in
      `markdown.rs` — base16-ocean.dark, Solarized, etc., which only color fenced code, not
      the app chrome). The "Dracula/Solarized/Gruvbox" feel is currently just that syntect
      set, not full app themes — there's no bookish/literary theme yet at either layer.
- [ ] Design a batch of literary/reading-styled themes (serif-forward, warm paper tones,
      sepia, letterpress, library-at-night, etc.) as full UI themes, not just code themes —
      distinct from the existing programmer-coded palettes.
- [ ] Each theme should suggest a display font (e.g. a serif built for long-form reading)
      but must degrade gracefully on a machine that doesn't have it — system font stack
      fallbacks, no network font fetches (WebView, no build step, no CDN dependency).
- [ ] Pick/vet a shortlist of reading-oriented font stacks (serif body text + a matching
      monospace for code blocks) that are either system-common or safe to bundle locally.
- [ ] Wire the chosen syntect code-theme per UI theme too, so code blocks don't clash with
      a warm/sepia reading theme the way base16-ocean.dark currently would.

## Dark/light toggle

- [ ] Every shipped theme (existing "dream" default + the new literary set) needs a paired
      dark/light variant, not just the palette swap dreamd has today.
- [ ] Add a single dark/light toggle in the settings panel that swaps the active theme to
      its sibling variant, independent of which theme family is selected.
- [ ] Settle the CSS variable/file structure that makes "same theme, other mode" a
      mechanical swap (e.g. shared variable names across a `-dark.css`/`-light.css` pair,
      or a `[data-mode]` attribute switch within one file) rather than bespoke work per
      theme.

## Default "dream" theme

- [ ] Spend real design attention on `ui/theme.css` itself (the bundled default) — currently
      described in its own header comment as "tuned for long-form markdown" but otherwise a
      fairly plain dark palette. Make it the theme that sells the app, not just a safe
      default.
- [ ] Give the default theme its own light-mode sibling as part of the dark/light toggle
      work above, rather than treating it as the one theme exempt from pairing.
