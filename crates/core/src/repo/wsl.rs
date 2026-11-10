//! Repositories that live inside WSL2, which Windows reaches as `\\wsl.localhost\<distro>\…`
//! (older builds: `\\wsl$\<distro>\…`).
//!
//! Git for Windows is slow and wrong about such folders (permissions, line endings, file
//! watching), so git runs inside the distribution instead, through `wsl.exe`, and the paths
//! it prints are turned back into the Windows ones the rest of the app uses. All of this is
//! text, so it is the same on every system and tested everywhere; only a Windows path ever
//! matches.

use std::collections::hash_map::RandomState;
use std::ffi::OsString;
use std::hash::{BuildHasher, Hasher};
use std::path::{Path, PathBuf};
use std::process::Command;

/// The program that runs a command inside a distribution.
pub const PROGRAM: &str = "wsl.exe";

/// How many arguments `wsl.exe -d <distro> --cd <folder> --exec sh -c <script> sh` takes
/// before git's own.
const PREFIX: usize = 9;

/// The variables that cross from Windows into the distribution. Nothing else does, so the
/// distribution's own agent, credential helper and `SSH_ASKPASS` are never replaced by
/// anything from Windows.
const FORWARDED: [&str; 3] = ["GIT_OPTIONAL_LOCKS", "GIT_TERMINAL_PROMPT", TOKEN_VAR];

/// A shell script that runs `git "$@"` with a temporary askpass program installed, unless
/// the distribution sets its own. The program prints the question on stderr after the
/// marker and token, and reads `ok:<answer>` or `cancel` from a fifo, so a question reaches
/// the app through git's own output, with no socket and no program from Windows.
///
/// The answer comes by fifo and not by the program's stdin because ssh starts it with its
/// own stdin (the pipe to git, or `/dev/null`), never the one the app writes to. A loop
/// copies each line of the script's stdin into the fifo.
///
/// The token is made up for each command (see [`new_token`]): without it, a remote could
/// print a marker of its own among git's output and have the app ask a question it wrote.
pub const BOOTSTRAP: &str = r#"
d=$(mktemp -d 2>/dev/null) || exec git "$@"
f="$d/askpass"
if mkfifo "$d/reply" 2>/dev/null; then
exec 3<&0
( while IFS= read -r line <&3; do printf '%s\n' "$line" > "$d/reply"; done ) &
relay=$!
fi
cat > "$f" <<'ASKPASS'
#!/bin/sh
p=$(printf '%s' "$1" | tr '\n\t' '  ')
printf '\037\037GITCLIENT-ASKPASS:%s\t%s\n' "$GITCLIENT_ASKPASS_TOKEN" "$p" >&2
IFS= read -r reply < "${0%/*}/reply" || exit 1
case "$reply" in ok:*) printf '%s\n' "${reply#ok:}";; *) exit 1;; esac
ASKPASS
chmod 700 "$f"
if [ -z "$SSH_ASKPASS" ]; then SSH_ASKPASS="$f"; SSH_ASKPASS_REQUIRE=force; export SSH_ASKPASS SSH_ASKPASS_REQUIRE; fi
if [ -z "$GIT_ASKPASS" ]; then GIT_ASKPASS="$f"; export GIT_ASKPASS; fi
git "$@" 3<&-
s=$?
[ -n "$relay" ] && kill "$relay" 2>/dev/null
rm -rf "$d"
exit $s
"#;

/// What carries the token to the askpass program.
pub const TOKEN_VAR: &str = "GITCLIENT_ASKPASS_TOKEN";

const MARKER: &str = "\x1f\x1fGITCLIENT-ASKPASS:";

/// A hard-to-guess token for one command, from the operating system's randomness.
pub fn new_token() -> String {
    // A new `RandomState` has keys of its own, and hashing nothing with them gives a word
    // that can't be guessed without knowing them.
    let word = || RandomState::new().build_hasher().finish();
    format!("{:016x}{:016x}", word(), word())
}

/// The question in `line` if it is one from the askpass program of the command that was
/// given `token`, and nothing otherwise.
pub fn question<'a>(line: &'a str, token: &str) -> Option<&'a str> {
    if token.is_empty() {
        return None;
    }
    let rest = line.strip_prefix(MARKER)?.strip_prefix(token)?;
    rest.strip_prefix('\t')
        .map(|prompt| prompt.trim_end_matches(['\n', '\r']))
}

/// The line the askpass program reads for an answer, or for none.
pub fn reply(answer: Option<&str>) -> String {
    match answer {
        Some(answer) => format!("ok:{}\n", answer.replace(['\n', '\r'], "")),
        None => "cancel\n".to_owned(),
    }
}

/// A folder inside a WSL distribution.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Location {
    pub distro: String,
    /// The folder as the distribution sees it: `/home/ada/project`.
    pub linux_path: String,
}

/// Where `path` is inside WSL, or `None` for any other path.
pub fn locate(path: &Path) -> Option<Location> {
    let text = path.to_string_lossy();
    // `canonicalize` turns `\\server\share` into `\\?\UNC\server\share`.
    let rest = text
        .strip_prefix(r"\\?\UNC\")
        .or_else(|| text.strip_prefix(r"\\"))?;
    let mut parts = rest.split(['\\', '/']);
    let host = parts.next()?;
    if !(host.eq_ignore_ascii_case("wsl$") || host.eq_ignore_ascii_case("wsl.localhost")) {
        return None;
    }
    let distro = parts.next().filter(|name| !name.is_empty())?;
    let mut linux_path = String::new();
    for part in parts.filter(|part| !part.is_empty()) {
        linux_path.push('/');
        linux_path.push_str(part);
    }
    if linux_path.is_empty() {
        linux_path.push('/');
    }
    Some(Location {
        distro: distro.to_owned(),
        linux_path,
    })
}

impl Location {
    /// The Windows path of a path inside this distribution.
    pub fn to_windows(&self, linux_path: &str) -> PathBuf {
        let mut path = format!(r"\\wsl.localhost\{}", self.distro);
        for part in linux_path.split('/').filter(|part| !part.is_empty()) {
            path.push('\\');
            path.push_str(part);
        }
        PathBuf::from(path)
    }

    /// A git command that runs in this folder, inside the distribution. The arguments added
    /// to it are git's own: the script that starts git is already in place.
    pub fn git(&self) -> Command {
        let mut command = Command::new(PROGRAM);
        command
            .args(["-d", &self.distro, "--cd", &self.linux_path, "--exec"])
            .args(["sh", "-c", BOOTSTRAP, "sh"])
            .env("WSLENV", wslenv(std::env::var("WSLENV").ok().as_deref()));
        command
    }
}

/// `WSLENV` with [`FORWARDED`] added to the user's own entries, which stay.
fn wslenv(existing: Option<&str>) -> String {
    let mut entries: Vec<&str> = existing
        .unwrap_or_default()
        .split(':')
        .filter(|entry| !entry.is_empty())
        .collect();
    for name in FORWARDED {
        if !entries.contains(&name) {
            entries.push(name);
        }
    }
    entries.join(":")
}

/// A path git printed for the repository at `repo`, as the app should hold it: the same
/// text for an ordinary repository, a Windows path for one in WSL.
pub fn from_git(repo: &Path, printed: &str) -> PathBuf {
    match locate(repo) {
        Some(location) if printed.starts_with('/') => location.to_windows(printed),
        _ => PathBuf::from(printed),
    }
}

/// A path to give git for the repository at `repo`, the inverse of [`from_git`].
pub fn for_git(repo: &Path, path: &Path) -> String {
    match locate(path).filter(|_| locate(repo).is_some()) {
        Some(location) => location.linux_path,
        None => path.to_string_lossy().into_owned(),
    }
}

/// A command made by [`Location::git`], told apart from any other so that the log can show
/// what the person ran rather than the script around it.
pub struct Invocation {
    /// The repository it runs in, as a Windows path.
    pub repo: PathBuf,
    /// Git's own arguments, without the `wsl.exe` part.
    pub args: Vec<OsString>,
}

impl Invocation {
    /// What `command` is, or `None` when it isn't git in WSL.
    pub fn of(command: &Command) -> Option<Self> {
        if command.get_program() != PROGRAM {
            return None;
        }
        let all: Vec<_> = command.get_args().collect();
        let text = |index: usize| all.get(index).and_then(|arg| arg.to_str());
        let [d, cd, exec, sh, c, script] = [0, 2, 4, 5, 6, 7].map(text);
        let shape = [d, cd, exec, sh, c, script];
        let expected = ["-d", "--cd", "--exec", "sh", "-c", BOOTSTRAP].map(Some);
        if shape != expected || all.len() < PREFIX {
            return None;
        }
        let location = Location {
            distro: text(1)?.to_owned(),
            linux_path: text(3)?.to_owned(),
        };
        Some(Self {
            repo: location.to_windows(&location.linux_path),
            args: all[PREFIX..].iter().map(|arg| (*arg).to_owned()).collect(),
        })
    }

    /// Whether git could ask a question of a remote: its subcommand is one that connects.
    /// The check is on the subcommand and not on every argument, so that a branch that is
    /// called `fetch` doesn't count.
    pub fn may_ask(&self) -> bool {
        const CONNECTS: [&str; 7] = [
            "fetch",
            "pull",
            "push",
            "clone",
            "ls-remote",
            "remote",
            "submodule",
        ];
        let mut args = self.args.iter();
        while let Some(arg) = args.next() {
            let arg = arg.to_string_lossy();
            match arg.as_ref() {
                // These options take the next argument as their value.
                "-c" | "-C" => {
                    args.next();
                }
                option if option.starts_with('-') => {}
                subcommand => return CONNECTS.contains(&subcommand),
            }
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(path: &str) -> Option<Location> {
        locate(Path::new(path))
    }

    #[test]
    fn both_unc_spellings_and_the_canonical_form_are_found() {
        let expected = Some(Location {
            distro: "Ubuntu".into(),
            linux_path: "/home/ada/my repo".into(),
        });
        assert_eq!(at(r"\\wsl.localhost\Ubuntu\home\ada\my repo"), expected);
        assert_eq!(at(r"\\wsl$\Ubuntu\home\ada\my repo\"), expected);
        assert_eq!(
            at(r"\\?\UNC\wsl.localhost\Ubuntu\home\ada\my repo"),
            expected
        );
        assert_eq!(at(r"\\WSL$\Ubuntu/home/ada/my repo"), expected);
    }

    #[test]
    fn the_root_of_a_distribution_is_a_slash() {
        assert_eq!(at(r"\\wsl$\Debian").unwrap().linux_path, "/");
    }

    #[test]
    fn other_paths_are_not_wsl() {
        assert_eq!(at(r"C:\Users\ada\repo"), None);
        assert_eq!(at(r"\\fileserver\share\repo"), None);
        assert_eq!(at(r"\\wsl$"), None);
        assert_eq!(at("/home/ada/repo"), None);
    }

    #[test]
    fn git_runs_in_the_distribution_in_the_folder() {
        let command = at(r"\\wsl$\Ubuntu\home\ada").unwrap().git();
        let args: Vec<_> = command.get_args().map(|a| a.to_string_lossy()).collect();
        assert_eq!(command.get_program(), PROGRAM);
        assert_eq!(args[..5], ["-d", "Ubuntu", "--cd", "/home/ada", "--exec"]);
        assert_eq!(args[5..7], ["sh", "-c"]);
        assert_eq!(args.len(), PREFIX, "git's own arguments are added after");
    }

    #[test]
    fn the_users_own_wslenv_is_kept() {
        let ours = "GIT_OPTIONAL_LOCKS:GIT_TERMINAL_PROMPT:GITCLIENT_ASKPASS_TOKEN";
        assert_eq!(wslenv(None), ours);
        assert_eq!(
            wslenv(Some("FOO/p:GIT_TERMINAL_PROMPT")),
            "FOO/p:GIT_TERMINAL_PROMPT:GIT_OPTIONAL_LOCKS:GITCLIENT_ASKPASS_TOKEN"
        );
    }

    #[test]
    fn printed_paths_come_back_as_windows_paths() {
        let repo = Path::new(r"\\wsl.localhost\Ubuntu\home\ada\repo");
        assert_eq!(
            from_git(repo, "/home/ada/repo/sub"),
            PathBuf::from(r"\\wsl.localhost\Ubuntu\home\ada\repo\sub")
        );
        assert_eq!(
            from_git(Path::new("/tmp/r"), "/tmp/r/sub"),
            PathBuf::from("/tmp/r/sub")
        );
    }

    #[test]
    fn paths_given_to_git_are_the_distributions_own() {
        let repo = Path::new(r"\\wsl$\Ubuntu\home\ada\repo");
        let other = Path::new(r"\\wsl.localhost\Ubuntu\home\ada\wt");
        assert_eq!(for_git(repo, other), "/home/ada/wt");
        assert_eq!(for_git(Path::new("/r"), Path::new("/r/wt")), "/r/wt");
    }

    #[test]
    fn a_command_made_for_wsl_is_recognised_and_other_commands_are_not() {
        let mut command = at(r"\\wsl$\Ubuntu\home\ada").unwrap().git();
        command.args(["-C", "x y", "log"]);
        let invocation = Invocation::of(&command).expect("recognised");
        assert_eq!(
            invocation.repo,
            PathBuf::from(r"\\wsl.localhost\Ubuntu\home\ada")
        );
        assert_eq!(invocation.args, ["-C", "x y", "log"]);

        let mut plain = Command::new("git");
        plain.args(["-C", "/r", "log"]);
        assert!(Invocation::of(&plain).is_none());
        let mut other = Command::new(PROGRAM);
        other.args(["-d", "Ubuntu", "--cd", "/", "--exec", "rm", "-rf", "x", "y"]);
        assert!(Invocation::of(&other).is_none(), "only our own script");
    }

    fn may_ask(args: &[&str]) -> bool {
        let mut command = at(r"\\wsl$\Ubuntu\home").unwrap().git();
        command.args(args);
        Invocation::of(&command).unwrap().may_ask()
    }

    #[test]
    fn only_commands_that_connect_to_a_remote_may_ask() {
        assert!(may_ask(&["fetch", "--all"]));
        assert!(may_ask(&["-c", "x.y=z", "push", "origin", "main"]));
        assert!(may_ask(&["-C", "/elsewhere", "pull"]));
        assert!(
            !may_ask(&["log", "--oneline", "fetch"]),
            "only the subcommand counts"
        );
        assert!(!may_ask(&["rev-parse", "HEAD"]));
        assert!(!may_ask(&[]));
    }

    #[test]
    fn a_question_is_only_taken_from_the_command_that_has_its_token() {
        let token = new_token();
        assert_eq!(token.len(), 32);
        assert_ne!(token, new_token(), "every command gets its own");
        let line = format!("{MARKER}{token}\tEnter passphrase for key 'k': \n");
        assert_eq!(
            question(&line, &token),
            Some("Enter passphrase for key 'k': ")
        );
        assert_eq!(question(&line, "another"), None, "a forged marker");
        assert_eq!(question(&line, ""), None);
        assert_eq!(question("remote: hello\n", &token), None);
    }

    #[test]
    fn answers_are_one_line_whatever_was_typed() {
        assert_eq!(reply(Some("pass\nword")), "ok:password\n");
        assert_eq!(reply(None), "cancel\n");
    }
}
