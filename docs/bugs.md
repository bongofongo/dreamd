# Bug log

Newest first. One entry per bug that reached a release artifact: the symptom as
the user saw it, the actual cause, and what would have caught it.

## 2026-07-30 — the pane exits 127 in the release build

**Symptom.** In the installed `.app`, opening the Claude Code pane printed
`zsh:1: command not found: claude` and the header read `exited (127)`. The MCP
banner ("no agent has reached dreamd yet") sat above it and read as the cause; it
was not — the server was registered and fine, no agent had simply ever connected
because no agent ever started.

**Cause.** `pty::Pty::spawn` ran `$SHELL -l -c "exec claude …"`. A login,
*non-interactive* zsh reads `.zshenv`, `.zprofile`, `.zlogin` — never `.zshrc`.
`claude` installs to `~/.local/bin`, and `.zshrc` is the only file putting that
on `PATH`. A `.app` launched from Finder starts from launchd's minimal `PATH`, so
`exec claude` had nothing to resolve.

**Why development never showed it.** `cargo tauri dev` is started from an
interactive terminal, so the GUI process — and the pty child after it — inherits
a `PATH` that already contains `~/.local/bin`. The bug was reachable only from a
bundle, and only from Finder.

**Fix.** `-l -i -c` (`SHELL_FLAGS` in `src-tauri/src/pty.rs`). `-i` is what
sources `.zshrc`. Verified no extra output before the exec, and `/bin/sh -l -i
-c` still exits 0 on the Linux fallback path.

**What now catches it.** Nothing automatic — the smoke tests do not open the
pane, and no CI runner has a Finder. `SHELL_FLAGS` is a const with a test pinning
both flags, which turns a silent regression into a red test at least.
