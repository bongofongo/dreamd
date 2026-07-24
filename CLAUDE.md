# dreamd

GUI markdown **reader** for a tmux + Neovim + Claude Code workflow. Rust + Tauri 2 +
plain HTML/CSS/JS. The product is the reading experience plus the
highlight → annotation → stack → send loop.

## Tenets

1. **Read-only.** dreamd never writes to the user's markdown. Editing stays in Neovim.
2. **Nothing persists.** Highlights, annotations, and the stack are in-memory and die
   with the process. Don't add a database without an explicit decision to.
3. **No shell interpolation of user content.** Sent queries go through a temp file and
   a fixed `read @<file>` prompt. Highlighted text never enters a command line.
4. **Escape, don't execute.** Raw HTML in markdown is escaped. External links are
   restricted to `http`/`https`/`mailto`; relative images must resolve inside the repo
   root.
5. **Themeable to the CSS level.** `ui/theme.css` is a user-facing surface, hot-reloaded.

## Working practices

- Commits go **straight to main** — no branches, no PRs.
- `cargo build` must pass before any commit touching `src-tauri/`.
- Repeatable flows become skills in `.claude/skills/`.

## Docs

- `docs/session-log.md` — running session log, **newest section first**. Written by
  the `/wrap-up` skill at the end of a session.
- `engies/project.md` — the human landing page: a 2–3 page plain-language brief
  written for an entry-level reader, ending with "Recent updates". Refreshed daily by
  a scheduled job and by the `/update-project-doc` skill. If a session materially
  changes the project story, update it in the same session rather than waiting.
- `docs/plan.md` — original design intent. Historical; don't rewrite it.

Keep this CLAUDE.md terse and machine-facing — human-facing guidance belongs in
`engies/`.
