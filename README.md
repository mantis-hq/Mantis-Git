# gitclient

A desktop Git client that stays fast on big repositories.

Most Git GUIs are fine until you point them at something the size of a real monorepo,
and then every click is a spinner. gitclient is built around not doing that. It's written
in Rust on [gpui-kit](https://crates.io/crates/gpui-kit), it talks to your installed `git`,
and the window never waits on it.

The second goal is that you can tell what's going on in a couple of seconds. The header
says where you are in a sentence ("On feat/commit-graph, 2 commits to push"), your branch
is the straight line down the graph, and buttons are named for what they'll do ("Push 2
commits", not "Push").

## Why it's fast

- **Git never runs on the UI thread.** Every call goes through one module and is handed to
  a background task, so scrolling and typing keep going while git thinks.
- **Lists draw only what's in view.** The graph, a commit's files, the working changes, the
  sidebar and the conflict resolver are all virtualised, so a commit touching 10,000 files
  costs about the same as one touching three.
- **History loads in pages.** It reads 500 commits by default (100 to 20,000 if you want),
  and loads the next page when you scroll near the end. After a fetch or a commit it reads
  only the new commits when refs just moved forward.
- **Untracked files come last.** Git has to walk the whole working tree to find them, which
  can take seconds, so the changed files show first and the walk finishes behind them.
- **A cache on disk.** Commit details and file diffs are kept between runs, so a commit
  you've opened before opens at once. It has a size limit (256 MB by default) and it's
  always safe to clear.
- **It can tell you when git is the slow part.** The dock at the bottom logs every command
  the app ran, with its timing, so a slow repository doesn't turn into a silent wait.
- **Git's commit graph.** Settings → Repository says whether git's commit-graph file covers
  your history, and writes it for you if it doesn't. That's the single biggest thing you
  can do for a huge repo, with any client.

## What it does

The basics are all there: browse history as a lane graph, stage individual hunks, commit,
stash, branch, merge, rebase, cherry-pick and revert. A few things that are a bit
different:

- **Drag a branch onto another** and get a menu of what makes sense for how the two
  histories relate (fast-forward, merge, rebase). Dropping never does anything by itself.
- **Branch names instead of "ours" and "theirs".** Those swap meaning during a rebase, so
  the conflict resolver shows both versions of the whole file side by side, each labelled
  with its branch, next to an editable result.
- **Say what will happen, and how to undo it.** Anything risky explains itself first, and
  actions that add a commit tell you the command that takes it back.
- **Conventional Commits built in.** The commit form knows the types, and if you tag
  releases like `v1.2.3` it shows what landing the commit would do to the version.
- **Worktrees, submodules, stashes and remotes** all live in the sidebar and have their own
  right-click menus.
- **Pull and merge requests** for GitHub, GitLab and Gitea remotes are listed from the
  refs those hosts publish, using git alone. There's no sign-in and no API token.
- **Tabs and a dock.** One repository per tab, with a command log and real terminals at the
  bottom that start in the repository's folder.
- **Identities.** Named profiles (name, email, signing key, SSH key) that get picked by
  rule, such as an SSH host or a folder.
- It follows your OS's light or dark setting. Dark is [Catppuccin](https://catppuccin.com)
  Mocha, light is Latte, and the one typeface is [Maple Mono](https://github.com/subframe7536/maple-font).

## Installing

Download the archive for your system from the
[releases](https://github.com/pretext8335/gitclient/releases) page: `.tar.gz` for Linux and
macOS, `.zip` for Windows. Unpack it anywhere and run `gitclient`. You need `git` on your
`PATH`.

The first time it runs, a welcome panel explains how the window works, and offers to add
gitclient to the application menu (Linux) or the Start menu (Windows). On Linux that writes
`gitclient.desktop` and an icon under `~/.local/share`; Settings → App adds or removes it
later.

A copy unpacked this way looks for a newer release when it starts, and asks before
installing one. Settings → About can follow the nightly pre-release (built from every push to
`main`) instead of full releases. The archive is checked against the release's `SHA256SUMS.txt`
first. A copy
installed by a package manager neither adds a launcher nor updates itself; a package recipe
marks its build with `GITCLIENT_INSTALL_CHANNEL=<name>` (anything but `portable`) at build
time.

## Building from source

You need a recent stable Rust toolchain. On Linux you also need the usual GUI libraries:

```sh
sudo apt-get install pkg-config libxcb1-dev libxkbcommon-dev \
  libxkbcommon-x11-dev libfontconfig1-dev libfreetype-dev libwayland-dev
```

Then:

```sh
cargo run --release
cargo test                # both crates
cargo test --test render  # renders through the real GPU pipeline, opt-in
cargo fmt --all           # plain `cargo fmt` skips crates/core, and CI checks it
```

## How it's put together

Everything about a repository lives in `crates/core` (`gitclient-core`), and it never
depends on the UI toolkit. The window in `src/` asks core what's true and draws the
answer. Rules, wording and what an action allows are all decided in core, so they can be
tested without opening a window. All git goes through `crates/core/src/ops.rs`.

The look is written down in [docs/design.md](docs/design.md). It's long, and it is the
spec for how the app behaves as much as how it looks.

## AI disclosure

This project is mostly vibe coded. I've used [Claude Code](https://claude.com/claude-code)
for most of the code, the tests, the design doc and the commit messages, and I steer it
more than I write. I decide what the app should do and how it should feel, I use it every
day, and I read what comes out when something looks off, but I haven't read every line and
I won't pretend I have.

Why I'm telling you: you should know how the code you're running was made, and if that
puts you off, that's a fair call.

What I can say in its favour is that it's more stable than that probably sounds. There's a
real test suite built on throwaway repositories, CI runs formatting, clippy with warnings
as errors and the tests on Linux, macOS and Windows, and the thing never touches your files
without saying what it's about to do. It was made with care and I'd like it to be good.

Bug reports are welcome, and so are PRs, whoever or whatever wrote them. Please say if
yours was generated.

## Contributing

Work on short-lived branches and open PRs into `main`. [CLAUDE.md](CLAUDE.md) has the
layout rules and the commands, and it's worth reading even if you're not a robot.

## Releasing

PRs are squash-merged, so the PR title must be a
[Conventional Commit](https://www.conventionalcommits.org):

| PR title                       | Version bump before 1.0 | After 1.0 |
| ------------------------------ | ----------------------- | --------- |
| `fix: ...`                     | patch                   | patch     |
| `feat: ...`                    | minor                   | minor     |
| `feat!: ...`                   | minor                   | major     |
| `chore:`, `docs:`, `ci:`, etc. | none                    | none      |

Pushes to `main` keep a Release PR up to date with the next version and changelog.
Merge that PR when you want to ship: it tags the version, builds Linux, macOS, and
Windows binaries, and publishes the GitHub Release. To ship 1.0.0, or any other
specific version, add a `Release-As: 1.0.0` footer to a commit on `main`.

## License

gitclient is free software, licensed under the GNU General Public License, version 3 or
(at your option) any later version. See [LICENSE](LICENSE). You can use, modify and share
it, including commercially, as long as anything you distribute stays under the same
license with its source.

Maple Mono is bundled under the SIL Open Font License; see `src/fonts/OFL.txt`.

The licenses of everything compiled into gitclient are in the program itself, under
Settings → About, and in [THIRD-PARTY-LICENSES.txt](THIRD-PARTY-LICENSES.txt). That file is
written from `Cargo.lock`; after a dependency changes, run
`cargo install --locked cargo-about --features cli` once, then
`cargo about generate --locked about.hbs -o THIRD-PARTY-LICENSES.txt`. CI fails while it is
out of date, and when a dependency has a license `about.toml` doesn't accept.
