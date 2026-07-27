// Path containment for relative links and images — the frontend half of
// tenet 4. A document may reference `../../../../etc/passwd`; these two
// functions are what stop that resolving to something the webview will load.
//
// Split out of app.js so `ui/paths.test.mjs` can drive them under `node --test`
// with no Playwright, no Chromium and no Tauri stub. A plain classic script, not
// an ES module: the frontend has no build step, `index.html` loads this before
// `app.js`, and both share the global scope. The CSP (`script-src 'self'`)
// permits it — only *inline* script is blocked.
//
// The Rust side has the same guard in `src-tauri/src/guard.rs::inside_root`,
// for `delete_file`. Change one, look at the other.

/// Collapse `.` and `..` segments, returning an absolute path.
///
/// `..` past the top is absorbed rather than escaping — `/a/../../b` is `/b`,
/// not something above the filesystem root. That matters only because the
/// result is then handed to `insideRepo`, which would otherwise be asked to
/// judge a path that cannot exist.
function normalizePath(p) {
  const parts = [];
  for (const seg of p.split("/")) {
    if (seg === "..") parts.pop();
    else if (seg !== "." && seg !== "") parts.push(seg);
  }
  return "/" + parts.join("/");
}

/// Is this already-normalized absolute path inside `repoRoot`?
///
/// A bare `startsWith(repoRoot)` is *not* containment: with a root of
/// `/w/notes` it also accepts `/w/notes-private/secret.md`, because the check
/// never reaches a path separator. The boundary has to be the separator, so
/// the test is "equal to the root, or under root + `/`". With no repo open
/// nothing is inside, which is what the image handler has always done.
function insideRepo(abs, repoRoot) {
  if (!repoRoot) return false;
  const root = repoRoot.replace(/\/+$/, "");
  return abs === root || abs.startsWith(root + "/");
}
