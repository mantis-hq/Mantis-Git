# gitclient

## Installing

Download the archive for your system from the
[releases](https://github.com/pretext8335/gitclient/releases) page: `.tar.gz` for Linux and
macOS, `.zip` for Windows. Unpack it anywhere and run `gitclient`.

The first time it runs, gitclient asks once whether to add itself to the application menu
(Linux) or the Start menu (Windows). On Linux that writes `gitclient.desktop` and an icon
under `~/.local/share`; Settings → App adds or removes it later.

A copy unpacked this way looks for a newer release when it starts, and asks before
installing one. The archive is checked against the release's `SHA256SUMS.txt` first. A copy
installed by a package manager neither adds a launcher nor updates itself; a package recipe
marks its build with `GITCLIENT_INSTALL_CHANNEL=<name>` (anything but `portable`) at build
time.

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
