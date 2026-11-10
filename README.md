# gitclient

A desktop Git client built for performance. Speed is the point: the window stays
responsive and history opens quickly, whatever the size of the repository.

It's written in Rust on [gpui-kit](https://crates.io/crates/gpui-kit) and talks to your
installed `git`.

![gitclient showing a repository's branches, its history as a lane graph, and the working changes](docs/screenshot.png)

## What it does

- **Built for performance.** The window stays responsive and history opens quickly,
  whatever the size of the repository.
- **Understand a repository at a glance.** Your branch is the straight line down the
  graph, and a plain sentence says where you are and what is waiting.
- **Conventional Commits built in.** Pick `feat`, `fix`, `docs` and the other kinds of
  change in the commit form, and it writes the commit line for you.
- **Pull and merge requests** for GitHub, GitLab and Gitea remotes are listed using git
  alone, with no sign-in and no API token.
- **A clean UI.** Few controls, one typeface, and nothing on screen that the task doesn't
  need.
- **A fast flow, whatever your workflow.** The interface is made so the next step is
  always close at hand, whether you stage hunks, rebase, or review branches.

## Installing

Download the archive for your system from the
[releases](https://github.com/pretext8335/gitclient/releases) page, unpack it and run
`gitclient`.

## Dependencies

- `git` on your `PATH`. Version 2.41 or newer is recommended, as the sidebar's branch
  counts need it.
- On Windows, repositories inside WSL2 open like any other. Git runs inside the
  distribution.

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

Everything about a repository lives in `crates/core` (`gitclient-core`), which never
depends on the UI toolkit. The window in `src/` asks core what's true and draws the
answer, so rules and wording can be tested without opening a window. All git goes through
`crates/core/src/ops.rs`. The look and behaviour are specified in
[docs/design.md](docs/design.md).

## AI disclosure

Most of this project's code, tests, design document and commit messages were written with
[Claude Code](https://claude.com/claude-code). I decide what the application does and how
it should behave, direct the work, and use the application daily, but I have not reviewed
every line.

Quality is checked by a test suite built on throwaway repositories, and by CI, which runs
formatting, clippy with warnings as errors, and the tests.

Bug reports and pull requests are welcome. Please say if a pull request was generated.

## Contributing

Work on short-lived branches and open PRs into `main`. [CLAUDE.md](CLAUDE.md) has the
layout rules and the commands. PRs are squash-merged, so give yours a
[Conventional Commit](https://www.conventionalcommits.org) title such as `fix: ...` or
`feat: ...`.

`THIRD-PARTY-LICENSES.txt` is generated from `Cargo.lock`. After a change to dependencies
lands on `main`, CI opens a pull request that updates it, so there is nothing to run by
hand.

## License

GNU General Public License, version 3 or (at your option) any later version. See
[LICENSE](LICENSE). The licenses of everything compiled in are under Settings → About and
in [THIRD-PARTY-LICENSES.txt](THIRD-PARTY-LICENSES.txt).

The interface uses [Catppuccin](https://catppuccin.com) colours and
[Maple Mono](https://github.com/subframe7536/maple-font), which is bundled under the SIL
Open Font License; see `src/fonts/OFL.txt`.
