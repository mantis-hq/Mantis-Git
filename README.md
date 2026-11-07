# gitclient

## Releasing

Work on short-lived branches and open PRs into `main`. PRs are squash-merged, so
the PR title must be a [Conventional Commit](https://www.conventionalcommits.org):

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
