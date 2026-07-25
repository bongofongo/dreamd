# icons

`icon.png` (1024×1024, transparent corners) is a rasterization of the website's
mark — `website/public/favicon.svg` is the single source of truth for the shape,
so change it there and re-render, never edit the PNG.

No `rsvg-convert`/ImageMagick on this machine; the render goes through the
Playwright Chromium already installed for the perf harness. From
`perf/harness/` (so `playwright` resolves), with a script that loads the SVG at
1024×1024 and screenshots it with `omitBackground: true`.

`bundle.active` is `false` in `tauri.conf.json`, so this PNG is the entire icon
set. Packaging later means generating `.icns`/`.ico` too — `cargo tauri icon
../../website/public/favicon.svg` does it, and needs `tauri-cli` installed.
