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
- Graph window: 300 px branch sidebar, graph, 380 px inspector. A file's diff opens over
  the whole graph, between the sidebar and the inspector, so it has the room to be read.
  The inspector stays beside it for picking another file, `Esc` or "Close" brings the graph
  back, and the diff has a 40 px header: the path, two chips "Changes" and "Whole file"
  (the picked one ticked), and a "Hide white space" chip that ticks when on. The choices
  last until the window closes and apply to every file. In the whole file, where changes can
  be screens apart, each run of added or removed lines is marked beside the scrollbar in Green
  or Red, at its place in the file. A file whose only changes were white
  space says so. For working changes the header starts with a tick box for the whole file,
  and each hunk's header starts with one too (see "Staging").
- The sidebar starts closed below 1500 px, because the two side panels would leave 520 px
  for the graph at the default window. A quiet button and `Ctrl B` open it at any width.
- There are no task screens. Committing is the foot of the inspector, and so are conflicts
  (see "Conflicts"): the graph window is the one window for everything about a repository.
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
  - **App** holds what gitclient itself remembers, apart from git. The first is "Reopen
    repositories on launch", on by default: a tick box in a card under "Startup", with a
    sentence that says what it does now. The open repositories and the front tab are saved
    as they change, in a small text file in the system's config folder (`state` inside
    `gitclient`), and empty tabs are left out. A path on the command line wins over the
    remembered tabs, and a folder that is gone is skipped. If there is no config folder the
    page says so and the tick box is not drawn.
    Under "Graph", a row of chips picks how many commits the graph loads from a repository
    (100, 500 by default, 1000, 5000, 20000), kept as `commit_limit` in the same file. The
    newest commits by history order come first. Scrolling to within 200 rows of the end
    loads the next as many, in the same lanes, and a reload keeps what was already
    scrolled into view. A change applies the next time a repository loads.
    Under "Cache", a tick box "Keep a cache on disk" (on by default) says that what git
    reports about commits and their file diffs is kept between runs, so a commit seen before
    opens at once. When it is on, a row of chips picks the size limit (64 MB, 256 MB by
    default, 1 GB, 4 GB), and a row says how much is in use beside a quiet "Clear cache"
    button. Changes apply at once to the running app. The cache lives in the system's cache
    folder (`$XDG_CACHE_HOME` or `~/.cache` on Linux), never in the config folder, and an
    entry that is damaged or out of date is simply read from git again.
- Dock: a 36 px Mantle bar along the bottom of the window with a 1 px Surface 0 top
  border, always visible. Open, it adds a body of 40% of the window height (160 to 420 px)
  under the bar, so history stays in view. `Ctrl \`` shows and hides it, and
  `Ctrl Shift \`` opens a terminal. Closed, the bar still says which command is running
  and for how long, so a slow command is never a silent wait.
- Commit rows are 32 px, or 28 px in compact density. Lanes are 16 px wide. Rows have no
  column headings or hash: the refs column, the graph, the subject, then author and a short time
  ("12m", "3h", "Tue", "Oct 30"). The full hash belongs to the inspector.
- Resizing: every panel edge drags, so a panel can be made smaller or bigger. These are
  the sidebar, inspector, dock and its log list, and the settings side panel. The pointer shows a resize cursor
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
- Worktrees: when a repository has more than one working tree, the sidebar lists them
  under "Worktrees", open by default, between Remotes and Tags. A row is the folder's
  name, then its branch (or `detached at <hash>`) in Overlay 2, with a dot in that
  branch's colour. The one on show is selected and semibold. Clicking another switches
  the current tab to it, so the tab strip doesn't grow. A worktree whose
  folder is gone is dimmed and says so.
  Right-clicking one opens the usual menu: "Switch to <name>", "Open <name> in a new tab",
  "Copy path", and in red "Remove worktree <name>", which asks first and gives the
  `git worktree add` that brings it back. The main worktree, the one on show and a locked
  one stay, and the menu says why. Git refuses to remove one with uncommitted changes,
  and the app never forces it. With one working tree the section isn't drawn.
- Submodules: when a repository has any, the sidebar lists them under "Submodules", open
  by default, as the last section, after Stashes. A row is the submodule's path, then where it stands in
  Overlay 2 (its short commit, `moved to <hash>`, `not downloaded` or `in conflict`), with
  a dot that says the same: Green at the commit the repository records, Yellow on another,
  Peach in conflict, Overlay 2 when not downloaded. Clicking a downloaded one opens it in
  a tab of its own, or brings its tab forward. Right-clicking opens the usual menu: "Open
  <path> in a new tab", a line that names its result ("Download lib at 1a2b3c4", "Check
  out 1a2b3c4 in lib", "Update lib"), "Update 2 submodules" when more than one needs it,
  and "Copy path" and "Copy URL". A line that can't be used stays and says why. Checking
  out the recorded commit in a submodule that is on another one asks first, and gives the
  `git -C <path> checkout <hash>` that goes back. The app never forces anything. With no
  submodules the section isn't drawn.
- New branch: a quiet "New branch" link ends the Branches heading, and branch, tag and
  commit menus have "New branch from <name or hash>". Either opens a form at the top of the
  sidebar (the sidebar opens if closed): a section label saying where the branch starts
  ("New branch, starting at main"), a name input with the cursor in it, and quiet "Cancel"
  and "Create <name>" buttons, never blue. Enter creates it. The new branch is checked out,
  and the band under the header says how to undo it. A name git would refuse, or one
  already taken, is explained in that band and nothing changes.
- Remotes: the "Remotes" header ends in a quiet blue "+ Add" that opens a form band under
  the header (the notice band's look), with a name, an address, and quiet "Cancel" and
  result-named buttons ("Add remote", "Save origin"). `Enter` saves and `Esc` closes. Each
  remote's top folder in the list, or a row saying "no branches yet" for one nothing has
  been fetched from, opens a menu on right-click: "Edit <name>" (the same form, filled in;
  renaming also renames its remote branches), "Copy address", and in red "Remove remote
  <name>", which asks first and gives the `git remote add` that brings it back. Names that
  can't work are turned down in the form in Peach, before git runs.
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
- **Delete confirmation:** deleting a branch, tag, worktree or remote asks in a small popup centred
  over the window, never in the band under the header. A card on Mantle with a Surface 0
  border and 8 px radius holds the sentence (which says how to undo it), a line naming the
  keys, and "Cancel" and the result ("Delete") as quiet buttons. `Enter` confirms and `Esc`
  cancels; clicking outside cancels too. A held-down key never answers. Deleting a branch
  that is neither merged into the default branch nor pushed loses commits nothing else
  keeps, so `Enter` has to be pressed twice within 600 ms: the first press turns the key
  line Red and says "Press Enter again to delete." Rules for which is which live in core
  (`Weight` in `crates/core/src/menu/command.rs`). Other risky steps (rebase, force push, hard
  reset) still use the band.
- **Double-click:** double-clicking a branch, in the sidebar or on a chip, checks it out. A
  remote branch gets a local copy that follows it. The checked-out branch does nothing.
- **Drag and drop:** branches and tags drag from the sidebar and from the refs column,
  and drop on local branches, which tint with 14% Blue. Dropping never acts: it opens a
  menu of what fits how the two histories relate (fast-forward, merge, rebase).
- **Selecting commits:** a click selects one, `Ctrl`-click adds or removes one, and
  `Shift`-click adds the range. Selected rows take the selected-row fill, and the menu
  then acts on all of them.
- **Working changes:** the inspector opens with the status sentence, the List and Tree
  chips, then two file lists above each other, and the commit form under them. The top list
  is "Unstaged" and holds only the files with changes the index doesn't have, the bottom
  one is "Staged" and holds only what the next commit contains. A file changed both ways is
  in both, each with its own diff. A list with no files says so in a sentence. Diffs open on
  a click, never on their own. The lists and an open diff refresh when the window is used
  again.
- **Staging:** the check marks of the commit form are the index. A file, a folder and a
  whole list each start with a tick box, ticked in "Staged" and clear in "Unstaged";
  ticking one in "Unstaged" stages it and clearing one in "Staged" unstages it, and the
  file moves to the other list at once. A folder's box moves everything under it, and a
  list's header box moves everything in it. In a diff of working changes each hunk's header
  has the same box, with "left out" or "in this commit" after its title, and the file's box
  sits before its path. A file that is added, removed or binary has no hunks to move on
  their own, and nor does a diff that is showing the whole file or hiding white space. The
  diff follows a file to the other list when its last hunk moves. Unstaging never touches
  the file itself.
- **File menus:** right-clicking a file in the inspector opens the usual menu: its path as
  the heading, "Show changes in <name>", for working changes "Stage <name>" or "Unstage
  <name>", and "Copy path". A folder in the tree has "Stage 3 files in <name>/" (or
  "Unstage"), and "Copy path".
- **Merges in the inspector:** a merge lists and diffs what it brought in over its first
  parent, the branch it was made on, so its files match what reverting it with `-m 1` undoes.
- **Inspector actions:** "Check out" names the branch when one is at the commit. Commands
  that add a commit say how to undo it in the band under the header.
- **Long lists:** lists that can grow without limit, such as a commit's files, the two
  lists of working changes and the sidebar, draw only the rows in view. Their rows share one height.
- **Loading:** where a list is still being read, a small spinner and a sentence saying
  what is coming ("Loading changed files…"), over grey placeholder rows in the Surface 0
  fill that breathe between 35% and 100% opacity. The placeholders are as tall as a real
  row, so the list takes their place without a jump. It is one component (`loading` in
  `src/view/parts.rs`) for every list that loads. Picking another commit stops that read.
- **Commit form:** the foot of the inspector for working changes, under the two lists and
  a Surface 0 border. Its heading is a quiet "Commit" with an arrow, which folds the form away
  to give the lists the room (folded, it says how many files are staged), and the one blue
  button, named for its result ("Commit 2 files", or "Stage a file to commit" while nothing
  is staged and nothing can happen), so it is never out of sight. Under it: the kinds of
  change as chips in one line, the commit line as it will be written with its length against
  72, the summary, a details box, "Breaking change" for `feat`, and a one-line card for what
  landing it would do to the version, shown only when the repository has `v1.2.3` style tags.
  It commits exactly what is staged. After a commit the form empties, and the band under the
  header says how to undo it. If a hook refuses, the message stays. The form takes at most
  42% of the inspector and scrolls past that, and each list keeps at least 110 px.
- **File lists:** the inspector's lists of a commit's (or the working changes') files share
  one choice. Its header always holds two chips, "List" (every file by its path, the
  default) and "Tree" (files grouped by folder), the picked one with a tick. A folder row
  has an arrow, in working changes a tick box (see "Staging"), its name in Overlay 2 and a
  file count. A click opens or closes it. A chain of folders with one child each is one row
  (`crates/core/src/`), and folders come before files. The choice applies to every repository
  and is saved at once as `files_view` in the app's `state` file.
- **Conflicts:** a paused rebase, cherry-pick, revert or merge shows in the inspector, and
  the working changes are selected when one appears. The inspector then holds a Peach pill
  ("Rebase paused"), the sentence that says where it stopped, a "Conflicts" list of the files
  that need a decision (a "!" and how many conflicts are left, or a tick and "Resolved" once
  saved, which stay listed until the operation goes on), and at the foot "Cancel rebase" and
  the one blue "Continue rebase", which waits until every file is saved. The two lists and the
  commit form wait while it is paused.
  Clicking a file opens the resolver over the graph, as a diff does, and `Esc` or "Close"
  brings the graph back. It is one file at a time, in three parts that share the height:
  - **Conflicts**, shown in the context of the file, as a list of rows that draws only
    those in view, so a file of any size costs the same. A bar over it names the two
    branches. The lines both sides agree on are plain rows with their line numbers; each
    conflict is a header row, "Conflict 2 of 3 · line 40" with "Use feat/x", "Use main" and,
    once lines are ticked, "Use 3 selected lines" (the ticked lines of the first side, then
    the second's), and under it the two sides' lines next to each other, each with a tick box
    and a tint in its branch's colour. A conflict already settled in the result dims, says so
    and loses its buttons. The branches are named, never "mine" or "theirs", in line with
    principle 5. The header has the same "Changes" and "Whole file" chips as a diff, and they
    are the same setting: "Changes" shows each conflict with three lines of context around it
    and "⋯ 17 unchanged lines" for what was left out, "Whole file" shows every line and marks
    each conflict beside the scrollbar in Peach.
  - **Result**, the whole file in an editor, which starts as the file as it stands with its
    conflict markers. The cards edit it, and so can the user, by hand, anywhere. A conflict
    counts as settled when its markers are gone from the result, however that happened. The
    top bar says how many are left ("2 conflicts left in the result", Peach) or "Ready to
    save" (Green).
  - **Save and mark as resolved**, a large blue button across the foot, enabled once no
    conflict is left. It writes the result and stages the file, which takes the conflict state
    off it, then opens the next file that needs a decision, or closes when there is none.
  A file with nothing to merge line by line, such as a binary file or a deleted one, has two
  cards with a "Use feat/x" choice each instead of the editor, saved with the same button.
  Right-clicking a conflicted file offers "Show both sides", "Use feat/x" and "Use main" for
  the whole file, which save it at once, and "Copy path". When the operation finishes, the
  band under the header says what is now true.
- **Files:** a status letter, then the path with its folder muted, then `+n −n`.
- **Diffs:** each hunk is a card on Mantle with a Surface 0 border and 8 px radius, over the
  Base of the pane. Its header has the hunk's tick box (working changes), where it starts or
  what git says it is inside, and "left out" or "in this commit". Its lines have one line
  number, a sign and the text. Added and removed lines use a 10% green or red wash. Text
  stays Text. The file's name is in the bar above, once.
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
  only while the terminal has focus. While it has focus it takes every key, Tab, Ctrl C
  and Ctrl W included, before any shortcut of the app, except `Ctrl \`` and
  `Ctrl Shift \``. Colours the shell asks for by number use Catppuccin's terminal
  mapping: black Surface 1, red, green, yellow, blue, Pink for magenta, Teal for cyan,
  white Subtext 1, and the bright set is the same but for Surface 2 as bright black
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
