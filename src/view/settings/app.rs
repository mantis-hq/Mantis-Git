//! The App page: what gitclient itself remembers, apart from git. Reopening tabs on
//! launch, how many commits the graph loads, and the cache on disk.

use super::super::avatars::{Face, disc};
use super::*;
use crate::theme::lane_color;
use gitclient_core::history::avatar::{self, Style as AvatarStyle};
use gitclient_core::history::lanes::LaneColor;
use gitclient_core::install::exe_path;
use gitclient_core::install::update::{self, Track};
use gitclient_core::repo::auto_fetch;

impl Settings {
    /// Reads how the launcher stands, which is what the page shows. Settings reads the disk
    /// rather than a remembered choice, so a launcher added or removed by hand is seen.
    pub(super) fn load_launcher(&mut self) {
        self.launcher_status = self
            .launcher
            .as_ref()
            .map(|launcher| launcher.status(&exe_path()));
    }

    /// Adds the launcher to the menu, or takes it away, and says which in the header. The
    /// welcome counts as seen too, so its choice isn't offered after the user has chosen.
    pub(super) fn set_launcher(&mut self, on: bool, cx: &mut Context<Self>) {
        let Some(launcher) = &self.launcher else {
            return;
        };
        let exe = exe_path();
        let result = if on {
            launcher.add(&exe).map(|()| launcher.added_message())
        } else {
            launcher.remove().map(|()| launcher.removed_message())
        };
        self.feedback = match result {
            Ok(message) => {
                if let Some(file) = &self.state_file
                    && let Ok(state) = AppState::change(file, |s| s.welcome_seen = true)
                {
                    self.app = state;
                }
                Feedback::Saved(message)
            }
            Err(error) => Feedback::Failed(error),
        };
        self.load_launcher();
        cx.notify();
    }

    /// Turns "look for a newer version when gitclient starts" on or off, and saves it at once.
    pub(super) fn set_check_updates(&mut self, on: bool, cx: &mut Context<Self>) {
        let Some(file) = &self.state_file else {
            return;
        };
        match AppState::change(file, |state| state.check_updates = on) {
            Ok(state) => {
                self.feedback = Feedback::Saved(update::check_saved_message(on).to_owned());
                self.app = state;
            }
            Err(error) => self.feedback = Feedback::Failed(error),
        }
        cx.notify();
    }

    /// Turns "fetch in the background" on or off, and saves it at once.
    pub(super) fn set_auto_fetch(&mut self, on: bool, cx: &mut Context<Self>) {
        let Some(file) = &self.state_file else {
            return;
        };
        match AppState::change(file, |state| state.auto_fetch = on) {
            Ok(state) => {
                self.feedback = Feedback::Saved(auto_fetch::saved_message(on).to_owned());
                self.app = state;
            }
            Err(error) => self.feedback = Feedback::Failed(error),
        }
        cx.notify();
    }

    /// The "Remotes" card: whether the app fetches by itself.
    fn render_remotes_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let enabled = self.state_file.is_some();
        let on = self.app.auto_fetch;
        let switch = div()
            .px_4()
            .py_3()
            .gap_3()
            .h_flex()
            .child(self.row_words(
                "Fetch in the background",
                if enabled {
                    auto_fetch::sentence(on)
                } else {
                    "gitclient can't find a folder to keep this in."
                },
                cx,
            ))
            .children(enabled.then(|| {
                checkbox("auto-fetch", on, false, cx)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_auto_fetch(!on, cx)))
            }))
            .into_any_element();
        self.render_group("Remotes", vec![switch], cx)
    }

    /// Chooses full releases or the nightly build, saves it at once and tells the updater,
    /// which looks again on the new track.
    pub(super) fn set_update_track(&mut self, track: Track, cx: &mut Context<Self>) {
        let Some(file) = &self.state_file else {
            return;
        };
        match AppState::change(file, |state| state.update_track = track) {
            Ok(state) => {
                self.feedback = Feedback::Saved(track.saved_message().to_owned());
                self.app = state;
                if let Some(updates) = &self.updates {
                    updates.update(cx, |updates, cx| updates.set_track(track, cx));
                }
            }
            Err(error) => self.feedback = Feedback::Failed(error),
        }
        cx.notify();
    }

    /// A title over a muted sentence, the left side of a row in a card.
    fn row_words(&self, title: &str, note: &str, cx: &App) -> Div {
        div()
            .v_flex()
            .flex_1()
            .min_w_0()
            .gap_0p5()
            .child(
                div()
                    .text_size(TEXT_BODY)
                    .line_height(px(20.))
                    .child(title.to_owned()),
            )
            .child(
                div()
                    .text_size(TEXT_SMALL)
                    .line_height(px(17.))
                    .text_color(cx.theme().muted_foreground)
                    .child(note.to_owned()),
            )
    }

    /// The "Launcher" card: whether gitclient is in the application menu. Only a portable
    /// copy on a system with such a menu has one.
    fn render_launcher_group(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let launcher = self.launcher.as_ref()?;
        let status = self.launcher_status?;
        let on = matches!(status, LauncherStatus::Present { .. });
        // A launcher that isn't ours stays as it is, so there is nothing to tick.
        let box_ = (status != LauncherStatus::Foreign).then(|| {
            checkbox("launcher", on, false, cx)
                .on_click(cx.listener(move |this, _, _, cx| this.set_launcher(!on, cx)))
        });
        let row = div()
            .px_4()
            .py_3()
            .gap_3()
            .h_flex()
            .child(self.row_words(&launcher.title(), &launcher.sentence(&exe_path()), cx))
            .children(box_)
            .into_any_element();
        Some(self.render_group("Launcher", vec![row], cx))
    }

    /// The "Updates" card. A copy a package manager installed says so and has nothing to
    /// set. A portable one says where an update stands, and acts on it.
    fn render_updates_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let updates = self.updates.clone();
        let reason = match &updates {
            Some(updates) => updates.read(cx).reason().map(str::to_owned),
            None => Some(match self.channel {
                Channel::Packaged { by } => {
                    format!("gitclient was installed with {by}, which updates it.")
                }
                Channel::Portable => "Updates aren't set up in this window.".to_owned(),
            }),
        };
        let (Some(updates), None) = (updates, reason.clone()) else {
            let row = div()
                .px_4()
                .py_3()
                .child(self.row_words("Updates", reason.as_deref().unwrap_or_default(), cx))
                .into_any_element();
            return self.render_group("Updates", vec![row], cx);
        };

        let enabled = self.state_file.is_some();
        let on = self.app.check_updates;
        let switch = div()
            .px_4()
            .py_3()
            .gap_3()
            .h_flex()
            .child(self.row_words(
                "Look for a newer version when gitclient starts",
                if enabled {
                    update::check_sentence(on)
                } else {
                    "gitclient can't find a folder to keep this in."
                },
                cx,
            ))
            .children(enabled.then(|| {
                checkbox("check-updates", on, false, cx)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_check_updates(!on, cx)))
            }))
            .into_any_element();

        let track = self.app.update_track;
        let chips = Track::ALL.into_iter().map(|choice| {
            chip(
                ("update-track", choice as usize),
                choice.label(),
                choice == track,
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| this.set_update_track(choice, cx)))
        });
        let choice = div()
            .v_flex()
            .px_4()
            .py_3()
            .gap_2()
            .child(self.row_words(
                "Which versions to update to",
                if enabled {
                    track.sentence()
                } else {
                    "gitclient can't find a folder to keep this in."
                },
                cx,
            ))
            .children(enabled.then(|| div().flex().flex_wrap().gap_1().children(chips)))
            .into_any_element();

        let (status, build) = {
            let updates = updates.read(cx);
            (updates.status().clone(), updates.build().clone())
        };
        let button = status.button().map(|label| {
            let updates = updates.clone();
            Button::new("update-act")
                .ghost()
                .small()
                .label(label)
                .on_click(move |_, _, cx| updates.update(cx, |updates, cx| updates.act(cx)))
        });
        let state = div()
            .px_4()
            .py_3()
            .gap_3()
            .h_flex()
            .child(self.row_words("Check for updates", &status.sentence(&build, track), cx))
            .children(button)
            .into_any_element();
        self.render_group("Updates", vec![switch, choice, state], cx)
    }

    /// Turns "reopen repositories on launch" on or off and saves it at once.
    pub(super) fn set_reopen_tabs(&mut self, on: bool, cx: &mut Context<Self>) {
        let Some(file) = &self.state_file else {
            return;
        };
        // Read again, so that the tabs the main window saved meanwhile are kept.
        let mut state = AppState::load(file);
        state.reopen_tabs = on;
        match state.save(file) {
            Ok(()) => {
                self.feedback = Feedback::Saved(state.reopen_saved_message().to_owned());
                self.app = state;
            }
            Err(error) => self.feedback = Feedback::Failed(error),
        }
        cx.notify();
    }

    /// Counts what the caches hold on disk. The count waits behind any write or trim
    /// already queued, so it is read off the UI thread.
    pub(super) fn load_cache_size(&mut self, cx: &mut Context<Self>) {
        let caches = self.caches.clone();
        cx.spawn(async move |this, cx| {
            let usage = cx.background_spawn(async move { caches.usage() }).await;
            this.update(cx, |this, cx| {
                this.cache_usage = Some(usage);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// Saves a change to the cache settings at once and applies it to the caches in use.
    fn change_cache_setting(
        &mut self,
        change: impl FnOnce(&mut AppState),
        said: impl FnOnce(&AppState) -> String,
        cx: &mut Context<Self>,
    ) {
        let Some(file) = &self.state_file else {
            return;
        };
        // Read again, so that the tabs the main window saved meanwhile are kept.
        let mut state = AppState::load(file);
        change(&mut state);
        match state.save(file) {
            Ok(()) => {
                self.caches.set_disk(state.disk_settings());
                self.feedback = Feedback::Saved(said(&state));
                self.app = state;
                self.load_cache_size(cx);
            }
            Err(error) => self.feedback = Feedback::Failed(error),
        }
        cx.notify();
    }

    pub(super) fn set_disk_cache(&mut self, on: bool, cx: &mut Context<Self>) {
        self.change_cache_setting(
            |state| state.disk_cache = on,
            AppState::disk_cache_saved_message,
            cx,
        );
    }

    pub(super) fn set_cache_mb(&mut self, megabytes: u32, cx: &mut Context<Self>) {
        self.change_cache_setting(
            |state| state.cache_mb = megabytes,
            AppState::cache_size_saved_message,
            cx,
        );
    }

    fn set_commit_limit(&mut self, limit: u32, cx: &mut Context<Self>) {
        self.change_cache_setting(
            |state| state.commit_limit = limit,
            AppState::commit_limit_saved_message,
            cx,
        );
    }

    pub(super) fn set_avatar_style(&mut self, style: AvatarStyle, cx: &mut Context<Self>) {
        self.change_cache_setting(
            |state| state.avatars = style,
            move |_| style.saved_message().to_owned(),
            cx,
        );
    }

    pub(super) fn set_own_avatar(&mut self, file: Option<PathBuf>, cx: &mut Context<Self>) {
        let said = if file.is_some() {
            "Saved. Your commits show the picture you chose."
        } else {
            "Saved. Your commits no longer show a picture of your own."
        };
        self.change_cache_setting(
            |state| state.own_avatar = file,
            move |_| said.to_owned(),
            cx,
        );
    }

    /// Asks for a picture from the computer, and keeps it if the app can draw it.
    fn choose_own_avatar(&mut self, cx: &mut Context<Self>) {
        let picked = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: false,
            prompt: Some("Use as my avatar".into()),
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = picked.await else {
                return;
            };
            let Some(file) = paths.into_iter().next() else {
                return;
            };
            let checked = cx
                .background_spawn({
                    let file = file.clone();
                    async move { avatar::read_own(&file).map(|_| ()) }
                })
                .await;
            this.update(cx, |this, cx| match checked {
                Ok(()) => this.set_own_avatar(Some(file), cx),
                Err(error) => {
                    this.feedback = Feedback::Failed(error);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Who the open repository commits as, for the preview of the user's own avatar.
    fn committer(&self) -> (String, String) {
        let (name, email) = match &self.repo_state {
            Some(Ok(state)) => (
                state.committing.name.clone(),
                state.committing.email.clone(),
            ),
            _ => (
                self.sources.get("user.name").map(|s| s.value.clone()),
                self.sources.get("user.email").map(|s| s.value.clone()),
            ),
        };
        (name.unwrap_or_default(), email.unwrap_or_default())
    }

    /// The "Avatars" card. Two rows that read as two groups of people: "You", a person with
    /// a picture of their own to change, and "Everyone else", shown by samples of what the
    /// chips would make of other authors.
    fn render_avatars_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, border, text) = (theme.muted_foreground, theme.border, theme.foreground);
        let (on_tint, is_dark) = (theme.primary_foreground, theme.is_dark());
        if self.state_file.is_none() {
            let row = div()
                .px_4()
                .py_3()
                .text_size(TEXT_SMALL)
                .text_color(muted)
                .child("gitclient can't find a folder to keep this in.")
                .into_any_element();
            return self.render_group("Avatars", vec![row], cx);
        }
        let style = self.app.avatars;
        let ring = border;
        let (name, email) = self.committer();
        let title = |label: &str| {
            div()
                .text_size(TEXT_BODY)
                .line_height(px(20.))
                .child(label.to_owned())
        };
        let small = |label: String| {
            div()
                .text_size(TEXT_SMALL)
                .line_height(px(17.))
                .text_color(muted)
                .truncate()
                .child(label)
        };

        // Everyone else: three samples in the picked style, then the choice.
        let samples = div().h_flex().flex_none().children(
            ["Ada Lovelace", "Alan Turing", "Grace Hopper"]
                .into_iter()
                .enumerate()
                .map(|(index, name)| {
                    let face = if style == AvatarStyle::Off {
                        Face::None
                    } else {
                        Face::Initials {
                            text: avatar::initials(name),
                            tint: avatar::tint("", name),
                        }
                    };
                    let sample = if matches!(face, Face::None) {
                        // A plain dot, as the graph draws one.
                        div()
                            .size(px(10.))
                            .m(px(5.))
                            .rounded_full()
                            .bg(muted)
                            .into_any_element()
                    } else {
                        disc(face, 28., 2., ring, is_dark, on_tint).into_any_element()
                    };
                    div()
                        .when(index > 0, |sample| sample.ml(px(-6.)))
                        .child(sample)
                }),
        );
        let chips = AvatarStyle::ALL.into_iter().map(|choice| {
            chip(
                ("avatar-style", choice as usize),
                choice.label(),
                choice == style,
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| this.set_avatar_style(choice, cx)))
        });
        let others = div()
            .px_4()
            .py_3()
            .gap_3()
            .h_flex()
            .child(
                div()
                    .v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .child(title("Everyone else"))
                    .child(div().flex().flex_wrap().gap_1().children(chips))
                    .children((style == AvatarStyle::Gravatar).then(|| {
                        small("Asks gravatar.com with a hash of each author's email.".to_owned())
                    })),
            )
            .child(samples)
            .into_any_element();
        if style == AvatarStyle::Off {
            return self.render_group("Avatars", vec![others], cx);
        }

        // You: your own picture, or what you would get without one.
        let own = self.app.own_avatar.clone();
        let face = match &own {
            Some(file) => Face::File(file.clone()),
            None => Face::Initials {
                text: avatar::initials(&name),
                tint: avatar::tint(&email, &name),
            },
        };
        let buttons = div()
            .h_flex()
            .flex_none()
            .gap_1()
            .child(
                Button::new("choose-own-avatar")
                    .ghost()
                    .small()
                    .label(if own.is_some() {
                        "Change picture"
                    } else {
                        "Choose a picture"
                    })
                    .on_click(cx.listener(|this, _, _, cx| this.choose_own_avatar(cx))),
            )
            .children(own.is_some().then(|| {
                Button::new("clear-own-avatar")
                    .ghost()
                    .small()
                    .label("Remove")
                    .on_click(cx.listener(|this, _, _, cx| this.set_own_avatar(None, cx)))
            }));
        let who = if name.is_empty() {
            "You".to_owned()
        } else {
            name.clone()
        };
        let you = div()
            .px_4()
            .py_3()
            .gap_3()
            .h_flex()
            .child(disc(face, 44., 2., ring, is_dark, on_tint))
            .child(
                div()
                    .v_flex()
                    .flex_1()
                    .min_w_0()
                    .child(
                        div()
                            .text_size(TEXT_BODY)
                            .line_height(px(20.))
                            .text_color(text)
                            .truncate()
                            .child(who),
                    )
                    .child(small(if email.is_empty() {
                        "No email set in git".to_owned()
                    } else {
                        email
                    })),
            )
            .child(buttons)
            .into_any_element();
        self.render_group("Avatars", vec![you, others], cx)
    }

    /// The "Graph" card: how many commits are loaded from a repository.
    fn render_graph_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let mut limits: Vec<u32> = COMMIT_LIMITS.to_vec();
        // A limit set by hand, which no chip stands for, stays visible.
        if !limits.contains(&self.app.commit_limit) {
            limits.push(self.app.commit_limit);
            limits.sort_unstable();
        }
        let chips = limits.into_iter().map(|limit| {
            chip(
                ("commit-limit", limit as usize),
                &count(limit, "commit"),
                limit == self.app.commit_limit,
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| this.set_commit_limit(limit, cx)))
        });
        let note = if self.state_file.is_some() {
            self.app.commit_limit_sentence()
        } else {
            "gitclient can't find a folder to keep this in.".to_owned()
        };
        let row = div()
            .v_flex()
            .px_4()
            .py_3()
            .gap_2()
            .child(
                div()
                    .v_flex()
                    .gap_0p5()
                    .child(
                        div()
                            .text_size(TEXT_BODY)
                            .line_height(px(20.))
                            .child("Commits to load"),
                    )
                    .child(
                        div()
                            .text_size(TEXT_SMALL)
                            .line_height(px(17.))
                            .text_color(muted)
                            .child(note),
                    ),
            )
            .children(
                self.state_file
                    .is_some()
                    .then(|| div().flex().flex_wrap().gap_1().children(chips)),
            )
            .into_any_element();
        self.render_group("Graph", vec![row], cx)
    }

    pub(super) fn clear_cache(&mut self, cx: &mut Context<Self>) {
        self.caches.clear();
        self.feedback = Feedback::Saved(AppState::cache_cleared_message().to_owned());
        self.load_cache_size(cx);
        cx.notify();
    }

    /// The "Cache" card: whether commits are kept on disk, how much, and how to empty it.
    fn render_cache_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let enabled = self.state_file.is_some();
        let on = self.app.disk_cache;
        let words = |title: &str, note: String| {
            div()
                .v_flex()
                .flex_1()
                .min_w_0()
                .gap_0p5()
                .child(
                    div()
                        .text_size(TEXT_BODY)
                        .line_height(px(20.))
                        .child(title.to_owned()),
                )
                .child(
                    div()
                        .text_size(TEXT_SMALL)
                        .line_height(px(17.))
                        .text_color(muted)
                        .child(note),
                )
        };

        let switch = div()
            .px_4()
            .py_3()
            .gap_3()
            .h_flex()
            .child(words(
                "Keep a cache on disk",
                if enabled {
                    self.app.disk_cache_sentence()
                } else {
                    "gitclient can't find a folder to keep this in.".to_owned()
                },
            ))
            .children(enabled.then(|| {
                checkbox("disk-cache", on, false, cx)
                    .on_click(cx.listener(move |this, _, _, cx| this.set_disk_cache(!on, cx)))
            }))
            .into_any_element();

        let mut sizes: Vec<u32> = CACHE_SIZES_MB.to_vec();
        // A size set by hand, which no chip stands for, stays visible.
        if !sizes.contains(&self.app.cache_mb) {
            sizes.push(self.app.cache_mb);
        }
        let chips = sizes.into_iter().map(|megabytes| {
            chip(
                ("cache-size", megabytes as usize),
                &cache_size_label(megabytes),
                megabytes == self.app.cache_mb,
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| this.set_cache_mb(megabytes, cx)))
        });
        let size = div()
            .v_flex()
            .px_4()
            .py_3()
            .gap_2()
            .child(words(
                "Size limit",
                "The oldest entries go first when the cache is full.".to_owned(),
            ))
            .child(div().flex().flex_wrap().gap_1().children(chips))
            .into_any_element();

        let usage = self.cache_usage.clone().unwrap_or_default();
        let used: u64 = usage.iter().map(|part| part.bytes).sum();
        let sentence = match self.cache_usage {
            Some(_) => self.app.cache_usage_sentence(used),
            None => "Counting…".to_owned(),
        };
        let clear = div()
            .v_flex()
            .px_4()
            .py_3()
            .gap_3()
            .child(
                div()
                    .h_flex()
                    .gap_3()
                    .child(words("Storage", sentence))
                    .child(
                        Button::new("clear-cache")
                            .ghost()
                            .small()
                            .label("Clear cache")
                            .on_click(cx.listener(|this, _, _, cx| this.clear_cache(cx))),
                    ),
            )
            .children((!usage.is_empty()).then(|| self.render_usage_bar(&usage, cx)))
            .into_any_element();

        let rows = if enabled && on {
            vec![switch, size, clear]
        } else if enabled {
            vec![switch, clear]
        } else {
            vec![switch]
        };
        self.render_group("Cache", rows, cx)
    }

    /// One bar for the whole cache, as long as the size limit while it is on, cut into what
    /// each cache holds, with a legend that names each in words so that the shades aren't
    /// all there is to go by. The shades are the muted greys of the palette, never a colour
    /// that means something else.
    fn render_usage_bar(&self, usage: &[Usage], cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (track, bar_gap, muted) = (theme.border, theme.list_head, theme.muted_foreground);
        let is_dark = theme.is_dark();
        let text = theme.foreground;
        // The theme's own palette, in Latte as well as Mocha, so each cache has a colour.
        let shades = [
            lane_color(LaneColor::Branch(1), is_dark),
            lane_color(LaneColor::Branch(2), is_dark),
            lane_color(LaneColor::Branch(3), is_dark),
        ];
        let used: u64 = usage.iter().map(|part| part.bytes).sum();
        // Against the limit while it applies, else the whole bar is what is there.
        let scale = if self.app.disk_cache {
            (u64::from(self.app.cache_mb) << 20).max(used)
        } else {
            used
        }
        .max(1);
        let segments = usage
            .iter()
            .enumerate()
            .filter(|(_, part)| part.bytes > 0)
            .map(|(index, part)| {
                div()
                    .h_full()
                    .flex_none()
                    .w(relative(part.bytes as f32 / scale as f32))
                    .bg(shades[index % shades.len()])
                    .border_r_2()
                    .border_color(bar_gap)
            });
        let legend = usage.iter().enumerate().map(|(index, part)| {
            div()
                .h_flex()
                .gap_1p5()
                .child(
                    div()
                        .size(px(8.))
                        .rounded(px(2.))
                        .bg(shades[index % shades.len()]),
                )
                .child(
                    div()
                        .h_flex()
                        .gap_1()
                        .text_size(TEXT_SMALL)
                        .child(div().text_color(text).child(part.label))
                        .child(div().text_color(muted).child(bytes_label(part.bytes))),
                )
        });
        div()
            .v_flex()
            .gap_2()
            .child(
                div()
                    .h(px(8.))
                    .w_full()
                    .rounded(px(4.))
                    .overflow_hidden()
                    .bg(track)
                    .h_flex()
                    .children(segments),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_x_4()
                    .gap_y_1()
                    .children(legend),
            )
            .into_any_element()
    }

    pub(super) fn render_app(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let on = self.app.reopen_tabs;
        let words = div()
            .v_flex()
            .flex_1()
            .min_w_0()
            .gap_0p5()
            .child(
                div()
                    .text_size(TEXT_BODY)
                    .line_height(px(20.))
                    .child("Reopen repositories on launch"),
            )
            .child(
                div()
                    .text_size(TEXT_SMALL)
                    .line_height(px(17.))
                    .text_color(muted)
                    .child(match &self.state_file {
                        Some(_) => self.app.reopen_sentence(),
                        None => "gitclient can't find a folder to keep this in.",
                    }),
            );
        let switch = self.state_file.is_some().then(|| {
            checkbox("reopen-tabs", on, false, cx)
                .on_click(cx.listener(move |this, _, _, cx| this.set_reopen_tabs(!on, cx)))
        });
        div()
            .v_flex()
            .gap_6()
            .w_full()
            .max_w(PAGE_WIDTH)
            .child(self.page_head(
                "App",
                "What gitclient itself remembers. Git settings are on the other pages.",
                cx,
            ))
            .child(self.render_group(
                "Startup",
                vec![
                    div()
                        .px_4()
                        .py_3()
                        .gap_3()
                        .h_flex()
                        .child(words)
                        .children(switch)
                        .into_any_element(),
                ],
                cx,
            ))
            .children(self.render_launcher_group(cx))
            .child(self.render_updates_group(cx))
            .child(self.render_remotes_group(cx))
            .child(self.render_graph_group(cx))
            .child(self.render_avatars_group(cx))
            .child(self.render_cache_group(cx))
            .into_any_element()
    }
}
