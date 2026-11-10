# gitclient

A desktop Git client written in Rust on `gpui-kit`.

## Layout

`crates/core` (`gitclient-core`) is everything about a repository, and `src` is the
interface on top of it. If something doesn't need a window, it belongs in `crates/core`.

- `crates/core` must never depend on gpui-kit. Lane colours stay abstract there, and
  `src/theme.rs` turns them into real colours.
- Views ask core what is true and draw the answer. Rules live in core, including what an
  action allows, what a selection means, and how to read refs. A view can pick a colour
  from a ref's kind, but it shouldn't count parents or know a commit type by name.
- Words about repository state live in core too, such as status sentences, button labels
  that name a result, and undo instructions. Core returns plain strings or data, so they
  can be tested without a window.
- All git goes through `crates/core/src/ops.rs`. Git blocks, so views call core through
  `GraphView::spawn_blocking` or `GraphView::run_operation`, never on the UI thread.
- Before adding a helper, look for one to reuse: `crates/core/src/text.rs` for wording
  and `src/view/parts.rs` for anything more than one region draws.
- Tests that need a repository use `gitclient_core::testing::Repo`, not their own fixture.

Where things go in `crates/core/src`:

- `history/`: commits as they load, the refs that point at them (`refs.rs`), lanes, and
  which commits and branches are picked.
- `commit/`: one commit's files and diffs (`details.rs`) and the commands that act on it
  (`actions.rs`).
- `working/`: staging and committing, with the commit's wording and version bumps.
- `repo/`: branches, remotes, worktrees, submodules, a paused operation, and status. Where
  a remote is hosted (`forge.rs`) lives here too: GitHub, GitLab and Gitea pages and their
  pull and merge requests, read with git alone.
- `install/`: how this copy was installed (`channel.rs`), its application menu launcher
  (`menu_entry/`), and updating itself from GitHub releases (`update.rs`). Only a portable
  copy does either; one a package manager installed must never write a launcher or replace
  itself.
- `settings/`: git config, identities, SSH suggestions, and what the app remembers.
- `menu/`: what each menu offers, in `command.rs` what its lines run, and in `form.rs`
  what the popups that ask for a name say and do.
- `about.rs`: which build this is and the licenses inside it, read from `THIRD-PARTY-LICENSES.txt`.
- `cache/`, `patch.rs`, `tree.rs`, `log.rs`, `terminal.rs`, `ops.rs` and `text.rs` stand
  alone.

In `src/view`, a region with more than one file gets a folder (`sidebar/`, `inspector/`,
`conflicts/`, `dialog/`, `dock/`, `settings/`), and `view.rs` holds `GraphView` and the layout.

## UI style

All UI work must follow [docs/design.md](docs/design.md): Catppuccin Mocha, Blue as the
accent, Maple Mono as the only typeface. Read it before adding or changing anything the
user can see. Do not introduce other colours, fonts, or visual patterns. If a change
needs something the document doesn't cover, follow its principles and update the
document in the same change.

@docs/design.md

## Commands

- `cargo test` runs the test suite for both crates.
- `cargo test --test render` renders through the real GPU pipeline. It is opt-in.
- `cargo fmt --all` formats both crates. Plain `cargo fmt` skips `crates/core`, and CI
  checks it.

## Confirmation popups

Every `Confirmation` in `crates/core/src/menu/command.rs` carries a `Weight`. Choose it by
what a mistake costs, and say so in the same change as the command:

- `Weight::DoublePress` (Enter twice) when the step loses something git can't give back:
  uncommitted or unstaged work (discard, hard reset), commits nothing else keeps (deleting
  an unmerged, unpushed branch or a stash), or something that changes for everyone who uses
  a remote (deleting a remote branch, force push). A new command that does any of these
  must be `DoublePress`.
- `Weight::Popup` (Enter once) when the popup gives the exact command that undoes the step,
  or git itself refuses to lose data (rebase, removing a remote or a clean worktree,
  checking out a submodule commit).

Keep "Delete confirmation" in `docs/design.md` in step, and cover the weight of a new
confirmation with a test in `crates/core/src/menu`.

## Commits and releases

Only create a branch when asked to, or when working in a worktree; otherwise commit on the
current branch. When a PR is wanted, open it into `main`. PRs are squash-merged, so PR
titles must be Conventional Commits. See "Releasing" in `README.md`.
