# ui/vendor

Third-party JavaScript, checked in verbatim. **The only vendored JS in dreamd**,
and the only exception to "the frontend has no build step" — which it still
doesn't: these are the publishers' own prebuilt UMD bundles, copied out of their
npm tarballs and loaded with a plain `<script src>`.

Vendoring is not a preference here, it is the only option. The CSP is
`script-src 'self'`: a CDN is blocked, an inline `<script>` is blocked silently,
and WASM is blocked. A same-origin file is what remains.

| File | Package | Version | License |
|---|---|---|---|
| `xterm.js` | `@xterm/xterm` | 5.5.0 | MIT |
| `xterm.css` | `@xterm/xterm` | 5.5.0 | MIT |
| `addon-fit.js` | `@xterm/addon-fit` | 0.10.0 | MIT |

`LICENSE` is xterm.js's, and covers all three — the fit addon ships the same
text.

## How these were produced

```sh
npm pack @xterm/xterm@5.5.0 @xterm/addon-fit@0.10.0
tar xzf xterm-xterm-5.5.0.tgz          # package/lib/xterm.js, package/css/xterm.css
tar xzf xterm-addon-fit-0.10.0.tgz     # package/lib/addon-fit.js
```

Byte-for-byte the published files, with one edit: the trailing
`//# sourceMappingURL=` comment is removed, because the `.map` files are not
vendored (`xterm.js.map` alone is 1.1 MB, four times the code) and the comment
would otherwise make the webview fetch a file that isn't there.

To upgrade: repeat the above with the new version, redo that one edit, bump this
table, and open the pane — `app.js`'s `loadTerminalVendor` names both globals
(`Terminal`, `FitAddon.FitAddon`), so a UMD shape change fails loudly at first
open rather than quietly.

## Why the bundles are loaded lazily

`app.js` injects these `<script>`/`<link>` tags on the **first pane open**, not
from `index.html`. A `<script defer>` in the document costs its parse on every
launch, including the overwhelming majority that never open a terminal, and
first paint is the number dreamd measures. Injecting a same-origin script tag at
runtime is as CSP-clean as declaring it.
