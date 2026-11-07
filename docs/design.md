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
| Commit title    | 16 / 22            | 600    |
| Status sentence | 14 / 20            | 400    |
| Lists and body  | 13 / 18            | 400    |
| Section label   | 12 / 16            | 500    |
| Code and diffs  | 12 / 20            | 400    |
| Chips           | 11                 | 600    |

These follow common desktop and editor practice, where interface text is 12 to 13 px
and code is 12 to 13 px. Larger text would crowd the three-panel layout at 1200×800.

In commit subjects, draw the Conventional Commit prefix (`feat: `) in Overlay 2 so the
description stands out. Draw merge commit subjects in Subtext 0.

## Layout

Designs target 2560×1440 and must stay usable at the 1200×800 default window.

- Header: 52 px, Mantle, with a 1 px Surface 0 bottom border.
- Tabs: one repository per tab, in a 36 px Mantle strip above the header with a 1 px
  Surface 0 bottom border. The strip only shows from the second tab on, since a lone tab
  is a control that does nothing. A tab is a 28 px quiet button with the folder name; the
  active one takes the selected-row fill (Surface 0) and Text colour, the rest are Overlay 2.
  Tabs have a `×` to close and the strip ends in a `+`. `Ctrl T` opens a tab, `Ctrl W`
  closes one, `Ctrl Tab` and `Ctrl Shift Tab` move between them, and `Ctrl O` opens a
  repository in a new tab, or in the current one while it is empty. Opening a repository
  that is already open switches to its tab. Closing the last tab leaves an empty one.
- Graph window: 300 px branch sidebar, graph, 380 px inspector. The diff sits under the
  graph, so history stays visible while reading it.
- The sidebar starts closed below 1500 px, because the two side panels would leave 520 px
  for the graph at the default window. A quiet button and `Ctrl B` open it at any width.
- Task screens (commit, conflict) replace the sidebar and link back to the graph.
- Settings: a quiet "Settings" button at the end of the header, or `Ctrl ,`, opens a
  second window (1000×720) and brings it forward if it is already open. It has the 52 px
  header, a 220 px Mantle side panel of pages, and the page in a column up to 720 px wide.
  The header sentence says what was saved or why not. There is no Save button: toggles and
  choices apply when clicked, text applies on Enter or when you leave the field. Settings
  sit in Mantle cards (8 px radius) grouped under section labels. A choice is a row of
  chips, the picked one with a tick and the Surface 0 fill, never blue. Closing settings
  leaves the app open; closing the main window quits it.
  - **Git config** edits one config at a time, "Everywhere" or "Only <repository>", chosen
    with chips above the cards. A setting the repository leaves unset says where it
    inherits its value from, and its reset button says "Inherit". A setting the installed
    git is too old for is dimmed and says which version it needs. If git can't be run, the
    page says so and offers "Try again".
  - **Identities** are named profiles (a name, email, signing key and an optional SSH key)
    that are reused
    wherever they are picked. The first card says who the open repository commits as and
    why, with one row of chips to change it: Automatic, then each identity. The picked chip
    has a tick, as in every choice. Below, the Default (the global name and email) is the
    first identity, and "This repository's own" appears as a chip when the repository sets
    its own. Each identity lists the rules that pick it (an SSH host, a folder or a remote
    pattern), and one identity can have many. A repository's pick wins over rules, and a
    name set only in a repository is kept as an identity when another is picked. The SSH
    key is a path, typed or picked from chips for the key pairs in `~/.ssh`, and makes git
    fetch, pull and push with that key alone wherever the identity applies. There is no
    page for SSH config: the app never reads or writes it beyond suggesting host names.
- Dock: a 36 px Mantle bar along the bottom of the window with a 1 px Surface 0 top
  border, always visible. Open, it adds a body of 40% of the window height (160 to 420 px)
  under the bar, so history stays in view. `Ctrl \`` shows and hides it, and
  `Ctrl Shift \`` opens a terminal. Closed, the bar still says which command is running
  and for how long, so a slow command is never a silent wait.
- Commit rows are 32 px, or 28 px in compact density. Lanes are 16 px wide. Rows have no
  column headings or hash: the refs column, the graph, the subject, then author and a short time
  ("12m", "3h", "Tue", "Oct 30"). The full hash belongs to the inspector.
- Resizing: every panel edge drags, so a panel can be made smaller or bigger. These are
  the sidebar, inspector, diff, dock and its log list, the commit screen's file list and
  form, the conflict steps, and the settings side panel. The pointer shows a resize cursor
  over a 4 px strip on the edge, which tints 50% Blue. A panel stops at a minimum, and at
  the point where the main area would be under 320 px. Sizes last until the window closes.
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
  `Ctrl`-click adds or removes branches, `Shift`-click picks the range down from the last
  one, and the graph lights all of their histories. Right-clicking one of several picked
  refs opens a menu for all of them ("Delete 3 branches"), which asks first and lists the
  command to bring each back. A checked-out branch and remote branches stay, and the menu
  says so.
- A branch row ends with its count against the default branch (`+2 −1`), then against
  the remote branch it tracks (`↑1 ↓2`: to push, to pull). Level or untracked shows nothing.

## Components

- **Primary button:** 32 px, Blue fill, Crust text, weight 600. One per screen.
- **Quiet button:** text only, Subtext 1, with a faint hover fill.
- **Ref chips:** tinted with 14% of the branch colour, text in full colour. Only the
  current branch gets a solid fill. Chips sit in their own 180 px column left of the graph (one chip, then `+n`), so subjects stay aligned. Hovering a commit with more
  refs opens a card over the column that lists every one, and each chip on it drags and
  right-clicks like the first.
  A branch and its remote at the same commit are one chip with a small cloud.
- **Menus:** right-clicking a branch, tag or commit opens one menu, and so does dropping
  a branch or tag on a branch. A card on Mantle with a Surface 0 border, 8 px radius and
  28 px lines, no shadow. A quiet heading says what it applies to; lines name their result
  ("Merge feat/x into main", "Rebase main onto feat/x"). A line that can't be used stays,
  in Overlay 2, and says why. Only lines that remove something are red, and they ask
  first, saying how to undo. `Up`/`Down` move, `Enter` chooses, `Esc` closes.
- **Drag and drop:** branches and tags drag from the sidebar and from the refs column,
  and drop on local branches, which tint with 14% Blue. Dropping never acts: it opens a
  menu of what fits how the two histories relate (fast-forward, merge, rebase).
- **Selecting commits:** a click selects one, `Ctrl`-click adds or removes one, and
  `Shift`-click adds the range. Selected rows take the selected-row fill, and the menu
  then acts on all of them.
- **Working changes:** the inspector opens with the status sentence and the one blue
  button, "Review and commit", and lists the changed files under them exactly as it does
  for a commit, so a file's diff is one click away. Diffs open on a click, never on
  their own. The list and an open diff refresh when the window is used again.
- **Inspector actions:** "Check out" names the branch when one is at the commit. Commands
  that add a commit say how to undo it in the band under the header.
- **Commit form:** the bump card only shows when the repository has `v1.2.3` style tags to
  count from. Hunks start ticked, except that files with nothing staged start unticked
  when something else is staged, so work staged from a terminal carries over.
- **Conflicts:** buttons name the branch ("Use feat/x"), never "mine", in line with
  principle 5. A file with nothing to merge line by line, such as a binary file, is one
  choice between the two sides.
- **Files:** a status letter, then the path with its folder muted, then `+n −n`.
- **Diffs:** added and removed lines use a 10% green or red wash. Text stays Text.
- **Dock tabs:** the bar holds tabs like the repository tabs, the same 28 px quiet button.
  The first is always the **Log**, then one per terminal, a `+` to open another, and
  `Clear` and `Hide` at the right. Terminals start in the repository's folder and keep
  their number as others close.
- **Log:** a 280 px sidebar of commands beside the output of one. The sidebar lists the
  last 50, newest first, each as its short form (`status --short`, without the
  repository) with how long it took at the right in Overlay 2, or `running`, or in Red
  why it failed (`exit 128`). The picked row takes the selected-row fill. Up and Down
  move through it. Newest is picked until you pick another, and picking the newest
  follows new commands again. The pane shows that one command as the app ran it, after a
  muted `$`, in weight 600 and Text colour, with how it stands at the right in words:
  `running · 12s`, `exit 0 · 0.2s` in Green, `exit 128 · 0.2s` in Red. Its output sits
  under it as it arrives, stdout in Subtext 0, stderr in Overlay 2, and in Red when the
  command failed. A running command keeps to its newest line unless you scroll up.
- **Terminal:** code type on Mantle. The cursor is a Blue block with Crust text and shows
  only while the terminal has focus. Colours the shell asks for by number use Catppuccin's
  terminal mapping: black Surface 1, red, green, yellow, blue, Pink for magenta, Teal for
  cyan, white Subtext 1, and the bright set is the same but for Surface 2 as bright black
  and Subtext 0 as bright white. Latte takes the same-named Latte values. The 256-colour
  cube and the grey ramp are the standard ones, since programs rely on them.
- **Focus:** every control shows a 2 px blue focus ring, and each action has a key.

## In code

Set colours through the gpui-kit theme and read them from `cx.theme()`. Do not hard-code
colours in views. The themes are in `src/catppuccin.json` and the lane palette is in
`src/theme.rs`, along with the few tokens gpui-kit has no field for (Peach, Subtext 0,
Overlay 1). Maple Mono is bundled in `src/fonts`. Size text with the `TEXT_*` constants in
`src/view/parts.rs`, one per row of the type table, and not with literal pixels. Anything more than one region draws
(the header bar, panel rows, file paths, diff line colours) lives in `src/view/parts.rs`.

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
