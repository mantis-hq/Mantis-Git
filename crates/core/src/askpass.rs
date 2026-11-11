//! Lets git and ssh ask the user a question: a key's passphrase, an HTTPS login, whether
//! to trust a new host. They have no terminal to ask on, so they run an "askpass" program
//! and read its answer. That program is this app run again (see [`run_helper`]). It
//! hands the question over a private socket to the running app, which asks in a popup
//! and sends the answer back. All of it is plain data, so it works without a window.
//!
//! The app calls [`serve`] once and gets the questions. Without it, as in tests, git is
//! told to fail at once rather than wait for an answer nobody can give.

use std::collections::HashMap;
#[cfg(unix)]
use std::process::Command;
use std::sync::{Mutex, OnceLock};

/// What the helper reads its socket path from. Its presence is what makes the app run as
/// the helper rather than open a window.
#[cfg(unix)]
const SOCKET_VAR: &str = "MANTISGIT_ASKPASS_SOCKET";

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
        let kept = if self.is_secret() {
            " It is kept until Mantis Git closes."
        } else {
            ""
        };
        format!("Press Enter to {}, or Esc to cancel.{kept}", self.verb())
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

/// Where questions go, once the app is serving.
static QUESTIONS: OnceLock<async_channel::Sender<Request>> = OnceLock::new();
static REMEMBERED: Mutex<Option<Remembered>> = Mutex::new(None);

/// Answers kept until the app closes, so the same key isn't asked for on every fetch.
/// Each remembers which process it last answered: ssh asking the same question again
/// itself means the answer was wrong, and it is forgotten.
#[derive(Default)]
struct Remembered(HashMap<String, (String, u32)>);

impl Remembered {
    fn recall(&mut self, prompt: &str, asker: u32) -> Option<String> {
        match self.0.get_mut(prompt) {
            Some((answer, last)) if *last != asker => {
                *last = asker;
                Some(answer.clone())
            }
            Some(_) => {
                self.0.remove(prompt);
                None
            }
            None => None,
        }
    }

    fn keep(&mut self, prompt: &str, answer: &str, asker: u32) {
        self.0.insert(prompt.to_owned(), (answer.to_owned(), asker));
    }
}

fn remembered<T>(f: impl FnOnce(&mut Remembered) -> T) -> T {
    let mut slot = REMEMBERED
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    f(slot.get_or_insert_with(Remembered::default))
}

/// Asks the window, unless an earlier answer to the same question can be reused. `None`
/// means the person cancelled, or nothing is serving. This blocks until they answer.
pub(crate) fn ask(asker: u32, prompt: &str) -> Option<String> {
    ask_through(QUESTIONS.get()?, asker, prompt)
}

fn ask_through(
    questions: &async_channel::Sender<Request>,
    asker: u32,
    prompt: &str,
) -> Option<String> {
    if let Some(value) = remembered(|kept| kept.recall(prompt, asker)) {
        return Some(value);
    }
    let (reply, answer) = async_channel::bounded(1);
    let request = Request {
        prompt: prompt.to_owned(),
        reply,
    };
    questions.send_blocking(request).ok()?;
    // A closed or dropped request is a cancel.
    let value = answer.recv_blocking().ok()??;
    remembered(|kept| kept.keep(prompt, &value, asker));
    Some(value)
}

#[cfg(unix)]
pub use unix::{run_helper, variables};

/// Starts listening for questions from git and ssh, and returns them as they come. Call
/// it once.
#[cfg(unix)]
pub fn serve() -> Option<Questions> {
    unix::serve()
}

/// Where there are no sockets, questions come only from git running in WSL, which the
/// output of the command carries, so there is nothing to listen on.
#[cfg(not(unix))]
pub fn serve() -> Option<Questions> {
    let (questions, receiver) = async_channel::unbounded();
    QUESTIONS.set(questions).ok()?;
    Some(receiver)
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
#[cfg(unix)]
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
    use super::{QUESTIONS, Questions, Request, SOCKET_VAR, ask_through};
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
        let dir = std::env::temp_dir().join(format!("mantisgit-askpass-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::DirBuilder::new().mode(0o700).create(&dir).ok()?;
        let path = dir.join("socket");
        let listener = UnixListener::bind(&path).ok()?;
        SOCKET.set(path).ok()?;
        let (requests, receiver) = async_channel::unbounded();
        let _ = QUESTIONS.set(requests.clone());
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
        // The process that is asking, a tab, then the question.
        let line = line.trim_end_matches(['\n', '\r']);
        let (asker, prompt) = line.split_once('\t').unwrap_or(("0", line));
        let asker: u32 = asker.parse().unwrap_or(0);
        let prompt = prompt.to_owned();
        let mut stream = stream;
        let _ = match ask_through(requests, asker, &prompt) {
            Some(value) => writeln!(stream, "ok\n{value}"),
            None => writeln!(stream, "cancel"),
        };
    }

    /// The environment that makes git and ssh run this app as their askpass program.
    pub fn variables() -> Option<Vec<(&'static str, OsString)>> {
        let socket = SOCKET.get()?;
        let exe = crate::install::exe_path();
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
        let asker = std::os::unix::process::parent_id();
        Some(match ask_app(&PathBuf::from(socket), asker, &prompt) {
            Some(answer) => {
                println!("{answer}");
                0
            }
            None => 1,
        })
    }

    fn ask_app(socket: &PathBuf, asker: u32, prompt: &str) -> Option<String> {
        let mut stream = UnixStream::connect(socket).ok()?;
        // A prompt is one line to the app, whatever it held.
        writeln!(stream, "{asker}\t{}", prompt.replace(['\n', '\t'], " ")).ok()?;
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
        use super::super::Remembered;
        use super::*;

        #[test]
        fn a_question_is_answered_through_the_socket_and_cancelled_when_dropped() {
            let requests = serve().expect("the socket binds");
            let socket = SOCKET.get().unwrap().clone();

            let first = {
                let socket = socket.clone();
                std::thread::spawn(move || ask_app(&socket, 1, "Enter passphrase for key 'k': "))
            };
            let request = requests.recv_blocking().unwrap();
            assert_eq!(request.sentence(), "Enter passphrase for key 'k'");
            assert!(request.is_secret());
            request.answer(Some("hunter2".into()));
            assert_eq!(first.join().unwrap().as_deref(), Some("hunter2"));

            let second =
                std::thread::spawn(move || ask_app(&socket, 1, "Username for 'https://x': "));
            let request = requests.recv_blocking().unwrap();
            assert!(!request.is_secret());
            drop(request);
            assert_eq!(second.join().unwrap(), None);
        }

        #[test]
        fn an_answer_is_reused_by_the_next_process_but_not_by_one_asking_again() {
            let mut kept = Remembered::default();
            assert_eq!(kept.recall("key: ", 10), None);
            kept.keep("key: ", "secret", 10);
            // The next fetch is a new ssh, so it gets the answer without asking.
            assert_eq!(kept.recall("key: ", 11).as_deref(), Some("secret"));
            // The same ssh asking again means the answer was refused.
            assert_eq!(kept.recall("key: ", 11), None);
            assert_eq!(kept.recall("key: ", 12), None, "and it is forgotten");
        }
    }
}
