# Mantis Git UI style

Every window, panel, and component in Mantis Git follows this document. The visual
reference is the design canvas, which is kept outside the repository
and private to its owner. If the canvas and this file disagree, this file wins until it
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
greys. The tables give Mocha; see "Light mode" for Latte. Catppuccin is the default and
the design: a user can change these tokens on the Theme settings page, but the app itself
never adds a colour that is not one of them, and every rule here holds whatever values they
take.

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
  Surface 0 bottom border. The strip always shows, even for one tab. On Windows it is also
  the title bar: the window has no frame of its own, the empty part of the strip drags the
  window, and minimise, maximise and close end the row (46 px wide, Surface 0 on hover,
  Red for close). A tab is a 28 px quiet button with the folder name; the
  active one takes the selected-row fill (Surface 0) and Text colour, the rest are Overlay 2.
  Tabs have a `×` to close and the strip ends in a `+`. A new tab opens at the end of the
  strip. `Ctrl T` opens a tab, `Ctrl W` closes one, `Ctrl Tab` and `Ctrl Shift Tab` move between them, and `Ctrl Shift PageUp`
  and `Ctrl Shift PageDown` move the front tab along the strip. A tab also drags onto another
  to take its place, and the tab under it tints with 14% Blue; the order is saved with the
  open tabs (see "App"). `Ctrl O` opens a
  repository in a new tab, or in the current one while it is empty. On Windows a folder
  inside WSL2 (`\\wsl.localhost\Ubuntu\home\ada\repo`, or `\\wsl$\…`) opens like any other and
  is told apart by its path alone: git, and the dock's terminals, run inside the
  distribution through `wsl.exe`, started in the folder, and the paths git prints are turned
  back into Windows ones (`mantisgit_core::repo::wsl`). Nothing in the interface changes.
  Questions from git (see "Questions from git") are asked in the same popup, with nothing
  from Windows involved: a small script starts git in the distribution with its own
  askpass program, which prints the question on stderr after a marker and reads the answer
  from a fifo, since ssh gives its askpass program a stdin of its own; a loop in the script
  copies the app's answer from the script's stdin into the fifo. The app reads the marker,
  asks, and writes the answer back, so the ssh key
  and agent are the distribution's own. The marker carries a token made up for each
  command, so a remote can't print one of its own to make the app ask a question. Only
  commands whose subcommand connects to a remote (`fetch`, `pull`, `push`, `clone`, …) keep
  stdin open for this. A distribution that sets its own `SSH_ASKPASS` or `GIT_ASKPASS` keeps it.
  `Ctrl Shift O`, or "Clone from an address" under "Open repository" on an empty tab,
  opens a form popup ("Clone a repository") with an "Address" and a "Clone into" input and
  "Clone <name>" as its result, the name being the address's last part without `.git`. The
  address starts as what is on the clipboard when that is a link to a repository
  (`https://`, `ssh://`, `git://` or `git@host:path`), so copying a link and pressing the
  key is enough, and the folder starts as the one last cloned into (`clone_dir` in the
  `state` file) or else the home folder; `~` works in it. The folder made is
  `<Clone into>/<name>`, and an address or folder that can't work, or a folder that already
  holds something, is said in Peach before git runs. The popup stays open with the header
  spinner while git runs, and git's questions come in the same way as for a fetch. When it
  is done the repository opens like any other, in the front tab if that is empty, else in a
  new one. What it checks and runs lives in `mantisgit_core::repo::clone`. Opening a repository
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
  - **Theme** lets someone who likes to rice change the colours. Dark and light are two
    themes (chips "Dark" and "Light", starting on the one in use), each a list of changes on
    top of Catppuccin kept as `theme_dark` and `theme_light` in the `state` file
    (`base:1a1b26,blue:7aa2f7`), so a colour left alone stays as designed. The page has a
    "Colours" card with the appearance chips, a "Theme" row, then one import row for each
    desktop theme found, then cards for "Surfaces and text", "Meaning" and "Branch
    lanes": a row for each token of this document (`Slot` in `mantisgit_core::settings::theme`)
    with a 20 px swatch, its name and what it is for, a hex input and a quiet "Reset" while
    it is changed. The "Theme" row always offers the built-in theme, "Catppuccin Mocha" for
    dark and "Catppuccin Latte" for light, as a chip ticked while nothing is changed; choosing
    it puts every colour of that appearance back. Then come the theme files of that appearance
    (below), and once a colour is changed on the page a "Custom" chip is ticked. A typed
    colour applies in every window on Enter or when the input is left, and one that isn't six
    hex digits is said in the header and put back. Changing a colour while a theme file is
    picked starts from the file's colours and leaves the file alone, so the theme becomes
    "Custom" and nothing the user wrote is overwritten. The import rows
    are "Import KDE colours" (`kdeglobals`), "Import qt6ct palette" and "Import qt5ct palette"
    (the palette the tool points at), and "Import GTK 4 theme" and "Import GTK 3 theme" (the
    `@define-color` names in `gtk.css`, with the `@import`ed files beside it, else the theme
    `settings.ini` names). A row is drawn only for a theme that exists on this system, so
    none is, and no import card, on Windows, which has none to read. An import is written as
    a theme file (`kde.theme`, `gtk-4.theme`, and so on, replacing the last import of the
    same kind) and picked, so what it took can be seen and edited. Only colours are read:
    no fonts, sizes, icons or other CSS, and only the theme of the appearance the scheme's
    background suits is replaced. Base, Mantle, Text, the accent (Blue) and
    the text on it (Crust) come from the scheme and the greys between are mixed from Base
    towards Text, so contrast holds; Green, Red and Peach are taken only when the scheme gives
    a real colour and not a grey; Yellow and the lanes stay as designed. Colours are changed by
    swapping each Catppuccin value in the bundled theme file, alpha kept
    (`Overrides::recolor_themes`), so a colour the file uses that no token covers is a bug a
    test catches.

    **Theme files** let a theme live in the user's own folder, for dotfiles and sharing. The
    folder is `themes` beside the `state` file, and each `*.theme` file in it is a theme named
    for the file (`mantisgit_core::settings::themes`). A file holds comments (`#`), an
    `appearance = dark` or `light` line (taken from the Base colour when left out) and one
    `Name = #rrggbb` line for each colour it sets, using the design's own names in any case
    and spacing (`Surface 0`, `surface0`), with a note allowed after the value. A colour left
    out stays Catppuccin's. Nothing else can be set from a file: no fonts, sizes or settings.
    A file is read whole or not at all. A bad line (an unknown name, a value that isn't six
    hex digits, an unknown appearance) leaves the theme that was showing, and the Theme row
    says in Peach which file and which line ("night.theme, line 7: ..."); a file with no
    colours, one named like a built-in theme, and one for the other appearance are said the
    same way. The picked file of each appearance is kept by name as `theme_dark_file` and
    `theme_light_file` in the `state` file and wins over the colours changed on the page. If
    it is gone the built-in theme shows and the row says so. The folder is read again every
    2 seconds by the app, and the Theme page does so too while it is showing and whenever its
    window is used again, so saving a file in an editor shows in every window with no action.
    The "Theme files" card has "Save these colours as a theme", a name input and "Save"
    (Enter too), which writes every colour on show with a comment saying what each is for,
    so the first file is also the example of the format, refuses a name that is taken or
    can't be a file's name or is a built-in's, and picks the new file; and "Themes folder",
    which says where it is and has "Open themes folder" (made first, shown in the system's
    file manager). The card is not drawn where the app has no folder to keep settings in.

  - **Identities** are named profiles (a name, email, signing key and an optional SSH key)
    that are reused
    wherever they are picked. The first card says who the open repository commits as and
    why, with one row of chips to change it: Automatic, then each identity. The picked chip
    has a tick, as in every choice. Below, the Default (the global name and email) is the
    first identity, its name and email shown and changed on the Git config page but its SSH
    key set here like any other's, and "This repository's own" appears as a chip when the repository sets
    its own. Each identity lists the rules that pick it (an SSH host, a folder or a remote
    pattern), and one identity can have many. A repository's pick wins over rules, and a
    name set only in a repository is kept as an identity when another is picked. The SSH
    key is a path, typed or picked from chips for the key pairs in `~/.ssh`, and makes git
    fetch, pull and push with that key alone wherever the identity applies. There is no
    page for SSH config: the app never reads or writes it beyond suggesting host names.
  - **App** holds what Mantis Git itself remembers, apart from git. The first is "Reopen
    repositories on launch", on by default: a tick box in a card under "Startup", with a
    sentence that says what it does now. The open repositories and the front tab are saved
    as they change, in a small text file in the system's config folder (`state` inside
    `mantisgit`), and empty tabs are left out. A path on the command line wins over the
    remembered tabs, and a folder that is gone is skipped. If there is no config folder the
    page says so and the tick box is not drawn.
    Under "Remotes", a tick box "Fetch in the background" (on by default, `auto_fetch` in the
    same file) has the front tab fetch every remote every 5 minutes (`EVERY` in
    `mantisgit_core::repo::auto_fetch`), so what is ahead and behind stays true; the graph
    reloads only if a ref moved. It is a plain `git fetch --all`: no branch moves and none is
    deleted. It never asks a question, so a key with a passphrase no agent holds, or a login,
    makes it fail without a word (the Log has what git said) until a fetch by hand answers
    it. It skips a tab that is busy, still loading or behind another tab, and a fetch with
    the box clear is only the header's "Fetch".
    Under "Graph", a row of chips picks how many commits the graph loads from a repository
    (100, 500 by default, 1000, 5000, 20000), kept as `commit_limit` in the same file. The
    newest commits by history order come first. Scrolling to within 200 rows of the end
    loads the next as many, in the same lanes, and a reload keeps what was already
    scrolled into view. When refs only moved forward, a reload reads just the new commits
    and lays out the rest again; after a rewrite or a deleted branch it reads everything.
    A change applies the next time a repository loads.
    A card "Avatars" follows, with two rows that read as two groups of people. "You" is a
    person row: a 44 px preview of the avatar your commits get (your picture, or your
    initials on your colour), your name and email from git, and "Choose a picture" (a file
    dialog; PNG, JPEG, GIF, WebP or BMP up to 8 MB, checked by its first bytes) with
    "Remove" once one is set, kept as `own_avatar`. It is drawn on commits made with the
    email `git config user.email` gives in the open repository, and beats a Gravatar. "Everyone
    else" has the chips "Initials" (the default), "Gravatar" and "Plain dots" (`avatars` in
    the same file), the picked one ticked and never blue, and three overlapping 28 px
    samples in the picked style. Gravatar is opt-in because it sends gravatar.com a SHA-256
    hash of each author's email address, once per author per session, and its chip says so
    in one line; initials send nothing. With plain dots the "You" row goes. A file that
    can't be used is said in the header.
    Under "Cache", a tick box "Keep a cache on disk" (on by default) says that what git
    reports about commits and their file diffs is kept between runs, so a commit seen before
    opens at once. When it is on, a row of chips picks the size limit (64 MB, 256 MB by
    default, 1 GB, 4 GB). It is one limit for every cache together: when they are over it
    the least recently used entries of any of them go first, so a cache that fills gets the
    room the others leave (`Budget` in `mantisgit_core::cache`). A storage row says how much
    is in use ("Using 98 MB of 256 MB.") beside a quiet "Clear cache" button that empties
    every cache, and under it one 8 px bar, as long as the limit while the cache is on, cut
    into what each cache holds (commits, file diffs, avatars; a commit's files and its signature check are one part), largest first. The cuts
    take three of the branch palette's colours (Teal, Pink, Sky) with a gap of Mantle
    between them on a Surface 0 track, and a legend names each, its name in Text and its
    size ("< 1 MB" when under a megabyte) in Overlay 2, so no meaning rests on colour alone. Changes apply at once to the running app. The cache lives in the
    system's cache folder (`$XDG_CACHE_HOME` or `~/.cache` on Linux), never in the config
    folder, and an
    entry that is damaged or out of date is simply read from git again. What gravatar.com
    said about an author is kept there too (a few kilobytes each,
    found by the hash of the author's address): a picture until the cache is cleared, and
    "no Gravatar" for a week. Gravatar is asked about an address only when neither is there,
    at most once per session across every tab whatever the answer, so a failed request isn't
    repeated until the app restarts and is said once in the band, and four requests at a
    time. Addresses that can't have a Gravatar (`noreply`, `localhost`, `.local`, no `@`) are
    never asked.
    Only a copy unpacked from a release archive looks after itself, which is called
    portable. One a package manager installed (found by a build flag, `/usr`, Flatpak, an
    AppImage, Homebrew, Nix, Scoop and the like) leaves its launcher and its updates to the
    package manager, shows no "Launcher" card, and says on the About page who updates it.
    Under "Launcher", a tick box "Add Mantis Git to the application menu" ("Start menu" on
    Windows) is ticked when a launcher is on disk: on Linux a `mantisgit.desktop` and an SVG
    icon under `$XDG_DATA_HOME` (`~/.local/share` by default), on Windows one shortcut in the
    user's Start menu folder, and nothing on a system with neither. The sentence says where
    Mantis Git starts from. What is on disk is the truth, so a launcher added or removed by
    hand shows as it is; a `mantisgit.desktop` without Mantis Git's marker line is someone
    else's, is never replaced or removed, and has no tick box. Ticking or clearing it applies
    at once, and the header says how to undo it. Every start mends a launcher of Mantis Git's
    own that points at a copy that has moved, and never adds one that was taken away.
  - **Repository** holds what belongs to the open repository alone, apart from its git
    config, and is only in the side panel while a repository is open. The header says which
    one. Under "Hosting", a "Forge" row says which forge the remote is on ("Mantis Git can tell
    that origin is on GitLab", or that it can't tell) beside a row of chips, Automatic then
    GitHub, GitLab and Gitea, the picked one ticked and never blue. Automatic leaves it to
    Mantis Git (see "Forges"), and a choice is kept in the repository's own git config as
    `mantisgit.forge`. A choice applies at once, and the header says what it now does. Under "History", a "Commit graph" row says whether git's commit graph lists every
    commit the branches and tags reach ("The commit graph lists 72712 of the 73143 commits
    your branches and tags reach."), and why git can't use one in a shallow clone, with
    `core.commitGraph` off, or with replace refs. When something is missing a quiet button
    names the result ("Add 431 commits to the commit graph", "Write the commit graph") and
    runs `git commit-graph write --reachable --split`, as `git maintenance` does. If
    `gc.writeCommitGraph` or `maintenance.commit-graph.enabled` is off, the row says that
    git won't write it on its own. The page reads all this each time it opens. The main
    window says nothing about the commit graph unless it is the reason for a wait (see
    "Commit graph card"). Both sentences for a missing or behind graph add that in a large
    repository it makes branch counts, search and loading history slow.
  - **About** is the last page, always in the side panel. Its "Version" card says which
    build this is ("Mantis Git 0.1.0", with the commit in brackets for a nightly), what
    Mantis Git is licensed under and that it contains so many projects by others, with a quiet
    "Open the source on GitHub" button, then a row that names the installed git ("git 2.56")
    with a sentence under it, and under that holds everything about updating, in rows. A git
    older than 2.41 (`RECOMMENDED` in `mantisgit_core::settings::config`), which is the first
    with `for-each-ref --format=%(ahead-behind:…)` and so the first to count every branch
    against the default one in a single call, says what it lacks ("Branch counts in the
    sidebar need git 2.41 or newer.") and ends its row in a quiet "Get a newer git" that
    opens git-scm.com. The same
    git also gets a toast at launch, in the front tab, which gives way to any message already
    showing and closes like any other (`about::old_git_notice`). A git that is recent enough
    says only that it is; one that can't be run is said on the Git config page already, and
    the row repeats it. A tick box "Look for a newer version when Mantis Git starts" (on by default,
    `check_updates` in the `state` file), a row of chips "Which versions to update to"
    ("Full releases", the default, or "Nightly builds", kept as `update_track`, the picked one
    ticked and never blue) and a row that says where an update stands ("Mantis Git 0.2.0 is
    available. This is 0.1.0.") beside one quiet button named for its result: "Check now",
    "Update to 0.2.0" or "Restart now". A copy a package manager installed has one row saying
    who updates it, instead. Full releases are the newest tag that is a plain version
    (`v1.2.3`). The nightly is the one pre-release every push to `main` replaces; all
    nightlies share a version, so one is told from the next by the commit it was built from
    (`MANTISGIT_COMMIT`, set at build time), and is named "nightly 1a2b3c4". Choosing a
    track looks again at once. Releases are read from one host, GitHub, set in one
    place (`HOST` in `mantisgit_core::install::update`), and moving to another is a one-line
    change.
    A "Licenses" card lists the licenses that have to be shown for what is in the program:
    those that ask for their text to go with a copy, so 0BSD, CC0 and the Unlicense are left
    out (`License::must_be_shown`). There is one row for each license by name, so every
    crate under "MIT License" is in one row (`combine`). The list is a virtual list of 48 px
    rows across the card's full width, about six in view with a scrollbar, that draws only
    the rows in view. A row has the license's name and how many projects are under it, and
    the projects in Overlay 2 on a line under it. The first is Mantis Git's own, then Maple
    Mono's, then the crates'. A click picks one, which takes the selected-row fill and a bold
    name, and clicking it again puts it down. The picked license's text shows under the list,
    on Base in a pane of its own that scrolls past 320 px. Crates word a license a little
    differently, so a license has one text for each wording that really differs, with a rule
    between them and "Used by" over each, naming the projects or counting them past eight.
    Wordings that differ only in white space or in their copyright lines are one text. A
    copyright line is a line that begins "Copyright" and a year, "(c)" or "©", never one of
    the license's own sentences that merely say the word. Those lines come before the text,
    each set with a muted "— " line naming the projects it came with ("Copyright 2023 Jacob
    Pratt et al." — `deranged`, `powerfmt`), so one project's year and holder is never read
    as another's; the line is left out when every project of the wording shares it. No line
    is lost, and a test checks that each project keeps its own. The texts are inside the
    program, so they can't be separated from a copy that is
    passed on: `cargo about` writes them to `THIRD-PARTY-LICENSES.txt` from `Cargo.lock`
    (`about.toml` says which licenses are accepted, `about.hbs` the layout), CI fails when that
    file is out of date, and the release attaches it and `LICENSE` too. What the page says and
    how the file is read lives in `mantisgit_core::about`.
- Dock: a 36 px Mantle bar along the bottom of the graph column, under the graph and
  between the sidebar and the inspector, which keep their full height, with a 1 px
  Surface 0 top border, always visible. Open, it adds a body of 40% of the window height (160 to 420 px)
  under the bar, so history stays in view. `Ctrl \`` shows and hides it, and
  `Ctrl Shift \`` opens a terminal. Closed, the bar still says which command is running
  and for how long, so a slow command is never a silent wait. Terminals belong to the
  repository tab they were opened in, and start in its folder. The dock shows the front
  tab's terminals; switching tabs brings that tab's own back, and with the dock open a tab
  that has none gets a clean one at once. Each tab remembers which of its terminals was in front, and the Log is the dock's own
  choice: when it is in front it stays in front in every tab, and a tab's terminal is
  found again when a terminal is picked. Closing a tab ends its shells. Whether the dock is
  open, how many terminals the front tab has and which of its tabs is in front are saved with
  the open tabs (see "App") and come back on launch, the terminals as fresh shells in the
  front repository's folder. The keyboard starts with the repository, not a terminal.
- Avatars: unless turned off, a commit's dot is covered by a 20 px circle with a 2 px ring
  in the lane's colour (Blue for HEAD), so a branch keeps its colour. A row holds one commit
  and is 32 px tall, so no two circles touch and the lines of the lanes beside it pass
  clear. It holds the author's picture (own or Gravatar) or else their initials: the first
  letter of the first and of the last word of the name, in capitals (`avatar::initials`),
  in Crust at 45% of the circle's width (9 px here, so smaller than the type table, as an
  avatar has to be) on one of the branch palette's colours, picked by a hash of the author's
  address so a person has the same colour in every repository and session (`avatar::tint`).
  With Gravatar, an author who has none keeps their initials. Working changes, stashes and
  commits past the last lane keep their own marks. A merge never wears an avatar: it stays a
  hollow dot in its lane's colour (a blue ring when it is HEAD), so the lines it joins read
  at a glance (`Commit::wears_avatar`).
- Signed commits: a signed commit ends its subject in a 14 px shield with a check, in
  Green at 70% opacity (finished, but quieter than the subject), with the tooltip "Signed" (or "Signed, but git can't check the signature" when
  the key is missing, expired or there is nothing to check an SSH signature against). A
  signature that does not match the commit is a shield with an exclamation mark in Red and
  says so. Unsigned commits show nothing. The shape and the tooltip carry the meaning, not
  the colour. The status is `%G?`, which makes git run gpg or ssh-keygen for each signed
  commit and takes seconds in a repository of signed commits, so it is not part of the
  history read. The rows in view ask for theirs afterwards (`signature::read` in
  `mantisgit_core::history`), at most 64 commits to a call, and a shield appears when its
  answer does. Answers are kept in the disk cache under "Commits", with a commit's files,
  except "can't check", which is asked again in case the key has since been added.
- Commit rows are 32 px, or 28 px in compact density. Lanes are 16 px wide. Rows have no
  full hash: by default the refs column, the graph, the subject, the short hash (7
  characters, Overlay 2), then author and a short time ("12m", "3h", "Tue", "Oct 30"). The
  full hash belongs to the inspector.
- Column headings: a 28 px row on Mantle with a Surface 0 bottom border sits over the commits,
  with one heading per column ("Refs", "Graph", "Subject", "Hash", "Author", "Date") in the section
  label type, Overlay 2, and the same width as its column, inset 12 px from both edges like
  the rows. A heading drags onto another to
  take its place, the columns between shifting over by one, and the heading under the
  pointer tints with 14% Blue, as tabs do. The order is kept for every repository as
  `graph_columns` in the `state` file, a comma separated list of `refs`, `graph`, `subject`,
  `hash`, `author` and `date`. A list that is damaged, repeats a column or leaves one out is mended
  when read, so every column always shows once (`ColumnOrder` in
  `mantisgit_core::history::columns`). The headings stay over the graph, not over search
  results, which are a list of their own.
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
| Merge           | Hollow dot in its branch colour, never an avatar |

- Lines are 1.75 px with curved bends, and show no avatars.
- At most 8 lanes are visible. A commit past the edge gets a small dot in its branch
  colour at the graph's right edge, so no commit disappears.
- Your branch line is blue down to where it forks. Shared history below that takes
  `main`'s colour.
- Clicking a branch spotlights it: everything outside its history fades to about 28%,
  and clicking it again brings everything back.
  Clicking a commit outside the picked histories lets go of the picks, the same as clicking
  the branch again.
  `Ctrl`-click adds or removes branches, `Shift`-click picks the range down from the last
  one, and the graph lights all of their histories. Right-clicking one of several picked
  refs opens a menu for all of them ("Delete 3 branches"), which asks first and lists the
  command to bring each back. A checked-out branch stays, and the menu says so. Remote branches
  can be picked too and go with one `git push --delete` per remote, and asking then takes
  Enter twice.
- Worktrees: when a repository has more than one working tree, the sidebar lists them
  under "Worktrees", open by default, between Remotes and Tags. A row is the folder's
  name, then its branch (or `detached at <hash>`) in Overlay 2, with a dot in that
  branch's colour. The one on show is selected and semibold. Clicking another switches
  the current tab to it, so the tab strip doesn't grow. A locked worktree ends its row in a small lock (Overlay 2) with the tooltip "Locked", not
  a word. A worktree whose
  folder is gone is dimmed and says so.
  Right-clicking one opens the usual menu: "Switch to <name>", "Open <name> in a new tab",
  "Copy path", "Lock worktree <name>" or, when it is locked, "Unlock worktree <name>" (the
  main worktree can't be locked), and in red "Remove worktree <name>", which asks first and gives the
  `git worktree add` that brings it back. The main worktree, the one on show and a locked
  one stay, and the menu says why. Git refuses to remove one with uncommitted changes,
  and the app never forces it. With one working tree the section isn't drawn.
- Submodules: the sidebar ends in a "Submodules" section pinned to its foot, under a
  Surface 0 border, so it stays in view however long the lists above run. It is open by
  default and always drawn: with no submodules it says "This repository has no
  submodules." Its header ends in a quiet blue "+ Add", as Remotes does, which opens a form
  popup ("Add a submodule") with an "Address" and a "Folder" input (left empty, git names
  the folder after the address) and "Add submodule" as its result. It shows up to 5 rows
  and scrolls past that. A row is the submodule's path, then where it stands in
  Overlay 2 (its short commit, `moved to <hash>`, `not downloaded` or `in conflict`), with
  a dot that says the same: Green at the commit the repository records, Yellow on another,
  Peach in conflict, Overlay 2 when not downloaded. Clicking a downloaded one opens it in
  a tab of its own, or brings its tab forward. Right-clicking opens the usual menu: "Open
  <path> in a new tab", a line that names its result ("Download lib at 1a2b3c4", "Check
  out 1a2b3c4 in lib", "Update lib"), "Update 2 submodules" when more than one needs it,
  "Edit <path>" (the same form, filled in, saved with "Save <path>"; a new address is set
  with `git submodule set-url`, a new folder moves it with `git mv`), and "Copy path" and
  "Copy URL". Adding and editing stage the change, ready to commit, and the band says so.
  A folder outside the repository is turned down in the popup, before git runs. A line
  that can't be used stays and says why. Checking out the recorded commit in a submodule
  that is on another one asks first, and gives the `git -C <path> checkout <hash>` that
  goes back. The app never forces anything.
- Deleting a remote branch: right-clicking one (or several picked) ends its
  menu in red "Delete origin/feat/x on origin". It asks first in a popup that takes Enter twice (see "Delete confirmation"), which says it
  removes the branch for everyone who uses the remote and gives the `git push origin
  <hash>:refs/heads/feat/x` that brings it back, and runs `git push origin --delete`.
- Tags on remotes: a tag's menu works like a branch's across remotes. For each remote that
  doesn't hold it there is "Push tag v1 to origin" (a plain push, which git refuses for a tag
  the remote has elsewhere). Every way to delete is behind one red line, "Delete tag v1 ›",
  whose submenu has "Delete tag v1" (local only) and, for each remote that holds the tag, "Delete
  tag v1 on origin" (the local tag stays) and "Delete tag v1 here and on origin". With no
  remote holding it the submenu is dropped and the line is "Delete tag v1" itself. Git keeps
  no record of which remotes have a tag, so opening a tag's menu (or a commit's with a tag
  on it) asks each remote with `git ls-remote --tags` first, and keeps the answer for 30
  seconds or until a tag is pushed or deleted (`Refs::remote_tags`). A remote that can't be
  reached is offered everything, as before. Deleting on a remote changes it for everyone who
  uses it, so it takes Enter twice, and the popup gives the `git push origin v1` (and `git tag
  v1 <hash>`) that brings it back.
- Push by dropping: a local branch also drops on a remote branch, which tints like a
  local one. The menu holds one line, "Push feat/x to origin/main" when that
  fast-forwards the remote branch. When the remote branch has commits the branch lacks, the
  line reads "Push feat/x to origin/main (needs a force)": it still tries a plain push, git
  refuses it, and the force popup follows (see "Force push"), so nothing is forced unasked.
  When the remote branch already has everything, the line says so and can't be used. The
  band says how to undo a push.
- Pull and push from the menu: the menu of a local branch that follows a remote branch, checked
  out or not, has "Pull origin/x into x" and "Push 2 commits to origin/x". Pull
  asks the remote and only fast-forwards, so nothing is merged; on a branch that isn't checked
  out the working tree stays as it is. It is always offered, since the counts come from the last fetch. Push is
  offered when the branch is ahead, else it stays and says why. The band says how to undo
  each. A branch that follows nothing gets neither line.
- Force push: a push is never forced on its own. When git refuses one because the remote
  branch has commits the branch lacks, a popup follows that
  says so and that forcing replaces those commits for everyone who uses the remote, and gives
  the `git push --force-with-lease` that puts them back. Its result is "Force push" and it
  takes Enter twice (see "Delete confirmation"). It runs `git push --force-with-lease`
  against the commit the last fetch saw, so a remote that has moved since is refused, not
  overwritten. Any other failure is said in the toast as before. What is decided lives in
  core (`branch::push_was_rejected`, `Command::force_after_rejection`).
- Upstream: a local branch's menu has "Set upstream of <name>", which opens a form popup
  with the remote and the branch on one row, as `origin / main` reads: a "Remote" dropdown
  that starts on the remote holding a branch of the same name (or the first), a `/`, and a
  "Branch" field you type in. It starts with the branch's own name, and while it has the
  cursor a dropdown under it lists the remote's existing branches that contain what is
  typed (five at most). A click puts one in; typing a name the remote doesn't have is how
  a new upstream is made. The result names what will happen: "Follow origin/main" for one
  that exists, and "Create origin/feat/x" for a new one, which pushes the branch there and
  follows it. The band says how to undo either. A form can mix typed inputs and dropdowns,
  and fields that share a row are joined by a separator (`Form::separator` in core).
- New branch: a quiet "New branch" link ends the Branches heading, and branch, tag and
  commit menus have "New branch from <name or hash>". Either opens a form popup (see
  "Popups") that says where the branch starts ("New branch, starting at main"), with a
  "Name" input and "Create <name>" as its result. The new branch is checked out, and the
  band under the header says how to undo it. A name git would refuse, or one already
  taken, is explained in the popup, which stays open so the name can be put right.
- Remotes: the "Remotes" header ends in a quiet blue "+ Add" that opens a form popup
  ("Add a remote") with a "Name" and an "Address" input, and "Add remote" as its result.
  Each remote's top folder in the list, or a row saying "no branches yet" for one nothing
  has been fetched from, opens a menu on right-click: "Edit <name>" (the same form, filled
  in, saved with "Save <name>"; renaming also renames its remote branches), "Copy address",
  and in red "Remove remote <name>", which asks first and gives the `git remote add` that
  brings it back. Names that can't work are turned down in the popup, before git runs.
- Forges: Mantis Git tells which of GitHub, GitLab and Gitea a remote is on in this order.
  The forge chosen for the repository wins (the Repository page, kept as `mantisgit.forge`
  in its git config). Then the host's name: `github.com` or `github.*`, `gitlab.com` or
  `gitlab.*`, `gitea.com`, `codeberg.org`, `gitea.*` or `forgejo.*`. Then, for a server
  whose name says nothing, the server itself: after a repository loads, and before a drop
  menu opens, it is asked three questions at once over HTTP, one each forge answers in its
  own way without a sign-in (`/api/v1/version`, `/api/v3/meta`, `/api/v4/version`), with 2
  seconds to answer. A server that answered is asked once a session, whatever it said; one
  that didn't answer (a VPN that is down, a certificate nobody trusts) is asked again later.
  Hosts known to be something else, such as Bitbucket and Azure DevOps, are never asked.
  What is still unknown stays unknown, and the interface says so with a question mark.
- Pull and merge requests: when a remote is on GitHub, GitLab or Gitea (see "Forges"),
  the sidebar has a section between Remotes and
  Worktrees in that forge's own word, "Pull requests" or "Merge requests", for `origin` or
  else the first such remote. It starts closed, because asking reaches over the network,
  and opening it asks once. A quiet blue "Refresh" in its header asks again, and the count
  after it is how many the remote has. There is no sign-in and no API: the section uses
  git alone, `git ls-remote` on the refs hosts publish (`refs/pull/*/head`,
  `refs/merge-requests/*/head`). So it lists the newest 30 by number. On GitHub and GitLab
  only open requests are listed: those hosts drop a request's test merge ref
  (`refs/pull/*/merge`) when it is closed or merged, so a request without one is left out,
  and so is an open one that conflicts with its base. Gitea has no such ref, so it lists
  every request. A row is the number (`#42`, or `!42` on GitLab) in Text, then what the newest commit says in Overlay 2 when that commit
  is already in the repository, else its short hash. Clicking one opens it in the browser.
  Right-clicking opens the usual menu: "Open #42 on GitHub" and "Copy link". While asking,
  the section says "Asking origin…", and if git can't reach the remote it says why in Peach.
  A remote on no known forge has no section. What each host's pages are called and where
  they live is in core (`mantisgit_core::repo::forge`).
- A branch row ends with its count against the remote branch it tracks (`↑1 ↓2`: to push,
  to pull). Level or untracked shows nothing. Its count against the default branch (`+2 −1`)
  shows only while the pointer is over the row, so it takes room from the name only then.
  A branch the default branch already has every commit of, and that is behind it, shows a check (Overlay 2) with the
  tooltip "Merged into main", and one whose remote branch was deleted shows a broken cloud
  (Peach) with the tooltip "origin/x was deleted on the remote". Both are pictures served by
  the app (`src/icons.rs`), and their words live in core (`Branch::merged_hint`,
  `Branch::upstream_gone_hint`).

## Components

- **Primary button:** 32 px, Blue fill, Crust text, weight 600. One per screen.
- **Quiet button:** text only, Subtext 1, with a faint hover fill.
- **Ref chips:** tinted with 14% of the branch colour, text in full colour. Only the
  current branch gets a solid fill. Chips sit in their own column left of the graph (one chip, then `+n`), so subjects stay aligned. The column is as wide as the widest chip in the repository needs, from 48 to 180 px (`Refs::widest_chips`), so short names leave no empty strip. Hovering a commit with more
  refs opens a card over the column that lists every one, and each chip on it drags and
  right-clicks like the first.
  A branch and its remote at the same commit are one chip with a small cloud.
- **Menus:** right-clicking a branch, tag or commit opens one menu, and so does dropping
  a branch or tag on a branch. A card on Mantle with a Surface 0 border, 8 px radius and
  28 px lines, no shadow. It is 300 px wide, or as wide as its longest line up to 460 px, so
  a line can name its whole result. A quiet heading says what it applies to; lines name their
  result ("Merge feat/x into main", "Rebase main onto feat/x"). A line can start with a 14 px
  picture saying what it is about: the mark of a forge (GitHub, GitLab or Gitea) in the line's
  own colour, or a question mark in Peach when the line depends on something not known. The
  pictures are plain shapes served by the app (`src/icons.rs`), never emoji. A line that can't
  be used stays, in Overlay 2, and says why in as many rows as it needs, never cut short. Only lines that remove something are red, and they ask
  first, saying how to undo. `Up`/`Down` move, `Enter` chooses, `Esc` closes.
  A ref chip in the graph opens its commit's menu, the same as the row, so the two give the
  same lines; the sidebar's rows keep the menu of the ref itself.
  A commit's menu starts with a check-out line for each of its refs, as its chips show
  them ("Check out feat/x", "Check out origin/y as a local branch", "Check out v1.0.0
  without a branch"), then "Check out 1a2b3c4 without a branch" for the commit itself.
  A commit's menu holds every option of each branch, tag and remote branch at it, in groups
  split by a rule, most used first: push, then pull, for each local branch that follows a
  remote; "Check out", a submenu of a line for each ref ("Check out feat/x", "Check out
  origin/y as a local branch", "Check out v1.0.0 without a branch") and then the commit
  itself ("Check out 1a2b3c4 without a branch"), or the one line itself when that is all
  there is; bring in (merge and rebase each ref, or merge the commit when there is none,
  then cherry-pick); a new branch from the commit; copy; then the rarer ones near the foot,
  revert, set upstream for each local branch and "Reset main to here", a submenu of "Keep
  changes" and, in red, "Discard changes"; and last, in red, the deletion of each ref. A
  branch's own menu follows the same order.
  A line that ends in `›` has a submenu, a card of the same kind that pops out against its
  right edge, level with the line (240 px wide at least). The pointer opens it by resting on
  the line and another line takes it back in; a click or `Right` opens it with the keys in
  it, on its first choice, where `Up`/`Down` move and `Enter` chooses. `Left` takes it back
  in, and so does `Esc` before a second `Esc`, or a click outside, closes the menu. The
  confirmations of what a submenu holds are the same as anywhere else.
- **Popups:** a 420 px card centred over the window, on Mantle with a 1 px Blue outline
  and 8 px radius. It holds a sentence, then any inputs, then a line naming the keys
  ("Press Enter to create feat/x, or Esc to cancel.", in Overlay 2), and "Cancel" as a
  quiet button and the result as the popup's one primary button (Blue fill, Crust text), so
  it reads as a button whatever its words. A result too long for the card ends in an
  ellipsis. `Enter` answers and `Esc` cancels; clicking outside
  cancels too. A held-down key never answers. The answering button is off while git runs.
  Every popup is drawn by one frame (`src/view/dialog/mod.rs`), and there is at most one.
  - **Delete confirmation:** deleting a branch, tag, worktree or remote asks in a popup,
    never in a toast. The sentence says how to undo it and the result is
    "Delete". Deleting a branch that is neither merged into the default branch nor pushed
    loses commits nothing else keeps, so `Enter` has to be pressed twice within 600 ms: the
    first press turns the key line Red and says "Press Enter again to delete." Rules for
    which is which live in core (`Weight` in `crates/core/src/menu/command.rs`). Steps
    that lose what git can't give back (discarding changes, hard reset, force push, deleting
    a stash or a remote branch) take Enter twice; steps that say the command that undoes
    them (rebase, removing a remote or a worktree) ask in the same popup, where `Enter`
    answers once.
  - **Forms** ask for a few words: a new branch's name, a remote's name and address, or a
    submodule's address and folder. The
    sentence says what the form makes or changes, and each input sits under its section
    label, with the cursor in the first. `Tab` and `Shift Tab` move between inputs. The
    result is named for what was typed ("Create feat/x"). What can't work is said in Peach
    under the inputs, whether the form found it or git refused, and typing clears it. On
    success the popup closes and the band says what happened. What each form asks lives in
    core (`Form` in `crates/core/src/menu/form.rs`).
- **Welcome panel:** the first time Mantis Git runs, with a place to remember that it has,
  a 640 px panel opens over the front tab, on Mantle with a 1 px Blue outline and 8 px
  radius, drawn by the popup module so it shares the backdrop and keys. Its parts, top to
  bottom, each under a Surface 0 rule:
  - The app's mark (the icon, drawn at 56 px: a Blue disc (the accent) with the mantis on it in Crust,
    its antennae ending in commit nodes and a lane running from its chin to a HEAD node;
    `src/icons/mantis-git.svg`, the launcher's `mantisgit.svg` and `assets/mantisgit.ico`
    are the same drawing), "Welcome to Mantis Git" as a commit title, and one status sentence.
  - Three ideas side by side, each a picture on Base over a one-line heading and a few words:
    "Your branch is straight" (a small graph: HEAD's blue lane, `main` below the fork in
    its own colour, another branch bending away), "Say what will happen" (a primary button
    named for its result, with the line that undoes it) and "Branches, not “ours”" (a
    conflict header with each side named). Pictures use only the palette, and show chips and
    buttons as they are drawn elsewhere.
  - "Keys worth knowing": keycaps in Surface 0 (`Ctrl O` open a repository, `Ctrl T` new
    tab, `Ctrl B` sidebar, `Ctrl ,` settings).
  - For a portable copy that has no launcher, a tick box, ticked: "Add Mantis Git to the
    application menu" ("Start menu" on Windows), with a muted sentence that names what will
    be written and where, and says Settings → App takes it away. A launcher that is already
    there, one Mantis Git didn't write, or a package manager's copy shows no box.
  - A footer with the key line, "Skip" as a quiet button and the one blue button, named for
    its result: "Add to menu and start" while the box is ticked, otherwise "Get started".
  `Enter` goes on and does what the button says, `Space` ticks or clears the box, `Esc` or
  a click outside skips, and a key held down never goes on. Skipping writes nothing. It is
  shown once, whatever the answer, and the band says what was written and how to take it
  away. Settings → App adds or removes the launcher from then on. On a window too short for
  it, the panel scrolls.
- **Toast:** what just happened (and why a step failed, in Peach) is a card over the bottom
  right of the window, 420 px wide, on Mantle with a 1 px Blue outline and 8 px radius, and
  ends in a quiet `×` that closes it. It floats over the graph, so the rows never move.
  There are no bands: nothing in the window pushes the graph down. Where this document
  says "the band says" for an outcome, it means the toast. A card that offers something
  (the commit graph and the update, below) is the same card with its buttons under the
  text, at its right: quiet ones first, then the one blue button, the thing to do next.
  Every card stacks in one column against the window's own bottom right corner, 16 px from
  both edges, over the dock bar and the dock: the update first, then the commit graph, and
  the toast last, nearest the corner. A card that offers something never closes itself. How
  long a toast stays follows what it says (`toast_lifetime` in `mantisgit_core::text`): a confirmation such as
  "Copied 1a2b3c4" closes itself after 4 seconds, a longer one that says how to undo a step
  gets 50 ms more for each character past 30, up to 12 seconds, and a failure stays until
  its `×` is pressed. Hovering a toast holds it open, and it closes 2 seconds after the
  pointer leaves. A new one replaces the one showing.
- **Commit graph card:** a card under the toast, drawn only when a step took 2 seconds or more (`SLOW_AFTER` in
  `mantisgit_core::repo::commit_graph`) and the repository has no commit graph that git
  would use (`is_missing`: not a shallow clone, `core.commitGraph` not off, no replace
  refs). The steps are loading the history, timed until its first commits arrive, which is
  where git sorts everything, and counting the branches against the default branch, which is
  read after the graph is on screen and fills in when it arrives. It says in Peach what took
  how long and why ("Loading the history took 8 s because this repository has no commit
  graph. Writing one takes about as long, once, and makes this quick."; "Counting the
  branches took 7 s …"), with a quiet "Dismiss" (for that
  repository, until the window closes) and the blue "Write the commit graph", which runs the same
  write as Settings → Repository and says what it did in the usual band. A quick count, a
  graph that is only behind, or any other reason for being slow shows nothing. A branch level
  with the default branch is counted without asking git at all.
- **Update card:** when a newer release is found, a card in that corner:
  a blue "Update" pill, "Mantis Git 0.2.0 is available.", and under it a quiet "Later" (hides
  it until a newer release) and the blue "Update to 0.2.0". Nothing is downloaded until the button is pressed. It then says
  "Downloading Mantis Git 0.2.0…", and when the new program is in place "Mantis Git 0.2.0 is
  installed and starts the next time you open it. To go back, download 0.1.0 from
  github.com/…/releases/tag/v0.1.0." with "Restart now". A failure is said in Peach with
  "Try again". Looking and finding nothing, or failing to look, is only said in Settings.
  The same card offers a nightly ("Mantis Git nightly 1a2b3c4 is available.") on that track.
  The release archive is checked against the release's `SHA256SUMS.txt` before anything is
  replaced, and a mismatch installs nothing. What is said and decided lives in core
  (`mantisgit_core::install`).
- **Questions from git** (a key's passphrase, an HTTPS login, whether to trust a new host)
  open the same popup in the front tab, because git or ssh is waiting on it. The sentence is
  the question as they word it, with one input under it, hidden while typing when it is a
  passphrase, password or token. The result is named for what it does ("Unlock key",
  "Log in", else "Answer"). `Enter` sends the answer, `Esc` or a click outside cancels, and
  git then fails with its own message. git and ssh ask by running the app again as their
  askpass program, which passes the question to the open window over a private socket; the
  app keeps the answer in memory only, until it closes, so the next fetch doesn't ask again
  (the key line says so), and forgets it if ssh asks the same question again, which means it
  was wrong. The answering button stays on while git waits for it. Where the app isn't serving, they fail at once instead of
  asking a terminal nobody sees. What the popup says lives in `mantisgit_core::askpass`.
- **Double-click:** double-clicking a branch, in the sidebar or on a chip, checks it out. A
  remote branch gets a local copy that follows it. The checked-out branch does nothing.
- **Drag and drop:** branches and tags drag from the sidebar and from the refs column,
  and drop on local branches, which tint with 14% Blue. Dropping never acts: it opens a
  menu of what fits how the two histories relate (fast-forward, merge, rebase).
  A remote branch also takes another remote branch of the same remote, tinted the same,
  and the menu then proposes a pull or merge request bringing the dragged branch into the
  one it was dropped on: "Open a pull request for feat/x into main", which opens the page
  that starts one on the forge in the browser, with the forge's mark before it, and "Copy
  link". Git can't make a request, so Mantis Git opens the page and the band says so. The
  heading names the branches without their remote ("feat/x onto main"). When the forge isn't
  known, even after asking the server, the line is still there as "Open a request for feat/x
  into main", with a Peach question mark. Choosing it opens a popup that says Mantis Git can't
  tell which forge the remote is on, so it doesn't know which page starts a request, and that
  the repository's settings can say; its result is "Open settings", which opens the
  Repository page. A line that can't be used says why, briefly: the base already has all of
  the branch, or the two are on different remotes (a request between repositories isn't
  supported yet).
- **Search:** a quiet "Search" button in the header, or `Ctrl F`, opens a
  44 px bar on Mantle with a Surface 0 bottom border over the graph, with the cursor in its
  input. Two chips choose what is matched, the picked one ticked and never blue: "Message,
  author or hash" (the default; the start of a hash of 4 or more digits finds its commit) and
  "File" (a commit that changed a file whose path holds what was typed, in any case, so
  `main.rs` and `src/view` both work). Git looks over every branch and tag
  (`mantisgit_core::history::search`), 300 ms after typing pauses or at once on `Enter`,
  and a search that is still running is stopped when another starts. The found commits take
  the graph's place as a list of 32 px rows (short hash, subject with its prefix in Overlay 2,
  author, time) under a sentence that counts them ("3 commits match “main.rs”."); more than
  500 lists the newest 500 and says so, and a failure is said in Peach. Clicking a result
  selects it for the inspector, and the list stays so the next can be picked. A result
  older than what the graph has loaded is dimmed, and clicking it says so in the band.
  `Esc` or "Close" brings the graph back, scrolled to the picked commit. An open diff or
  resolver covers the bar like the rest of the graph.
- **Back to the top:** once the graph is scrolled down a screen or more, a 28 px quiet button
  with an up arrow (Mantle fill, Surface 0 border, 6 px radius) floats at the top middle of
  the list, with the tooltip "Back to the newest commit". Clicking it scrolls to the first
  row, and it goes away at the top.
- **Selecting commits:** a click selects one, `Ctrl`-click adds or removes one, and
  `Shift`-click adds the range. Selected rows take the selected-row fill, and the menu
  then acts on all of them.
- **Working changes:** the inspector opens with the status sentence, the List and Tree
  chips, then two file lists above each other, and the commit form under them. The top list
  is "Unstaged" and holds only the files with changes the index doesn't have, the bottom
  one is "Staged" and holds only what the next commit contains. A file changed both ways is
  in both, each with its own diff. A list with no files says so in a sentence. Diffs open on
  a click, never on their own. The lists and an open diff refresh when the window is used
  again. Git walks the whole working tree to find untracked files, which in a large
  repository takes seconds, so the changed files show first and the walk finishes after:
  while it runs, "Unstaged" says "Looking for untracked files…" with a small spinner, and
  an empty list says that instead of "Nothing left to stage."
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
  <name>", for a commit's file "Apply changes in <name> to working changes", "Show in folder" (the system file manager, at the file, or at the nearest folder
  that still exists for a deleted one), and "Copy
  path". Applying puts what the commit changed in that file (over its first parent, for a
  merge) into the working tree, unstaged, and leaves the branch alone. When the file has
  moved on, the two changes are merged; where they clash, or the file already has the
  change, nothing is written and the band says why. On success the band says how to undo
  it. A folder in the tree has "Stage 3 files in <name>/" (or "Unstage"), "Show in folder" and "Copy path". A conflicted file has both too.
- **Discarding:** a file in "Unstaged" has a red "Discard changes in <name>" line in its
  menu, and a folder in the tree has "Discard changes in 3 files in <name>/". The
  "Unstaged" header ends in a quiet "Discard all" button. Each asks in a popup first, which takes Enter twice (see
  "Delete confirmation"), and the sentence says briefly that the changes can't be
  brought back. It puts each tracked file back as the index has it, so staged changes stay, and
  deletes a file git doesn't track yet. Files in "Staged" can't be discarded from there:
  unstage them first.
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
  A quiet "Stash 2 files" button sits beside the blue one and runs `git stash push --staged`,
  so only what is staged is set aside and unstaged changes stay. A typed summary names the
  stash. The band says how to bring it back. The inspector doesn't list stashes; the sidebar does.
- **Stashes:** the graph draws each as a commit on the one it was made on, joined to it by a
  line in its lane's colour, with a dashed hollow dot like the working changes (the index and
  untracked commits git keeps beside it are not drawn). Right-clicking its row opens the same
  menu as the sidebar does. A plain click on a stash in the
  sidebar selects its row in the graph, which shows its changes in the inspector. The sidebar
  lists them under "Stashes", each row its number, what it says and
  the branch it was made on in Overlay 2. They pick like branches: a click picks one,
  `Ctrl`-click adds or removes one, `Shift`-click picks the run from the last. Right-clicking
  opens a menu for every picked stash, or the one under the pointer when it isn't picked:
  "Apply 3 stashes" (applies them and keeps them, oldest first, stopping at the first git
  can't apply), "Pop 3 stashes" (the same, deleting each as it applies, so one that git can't
  apply stays; the band gives the `git stash store <hash>` that puts each back), and in red "Delete 3 stashes", which asks first and takes Enter twice (see "Delete confirmation"). The band
  after a delete gives the `git stash store <hash>` that brings each one back.
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
  saved, which stay listed until the operation goes on, including those saved before the app
  was opened: git keeps a record of them in the index), then the "Staged" list as in the
  working changes, with its own List and Tree chips, so everything the next commit holds can
  be looked at and opened, and at the foot "Cancel rebase" and the one blue "Continue
  rebase", which waits until every file is saved. The "Unstaged" list and the commit form
  wait while it is paused.
  Clicking a file opens the resolver over the graph, as a diff does, at the file's first
  conflict still standing, and `Esc` or "Close" brings the graph back. It is one file at a
  time, in three parts that share the height:
  - **Both versions**, the whole file as each branch has it, next to each other, as a list of
    rows that draws only those in view, so a file of any size costs the same. There is no
    view of only the changes: resolving needs the whole file. A bar over it names the two
    branches, each over its own column. A line both versions have shows in both columns,
    each with its line number in that version. Each conflict is a header row across both,
    "Conflict 2 of 3 · line 40" with "Use feat/x" and "Use main", and under it each side's
    lines in its own column, with their line numbers, a tick box and a tint in the
    branch's colour; the side with fewer lines leaves the rest of its column empty. The
    ticks make up the result: each tick shows in it at once, the conflict's place holding
    the ticked lines of the first side, then the second's, and clearing every tick puts its
    markers back. "Use feat/x" ticks every line of that side and none of the other, which
    takes the side even when it has no lines, and the header then says "in the result".
    A conflict typed over in the result keeps what was typed: it dims, says "edited in the
    result", and its ticks and buttons go, so a tick never undoes typing.
    Each conflict is marked beside the scrollbar in Peach. The branches are named, never
    "mine" or "theirs", in line with principle 5. In the header, quiet "Previous" and "Next"
    buttons (`Alt ↑`, `Alt ↓`) bring the next conflict still left in the result to the top of
    the list, going on from the last one gone to, or from the top of the view once it has been
    scrolled past. They skip settled conflicts, go round at either end, and are dimmed when
    none is left.
  - **Result**, the whole file in an editor, which starts as the file as it stands with its
    conflict markers. Its lines don't wrap, as in a diff, so a file of any length opens
    quickly. The versions and the result scroll together: scrolling either brings the other
    to the same place in the file, line for line, with each conflict lined up with where it
    is in the result. The ticks above edit it, and so can the user, by hand, anywhere. A
    conflict counts as settled when its markers are gone from the result, however that
    happened. The
    top bar says how many are left ("2 conflicts left in the result", Peach) or "Ready to
    save" (Green).
  - **Save and mark as resolved**, a large blue button across the foot, enabled once no
    conflict is left. It writes the result and stages the file, which takes the conflict state
    off it, then opens the next file that needs a decision, or closes when there is none.
    The screen doesn't wait for git: a file whose conflicts are all decided can't stay in
    conflict, so it moves on and lists the file as resolved at once, with a small spinner
    and "Saving…" in place of the tick and "Resolved" until git has taken it and been read
    again. Saving moves no branch, so the graph isn't read again, and untracked files are
    not looked for again. If git refuses, the file comes back open, with what was typed in
    it, and the band says why. "Continue" doesn't wait for that read, only for the writes.
  A file with nothing to merge line by line, such as a binary file or a deleted one, has two
  cards with a "Use feat/x" choice each instead of the editor, saved with the same button.
  Right-clicking a conflicted file offers "Show both sides", "Use feat/x" and "Use main" for
  the whole file, which save it at once, and "Copy path". When the operation finishes, the
  band under the header says what is now true.
- **Files:** a status letter, then the path with its folder muted, then `+n −n`. A commit's
  files show at once, from their names alone; counting `+n −n` means diffing every file, so
  it is read after and the counts, and the totals in the header, appear when it is done. A
  file over 256 MB (`LARGE_FILE_BYTES` in `mantisgit_core::commit::details`) is never counted
  or diffed on its own: its row ends in its size, and opening it says how big it is and why
  nothing is shown, with one blue button "Show changes in <name> (300 MB)" that reads the
  diff for that file only. Git's diff takes time that grows faster than the file does when
  the versions differ a lot, so a huge file would otherwise hold everything up. The limit is
  high because Unity scenes of 35 MB diff in a fraction of a second. Output from git is read
  in large pieces and the command log keeps only its first 1000 lines, since a 65 MB diff
  read line by line made git wait ten times longer than it needed.
- **Diffs:** each hunk is a card on Mantle with a Surface 0 border and 8 px radius, over the
  Base of the pane. Its header has the hunk's tick box (working changes), where it starts or
  what git says it is inside, and "left out" or "in this commit". Its lines have one line
  number, a sign and the text. Added and removed lines use a 10% green or red wash. Text
  stays Text. The file's name is in the bar above, once. A line longer than 1000 characters
  is drawn as its first 1000 and "… 9 549 076 more characters", since one line of
  megabytes, as terrain and baked data have in a Unity scene, takes the window a second or
  more to lay out (`Line::clipped` in core). Staging works on git's own hunks, so it is
  unaffected.
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
  command failed. A running command keeps to its newest line unless you scroll up. Dragging
  over the output selects text, tinted 35% Blue, and `Ctrl C` (`Ctrl Shift C` too)
  copies it; a click clears it, and so does picking another command.
- **Terminal:** code type on Mantle. Dragging selects text, tinted 35% Blue, and `Ctrl Shift C`
  (and `Ctrl C` on Windows while something is selected) copies it, so a plain
  `Ctrl C` still interrupts the shell. Typing, scrolling or a click clears the selection. The cursor is a Blue block with Crust text and shows
  only while the terminal has focus. While it has focus it takes every key, Tab, Ctrl C
  and Ctrl W included, before any shortcut of the app, except `Ctrl \`` and
  `Ctrl Shift \``. Colours the shell asks for by number use Catppuccin's terminal
  mapping: black Surface 1, red, green, yellow, blue, Pink for magenta, Teal for cyan,
  white Subtext 1, and the bright set is the same but for Surface 2 as bright black
  and Subtext 0 as bright white. Latte takes the same-named Latte values. The 256-colour
  cube and the grey ramp are the standard ones, since programs rely on them.
- **Tooltips:** every tooltip opens as soon as the pointer is on its control, with no
  delay. `show_tooltips_at_once` sets this for the app's buttons, and a tooltip on a plain
  element sets `tooltip_show_delay(Duration::ZERO)` itself, since gpui times those on its own.
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
