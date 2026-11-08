//! The welcome panel, shown once the first time gitclient runs. It says in a few pictures
//! how the window thinks (your branch is the straight line, buttons name their result,
//! branches are named and never "ours" or "theirs"), lists the keys worth knowing, and
//! holds the one choice a portable copy has to make: whether to add itself to the
//! application menu.
//!
//! It is drawn by the popup module, since it takes the keys and the backdrop the same way:
//! Enter goes on, Space ticks the box, Escape or a click outside skips. Skipping writes
//! nothing, and it is never shown again either way. Settings → App adds or removes the
//! launcher later.

use super::{Dialog, backdrop};
use crate::theme::{lane_color, selected_row};
use crate::view::parts::{TEXT_BODY, TEXT_CHIP, TEXT_SMALL, TEXT_STATUS, TEXT_TITLE, checkbox};
use crate::view::{GraphView, Notice};
use gitclient_core::history::lanes::LaneColor;
use gitclient_core::install::exe_path;
use gitclient_core::install::menu_entry::{Launcher, Offer, Status};
use gitclient_core::settings::state::AppState;
use gpui_kit::component::button::*;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::Path;

const WIDTH: Pixels = px(640.);
const ACTION: &str = "Get started";

/// The welcome on show, and the launcher it offers if there is one to offer.
pub(in crate::view) struct WelcomeDialog {
    launcher: Option<LauncherChoice>,
}

/// The launcher as the box in the panel stands for it.
struct LauncherChoice {
    launcher: Launcher,
    offer: Offer,
    /// Whether the box is ticked. It starts ticked: the button then names what it does.
    add: bool,
}

/// At launch: mends a launcher that points at a copy that has moved, and if the welcome has
/// never been shown, shows it in `front`. `launcher` is `None` for a copy a package manager
/// installed, which then gets the welcome without the choice. `state_file` is where "shown"
/// is remembered; without one nothing is shown, since it would come back every time.
pub(in crate::view) fn offer(
    front: &Entity<GraphView>,
    launcher: Option<&Launcher>,
    state_file: &Path,
    cx: &mut App,
) {
    let exe = exe_path();
    if let Some(launcher) = launcher
        && let Err(error) = launcher.refresh(&exe)
    {
        eprintln!("{error}");
    }
    if AppState::load(state_file).welcome_seen {
        return;
    }
    // Shown once, even if the window is closed with the panel still open.
    if let Err(error) = AppState::change(state_file, |state| state.welcome_seen = true) {
        eprintln!("{error}");
        return;
    }
    // A launcher that is already there, from an earlier version or made by hand, needs no
    // offer.
    let launcher = launcher
        .filter(|launcher| launcher.status(&exe) == Status::Absent)
        .cloned();
    front.update(cx, |front, cx| front.show_welcome(launcher, cx));
}

impl GraphView {
    /// Shows the welcome panel, offering `launcher` if there is one to add. The launch step
    /// decides whether it is time; this only draws it, which is also what tests of the
    /// pixels do.
    pub fn show_welcome(&mut self, launcher: Option<Launcher>, cx: &mut Context<Self>) {
        let exe = exe_path();
        let launcher = launcher.map(|launcher| LauncherChoice {
            offer: launcher.offer(&exe),
            launcher,
            add: true,
        });
        self.dialog = Some(Dialog::Welcome(WelcomeDialog { launcher }));
        cx.notify();
    }

    /// Enter, or the button: closes the panel, adding the launcher if its box is ticked.
    pub(super) fn submit_welcome(&mut self, cx: &mut Context<Self>) {
        let Some(Dialog::Welcome(welcome)) = self.dialog.take() else {
            return;
        };
        cx.notify();
        if let Some(choice) = welcome.launcher.filter(|choice| choice.add) {
            self.add_launcher(choice.launcher, cx);
        }
    }

    /// Space, or a click on the box. Returns whether there was a box to tick.
    pub(in crate::view) fn toggle_welcome_launcher(&mut self, cx: &mut Context<Self>) -> bool {
        let Some(Dialog::Welcome(WelcomeDialog {
            launcher: Some(choice),
        })) = &mut self.dialog
        else {
            return false;
        };
        choice.add = !choice.add;
        cx.notify();
        true
    }

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

    /// The panel over the window, or `None` if it isn't open.
    pub(super) fn render_welcome(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let Some(Dialog::Welcome(welcome)) = &self.dialog else {
            return None;
        };
        let theme = cx.theme();
        let (panel, border, muted) = (theme.title_bar, theme.border, theme.muted_foreground);
        let choice = welcome.launcher.as_ref();
        let adding = choice.is_some_and(|choice| choice.add);
        let rule = || div().h(px(1.)).w_full().flex_none().bg(border);

        let hero = div()
            .h_flex()
            .gap_4()
            .px_6()
            .pt_6()
            .pb_5()
            .child(mark(cx))
            .child(
                div()
                    .v_flex()
                    .gap_0p5()
                    .child(
                        div()
                            .text_size(TEXT_TITLE)
                            .line_height(px(22.))
                            .font_semibold()
                            .child("Welcome to gitclient"),
                    )
                    .child(
                        div()
                            .text_size(TEXT_STATUS)
                            .line_height(px(20.))
                            .text_color(muted)
                            .child("See where you are in a repository, and what to do next."),
                    ),
            );

        let ideas = div()
            .h_flex()
            .items_start()
            .gap_3()
            .px_6()
            .py_5()
            .child(idea(
                graph_picture(cx),
                "Your branch is straight",
                "HEAD's history runs straight down the first lane. Other branches bend around \
                 it, and keep one colour in the sidebar, the graph and the chips.",
                cx,
            ))
            .child(idea(
                button_picture(cx),
                "Say what will happen",
                "“Commit 2 files”, not “Commit”. Before a risky step, gitclient tells you how \
                 to undo it.",
                cx,
            ))
            .child(idea(
                names_picture(cx),
                "Branches, not “ours”",
                "Ours and theirs swap meaning during a rebase, so each side of a conflict is \
                 named for its branch.",
                cx,
            ));

        let keys = div()
            .v_flex()
            .gap_2()
            .px_6()
            .pb_5()
            .child(
                div()
                    .text_size(TEXT_SMALL)
                    .line_height(px(16.))
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(muted)
                    .child("Keys worth knowing"),
            )
            .child(div().h_flex().flex_wrap().gap_x_5().gap_y_2().children(
                keys_worth_knowing().map(|(keys, what)| {
                    div()
                        .h_flex()
                        .gap_2()
                        .child(key_chip(&keys, cx))
                        .child(div().text_size(TEXT_SMALL).text_color(muted).child(what))
                }),
            ));

        let launcher = choice.map(|choice| {
            let on = choice.add;
            div().v_flex().child(rule()).child(
                div()
                    .id("welcome-launcher")
                    .h_flex()
                    .items_start()
                    .gap_3()
                    .px_6()
                    .py_4()
                    .cursor_pointer()
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.toggle_welcome_launcher(cx);
                    }))
                    .child(
                        div()
                            .pt_0p5()
                            .child(checkbox("welcome-launcher-box", on, false, cx)),
                    )
                    .child(
                        div()
                            .v_flex()
                            .flex_1()
                            .min_w_0()
                            .gap_0p5()
                            .child(
                                div()
                                    .text_size(TEXT_BODY)
                                    .line_height(px(20.))
                                    .font_semibold()
                                    .child(choice.offer.title.clone()),
                            )
                            .child(
                                div()
                                    .text_size(TEXT_SMALL)
                                    .line_height(px(17.))
                                    .text_color(muted)
                                    .child(choice.offer.detail.clone()),
                            ),
                    ),
            )
        });

        let hint = match (choice.is_some(), adding) {
            (true, true) => {
                "Press Enter to add it and start, Space to change the box, or Esc to skip."
            }
            (true, false) => {
                "Press Enter to start without it, Space to change the box, or Esc to skip."
            }
            (false, _) => "Press Enter to get started, or Esc to skip.",
        };
        let label = match choice {
            Some(choice) if adding => choice.offer.button,
            _ => ACTION,
        };
        let footer = div()
            .h_flex()
            .gap_3()
            .px_6()
            .py_4()
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_size(TEXT_SMALL)
                    .line_height(px(17.))
                    .text_color(muted)
                    .child(hint),
            )
            .child(
                Button::new("welcome-skip")
                    .ghost()
                    .label("Skip")
                    .on_click(cx.listener(|this, _, _, cx| this.press_escape(cx))),
            )
            .child(
                Button::new("welcome-start")
                    .primary()
                    .label(label)
                    .on_click(cx.listener(|this, _, _, cx| this.submit_welcome(cx))),
            );

        let card = div()
            .id("welcome")
            .v_flex()
            .w(WIDTH)
            .max_w_full()
            .max_h_full()
            .overflow_y_scroll()
            .rounded(px(8.))
            .bg(panel)
            .border_1()
            .border_color(border)
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .text_size(TEXT_BODY)
            .child(hero)
            .child(rule())
            .child(ideas)
            .child(keys)
            .children(launcher)
            .child(rule())
            .child(footer);
        Some(backdrop(card, cx))
    }
}

/// The keys the panel lists, with what each does, for this system's modifier.
fn keys_worth_knowing() -> impl Iterator<Item = (String, &'static str)> {
    let modifier = if cfg!(target_os = "macos") {
        "Cmd"
    } else {
        "Ctrl"
    };
    [
        ("O", "Open a repository"),
        ("T", "New tab"),
        ("B", "Sidebar"),
        (",", "Settings"),
    ]
    .into_iter()
    .map(move |(key, what)| (format!("{modifier} {key}"), what))
}

/// A key as a small keycap: Surface 0 with its label in semibold.
fn key_chip(keys: &str, cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .h_flex()
        .h(px(20.))
        .px_1p5()
        .rounded(px(4.))
        .bg(selected_row(theme))
        .text_size(TEXT_CHIP)
        .font_semibold()
        .text_color(theme.foreground)
        .child(keys.to_owned())
}

/// One of the three ideas: a picture on Base, a heading, and a few words.
fn idea(picture: Div, heading: &str, words: &str, cx: &App) -> Div {
    div()
        .v_flex()
        .flex_1()
        .min_w_0()
        .gap_3()
        .child(picture)
        .child(
            div()
                .v_flex()
                .gap_1()
                .child(
                    div()
                        .text_size(TEXT_BODY)
                        .line_height(px(18.))
                        .font_semibold()
                        .child(heading.to_owned()),
                )
                .child(
                    div()
                        .text_size(TEXT_SMALL)
                        .line_height(px(17.))
                        .text_color(cx.theme().muted_foreground)
                        .child(words.to_owned()),
                ),
        )
}

/// The frame every picture is drawn in: Base, with the panel's border.
fn stage(cx: &App) -> Div {
    let theme = cx.theme();
    div()
        .relative()
        .h(px(96.))
        .w_full()
        .flex_none()
        .overflow_hidden()
        .rounded(px(6.))
        .bg(theme.background)
        .border_1()
        .border_color(theme.border)
}

/// A dot centred on (`x`, `y`).
fn dot(x: f32, y: f32, diameter: f32, color: Hsla) -> Div {
    div()
        .absolute()
        .left(px(x - diameter / 2.))
        .top(px(y - diameter / 2.))
        .size(px(diameter))
        .rounded_full()
        .bg(color)
}

/// A vertical line centred on `x`, from `top` to `bottom`.
fn line(x: f32, top: f32, bottom: f32, thickness: f32, color: Hsla) -> Div {
    div()
        .absolute()
        .left(px(x - thickness / 2.))
        .top(px(top))
        .w(px(thickness))
        .h(px(bottom - top))
        .bg(color)
}

/// A line that leaves `from` to the right and bends up to end at `to`, as a branch bends
/// away from a lane. It is a box with only its bottom and right edges drawn, one corner
/// rounded.
fn elbow(from: (f32, f32), to: (f32, f32), thickness: f32, radius: f32, color: Hsla) -> Div {
    let mut elbow = div()
        .absolute()
        .left(px(from.0))
        .top(px(to.1))
        .w(px(to.0 - from.0 + thickness / 2.))
        .h(px(from.1 - to.1 + thickness / 2.))
        .rounded_br(px(radius))
        .border_color(color);
    elbow.style().border_widths.bottom = Some(px(thickness).into());
    elbow.style().border_widths.right = Some(px(thickness).into());
    elbow
}

/// The app's mark: a blue lane with HEAD on it, and a branch bending away. It is the icon
/// the launcher installs, drawn at 56 px.
fn mark(cx: &App) -> Div {
    let theme = cx.theme();
    let dark = theme.is_dark();
    let (head, mauve) = (
        lane_color(LaneColor::Head, dark),
        lane_color(LaneColor::Branch(0), dark),
    );
    let ring = dot(21., 14., 17.5, theme.background).border(px(1.5));
    div()
        .relative()
        .flex_none()
        .size(px(56.))
        .rounded(px(14.))
        .bg(theme.background)
        .border_1()
        .border_color(theme.border)
        .child(line(21., 16., 45., 3., head))
        .child(elbow((21., 37.), (36., 17.), 3., 8., mauve))
        .child(ring.border_color(head))
        .child(dot(21., 14., 11.4, head))
        .child(dot(36., 16.6, 8., mauve))
}

/// A ref chip: tinted with 14% of its colour, or solid for the current branch.
fn ref_chip(label: &str, color: Hsla, solid: bool, cx: &App) -> Div {
    let chip = div()
        .h_flex()
        .h(px(18.))
        .px_1p5()
        .rounded(px(4.))
        .text_size(TEXT_CHIP)
        .font_semibold();
    if solid {
        chip.bg(color).text_color(cx.theme().primary_foreground)
    } else {
        chip.bg(color.opacity(0.14)).text_color(color)
    }
    .child(label.to_owned())
}

/// A small graph: HEAD on its blue lane, `main` below the fork in its own colour, and a
/// second branch bending away.
fn graph_picture(cx: &App) -> Div {
    let dark = cx.theme().is_dark();
    let head = lane_color(LaneColor::Head, dark);
    let main = lane_color(LaneColor::Branch(0), dark);
    let other = lane_color(LaneColor::Branch(1), dark);
    let ring = dot(34., 20., 20., cx.theme().background).border(px(1.5));
    stage(cx)
        .child(line(34., 20., 62., 2., head))
        .child(line(34., 62., 96., 2., main))
        .child(elbow((34., 62.), (100., 40.), 2., 12., other))
        .child(ring.border_color(head))
        .child(dot(34., 20., 10., head))
        .child(dot(34., 62., 9., main))
        .child(dot(34., 83., 9., main))
        .child(dot(100., 40., 9., other))
        .child(
            div()
                .absolute()
                .left(px(54.))
                .top(px(11.))
                .child(ref_chip("feat/ui", head, true, cx)),
        )
        .child(
            div()
                .absolute()
                .left(px(50.))
                .top(px(74.))
                .child(ref_chip("main", main, false, cx)),
        )
}

/// A button named for its result, with the line that says how to undo it.
fn button_picture(cx: &App) -> Div {
    let theme = cx.theme();
    stage(cx)
        .v_flex()
        .items_center()
        .justify_center()
        .gap_2()
        .child(
            div()
                .h_flex()
                .h(px(28.))
                .px_3()
                .rounded(px(6.))
                .bg(theme.primary)
                .text_color(theme.primary_foreground)
                .text_size(TEXT_BODY)
                .font_semibold()
                .child("Commit 2 files"),
        )
        .child(
            div()
                .v_flex()
                .items_center()
                .text_size(TEXT_CHIP)
                .child(
                    div()
                        .text_color(theme.muted_foreground)
                        .child("To undo, run"),
                )
                .child(div().child("git reset --soft HEAD~1")),
        )
}

/// A conflict's header, with each side named for its branch.
fn names_picture(cx: &App) -> Div {
    let theme = cx.theme();
    let dark = theme.is_dark();
    let head = lane_color(LaneColor::Head, dark);
    let main = lane_color(LaneColor::Branch(0), dark);
    stage(cx)
        .v_flex()
        .items_center()
        .justify_center()
        .gap_2()
        .child(
            div()
                .text_size(TEXT_CHIP)
                .text_color(theme.muted_foreground)
                .child("Conflict 1 of 2 · line 40"),
        )
        .child(
            div()
                .h_flex()
                .gap_1p5()
                .child(ref_chip("Use feat/ui", head, false, cx))
                .child(ref_chip("Use main", main, false, cx)),
        )
}

#[cfg(test)]
mod tests;
