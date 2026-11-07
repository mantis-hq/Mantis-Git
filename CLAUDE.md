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

## Commits and releases

Only create a branch when asked to, or when working in a worktree; otherwise commit on the
current branch. When a PR is wanted, open it into `main`. PRs are squash-merged, so PR
titles must be Conventional Commits. See "Releasing" in `README.md`.
