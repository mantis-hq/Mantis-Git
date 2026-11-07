//! A shell running in a pseudo-terminal, and the screen it has drawn so far.
//!
//! This is the whole of a terminal except the window: it starts the shell, feeds what the
//! shell prints to a screen model, and turns keys into the bytes a shell expects. The
//! dock draws [`Terminal::snapshot`] and sends keys to [`Terminal::send_key`].

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};
use std::io::{Read, Write};
use std::num::NonZeroU16;
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;

/// Lines of history kept above the screen.
const SCROLLBACK: usize = 5000;

/// A colour the shell asked for. The view maps these onto the theme.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Color {
    Default,
    /// One of the 256 terminal colours. 0 to 15 are the named ones.
    Indexed(u8),
    Rgb(u8, u8, u8),
}

impl From<vt100::Color> for Color {
    fn from(color: vt100::Color) -> Self {
        match color {
            vt100::Color::Default => Self::Default,
            vt100::Color::Idx(index) => Self::Indexed(index),
            vt100::Color::Rgb(r, g, b) => Self::Rgb(r, g, b),
        }
    }
}

/// Neighbouring cells that look the same.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Run {
    pub text: String,
    pub foreground: Color,
    pub background: Color,
    pub bold: bool,
    pub italic: bool,
    pub underline: bool,
    pub inverse: bool,
}

/// What is on screen.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Snapshot {
    /// One entry per row, each a list of runs that together fill the row.
    pub rows: Vec<Vec<Run>>,
    /// Where the cursor is, unless the shell hid it or the view is scrolled.
    pub cursor: Option<Cursor>,
}

/// The cursor's cell, found in the row's text so it can be drawn as a highlight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Cursor {
    pub row: usize,
    /// The bytes of the row's text that the cursor covers.
    pub bytes: std::ops::Range<usize>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Size {
    pub rows: u16,
    pub cols: u16,
}

impl Size {
    fn clamped(self) -> (NonZeroU16, NonZeroU16) {
        let one = |n: u16| NonZeroU16::new(n.max(1)).expect("at least one");
        (one(self.rows), one(self.cols))
    }
}

struct Shared {
    parser: vt100::Parser,
    /// How the shell ended, once it has.
    exit: Option<String>,
}

pub struct Terminal {
    shared: Arc<Mutex<Shared>>,
    master: Mutex<Box<dyn MasterPty + Send>>,
    writer: Mutex<Box<dyn Write + Send>>,
    child: Arc<Mutex<Box<dyn Child + Send + Sync>>>,
    changes: async_channel::Receiver<()>,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Terminal {
    /// Starts the user's shell in `directory`.
    pub fn spawn(directory: &Path, size: Size) -> Result<Self, String> {
        let pair = native_pty_system()
            .openpty(pty_size(size))
            .map_err(|error| format!("could not open a terminal: {error}"))?;

        let mut command = CommandBuilder::new(shell());
        if cfg!(target_os = "macos") {
            // Terminal apps on macOS start login shells. Without that, an app opened from
            // the Dock misses the PATH that the user's profile builds, and with it git.
            command.arg("-l");
        }
        command.cwd(directory);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        // Launched from inside a git hook these would point at some other repository.
        command.env_remove("GIT_DIR");
        command.env_remove("GIT_WORK_TREE");
        let child = pair
            .slave
            .spawn_command(command)
            .map_err(|error| format!("could not start the shell: {error}"))?;
        let mut reader = pair
            .master
            .try_clone_reader()
            .map_err(|error| format!("could not read the terminal: {error}"))?;
        let writer = pair
            .master
            .take_writer()
            .map_err(|error| format!("could not write to the terminal: {error}"))?;

        let (rows, cols) = size.clamped();
        let shared = Arc::new(Mutex::new(Shared {
            parser: vt100::Parser::new(rows, cols, SCROLLBACK),
            exit: None,
        }));
        let child = Arc::new(Mutex::new(child));
        let (wake, changes) = async_channel::bounded(1);

        let (output, process) = (shared.clone(), child.clone());
        thread::Builder::new()
            .name("terminal".into())
            .spawn(move || {
                let mut buffer = [0u8; 8192];
                // Read until the shell closes its side, which is how it exits.
                while let Ok(read) = reader.read(&mut buffer) {
                    if read == 0 {
                        break;
                    }
                    lock(&output).parser.process(&buffer[..read]);
                    let _ = wake.try_send(());
                }
                let status = lock(&process).wait();
                lock(&output).exit = Some(match status {
                    Ok(status) if status.success() => "The shell exited.".to_owned(),
                    Ok(status) => format!("The shell exited with code {}.", status.exit_code()),
                    Err(_) => "The shell stopped.".to_owned(),
                });
                let _ = wake.try_send(());
            })
            .map_err(|error| format!("could not start the terminal thread: {error}"))?;

        Ok(Self {
            shared,
            master: Mutex::new(pair.master),
            writer: Mutex::new(writer),
            child,
            changes,
        })
    }

    /// Wakes when the screen changes or the shell exits.
    pub fn changes(&self) -> async_channel::Receiver<()> {
        self.changes.clone()
    }

    /// How the shell ended, if it has.
    pub fn exit_message(&self) -> Option<String> {
        lock(&self.shared).exit.clone()
    }

    pub fn write(&self, bytes: &[u8]) {
        let mut writer = lock(&self.writer);
        let _ = writer.write_all(bytes).and_then(|()| writer.flush());
    }

    /// Sends pasted text, marked as a paste if the program asked to be told.
    pub fn paste(&self, text: &str) {
        let marked = lock(&self.shared).parser.screen().bracketed_paste();
        if marked {
            self.write(format!("\x1b[200~{text}\x1b[201~").as_bytes());
        } else {
            self.write(text.replace("\r\n", "\r").replace('\n', "\r").as_bytes());
        }
    }

    /// Sends a key press. Returns whether the terminal had a use for it.
    pub fn send_key(&self, key: &Key<'_>) -> bool {
        let application_cursor = lock(&self.shared).parser.screen().application_cursor();
        let Some(bytes) = encode_key(key, application_cursor) else {
            return false;
        };
        // Typing returns to the live screen, as every terminal does.
        lock(&self.shared).parser.screen_mut().set_scrollback(0);
        self.write(&bytes);
        true
    }

    /// Tells the shell, and the screen model, the area the view has for it. Returns whether
    /// that was a change.
    pub fn resize(&self, size: Size) -> bool {
        let (rows, cols) = size.clamped();
        {
            let mut shared = lock(&self.shared);
            if shared.parser.screen().size() == (rows, cols) {
                return false;
            }
            shared.parser.set_size(rows, cols);
        }
        let _ = lock(&self.master).resize(pty_size(size));
        true
    }

    /// Moves the view through history: positive goes back in time.
    pub fn scroll(&self, lines: isize) {
        let mut shared = lock(&self.shared);
        let screen = shared.parser.screen_mut();
        let target = screen.scrollback().saturating_add_signed(lines);
        screen.set_scrollback(target);
    }

    pub fn snapshot(&self) -> Snapshot {
        let mut shared = lock(&self.shared);
        let screen = shared.parser.screen_mut();
        let (rows, cols) = screen.size();
        let scrolled = screen.scrollback() > 0;
        let cursor_cell = (!screen.hide_cursor() && !scrolled).then(|| screen.cursor_position());
        let mut snapshot = Snapshot {
            rows: Vec::with_capacity(rows.get() as usize),
            cursor: None,
        };
        for row in 0..rows.get() {
            let mut runs: Vec<Run> = Vec::new();
            let mut length = 0;
            for col in 0..cols.get() {
                let Some(cell) = screen.cell(row, col) else {
                    continue;
                };
                if cell.is_wide_continuation() {
                    continue;
                }
                let text = if cell.has_contents() {
                    cell.contents()
                } else {
                    " "
                };
                if cursor_cell == Some((row, col)) {
                    snapshot.cursor = Some(Cursor {
                        row: row as usize,
                        bytes: length..length + text.len(),
                    });
                }
                length += text.len();
                let run = Run {
                    text: text.to_owned(),
                    foreground: cell.fgcolor().into(),
                    background: cell.bgcolor().into(),
                    bold: cell.bold(),
                    italic: cell.italic(),
                    underline: cell.underline(),
                    inverse: cell.inverse(),
                };
                match runs.last_mut() {
                    Some(last) if same_look(last, &run) => last.text.push_str(&run.text),
                    _ => runs.push(run),
                }
            }
            snapshot.rows.push(runs);
        }
        snapshot
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        // The reader thread ends once the shell is gone.
        let _ = lock(&self.child).kill();
    }
}

fn same_look(a: &Run, b: &Run) -> bool {
    (
        a.foreground,
        a.background,
        a.bold,
        a.italic,
        a.underline,
        a.inverse,
    ) == (
        b.foreground,
        b.background,
        b.bold,
        b.italic,
        b.underline,
        b.inverse,
    )
}

fn pty_size(size: Size) -> PtySize {
    PtySize {
        rows: size.rows.max(1),
        cols: size.cols.max(1),
        pixel_width: 0,
        pixel_height: 0,
    }
}

/// The user's shell. An app started from a launcher rather than a terminal has no `SHELL`
/// on macOS, so it falls back to that system's own default.
fn shell() -> String {
    let from_environment = if cfg!(windows) { "COMSPEC" } else { "SHELL" };
    std::env::var(from_environment)
        .ok()
        .filter(|shell| !shell.is_empty())
        .unwrap_or_else(|| {
            if cfg!(windows) {
                "powershell.exe"
            } else if cfg!(target_os = "macos") {
                "/bin/zsh"
            } else {
                "/bin/sh"
            }
            .to_owned()
        })
}

/// A key press, as the window reports it.
#[derive(Clone, Copy, Debug, Default)]
pub struct Key<'a> {
    /// The key's name: "a", "enter", "up", "f5".
    pub name: &'a str,
    /// The text the key types, if it types any.
    pub text: Option<&'a str>,
    pub control: bool,
    pub alt: bool,
    pub shift: bool,
}

/// The bytes a terminal program expects for `key`, or None for a key it has no use for.
pub fn encode_key(key: &Key<'_>, application_cursor: bool) -> Option<Vec<u8>> {
    let escape = |sequence: &str| Some(sequence.as_bytes().to_vec());
    // xterm's modifier parameter: 1 + shift + alt*2 + control*4.
    let modifier = 1 + key.shift as u8 + 2 * key.alt as u8 + 4 * key.control as u8;
    let cursor = |letter: char| {
        if modifier > 1 {
            format!("\x1b[1;{modifier}{letter}")
        } else if application_cursor {
            format!("\x1bO{letter}")
        } else {
            format!("\x1b[{letter}")
        }
    };
    let tilde = |number: u8| {
        if modifier > 1 {
            format!("\x1b[{number};{modifier}~")
        } else {
            format!("\x1b[{number}~")
        }
    };

    match key.name {
        "up" => return escape(&cursor('A')),
        "down" => return escape(&cursor('B')),
        "right" => return escape(&cursor('C')),
        "left" => return escape(&cursor('D')),
        "home" => return escape(&cursor('H')),
        "end" => return escape(&cursor('F')),
        "pageup" => return escape(&tilde(5)),
        "pagedown" => return escape(&tilde(6)),
        "insert" => return escape(&tilde(2)),
        "delete" => return escape(&tilde(3)),
        "enter" => {
            return Some(if key.alt {
                b"\x1b\r".to_vec()
            } else {
                b"\r".to_vec()
            });
        }
        "escape" => return escape("\x1b"),
        "backspace" => {
            return Some(if key.alt {
                b"\x1b\x7f".to_vec()
            } else {
                b"\x7f".to_vec()
            });
        }
        "tab" if key.shift => return escape("\x1b[Z"),
        "tab" => return escape("\t"),
        "f1" => return escape("\x1bOP"),
        "f2" => return escape("\x1bOQ"),
        "f3" => return escape("\x1bOR"),
        "f4" => return escape("\x1bOS"),
        "f5" => return escape(&tilde(15)),
        "f6" => return escape(&tilde(17)),
        "f7" => return escape(&tilde(18)),
        "f8" => return escape(&tilde(19)),
        "f9" => return escape(&tilde(20)),
        "f10" => return escape(&tilde(21)),
        "f11" => return escape(&tilde(23)),
        "f12" => return escape(&tilde(24)),
        _ => {}
    }

    // Control with a letter or one of a few symbols is a control character.
    if key.control {
        let mut chars = key.name.chars();
        if let (Some(c), None) = (chars.next(), chars.next()) {
            let code = match c.to_ascii_lowercase() {
                letter @ 'a'..='z' => Some(letter as u8 - b'a' + 1),
                '@' | ' ' | '2' => Some(0),
                '[' | '3' => Some(0x1b),
                '\\' | '4' => Some(0x1c),
                ']' | '5' => Some(0x1d),
                '^' | '6' => Some(0x1e),
                '_' | '7' | '-' => Some(0x1f),
                _ => None,
            };
            if let Some(code) = code {
                let mut bytes = Vec::new();
                if key.alt {
                    bytes.push(0x1b);
                }
                bytes.push(code);
                return Some(bytes);
            }
        }
        if key.name == "space" {
            return Some(vec![0]);
        }
    }

    let text = match key.text {
        Some(text) if !text.is_empty() => text,
        _ if key.name == "space" => " ",
        _ => return None,
    };
    let mut bytes = Vec::new();
    if key.alt {
        bytes.push(0x1b);
    }
    bytes.extend_from_slice(text.as_bytes());
    Some(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    fn key(name: &str) -> Key<'_> {
        Key {
            name,
            ..Key::default()
        }
    }

    fn encoded(key: Key<'_>) -> Vec<u8> {
        encode_key(&key, false).expect("the key is sent")
    }

    #[test]
    fn typing_sends_the_text() {
        let a = Key {
            name: "a",
            text: Some("a"),
            ..Key::default()
        };
        assert_eq!(encoded(a), b"a");
        assert_eq!(encoded(Key { alt: true, ..a }), b"\x1ba");
        let upper = Key {
            name: "a",
            text: Some("A"),
            shift: true,
            ..Key::default()
        };
        assert_eq!(encoded(upper), b"A");
        assert_eq!(encoded(key("space")), b" ");
    }

    #[test]
    fn control_letters_become_control_characters() {
        let control = |name| Key {
            control: true,
            ..key(name)
        };
        assert_eq!(encoded(control("c")), [3]);
        assert_eq!(encoded(control("w")), [23]);
        assert_eq!(encoded(control("r")), [18]);
        assert_eq!(encoded(control("[")), [0x1b]);
    }

    #[test]
    fn arrows_follow_the_application_cursor_mode_and_modifiers() {
        assert_eq!(encode_key(&key("up"), false).unwrap(), b"\x1b[A");
        assert_eq!(encode_key(&key("up"), true).unwrap(), b"\x1bOA");
        let control = Key {
            control: true,
            ..key("left")
        };
        assert_eq!(encode_key(&control, true).unwrap(), b"\x1b[1;5D");
        assert_eq!(encoded(key("delete")), b"\x1b[3~");
        assert_eq!(encoded(key("f5")), b"\x1b[15~");
    }

    #[test]
    fn editing_keys() {
        assert_eq!(encoded(key("enter")), b"\r");
        assert_eq!(encoded(key("backspace")), [0x7f]);
        assert_eq!(encoded(key("tab")), b"\t");
        assert_eq!(
            encoded(Key {
                shift: true,
                ..key("tab")
            }),
            b"\x1b[Z"
        );
    }

    #[test]
    fn keys_with_no_use_are_not_sent() {
        assert_eq!(encode_key(&key("shift"), false), None);
        assert_eq!(encode_key(&key("f13"), false), None);
    }

    fn screen_text(terminal: &Terminal) -> String {
        terminal
            .snapshot()
            .rows
            .iter()
            .map(|row| row.iter().map(|run| run.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn wait_for(terminal: &Terminal, wanted: &str) -> String {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let text = screen_text(terminal);
            if text.contains(wanted) || Instant::now() > deadline {
                return text;
            }
            thread::sleep(Duration::from_millis(20));
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_shell_runs_in_the_folder_and_its_output_reaches_the_screen() {
        let folder = tempfile::tempdir().unwrap();
        let terminal = Terminal::spawn(folder.path(), Size { rows: 10, cols: 80 }).unwrap();
        terminal.write(b"pwd; echo marker-$((6*7))\r");
        let text = wait_for(&terminal, "marker-42");
        assert!(text.contains("marker-42"), "{text}");
        let name = folder.path().file_name().unwrap().to_string_lossy();
        assert!(text.contains(name.as_ref()), "{text}");
        assert!(terminal.exit_message().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn resizing_changes_the_screen_and_exiting_is_reported() {
        let folder = tempfile::tempdir().unwrap();
        let terminal = Terminal::spawn(folder.path(), Size { rows: 10, cols: 80 }).unwrap();
        assert!(terminal.resize(Size { rows: 5, cols: 40 }));
        assert!(
            !terminal.resize(Size { rows: 5, cols: 40 }),
            "same size is no change"
        );
        assert_eq!(terminal.snapshot().rows.len(), 5);
        terminal.write(b"exit 3\r");
        let deadline = Instant::now() + Duration::from_secs(10);
        while terminal.exit_message().is_none() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(20));
        }
        assert_eq!(
            terminal.exit_message().as_deref(),
            Some("The shell exited with code 3.")
        );
    }
}
