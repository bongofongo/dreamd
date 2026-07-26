# Navigational preview

Distinct from a reading-progress indicator: not "how far am I," but a preview
surface for getting *around* the document quickly — closer in spirit to a
minimap or thumbnail strip than a percentage bar.

## Current state

Nothing exists here today, and it's the least specified of the current
ideas — worth narrowing down before building anything. Candidate shapes,
roughly in order of how much they'd cost:

- A heading-based jump list (this may just be `ideas/contents-outline-panel.md`
  under another name — worth confirming whether these are meant to be the
  same feature or two different ones).
- A scrollbar-track minimap in the spirit of VS Code/Sublime — a zoomed-out,
  unreadable-text visual of document shape you click/drag to jump around.
- A hover-preview: skim the heading list (or scrollbar), and see a snippet of
  that section's content before committing to jump there.

## Needs from you

This one needs another pass of dialogue before it's buildable — which of the
shapes above (or something else) matches what you meant by "just a
navigational preview"?
