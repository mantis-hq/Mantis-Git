//! Putting gitclient in the application menu (Linux) or the Start menu (Windows), for a
//! copy unpacked from a release archive.
//!
//! The first time it runs, the front tab asks once, in the usual popup. Whatever the
//! answer, it is not asked again: Settings → App adds or removes the launcher at any time,
//! and reads what is on disk rather than a remembered choice. Every start also mends a
//! launcher of ours that points at a copy that has moved.

use super::dialog::Confirm;
use super::{GraphView, Notice};
use gitclient_core::install::exe_path;
use gitclient_core::install::menu_entry::{Launcher, Status};
use gitclient_core::menu::command::Weight;
use gitclient_core::settings::state::AppState;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::Path;

/// At launch: mends a launcher that points at an old place, and if the user has never been
/// asked, asks in `front`. `state_file` is where the question is remembered; without one
/// nothing is asked, since it would come back every time.
pub(super) fn offer(
    front: &Entity<GraphView>,
    launcher: &Launcher,
    state_file: &Path,
    cx: &mut App,
) {
    let exe = exe_path();
    if let Err(error) = launcher.refresh(&exe) {
        eprintln!("{error}");
    }
    if AppState::load(state_file).menu_entry_asked {
        return;
    }
    // Asked once, even if the window is closed with the question still open.
    if let Err(error) = AppState::change(state_file, |state| state.menu_entry_asked = true) {
        eprintln!("{error}");
        return;
    }
    // A launcher that is already there, from an earlier version or made by hand, needs no
    // question.
    if launcher.status(&exe) != Status::Absent {
        return;
    }
    let question = launcher.question(&exe);
    let launcher = launcher.clone();
    let confirm = Confirm::new(
        question.message,
        question.action,
        Weight::Popup,
        move |view, cx| view.add_launcher(launcher.clone(), cx),
    );
    front.update(cx, |front, cx| front.ask(confirm, cx));
}

impl GraphView {
    /// Adds the launcher off the UI thread, and says in the band what happened.
    fn add_launcher(&mut self, launcher: Launcher, cx: &mut Context<Self>) {
        let work = launcher.clone();
        Self::spawn_blocking(
            move || work.add(&exe_path()),
            move |this, result, cx| {
                this.notice = Some(match result {
                    Ok(()) => Notice::Info(launcher.added_message().into()),
                    Err(error) => Notice::Error(error.into()),
                });
                cx.notify();
            },
            cx,
        );
    }
}

#[cfg(test)]
mod tests;
