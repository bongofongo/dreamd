# Keybind: next/previous file

Move through the repo's markdown files by keybind, without touching the file
tree — same traversal feel as the `/` search idea.

## Current state

No such navigation exists. The file tree (`fs_walk::FileNode`, a nested
tree) and the search/palette index are the two places an ordered file list
already exists in some form, but neither is flattened into a single
"next file after this one" sequence today.

## What's needed

- A flattened, stable ordering over the tree (depth-first over
  `FileNode`, matching the order the sidebar already displays, is the
  obvious choice — don't invent a second ordering).
- `Keymap` entries (`next_file`/`prev_file` or similar) + settings-panel
  action entries + keydown branches, same pattern as the other keybind
  ideas.
- Decide behavior at the ends of the list — wrap around, or stop.

## Open question

Default keys — `]`/`[` (common "next/prev thing" convention across editors)
or something else?
