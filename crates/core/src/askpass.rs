//! Lets git and ssh ask the user a question: a key's passphrase, an HTTPS login, whether
//! to trust a new host. They have no terminal to ask on, so they run an "askpass" program
//! and read its answer. That program is this app run again (see [`run_helper`]). It
//! hands the question over a private socket to the running app, which asks in a popup
//! and sends the answer back. All of it is plain data, so it works without a window.
//!
//! The app calls [`serve`] once and gets the questions. Without it, as in tests, git is
//! told to fail at once rather than wait for an answer nobody can give.

use std::process::Command;

/// What the helper reads its socket path from. Its presence is what makes the app run as
/// the helper rather than open a window.
const SOCKET_VAR: &str = "GITCLIENT_ASKPASS_SOCKET";

/// The questions as they come, from [`serve`].
pub type Questions = async_channel::Receiver<Request>;

/// A question waiting for an answer. Dropping it unanswered cancels it.
pub struct Request {
    prompt: String,
    reply: async_channel::Sender<Option<String>>,
}

impl Request {
    /// A question and the receiver of its answer, for tests that play git's part.
    #[cfg(any(test, feature = "testing"))]
    pub fn new(prompt: &str) -> (Self, async_channel::Receiver<Option<String>>) {
        let (reply, answer) = async_channel::bounded(1);
        let prompt = prompt.to_owned();
        (Self { prompt, reply }, answer)
    }

    /// The question as ssh or git worded it, such as
    /// "Enter passphrase for key '/home/me/.ssh/id_ed25519': ".
    pub fn prompt(&self) -> &str {
        &self.prompt
    }

    /// The question as a sentence for the popup, without the colon that waits for typing.
    pub fn sentence(&self) -> String {
        self.prompt
            .trim()
            .trim_end_matches(':')
            .trim_end()
            .to_owned()
    }

    /// Whether what is typed must be hidden: passphrases, passwords and PINs.
    pub fn is_secret(&self) -> bool {
        let prompt = self.prompt.to_lowercase();
        ["passphrase", "password", "pin for", "token"]
            .iter()
            .any(|word| prompt.contains(word))
    }

    /// The button that sends the answer, named for what it does.
    pub fn action(&self) -> &'static str {
        let prompt = self.prompt.to_lowercase();
        if prompt.contains("passphrase") {
            "Unlock key"
        } else if prompt.contains("password") {
            "Log in"
        } else {
            "Answer"
        }
    }

    /// The line naming the keys.
    pub fn hint(&self) -> String {
        format!("Press Enter to {}, or Esc to cancel.", self.verb())
    }

    fn verb(&self) -> &'static str {
        match self.action() {
            "Unlock key" => "unlock the key",
            "Log in" => "log in",
            _ => "answer",
        }
    }

    /// Sends the answer to the waiting git, or cancels with `None`.
    pub fn answer(self, value: Option<String>) {
        let _ = self.reply.send_blocking(value);
    }
}

#[cfg(unix)]
pub use unix::{run_helper, serve, variables};

#[cfg(not(unix))]
pub fn serve() -> Option<Questions> {
    None
}

/// Environment that makes git and ssh ask through the app, or `None` when it isn't serving.
#[cfg(not(unix))]
pub fn variables() -> Option<Vec<(&'static str, std::ffi::OsString)>> {
    None
}

/// When this process was started by git or ssh as the askpass program, answers the question
/// and returns the exit status for `main` to quit with. `None` means this is the app.
#[cfg(not(unix))]
pub fn run_helper() -> Option<i32> {
    None
}

/// Applies [`variables`] to `command`. Where the app isn't serving, or a program of the
/// user's own is set, ssh is told to use one that fails, so it never waits on a terminal
/// where nobody will see the question.
pub(crate) fn configure(command: &mut Command) {
    if std::env::var_os("SSH_ASKPASS").is_some() {
        return;
    }
    match variables() {
        Some(variables) => {
            command.envs(variables);
        }
        None => {
            command
                .env("SSH_ASKPASS", "/bin/false")
                .env("SSH_ASKPASS_REQUIRE", "force");
        }
    }
}

#[cfg(unix)]
mod unix {
    use super::{Questions, Request, SOCKET_VAR};
    use std::ffi::OsString;
    use std::io::{BufRead, BufReader, Write};
    use std::os::unix::fs::DirBuilderExt;
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::PathBuf;
    use std::sync::OnceLock;

    static SOCKET: OnceLock<PathBuf> = OnceLock::new();

    /// Starts listening for questions from git and ssh, and returns them as they come. Call
    /// it once; the socket lives in a folder only this user can open.
    pub fn serve() -> Option<Questions> {
        let dir = std::env::temp_dir().join(format!("gitclient-askpass-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::DirBuilder::new().mode(0o700).create(&dir).ok()?;
        let path = dir.join("socket");
        let listener = UnixListener::bind(&path).ok()?;
        SOCKET.set(path).ok()?;
        let (requests, receiver) = async_channel::unbounded();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let requests = requests.clone();
                // One thread per question, since each waits for a person.
                std::thread::spawn(move || ask(stream, &requests));
            }
        });
        Some(receiver)
    }

    /// Reads one question from the helper, waits for the window's answer and sends it back.
    fn ask(stream: UnixStream, requests: &async_channel::Sender<Request>) {
        let mut line = String::new();
        let Ok(clone) = stream.try_clone() else {
            return;
        };
        if BufReader::new(clone).read_line(&mut line).is_err() {
            return;
        }
        let prompt = line.trim_end_matches(['\n', '\r']).to_owned();
        let (reply, answer) = async_channel::bounded(1);
        if requests.send_blocking(Request { prompt, reply }).is_err() {
            return;
        }
        let mut stream = stream;
        // A closed or dropped request is a cancel.
        let _ = match answer.recv_blocking() {
            Ok(Some(value)) => writeln!(stream, "ok\n{value}"),
            _ => writeln!(stream, "cancel"),
        };
    }

    /// The environment that makes git and ssh run this app as their askpass program.
    pub fn variables() -> Option<Vec<(&'static str, OsString)>> {
        let socket = SOCKET.get()?;
        let exe = std::env::current_exe().ok()?;
        Some(vec![
            ("SSH_ASKPASS", exe.into_os_string()),
            // Even with a terminal to hand, ssh asks here rather than on it.
            ("SSH_ASKPASS_REQUIRE", "force".into()),
            (SOCKET_VAR, socket.clone().into_os_string()),
        ])
    }

    /// Runs as the askpass program when git or ssh started this process: asks the app the
    /// question given as the first argument, prints the answer, and returns the exit status.
    pub fn run_helper() -> Option<i32> {
        let socket = std::env::var_os(SOCKET_VAR)?;
        let prompt = std::env::args().nth(1).unwrap_or_default();
        Some(match ask_app(&PathBuf::from(socket), &prompt) {
            Some(answer) => {
                println!("{answer}");
                0
            }
            None => 1,
        })
    }

    fn ask_app(socket: &PathBuf, prompt: &str) -> Option<String> {
        let mut stream = UnixStream::connect(socket).ok()?;
        // A prompt is one line to the app, whatever it held.
        writeln!(stream, "{}", prompt.replace('\n', " ")).ok()?;
        let mut reader = BufReader::new(stream);
        let mut status = String::new();
        reader.read_line(&mut status).ok()?;
        if status.trim_end() != "ok" {
            return None;
        }
        let mut answer = String::new();
        reader.read_line(&mut answer).ok()?;
        Some(answer.trim_end_matches(['\n', '\r']).to_owned())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn a_question_is_answered_through_the_socket_and_cancelled_when_dropped() {
            let requests = serve().expect("the socket binds");
            let socket = SOCKET.get().unwrap().clone();

            let first = {
                let socket = socket.clone();
                std::thread::spawn(move || ask_app(&socket, "Enter passphrase for key 'k': "))
            };
            let request = requests.recv_blocking().unwrap();
            assert_eq!(request.sentence(), "Enter passphrase for key 'k'");
            assert!(request.is_secret());
            request.answer(Some("hunter2".into()));
            assert_eq!(first.join().unwrap().as_deref(), Some("hunter2"));

            let second = std::thread::spawn(move || ask_app(&socket, "Username for 'https://x': "));
            let request = requests.recv_blocking().unwrap();
            assert!(!request.is_secret());
            drop(request);
            assert_eq!(second.join().unwrap(), None);
        }
    }
}
