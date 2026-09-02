# Contributing to dreamd

Thanks for looking. A few things about this repo are unusual enough to be worth
saying up front, so nothing surprises you halfway through a change.

## This repo has no branches

dreamd is developed by one person and commits go **straight to `main`** — no
feature branches, no PRs, no review queue. If the branch list looks empty and the
history looks like a straight line, that is not a mistake.

That convention is for the maintainer's own commits. **You should still open a
PR**, because there is no other way to propose a change from outside. Fork, branch
in your fork, and open a pull request against `main`. It will be reviewed by hand
and will usually land as a squash or a rebase, so don't be alarmed if the merge
commit you expected doesn't appear.

Before you build anything large, open an issue and say what you have in mind.
dreamd is opinionated — see the **Tenets** section of [`CLAUDE.md`](CLAUDE.md) —
and a change that crosses one of them (writing to the user's markdown, adding a
build step to `ui/`, interpolating user content into a shell command) will be
turned down however good the code is. That is much better to hear before you write
it.

## Some of the repo is not here

The design notes, the session log, the idea backlog and the performance baseline
live in a separate private repo, cloned by the maintainer at `notes/` — which is
gitignored here. You will occasionally see a comment pointing at something like
`plans/md-to-pdf-export.md`; that is where it is, and you can't read it. Nothing
you need in order to build, test or change dreamd depends on it, and that is
enforced: **a fresh clone with no `notes/` must build and pass every harness.**
If you find something that breaks without it, that is a bug worth reporting.

The one visible consequence is performance: `perf/run.sh` has no baseline to diff
against in your checkout, so every tier runs and records rather than comparing.
That is the expected behaviour, not a failure, and the script exits zero.

## Building

```sh
cargo install tauri-cli --version "^2"     # once
cargo tauri dev                            # run it, watching the repo you're in
cargo tauri dev -- -- /path/to/some/repo   # run it against another repo
```

`ui/` is plain HTML/CSS/JS with **no build step** — `tauri.conf.json` points
`frontendDist` straight at the directory. There is no bundler, no transpiler and
no `npm install` for the app itself. `perf/harness/` has a `package.json`, but
that is test-only tooling and never ships.

Linux needs the WebKitGTK and GTK3 development packages; macOS needs nothing
beyond Xcode command line tools. The exact package names per distro are in
[`README.md`](README.md) under **Requirements (building from source)**.

## What to run before opening a PR

CI is the backstop, not the first check. Run these locally — the whole set is a
couple of minutes on a warm target directory.

```sh
cargo build                       # must pass before any commit touching src-tauri/
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features         # the pure core and the security tenets
node --test ui/paths.test.mjs     # the frontend containment guard, no deps
```

Then the example harnesses. These are not `cargo test` because they touch the
real filesystem, real sockets and real subprocesses; each exits non-zero on
failure, and between them they cover what a unit test cannot reach:

```sh
node perf/corpus/gen.mjs                      # 611 fixtures, needed by the next line
cargo run --release --example locate_check    # highlight anchoring across the corpus
cargo run --example config_check              # config layering + write-back
cargo run --example theme_check               # the bundled palettes
cargo run --example mcp_check                 # the MCP socket end to end
cargo run --example marks_check               # the marks file on disk
cargo run --example agent_check               # the permission gate over a real socket
```

If your change touches `ui/`, also run the frontend harness. It drives the page in
Chromium behind a stub of the Rust commands and asserts on the DOM and on which
IPC calls the page makes:

```sh
cd perf/harness && npm run setup    # once: Playwright + Chromium
node perf/harness/ui-check.mjs
```

**Two honest caveats about coverage.** `ui-check.mjs` asserts what the page
*knows*, not what it *paints* — it cannot catch a WebKit rendering bug, and the
GUI is verified by hand. If your change touches how a document is drawn, open
`testdocs/images.md` in dreamd and work down it; it lists what to look at. And
numbers out of `perf/harness/` are Chromium, not WKWebView; they are a relative
regression signal only, so say so if you quote one.

## Style

Match the surrounding code. The thing most likely to get comments on a PR here is
comments: this codebase explains *why*, often at length, next to the decision
rather than in a doc — including the failed approaches, so nobody re-tries them.
A patch that changes a load-bearing line and leaves the paragraph above it saying
the old thing is worse than no patch.

New backend logic goes in a module under `src-tauri/src/`, never in `main.rs`.
`main.rs` is a `[[bin]]` and therefore cannot be imported, so anything that lives
there is unreachable from tests and from `benches/`. That is the reason for the
library-crate split, not tidiness.

## Reporting a bug

Include your OS and version, how you installed dreamd (cask, AppImage, deb,
tarball, or from source), and what you did. If it is a startup or window problem
on Linux, `WEBKIT_DISABLE_DMABUF_RENDERER=1` in front of the command is worth
trying first and worth mentioning in the report either way — see the `webkit`
module in [`CLAUDE.md`](CLAUDE.md) for why.

## Licence

By contributing you agree your contribution is licensed under the Apache License
2.0, the same terms as the project — see [`LICENSE`](LICENSE).
