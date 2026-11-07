# gitclient

A desktop Git client written in Rust on `gpui-kit`.

## Layout

`crates/core` is everything about a repository, and `src` is the interface on top of it.
`crates/core` must never depend on gpui-kit: if something doesn't need a window, it
belongs there.

## UI style

All UI work must follow [docs/design.md](docs/design.md): Catppuccin Mocha, Blue as the
accent, Maple Mono as the only typeface. Read it before adding or changing anything the
user can see. Do not introduce other colours, fonts, or visual patterns. If a change
needs something the document doesn't cover, follow its principles and update the
document in the same change.

@docs/design.md

## Commands

- `cargo test` runs the test suite.
- `cargo test --test render` renders through the real GPU pipeline. It is opt-in.

## Commits and releases

Work on short-lived branches and open PRs into `main`. PRs are squash-merged, so PR
titles must be Conventional Commits. See "Releasing" in `README.md`.
