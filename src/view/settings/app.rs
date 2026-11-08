//! The App page: what gitclient itself remembers, apart from git. Reopening tabs on
//! launch, how many commits the graph loads, and the cache on disk.

use super::*;

impl Settings {
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
            .child(self.render_graph_group(cx))
            .child(self.render_cache_group(cx))
            .into_any_element()
    }
}
