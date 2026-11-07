# gitclient UI style

Every window, panel, and component in gitclient follows this document. The visual
reference is the design canvas: [link removed]
(private to its owner). If the canvas and this file disagree, this file wins until it
is updated.

The look is dark, modern, and minimal: Catppuccin Mocha, one typeface, and as few
controls on screen as the task allows. The app follows the OS appearance, so a light
OS gets the Latte counterpart described under "Light mode". Someone new should understand the state of
their repository in a few seconds.

## Principles

1. **State it in a sentence.** The header says where you are in plain words, such as
   "On feat/commit-graph, 2 commits to push." Prefer a sentence over a row of icons.
2. **One next action.** Each screen has at most one filled blue button: the thing to
   do next. Everything else is a quiet text button.
3. **Your branch is the straight line.** HEAD's history runs straight down the first
   lane, and other branches bend around it.
4. **One colour, one meaning.** A branch keeps its colour in the sidebar, lane, and
   chips. Semantic colours are never branch colours.
5. **Name things by branch.** Never show "ours" or "theirs". They swap meaning during a
   rebase, so label each side with its branch name.
6. **Say what will happen.** Controls name their result ("Push 2 commits", not "Push").
   Before a risky step, say how to undo it.

## Colour

Use the Catppuccin values listed here only. Do not invent colours, gradients, or new
greys. The tables give Mocha; see "Light mode" for Latte.

| Token      | Hex       | Use                                      |
| ---------- | --------- | ---------------------------------------- |
| Crust      | `#11111b` | Text on filled blue, code inputs         |
| Mantle     | `#181825` | Header, side panels, diffs               |
| Base       | `#1e1e2e` | Main content, the graph                  |
| Surface 0  | `#313244` | Borders, selected row, disabled buttons  |
| Surface 1  | `#45475a` | Input outlines                           |
| Overlay 1  | `#7f849c` | Line numbers and idle checkbox outlines  |
| Overlay 2  | `#9399b2` | Muted text: labels, times, authors       |
| Subtext 0  | `#a6adc8` | Secondary text                           |
| Subtext 1  | `#bac2de` | Quiet button text                        |
| Text       | `#cdd6f4` | Primary text                             |

Overlay 1 is too low in contrast for body text. Use Overlay 2 or lighter for anything
that must be read.

Semantic colours:

| Colour | Hex       | Meaning                             |
| ------ | --------- | ----------------------------------- |
| Blue   | `#89b4fa` | You: HEAD, your branch, next action |
| Green  | `#a6e3a1` | Added, finished                     |
| Red    | `#f38ba8` | Removed                             |
| Yellow | `#f9e2af` | Modified, tags                      |
| Peach  | `#fab387` | Paused, needs a decision            |

Branch lanes take these colours in order, then repeat: Mauve `#cba6f7`, Teal
`#94e2d5`, Pink `#f5c2e7`, Sky `#89dceb`, Rosewater `#f5e0dc`, Flamingo `#f2cdcd`.
`main` is the first of these. Lanes never use Blue, Green, Red, Yellow, or Peach.

### Light mode

Mocha is the design. When the OS asks for light, every token keeps its role and takes
the same-named Catppuccin Latte value (Base `#eff1f5`, Text `#4c4f69`, Blue `#1e66f5`,
and so on). Rules for usage are unchanged, and designs are reviewed in Mocha.

Never carry meaning by hue alone. Pair colour with a letter (`M`, `A`), a shape, or a
word.

## Type

Maple Mono is the only typeface. Build hierarchy from size, weight, and colour. Ligatures
are off, so that code reads as it was written.

| Role            | Size / line height | Weight |
| --------------- | ------------------ | ------ |
| Commit title    | 18 / 24            | 600    |
| Status sentence | 15 / 20            | 400    |
| Lists and body  | 14 / 20            | 400    |
| Section label   | 13 / 18            | 500    |
| Code and diffs  | 13 / 22            | 400    |
| Chips           | 12                 | 600    |

In commit subjects, draw the Conventional Commit prefix (`feat: `) in Overlay 2 so the
description stands out. Draw merge commit subjects in Subtext 0.

## Layout

Designs target 2560×1440 and must stay usable at the 1200×800 default window.

- Header: 52 px, Mantle, with a 1 px Surface 0 bottom border.
- Graph window: 300 px branch sidebar, graph, 380 px inspector. The diff sits under the
  graph, so history stays visible while reading it.
- Task screens (commit, conflict) replace the sidebar and link back to the graph.
- Commit rows are 32 px, or 28 px in compact density. Lanes are 16 px wide. Rows have no
  column headings or hash: the subject, then chips, then author and a short time
  ("12m", "3h", "Tue", "Oct 30"). The full hash belongs to the inspector.
- Radii: 4 px for chips and checkboxes, 6 px for buttons and inputs, 8 px for cards.
- Separate regions with Surface 0 borders and surface changes. No drop shadows, no
  coloured side stripes on cards, no emoji.

## Graph

| Mark            | Drawing                                      |
| --------------- | -------------------------------------------- |
| HEAD            | Filled blue dot with a blue ring, first lane |
| Working changes | Hollow dashed blue dot and a dashed line     |
| Commit          | Filled dot in its branch colour              |
| Merge           | Hollow dot in its branch colour              |

- Lines are 1.75 px with curved bends, and show no avatars.
- At most 8 lanes are visible. A commit past the edge gets a small dot in its branch
  colour at the graph's right edge, so no commit disappears.
- Your branch line is blue down to where it forks. Shared history below that takes
  `main`'s colour.
- Clicking a branch spotlights it: everything outside its history fades to about 28%.

## Components

- **Primary button:** 32 px, Blue fill, Crust text, weight 600. One per screen.
- **Quiet button:** text only, Subtext 1, with a faint hover fill.
- **Ref chips:** tinted with 14% of the branch colour, text in full colour. Only the
  current branch gets a solid fill. Chips follow the subject, so subjects stay aligned.
  A branch and its remote at the same commit are one chip with a small cloud.
- **Inspector actions:** "Check out" names the branch when one is at the commit. Commands
  that add a commit say how to undo it in the band under the header.
- **Files:** a status letter, then the path with its folder muted, then `+n −n`.
- **Diffs:** added and removed lines use a 10% green or red wash. Text stays Text.
- **Focus:** every control shows a 2 px blue focus ring, and each action has a key.

## In code

Set colours through the gpui-kit theme and read them from `cx.theme()`. Do not hard-code
colours in views. The themes are in `src/catppuccin.json` and the lane palette is in
`src/theme.rs`, along with the few tokens gpui-kit has no field for (Peach, Subtext 0,
Overlay 1). Maple Mono is bundled in `src/fonts`.

| gpui-kit theme field | Value     |
| -------------------- | --------- |
| `background`         | Base      |
| `foreground`         | Text      |
| `muted_foreground`   | Overlay 2 |
| `border`             | Surface 0 |
| `list_head`          | Mantle    |
| `list_active`        | Surface 0 |

Lanes: `GraphBuilder` reserves the first lane for HEAD before any commit arrives, and
`Refs` assigns each branch one colour, so lanes and chips can't disagree. Branches past
the default take palette colours in name order, so a colour doesn't change as commits land.
