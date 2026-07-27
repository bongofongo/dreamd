# icons

`website/public/favicon.svg` is the single source of truth for the *shape*.
Every file here is generated from it — change the SVG, regenerate, never
hand-edit a PNG:

```sh
cargo tauri icon website/public/favicon.svg -o src-tauri/icons   # from the repo root
rm -rf src-tauri/icons/android src-tauri/icons/ios               # no mobile target
rsvg-convert -w 48 -h 48 website/public/favicon.svg -o src-tauri/icons/48x48.png
```

`48x48.png` needs the third line because `cargo tauri icon` does not emit that
size — it generates 32, 64, 128 and 128@2x and stops. 48² is the size GNOME and
XFCE reach for in panels and app grids, so on Linux its absence is visible; see
the hicolor note below. librsvg is already a build dependency there, and it
renders this SVG to within an absolute error of 1.2 over 4096 pixels of what
`cargo tauri icon` produces at 64² — the same drawing, not a second one.

(The repo root `.gitignore` ignores `*.svg` wholesale; `website/.gitignore`
re-includes `public/favicon.svg` by negation, and the root re-includes
`macos.svg` below. Without those, a fresh clone could not regenerate any of
this.)

## `icon.icns` comes from `macos.svg`, not from the favicon

**Regenerating the whole directory from the favicon puts the wrong icon in the
Dock**, and the command above will happily do it. Re-run the one below
afterwards:

```sh
cargo tauri icon src-tauri/icons/macos.svg -o /tmp/dreamd-icon
cp /tmp/dreamd-icon/icon.icns src-tauri/icons/icon.icns
```

A macOS app icon is not a full-bleed square. Apple's grid for the
rounded-rectangle shape is **824×824 centred in a 1024×1024 canvas** — 100px of
transparent margin on all four sides, corner radius 185.4 — and every app in the
Dock is drawn to it. A favicon has no such convention and fills its square,
which is correct in a browser tab and, at the same nominal size in the Dock,
makes dreamd sit visibly larger than everything beside it. It reads as a
mistake, not as a choice. (Measured, not assumed: a shipped third-party app on
this machine renders 828×847 at (98,100) — the extra height is its drop shadow.)

`macos.svg` is the favicon's four shapes verbatim under a
`translate(100,100) scale(25.75)` — 824/32 — so the two files stay obviously the
same drawing. Only `icon.icns` is built from it. The PNGs stay full-bleed on
purpose: the Windows `.ico` and Store logos follow that platform's convention,
and the first PNG is the *window* icon, which is a Linux concern (see below).

To check it: expand and measure the alpha bounding box.

```sh
iconutil -c iconset src-tauri/icons/icon.icns -o /tmp/dreamd.iconset
# /tmp/dreamd.iconset/icon_512x512@2x.png must be artwork 824x824 at (100,100)
```

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

## On Linux the array *is* the installed icon set

Only the first `.png` is load-bearing for size. Every **other** `.png` in the
array is copied by the deb bundler into `usr/share/icons/hicolor/<w>x<h>/apps/`,
so the array is also the literal list of sizes a Linux desktop gets to choose
from — and, since the AppImage's AppDir and the `.tar.gz` are built from that
same staged tree, it is the list for all three artifacts. Adding a size to the
array anywhere after the first entry is therefore free and has no effect on
macOS.

32, 48, 64, 128 and 128@2x are listed. `128x128@2x.png` lands in a directory the
bundler names `256x256@2` — its own bug, since a `@2` directory named 256 means
a 512px image and the file is 256px. Harmless (the unscaled sizes are what get
picked) and not ours to fix.
