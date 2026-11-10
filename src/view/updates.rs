//! Updating a portable copy of gitclient from its releases. One model for the whole
//! app: the band over the window offers the step, and Settings → App shows where it stands.
//!
//! Everything about releases, downloads and wording is in `gitclient_core::install::update`.
//! This only runs it off the UI thread and draws the result. Nothing is downloaded until
//! the user asks, and a copy a package manager installed never looks.

use super::parts::{card, card_actions, pill};
use crate::theme::tokens;
use gitclient_core::install::update::{
    self, Build, HOST, Http, Status, Support, Target, Track, Web,
};
use gitclient_core::install::{Channel, exe_path};
use gpui_kit::component::button::*;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::path::PathBuf;
use std::sync::Arc;

pub struct Updates {
    exe: PathBuf,
    /// The build to update to, or `None` where updating isn't possible.
    target: Option<Target>,
    /// Why this copy doesn't update itself, in words.
    reason: Option<String>,
    http: Arc<dyn Http + Send + Sync>,
    /// Whether to look at full releases or the nightly build.
    track: Track,
    /// The program that is running, which a release has to be newer than.
    build: Build,
    status: Status,
    /// The user chose "Later" for the release on offer.
    later: bool,
    /// Why the new version couldn't be started, if it couldn't.
    problem: Option<String>,
}

struct AppUpdates(Entity<Updates>);

impl Global for AppUpdates {}

/// Makes the updater every window shares, for a copy installed as `channel`.
pub fn install_updates(
    channel: Channel,
    track: Track,
    build: Build,
    cx: &mut App,
) -> Entity<Updates> {
    let updates = cx.new(|_| {
        Updates::new(channel, exe_path(), Target::this_system(), Arc::new(Web))
            .following(track, build)
    });
    cx.set_global(AppUpdates(updates.clone()));
    updates
}

/// The updater of this app, if one was installed. Tests have none.
pub(super) fn shared_updates(cx: &App) -> Option<Entity<Updates>> {
    cx.try_global::<AppUpdates>().map(|shared| shared.0.clone())
}

impl Updates {
    pub fn new(
        channel: Channel,
        exe: PathBuf,
        target: Option<Target>,
        http: Arc<dyn Http + Send + Sync>,
    ) -> Self {
        let (target, reason) = match update::support(channel, target) {
            Support::Yes(target) => (Some(target), None),
            Support::No(reason) => (None, Some(reason)),
        };
        Self {
            exe,
            target,
            reason,
            http,
            track: Track::default(),
            build: Build::current(None),
            status: Status::Idle,
            later: false,
            problem: None,
        }
    }

    /// The same updater following `track`, as the build `build`.
    pub fn following(mut self, track: Track, build: Build) -> Self {
        self.track = track;
        self.build = build;
        self
    }

    pub fn track(&self) -> Track {
        self.track
    }

    pub fn build(&self) -> &Build {
        &self.build
    }

    /// Switches between full releases and the nightly build, and looks again, since what
    /// was found belongs to the other track. A download in progress or a finished one is
    /// left alone, and the choice applies to the next look.
    pub fn set_track(&mut self, track: Track, cx: &mut Context<Self>) {
        if track == self.track {
            return;
        }
        self.track = track;
        if matches!(self.status, Status::Installing(_) | Status::Ready { .. }) {
            cx.notify();
            return;
        }
        self.status = Status::Idle;
        self.later = false;
        self.problem = None;
        self.check(cx);
        cx.notify();
    }

    /// Why this copy doesn't update itself, or `None` if it does.
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    pub fn status(&self) -> &Status {
        &self.status
    }

    /// Looks for a newer release. Nothing is downloaded.
    pub fn check(&mut self, cx: &mut Context<Self>) {
        let Some(target) = self.target else {
            return;
        };
        if matches!(self.status, Status::Checking | Status::Installing(_)) {
            return;
        }
        self.status = Status::Checking;
        self.problem = None;
        cx.notify();
        let http = self.http.clone();
        let (track, build) = (self.track, self.build.clone());
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(
                    async move { update::check(&*http, &HOST, track, &build, target) },
                )
                .await;
            // The track changed while this was asking, so the answer is for the wrong one.
            if this
                .read_with(cx, |this, _| this.track != track)
                .unwrap_or(true)
            {
                return;
            }
            this.update(cx, |this, cx| {
                this.status = match result {
                    Ok(Some(found)) => {
                        this.later = false;
                        Status::Available(found)
                    }
                    Ok(None) => Status::UpToDate,
                    Err(error) => Status::CheckFailed(error),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Downloads the release on offer and swaps it in for this program.
    pub fn install(&mut self, cx: &mut Context<Self>) {
        let (Status::Available(found) | Status::InstallFailed(found, _)) = self.status.clone()
        else {
            return;
        };
        self.status = Status::Installing(found.clone());
        self.later = false;
        cx.notify();
        let (http, exe) = (self.http.clone(), self.exe.clone());
        let work = found.clone();
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { update::install(&*http, &work, &exe).map(|_| work) })
                .await;
            this.update(cx, |this, cx| {
                this.status = match result {
                    Ok(found) => Status::installed(&found),
                    Err(error) => Status::InstallFailed(found, error),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Starts the new version and quits this one.
    pub fn restart(&mut self, cx: &mut Context<Self>) {
        if !matches!(self.status, Status::Ready { .. }) {
            return;
        }
        match update::restart(&self.exe) {
            Ok(()) => cx.quit(),
            Err(error) => {
                self.problem = Some(error);
                cx.notify();
            }
        }
    }

    /// Hides the offer until a newer release turns up.
    pub fn put_off(&mut self, cx: &mut Context<Self>) {
        self.later = true;
        cx.notify();
    }

    /// What the button for the status does: looks, installs or restarts.
    pub fn act(&mut self, cx: &mut Context<Self>) {
        match self.status {
            Status::Available(_) | Status::InstallFailed(..) => self.install(cx),
            Status::Ready { .. } => self.restart(cx),
            _ => self.check(cx),
        }
    }

    /// The card over the window, if there is something to offer or say.
    fn band(&self) -> Option<update::Band> {
        if let Some(problem) = &self.problem {
            return Some(update::Band {
                message: problem.clone(),
                action: Some("Try again".to_owned()),
                warning: true,
            });
        }
        if self.later && matches!(self.status, Status::Available(_)) {
            return None;
        }
        self.status.band()
    }
}

/// The card over the window for `updates`, when it has something to say.
pub(super) fn render_band(updates: &Entity<Updates>, cx: &App) -> Option<AnyElement> {
    let (shown, offered) = {
        let this = updates.read(cx);
        (this.band()?, matches!(this.status, Status::Available(_)))
    };
    let theme = cx.theme();
    let color = if shown.warning {
        tokens(theme.is_dark()).peach
    } else {
        theme.secondary_foreground
    };
    let act = |updates: &Entity<Updates>| {
        let updates = updates.clone();
        move |_: &ClickEvent, _: &mut Window, cx: &mut App| {
            updates.update(cx, |updates, cx| updates.act(cx))
        }
    };
    Some(
        card("update", cx)
            .absolute()
            .bottom_4()
            .right_4()
            .child(
                div()
                    .h_flex()
                    .items_start()
                    .gap_2()
                    .child(pill("Update", theme.primary, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_color(color)
                            .child(shown.message),
                    ),
            )
            .child(
                card_actions()
                    .children(offered.then(|| {
                        let updates = updates.clone();
                        Button::new("update-later")
                            .ghost()
                            .small()
                            .label("Later")
                            .on_click(move |_, _, cx| {
                                updates.update(cx, |updates, cx| updates.put_off(cx))
                            })
                    }))
                    .children(shown.action.map(|action| {
                        Button::new("update-act")
                            .primary()
                            .small()
                            .label(action)
                            .on_click(act(updates))
                    })),
            )
            .into_any_element(),
    )
}

#[cfg(test)]
mod tests;
