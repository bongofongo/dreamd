# Export current document to PDF

A simple button: export the currently viewed markdown file as a PDF.

## Current state

Nothing exists here — no PDF dependency in `Cargo.toml`, no print
stylesheet in `ui/`, no export command.

## Recommended direction: don't add a PDF-generation crate

The cheapest, most WebView-native way to do this is `window.print()` against
the rendered `#content`, plus an `@media print` stylesheet (hide the
sidebar/titlebar/stack panel, keep just the reading content) — every OS
already gives you a "Save as PDF" print target for free. That avoids pulling
in a PDF-rendering dependency entirely and leans on exactly the WebView
customizability you've been pointed at elsewhere in this pass. Worth
confirming the CSP in `tauri.conf.json` doesn't need loosening for this (it
shouldn't — printing is a browser-native action, not a network/script one).

## Tenet 1 tension — smaller than it looks, but real

`CLAUDE.md`'s first tenet: dreamd is read-only and "never writes anything
inside the repo." A PDF isn't the user's markdown and isn't written
silently — it's one explicit, user-initiated save action, through the OS's
own save dialog, the same way `send.rs` already writes a temp file outside
the repo as part of an explicit user action (tenet 3). The one thing worth
deciding: does this need to actively steer the save dialog away from
locations inside the repo, or is "the user explicitly chose where to put
it" enough to consider it outside the read-only tenet's concern? Leaning
toward the latter — the tenet is about the app not mutating repo content on
its own, not about restricting where a user can point their own OS save
dialog.

## Open question

Print just the content, or fold in the highlight/annotation stack somehow
(e.g. highlights rendered as marginalia, or the stack as an appendix)? Worth
scoping this to plain content-only for a first pass.
