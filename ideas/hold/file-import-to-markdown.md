# Import/convert another file type to markdown

**Status: on hold — unchanged, and still the only idea in this batch nobody
built.** The tenet-1 conflict below is real enough that this isn't worth
designing further right now. Two blockers, neither resolved: which source
format(s) this actually means, and whether an importer may write inside the
repo root at all. Export-only work went ahead instead and is now done
(`ideas/done/md-to-pdf-export.md`, shipped 3218fdf) — that's evidence for the
"write outside the repo / separate CLI" options below rather than against them,
since print-to-PDF sidestepped the tenet entirely and cost one stylesheet.
Revisit only with an explicit decision on the tenet question.

A button to bring an easily-transferable file type in as markdown — the
dictation trailed off before naming the specific source format(s), worth
pinning down (plain `.txt`? `.docx`? `.rtf`? more than one?).

## Current state

Nothing exists here — no conversion dependency in `Cargo.toml` (no pandoc
shell-out, no pure-Rust docx/rtf reader), no import command or UI.

## This one runs harder into tenet 1 than the PDF-export idea does

`CLAUDE.md`'s first tenet is that dreamd is read-only and "never writes
anything inside the repo." PDF export sidesteps that because the output
isn't meant to live in the reading corpus — but the entire point of an
*importer* is the opposite: the converted `.md` needs to end up somewhere
dreamd will actually show it, or the feature doesn't deliver what it's for.
That's a real conflict, not a wording technicality, and it's worth deciding
deliberately rather than letting implementation quietly decide it:

- **Write it inside the repo root.** Delivers the feature as described
  (import → immediately browsable/readable), but is a direct, explicit
  exception to tenet 1 — same category of deliberate exception as the
  config/theme write path, and probably deserves the same explicit
  callout in `CLAUDE.md` if this ships.
- **Write it somewhere outside the repo** (Downloads, a save dialog) and
  leave moving it into the vault to the user's normal editor workflow.
  Keeps tenet 1 fully intact, but weakens "easily transferable" — the user
  still has to go do the transfer by hand afterward.
- **A separate companion CLI**, not a GUI button in the read-only app —
  `dreamd import file.docx > note.md` as its own small tool. Keeps the
  read-only app itself untouched by this tenet question entirely, at the
  cost of it not being "a button."

## Conversion mechanism, once the format is picked

Pandoc (shelled out) is the usual answer for docx/rtf → markdown and would
be the pragmatic choice over hand-rolling a parser — but per tenet 3 ("no
shell interpolation of user content"), it'd need to run via an argument
array with the file path passed as a discrete arg, never string-built,
mirroring how `send.rs` already avoids interpolating content into a shell
command.

## Open question

Which format(s) did you actually mean? That decides both the conversion
tool and how much tenet-1 tension this idea actually creates in practice.
