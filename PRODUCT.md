# Product

<!-- impeccable:product-schema 1 -->

## Platform

web

Note: Mantis Git is a native desktop application (Rust on gpui-kit; Linux, Windows, and WSL2 repositories), not a web app. `web` is recorded only as the closest schema value, so no iOS/Android guidance is loaded. Visual authority is `docs/design.md`, not a browser stack.

## Users

Working developers who commit all day: staging hunks, rebasing, reviewing branches, often in large repositories. They want speed and clarity rather than a feature-heavy GUI.

## Product Purpose

A desktop Git client that talks to the installed `git`. It lets someone understand a repository's state in seconds and take the next step without leaving one window. Success: the window stays responsive on repositories of any size, and the user always knows where they are and what will happen next.

## Positioning

Performance and plain language together. Your branch is the straight line down the graph, a sentence states the repository's state, and controls name their result and how to undo it. Pull and merge requests for GitHub, GitLab and Gitea are listed with git alone, with no sign-in or API token. Conventional Commits are built into the commit form.

## Operating Context

Daily, repeated use alongside a terminal (the app has its own dock with terminals and a command log). Repositories include very large ones and repositories inside WSL2. Installed either as a portable release archive or through a package manager.

Visual verification: the app renders headlessly to PNG through the real GPU pipeline on Linux and macOS (not Windows). Run `RENDER_OUTPUT=<dir> cargo test --test render`, then read the PNGs in `<dir>`. Frames cover the graph, inspector, diffs, conflicts and resolver, sidebar, dock log and terminal, and the settings pages, in Mocha and Latte (`tests/render.rs`, built on `HeadlessAppContext`). Use this instead of a browser to check UI work against `docs/design.md`, at the 1200×800 default window. It needs a working GPU or Vulkan stack and is opt-in, so it is not part of plain `cargo test`.

## Capabilities and Constraints

- Everything about a repository lives in `crates/core`, which never depends on gpui-kit. Views draw what core says.
- All git goes through `crates/core/src/ops.rs` and never runs on the UI thread.
- Large-repository performance is a durable constraint: lists are virtualised, work is off the UI thread, and slow steps are named rather than silent.
- No accounts and no sign-in. Network features beyond git itself (Gravatar, update checks) are explicit settings.
- Requires `git` on `PATH`; 2.41 or newer is recommended.

## Brand Commitments

Name: Mantis Git. The UI style (Catppuccin Mocha with Latte for light OS, Blue as the accent, Maple Mono as the only typeface) is binding and is recorded in `docs/design.md`.

## Evidence on Hand

`README.md`, `docs/design.md`, `docs/screenshot.png`. No testimonials, customers or benchmarks exist; do not invent them.

## Product Principles

1. State it in a sentence: prefer plain words about repository state over rows of icons.
2. One next action per screen; everything else stays quiet.
3. Say what will happen, and how to undo it before a risky step.
4. Name things by branch, never "ours" or "theirs".
5. Performance is a feature: never block, never leave a slow step silent.

## Accessibility & Inclusion

Every action has a key and a visible focus ring. Meaning is never carried by hue alone; colour is paired with a letter, shape or word.
