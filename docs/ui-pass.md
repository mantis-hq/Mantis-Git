# UI pass

Working list from the design critique. One item at a time; tick when merged.
Decisions come from the owner, not from the critique. Constraints: Maple Mono only,
Catppuccin only, no explanatory "reason" copy (see 3).

Status: `[ ]` open, `[~]` in progress, `[x]` done.

## 1. Graph: avatars and merges  `[x]`

Decided:
- Keep avatars on commits.
- A merge always draws a hollow dot in its branch colour, never an avatar or icon.
- Avatars and lanes do not overlap. Leave the 20 px circle and 16 px lane pitch alone.

To do:
- Merge rows: hollow dot, no avatar (reverses "Merges lose their hollow dot to the avatar" in `docs/design.md`).
- Update `docs/design.md` (Avatars, Graph table) and add a test in core for what a merge draws.

## 2. Refs column  `[x]`

Decided: it scales with what is in it (not a fixed 128 px).

To do:
- Width from the widest first chip, 48 to 180 px; `+n` and the hover card stay.
- The subject column takes the freed width. Chip nearer its dot follows from the narrower column.

## 3. Inspector  `[x]`

Decided: do the suggestions, but write no reason text under a disabled Commit button.

To do:
- "Discard all": quiet, Overlay 2, normal colour on hover.
- Selected commit-type chip (`feat`): ticked with the Surface 0 fill, not solid blue.
- Unstaged and Staged lists size to their content up to a cap, instead of reserving blank space.
- Disabled "Commit n files" only dims. No sentence.

## 4. Sidebar sections  `[~]`

Decided: sections may be closed by default.

Open question: what happens to an empty Stashes (0) and Submodules ("no submodules").

## 5. Type and symbols  `[ ]`

Not yet discussed:
- Weight and colour hierarchy in the graph rows (HEAD row semibold; Hash, Author, Date quieter).
- The bare ✓ for "merged" and the green release card.
