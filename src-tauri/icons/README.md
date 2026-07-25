# icons

`website/public/favicon.svg` is the single source of truth for the shape. Every
file here is generated from it — change the SVG, regenerate, never hand-edit a
PNG:

```sh
cargo tauri icon website/public/favicon.svg -o src-tauri/icons   # from the repo root
rm -rf src-tauri/icons/android src-tauri/icons/ios               # no mobile target
```

(The repo root `.gitignore` ignores `*.svg` wholesale; `website/.gitignore`
re-includes `public/favicon.svg` by negation. Without that, a fresh clone could
not regenerate any of this.)

## The array order in `tauri.conf.json` is load-bearing

`bundle.icon` is ordered smallest-PNG-first **on purpose**, and reordering it
silently adds megabytes to the binary:

- `.icns` is what macOS Finder and the Dock render. Full resolution, in the
  bundle, not in the executable.
- The first **`.png`** in the array becomes the *default window icon*, and
  tauri-codegen bakes it in as **raw uncompressed RGBA** — `width × height × 4`
  bytes, `include_bytes!`'d. `find_icon` in `tauri-codegen/src/context.rs` takes
  the first `.png` it sees.

`icon.png` used to be first, at 1024². That is 4,194,304 bytes — 43% of a
9.8 MB release binary — to carry an image that on macOS is *never read at all*
(`set_window_icon` is a documented no-op in `tao/src/platform_impl/macos/window.rs`).

`128x128.png` is first instead: 65,536 bytes. 32² would save another 61 KB, but
on Linux the window icon genuinely is used by the WM and taskbar, and 32² looks
bad there. That 64 KB is deliberate insurance for the Linux target.

If the binary ever jumps by megabytes for no obvious reason, check this array
first, then confirm with:

```sh
cargo build --release
find target/release/build/dreamd-*/out -size +1M   # should find no icon blob
```
