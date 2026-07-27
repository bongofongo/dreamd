# Copy button on code blocks

**Status: done — shipped in 558feae (2026-07-26).** Every rendered code block
gets a copy button in the top right (`addCopyButtons`, `ui/app.js:555`), using
the shared `button.icon` class and a static, author-written SVG.

code containers need to have a copy button somewhere - probably the standard top right.
