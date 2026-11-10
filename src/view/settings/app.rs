//! The App page: what gitclient itself remembers, apart from git. Reopening tabs on
//! launch, how many commits the graph loads, and the cache on disk.

use super::*;
use gitclient_core::history::avatar::{self, Style as AvatarStyle};
use gitclient_core::install::exe_path;
use gitclient_core::install::update::{self, Track};

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
            let size = cx.background_spawn(async move { caches.disk_size() }).await;
            this.update(cx, |this, cx| {
                this.cache_size = Some(size);
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

    /// The rows of the "Graph" card about avatars: what the dots show, and the user's own
    /// picture.
    fn avatar_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let muted = cx.theme().muted_foreground;
        let enabled = self.state_file.is_some();
        let style = self.app.avatars;
        let chips = AvatarStyle::ALL.into_iter().map(|choice| {
            chip(
                ("avatar-style", choice as usize),
                choice.label(),
                choice == style,
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| this.set_avatar_style(choice, cx)))
        });
        let title =
            |text: &'static str| div().text_size(TEXT_BODY).line_height(px(20.)).child(text);
        let note = |text: String| {
            div()
                .text_size(TEXT_SMALL)
                .line_height(px(17.))
                .text_color(muted)
                .child(text)
        };
        let shown = div()
            .v_flex()
            .px_4()
            .py_3()
            .gap_2()
            .child(
                div()
                    .v_flex()
                    .gap_0p5()
                    .child(title("Author avatars"))
                    .child(note(if enabled {
                        style.sentence().to_owned()
                    } else {
                        "gitclient can't find a folder to keep this in.".to_owned()
                    })),
            )
            .children(enabled.then(|| div().flex().flex_wrap().gap_1().children(chips)))
            .into_any_element();
        if !enabled || style == AvatarStyle::Off {
            return vec![shown];
        }
        let own = self.app.own_avatar.clone();
        let words = match &own {
            Some(file) => format!(
                "Your commits, the ones made with the email git has set, show {}.",
                file.display()
            ),
            None => "Pick a picture from this computer for your own commits. It is only read \
                     here, never sent anywhere."
                .to_owned(),
        };
        let buttons = div()
            .h_flex()
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
        let mine = div()
            .px_4()
            .py_3()
            .gap_3()
            .h_flex()
            .child(
                div()
                    .v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_0p5()
                    .child(title("Your own avatar"))
                    .child(note(words)),
            )
            .child(buttons)
            .into_any_element();
        vec![shown, mine]
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
        let mut rows = vec![row];
        rows.extend(self.avatar_rows(cx));
        self.render_group("Graph", rows, cx)
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

        let used = match self.cache_size {
            Some(bytes) => format!("Using {} on disk.", bytes_label(bytes)),
            None => "Counting…".to_owned(),
        };
        let clear = div()
            .px_4()
            .py_3()
            .gap_3()
            .h_flex()
            .child(words("Clear the cache", used))
            .child(
                Button::new("clear-cache")
                    .ghost()
                    .small()
                    .label("Clear cache")
                    .on_click(cx.listener(|this, _, _, cx| this.clear_cache(cx))),
            )
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
            .child(self.render_graph_group(cx))
            .child(self.render_cache_group(cx))
            .into_any_element()
    }
}
