# todo

Working list from the 2026-07-25 planning pass on settings/theming and startup.
Not committed to timing or ordering — pull items into session work as they come up.
Delete items here once they land; this is a queue, not a log (see `docs/session-log.md`
for history).

## Theming follow-ups

- [ ] Nobody has seen any of the ten themes in WKWebView. The whole pack was authored
      and checked through Chromium and the CSS; the literary faces in particular
      (Iowan Old Style, Charter, Hoefler Text) have never been rendered by the engine
      that will actually draw them.
- [ ] Verify that WKWebView's `prefers-color-scheme` tracks the effective NSApp
      appearance inside a Tauri window. The `system` mode rests on it. One
      `cargo tauri dev` with the OS appearance toggled settles it; the
      `tauri://theme-changed` listener is the fallback if it doesn't.
- [ ] `tauri.conf.json`'s `backgroundColor` is a static dark hex covering the
      window-creation → `.setup()` gap, so a light-mode launch flashes dark for that
      instant. Fixing it properly means `"create": false` and building the window in
      Rust with `WebviewWindowBuilder::from_config`. Worth a measurement first — it may
      not be visible.
- [ ] `AppState::syntax_theme()` re-resolves the theme from disk on *every*
      `render_markdown` — reads the palette, formats base+palette, now also runs
      `mode_slice`. Pre-existing and the wrong shape; wants a cached `Resolved` and a
      `/perf-quick` on either side of it.
- [ ] syntect only ships seven code themes and none is warm paper. `--code-bg` now owns
      the block's ground so the clash is much smaller, but the *token* colours are still
      picked from those seven. Loading a bundled `.tmTheme` needs syntect's `plist-load`
      feature.
