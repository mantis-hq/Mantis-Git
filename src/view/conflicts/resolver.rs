//! The resolver that opens over the graph: a conflicted file's two versions side by side,
//! whose ticked lines make up the file's result in an editor, saved with one button.

use super::*;

impl GraphView {
    /// Opens the resolver on a file at its first conflict, reading its result from the file
    /// as it stands and laying the file out for the list, which is work for another thread.
    pub fn show_conflict(&mut self, path: &str, cx: &mut Context<Self>) {
        // The file that was in the editor keeps what was typed into it.
        self.catch_up_draft(cx);
        self.conflict.shown = Some(path.to_owned());
        self.conflict.open = true;
        self.conflict.jumped = None;
        // The editor keeps a file it already holds, rather than loading it again.
        if self.conflict.editing.as_deref() != Some(path) {
            self.conflict.editing = None;
        }
        self.conflict.scroll.scroll_to_item(0, ScrollStrategy::Top);
        self.go_to_first_conflict(path);
        // Only one thing covers the graph at a time.
        self.close_diff(cx);
        cx.notify();
        let ready = self.conflict.views.contains_key(path);
        if self.is_whole_file(path) || ready {
            return;
        }
        let (Some(repo), Some(progress)) = (self.workdir(), self.conflict.progress.clone()) else {
            return;
        };
        let path = path.to_owned();
        let wanted = path.clone();
        Self::spawn_blocking(
            move || {
                let text: Arc<str> = conflict::read_text(&repo, &path)?.into();
                let view = Arc::new(FileView::new(&progress.pieces(&text)));
                let originals = Arc::new(conflict::marker_blocks(&text));
                let check = Check {
                    progress,
                    path,
                    text: text.clone(),
                    view: Some(view.clone()),
                    picked: Vec::new(),
                };
                Ok::<_, String>((text, view, originals, check.run()))
            },
            move |this, loaded, cx| {
                match loaded {
                    Ok((text, view, originals, found)) => {
                        this.conflict.views.insert(wanted.clone(), view);
                        (this.conflict.originals).insert(wanted.clone(), originals);
                        if this.conflict.drafts.contains_key(&wanted) {
                            // A result from earlier at this stop wins over the file.
                            this.refresh_open(&wanted);
                        } else {
                            this.conflict.drafts.insert(wanted.clone(), text);
                            this.apply_check(&wanted, found);
                        }
                        if this.conflict.shown.as_deref() == Some(wanted.as_str()) {
                            this.go_to_first_conflict(&wanted);
                        }
                    }
                    Err(message) => this.notice = Some(Notice::Error(message.into())),
                }
                cx.notify();
            },
            cx,
        );
    }

    /// Brings the first conflict still standing to the top of the list, or the first of all
    /// when none stands, once the file is laid out.
    fn go_to_first_conflict(&mut self, path: &str) {
        let Some(view) = self.conflict.views.get(path) else {
            return;
        };
        let standing = self.conflict.standing.get(path);
        let first = standing
            .and_then(|standing| standing.iter().position(|open| *open))
            .unwrap_or(0);
        if first < view.conflicts.len() {
            let row = view.header_row(first);
            self.conflict
                .scroll
                .scroll_to_item(row, ScrollStrategy::Top);
            self.conflict.jumped = Some(first);
        }
    }

    pub(in crate::view) fn close_resolver(&mut self, cx: &mut Context<Self>) {
        self.conflict.open = false;
        cx.notify();
    }

    /// Takes one side for every conflict in a file and saves it.
    pub(in crate::view) fn use_side_for_file(
        &mut self,
        path: &str,
        side: Side,
        cx: &mut Context<Self>,
    ) {
        let Some(progress) = self.conflict.progress.clone() else {
            return;
        };
        self.conflict.saving = Some(path.to_owned());
        let path = path.to_owned();
        self.run_operation(
            "Resolving",
            move |repo| conflict::resolve_with_side(repo, &progress, &path, side).map(|()| None),
            cx,
        );
    }

    /// The lines ticked in one conflict of the file on show.
    pub(super) fn ticked(&self, ordinal: usize) -> Option<&HashSet<(Side, usize)>> {
        let path = self.conflict.shown.as_deref()?;
        self.conflict.picks.get(path)?.get(&ordinal)
    }

    /// Whether ticks still make up a conflict in the result: while it stands, or while what
    /// they put there is as they put it. One edited by hand keeps what was typed.
    pub(in crate::view) fn ticks_apply(&self, path: &str, ordinal: usize) -> bool {
        let regions = self.conflict.regions.get(path);
        regions.is_none_or(|regions| regions.get(ordinal).is_some_and(Option::is_some))
    }

    /// Ticks or clears one line of a conflict, and puts what is ticked in the result.
    pub(in crate::view) fn toggle_line(
        &mut self,
        (ordinal, side, at): (usize, Side, usize),
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.conflict.shown.clone() else {
            return;
        };
        let conflicts = self.conflict.picks.entry(path).or_default();
        let ticked = conflicts.entry(ordinal).or_default();
        if !ticked.remove(&(side, at)) {
            ticked.insert((side, at));
        }
        // With nothing ticked the conflict goes back to its markers.
        if ticked.is_empty() {
            conflicts.remove(&ordinal);
        }
        self.apply_picks(ordinal, window, cx);
    }

    /// Ticks every line of one side of a conflict and none of the other, which takes that
    /// side into the result, even when it has no lines.
    pub(in crate::view) fn use_chunk(
        &mut self,
        ordinal: usize,
        side: Side,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(path) = self.conflict.shown.clone() else {
            return;
        };
        let Some(decision) = self.file_decisions(&path).nth(ordinal) else {
            return;
        };
        let lines = conflict::lines_of(decision.side(side)).len();
        let ticked = (0..lines).map(|at| (side, at)).collect();
        let conflicts = self.conflict.picks.entry(path).or_default();
        conflicts.insert(ordinal, ticked);
        self.apply_picks(ordinal, window, cx);
    }

    /// What the ticks put in place of one conflict: the ticked lines of the first side,
    /// then the second's. `None` with no entry, when the conflict keeps its markers.
    fn picked_lines(&self, ordinal: usize, decision: &Decision) -> Option<String> {
        let ticked = self.ticked(ordinal)?;
        let mut picked = String::new();
        for side in [Side::Mine, Side::Other] {
            for (at, line) in conflict::lines_of(decision.side(side)).iter().enumerate() {
                if ticked.contains(&(side, at)) {
                    picked.push_str(line);
                    if !line.ends_with('\n') {
                        picked.push('\n');
                    }
                }
            }
        }
        Some(picked)
    }

    /// Puts what is ticked in one conflict in the result, in its place: the ticked lines,
    /// or its markers when nothing is.
    fn apply_picks(&mut self, ordinal: usize, window: &mut Window, cx: &mut Context<Self>) {
        let Some(path) = self.conflict.shown.clone() else {
            return;
        };
        self.catch_up_draft(cx);
        // Where it is now, read here: typing may have moved it since the last check.
        let Some(check) = self.check_of(&path) else {
            return;
        };
        let found = check.run();
        let draft = check.text;
        let region = found.regions.get(ordinal).cloned().flatten();
        let decision = self.file_decisions(&path).nth(ordinal).cloned();
        let original =
            (self.conflict.originals.get(&path)).and_then(|all| all.get(ordinal).cloned());
        let (Some(region), Some(decision), Some(original)) = (region, decision, original) else {
            // Edited by hand: what was typed stays.
            cx.notify();
            return;
        };
        let picked = self.picked_lines(ordinal, &decision);
        let content = picked.clone().unwrap_or(original);

        let mut text = String::with_capacity(draft.len() + content.len());
        text.push_str(&draft[..region.start]);
        text.push_str(&content);
        text.push_str(&draft[region.end..]);
        self.conflict.drafts.insert(path.clone(), text.into());
        let count = self.file_decisions(&path).count();
        let all = (self.conflict.picked)
            .entry(path.clone())
            .or_insert_with(|| vec![None; count]);
        if let Some(slot) = all.get_mut(ordinal) {
            *slot = picked.map(Arc::from);
        }
        self.refresh_open(&path);

        // The editor changes only where the conflict is, so it keeps its place and the pick
        // can be undone there, and a large file isn't loaded into it again.
        let on_show = self.conflict.editing.as_deref() == Some(path.as_str());
        match self.conflict.editors.get_mut(&path) {
            // The region was found in the draft, which holds what the editor does.
            Some(editor) if on_show && editor.state.read(cx).text().len() == draft.len() => {
                editor.state.update(cx, |state, cx| {
                    state.set_selected_range(region, cx);
                    state.replace(content, window, cx);
                });
            }
            // So that the editor reads the new result the next time it is drawn.
            Some(editor) => {
                editor.loaded = false;
                self.conflict.editing = None;
            }
            None => {}
        }
        cx.notify();
    }

    /// Scrolls the list to the next conflict that still stands in the result, or the
    /// previous one, going round at either end. It goes on from the conflict last gone to
    /// while that is still in view or below it, and otherwise from the top of the view.
    pub(in crate::view) fn go_to_conflict(&mut self, forward: bool, cx: &mut Context<Self>) {
        if !self.conflict.open {
            return;
        }
        let Some(path) = self.conflict.shown.clone() else {
            return;
        };
        let Some(view) = self.conflict.views.get(&path).cloned() else {
            return;
        };
        let count = view.conflicts.len();
        let standing: Vec<bool> = match self.conflict.standing.get(&path) {
            Some(standing) => standing.clone(),
            None => vec![true; count],
        };
        // The row at the top of the list, or the one it is about to go to.
        let top = {
            let scroll = self.conflict.scroll.0.borrow();
            match &scroll.deferred_scroll_to_item {
                Some(deferred) => deferred.item_index,
                None => scroll.base_handle.logical_scroll_top().0,
            }
        };
        let from_last =
            (self.conflict.jumped).filter(|&last| last < count && view.header_row(last) >= top);
        let at = match from_last {
            Some(last) => last + usize::from(forward),
            None => view.conflicts_above(top, forward),
        };
        let Some(target) = conflict::next_standing(&standing, at, forward) else {
            return;
        };
        self.conflict.jumped = Some(target);
        let row = view.header_row(target);
        self.conflict
            .scroll
            .scroll_to_item(row, ScrollStrategy::Top);
        cx.notify();
    }

    /// Typing only marks the result as changed. The text is read once typing pauses, so a
    /// key costs the same in a file of any size.
    fn editor_changed(&mut self, cx: &mut Context<Self>) {
        if self.conflict.editing.is_none() {
            return;
        }
        self.conflict.behind = true;
        self.check_later(cx);
    }

    /// Keeps the editor on the result of the file on show, making it the first time it is
    /// needed.
    pub(in crate::view) fn sync_conflict_editor(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if !self.conflict.open {
            return;
        }
        let Some(path) = self.conflict.shown.clone() else {
            return;
        };
        let Some(draft) = self.conflict.drafts.get(&path).cloned() else {
            return;
        };
        if self.is_whole_file(&path) {
            return;
        }
        if self.conflict.editing.as_deref() == Some(path.as_str()) {
            return;
        }
        // The file that was on show keeps what was typed into it.
        self.catch_up_draft(cx);
        self.conflict.editing = None;
        match self.conflict.editors.get_mut(&path) {
            None => {
                // Made empty and drawn once, and the result goes in on the next frame: a
                // large file drawn full in a new editor's first frame takes far longer.
                let state = cx.new(|cx| {
                    // Wrapping is worked out for every line of the file, which takes most
                    // of a second for a large one and again at every change of width.
                    TextareaState::new(window, cx)
                        .soft_wrap(false)
                        .placeholder("The file as it will be saved.")
                });
                let owner = path.clone();
                let changes = cx.subscribe(&state, move |this, _, event, cx| {
                    let on_show = this.conflict.editing.as_deref() == Some(owner.as_str());
                    if on_show && matches!(event, InputEvent::Change) {
                        this.editor_changed(cx);
                    }
                });
                let scrolled = cx.observe(&state, |this, _, cx| this.result_scrolled(cx));
                let editor = ResultEditor {
                    state,
                    loaded: false,
                    _changes: changes,
                    _scrolled: scrolled,
                };
                self.conflict.editors.insert(path, editor);
                // A notify while drawing doesn't ask for another frame, so ask for it.
                draw_again(cx);
            }
            Some(editor) if !editor.loaded => {
                let state = editor.state.clone();
                editor.loaded = true;
                state.update(cx, |state, cx| state.set_value(draft, window, cx));
                self.conflict.editing = Some(path);
                self.conflict.synced = None;
            }
            // It still holds the result, which nothing else changes while it is away.
            Some(_) => {
                self.conflict.editing = Some(path);
                self.conflict.synced = None;
            }
        }
    }

    /// Saves the result of the file on show and stages it, which takes the conflict off it.
    pub(in crate::view) fn save_conflict(&mut self, cx: &mut Context<Self>) {
        let (Some(progress), Some(path)) =
            (self.conflict.progress.clone(), self.conflict.shown.clone())
        else {
            return;
        };
        // What was typed last may not have been checked yet.
        if self.conflict.behind {
            self.catch_up_draft(cx);
            self.refresh_open(&path);
        }
        if self.conflicts_left(&path) > 0 {
            return;
        }
        self.conflict.saving = Some(path.clone());
        if self.is_whole_file(&path) {
            let Some(side) = self.conflict.sides.get(&path).copied() else {
                return;
            };
            self.run_operation(
                "Saving",
                move |repo| {
                    conflict::resolve_with_side(repo, &progress, &path, side).map(|()| None)
                },
                cx,
            );
            return;
        }
        let Some(result) = self.conflict.drafts.get(&path).cloned() else {
            return;
        };
        self.run_operation(
            "Saving",
            move |repo| conflict::save_file(repo, &path, &result).map(|()| None),
            cx,
        );
    }

    /// The conflicted file on show, over the graph.
    pub(in crate::view) fn render_resolver(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.conflict.open {
            return None;
        }
        let progress = self.conflict.progress.clone()?;
        let path = self.conflict.shown.clone()?;
        Some(
            self.render_file_resolver(&progress, &path, cx)
                .into_any_element(),
        )
    }

    fn render_file_resolver(&self, progress: &Progress, path: &str, cx: &mut Context<Self>) -> Div {
        let theme = cx.theme();
        let is_dark = theme.is_dark();
        let (panel, border, muted) = (theme.title_bar, theme.border, theme.muted_foreground);
        let (success, peach) = (theme.success, tokens(is_dark).peach);
        let whole = self.is_whole_file(path);
        let loaded = whole || self.conflict.drafts.contains_key(path);
        let left = self.conflicts_left(path);
        let ready = loaded && left == 0;
        let hint = if whole {
            if ready {
                "Ready to save".to_owned()
            } else {
                "Choose which version to keep".to_owned()
            }
        } else {
            conflict::result_hint(left)
        };
        let busy = self.busy.is_some();

        let header = div()
            .h_flex()
            .h(px(40.))
            .flex_none()
            .px_4()
            .gap_3()
            .bg(panel)
            .border_b_1()
            .border_color(border)
            .text_size(TEXT_SMALL)
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .h_flex()
                    .gap_3()
                    .child(file_path(path, muted))
                    .child(
                        div()
                            .flex_none()
                            .text_color(if ready { success } else { peach })
                            .child(hint),
                    ),
            )
            .when(!whole, |bar| {
                let step = |id: &'static str, label: &'static str, tip: &'static str, forward| {
                    Button::new(id)
                        .ghost()
                        .xsmall()
                        .label(label)
                        .tooltip(tip)
                        .disabled(left == 0)
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.go_to_conflict(forward, cx)),
                        )
                };
                bar.child(
                    div()
                        .h_flex()
                        .flex_none()
                        .gap_1()
                        .child(step(
                            "previous-conflict",
                            "Previous",
                            "Previous conflict (Alt ↑)",
                            false,
                        ))
                        .child(step("next-conflict", "Next", "Next conflict (Alt ↓)", true)),
                )
            })
            .child(
                Button::new("close-resolver")
                    .ghost()
                    .xsmall()
                    .label("Close")
                    .on_click(cx.listener(|this, _, _, cx| this.close_resolver(cx))),
            );

        let body = if whole {
            self.render_whole_file_choice(progress, path, cx)
        } else {
            self.render_conflicts_and_result(progress, path, cx)
        };

        let save = div()
            .flex_none()
            .p_4()
            .bg(panel)
            .border_t_1()
            .border_color(border)
            .child(
                Button::new("save-conflict")
                    .primary()
                    .large()
                    .w_full()
                    .label("Save and mark as resolved")
                    .disabled(!ready || busy)
                    .on_click(cx.listener(|this, _, _, cx| this.save_conflict(cx))),
            );

        div()
            .v_flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(header)
            .child(body)
            .child(save)
    }

    /// A file with nothing to merge line by line: one version or the other, as a whole.
    fn render_whole_file_choice(
        &self,
        progress: &Progress,
        path: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = cx.theme();
        let is_dark = theme.is_dark();
        let (background, border, foreground) = (theme.background, theme.border, theme.foreground);
        let (on_color, subtext) = (theme.primary_foreground, tokens(is_dark).subtext0);
        let picked = self.conflict.sides.get(path).copied();
        let decision = progress
            .decisions
            .iter()
            .find(|decision| decision.path == path);
        let other = lane_color(self.refs.branch_color(&progress.other), is_dark);
        let sides = [
            (Side::Mine, &progress.mine, theme.primary),
            (Side::Other, &progress.other, other),
        ];
        let cards = sides.map(|(side, branch, color)| {
            let on = picked == Some(side);
            let (what, why) = decision.map_or(("", ""), |decision| match side {
                Side::Mine => (decision.mine.as_str(), decision.mine_why.as_str()),
                Side::Other => (decision.other.as_str(), decision.other_why.as_str()),
            });
            div()
                .v_flex()
                .flex_1()
                .min_w_0()
                .p_4()
                .gap_2()
                .rounded(px(8.))
                .bg(background)
                .border(px(if on { 2. } else { 1. }))
                .border_color(if on { color } else { border })
                .child(
                    div()
                        .h_flex()
                        .gap_2()
                        .font_semibold()
                        .text_color(color)
                        .child(div().size(px(8.)).flex_none().rounded_full().bg(color))
                        .child(div().min_w_0().truncate().child(branch.clone())),
                )
                .child(div().text_color(subtext).child(why.to_owned()))
                .child(div().child(what.to_owned()))
                .child(
                    div()
                        .id(match side {
                            Side::Mine => "keep-mine",
                            Side::Other => "keep-other",
                        })
                        .mt_auto()
                        .h_flex()
                        .h(px(34.))
                        .justify_center()
                        .rounded(px(6.))
                        .cursor_pointer()
                        .font_semibold()
                        .bg(if on { color } else { border })
                        .text_color(if on { on_color } else { foreground })
                        .child(div().min_w_0().truncate().child(format!("Use {branch}")))
                        .on_click({
                            let path = path.to_owned();
                            cx.listener(move |this, _, _, cx| {
                                this.conflict.sides.insert(path.clone(), side);
                                cx.notify();
                            })
                        }),
                )
        });
        div()
            .h_flex()
            .items_stretch()
            .flex_1()
            .min_h_0()
            .p_4()
            .gap_4()
            .children(cards)
    }

    /// The file's conflicts in the context of the file, over its result in an editor.
    fn render_conflicts_and_result(
        &self,
        progress: &Progress,
        path: &str,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = cx.theme();
        let (panel, border, background) = (theme.title_bar, theme.border, theme.background);
        let subtext = tokens(theme.is_dark()).subtext0;
        let draft = self.conflict.drafts.get(path);
        let shown = match self.conflict.views.get(path).cloned() {
            Some(view) => self.render_conflict_list(progress, path, &view, cx),
            None => div().flex_1().min_h_0().overflow_hidden().child(loading(
                "conflict-list-loading",
                "Reading the conflicts…",
                12,
                cx,
            )),
        };

        let result_loading = || {
            loading("conflict-result-loading", "Reading the file…", 8, cx)
                .size_full()
                .bg(background)
        };
        let editor = match (self.conflict.editors.get(path), draft) {
            // An editor is drawn from its first frame, empty until the result goes in, with
            // the loading rows over it until then.
            (Some(editor), Some(_)) => {
                let loaded = self.conflict.editing.as_deref() == Some(path);
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(Textarea::new(&editor.state).h_full())
                    .when(!loaded, |area| {
                        area.child(div().absolute().inset_0().child(result_loading()))
                    })
            }
            _ => div()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .child(result_loading()),
        };

        div()
            .v_flex()
            .flex_1()
            .min_h_0()
            .p_4()
            .gap_4()
            .child(
                div()
                    .v_flex()
                    .flex_1()
                    .min_h(px(160.))
                    .rounded(px(8.))
                    .overflow_hidden()
                    .bg(panel)
                    .border_1()
                    .border_color(border)
                    .child(shown),
            )
            .child(
                div()
                    .v_flex()
                    .flex_1()
                    .min_h(px(200.))
                    .gap_2()
                    .child(
                        div()
                            .h_flex()
                            .justify_between()
                            .gap_3()
                            .child(div().font_semibold().child("Result"))
                            .child(
                                div()
                                    .text_size(TEXT_SMALL)
                                    .text_color(subtext)
                                    .child("The whole file. Pick above, or edit it by hand."),
                            ),
                    )
                    .child(editor),
            )
    }

    /// The branches' names over a list of the file, and the list: the lines both sides
    /// agree on as context, and each conflict with its two sides next to each other. Only
    /// the rows in view are drawn, so a file of any size costs the same to show.
    fn render_conflict_list(
        &self,
        progress: &Progress,
        path: &str,
        view: &Arc<FileView>,
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = cx.theme();
        let is_dark = theme.is_dark();
        let (border, muted, peach) = (theme.border, theme.muted_foreground, tokens(is_dark).peach);
        let colors = (
            theme.primary,
            lane_color(self.refs.branch_color(&progress.other), is_dark),
        );
        let len = view.len();
        let first = self.file_decisions(path).next();
        let side = |color: Hsla, branch: &str, why: Option<&str>| {
            div()
                .h_flex()
                .flex_1()
                .min_w_0()
                .gap_2()
                .px_3()
                .child(div().size(px(8.)).flex_none().rounded_full().bg(color))
                .child(
                    div()
                        .flex_none()
                        .font_semibold()
                        .text_color(color)
                        .child(branch.to_owned()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_size(TEXT_CHIP)
                        .text_color(muted)
                        .child(why.unwrap_or_default().to_owned()),
                )
        };
        let columns = div()
            .h_flex()
            .h(px(30.))
            .flex_none()
            .border_b_1()
            .border_color(border)
            .text_size(TEXT_SMALL)
            .child(side(
                colors.0,
                &progress.mine,
                first.map(|d| d.mine_why.as_str()),
            ))
            .child(div().w(px(1.)).h_full().flex_none().bg(border))
            .child(side(
                colors.1,
                &progress.other,
                first.map(|d| d.other_why.as_str()),
            ));

        let rows_view = view.clone();
        let owner = path.to_owned();
        let marks = scroll_marks(
            view.conflicts
                .iter()
                .map(|&(start, rows)| (start, rows, peach)),
            len,
        );
        let list = div()
            .relative()
            .flex_1()
            .min_h_0()
            .child(
                uniform_list(
                    "conflict-rows",
                    len,
                    cx.processor(move |this, rows_in_view: Range<usize>, _, cx| {
                        rows_in_view
                            .filter(|at| *at < len)
                            .map(|at| this.render_conflict_row(&rows_view, &owner, at, colors, cx))
                            .collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.conflict.scroll)
                .size_full(),
            )
            .children(marks)
            .scrollbar(&self.conflict.scroll, ScrollbarAxis::Vertical);

        div().v_flex().size_full().child(columns).child(list)
    }

    /// One row of the list: a line of both versions, a conflict's header, or a line of
    /// each side of a conflict.
    fn render_conflict_row(
        &self,
        view: &FileView,
        path: &str,
        at: usize,
        colors: (Hsla, Hsla),
        cx: &mut Context<Self>,
    ) -> Div {
        let theme = cx.theme();
        let (border, muted, foreground) = (theme.border, theme.muted_foreground, theme.foreground);
        let subtext = tokens(theme.is_dark()).subtext0;
        let overlay = tokens(theme.is_dark()).overlay1;
        let row = div()
            .h_flex()
            .h(ROW_HEIGHT)
            .w_full()
            .text_size(TEXT_SMALL)
            .whitespace_nowrap();
        let number = |line: Option<u32>| {
            div()
                .w(NUMBER_WIDTH)
                .flex_none()
                .pr_3()
                .text_right()
                .text_color(overlay)
                .children(line.map(|line| line.to_string()))
        };
        let open_at = |ordinal: usize| {
            let open = self.conflict.standing.get(path);
            open.and_then(|open| open.get(ordinal))
                .copied()
                .unwrap_or(true)
        };

        match &view.rows[at] {
            // Both versions have it, so it shows in both, each with its own number.
            Row::Context { mine, other, text } => {
                let half = |line: u32| {
                    div()
                        .h_flex()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .child(number(Some(line)))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .pr_3()
                                .truncate()
                                .text_color(subtext)
                                .child(expand_tabs(text)),
                        )
                };
                row.child(half(*mine))
                    .child(div().w(px(1.)).h_full().flex_none().bg(border))
                    .child(half(*other))
            }
            Row::Header { ordinal, line } => {
                let ordinal = *ordinal;
                let applies = self.ticks_apply(path, ordinal);
                let note = conflict::result_note(open_at(ordinal), applies);
                let (mine, other) = self
                    .conflict
                    .progress
                    .as_ref()
                    .map_or_else(Default::default, |p| (p.mine.clone(), p.other.clone()));
                let button = |id: &'static str, label: String, side: Side| {
                    Button::new((id, ordinal))
                        .ghost()
                        .xsmall()
                        .label(label)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.use_chunk(ordinal, side, window, cx)
                        }))
                };
                row.px_3()
                    .gap_2()
                    .bg(theme.title_bar)
                    .border_t_1()
                    .border_b_1()
                    .border_color(border)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(muted)
                            .child(format!(
                                "Conflict {} of {} · line {line}",
                                ordinal + 1,
                                view.conflicts.len()
                            )),
                    )
                    .children(note.map(|note| div().text_color(muted).child(note)))
                    .when(applies, |row| {
                        row.child(button("use-mine", format!("Use {mine}"), Side::Mine))
                            .child(button("use-other", format!("Use {other}"), Side::Other))
                    })
            }
            Row::Pair {
                ordinal,
                at: line,
                mine,
                other,
            } => {
                let ordinal = *ordinal;
                // Ticks change the result while they make it up; once typed over, they don't.
                let open = self.ticks_apply(path, ordinal);
                let ticked = self.ticked(ordinal);
                let half = |side: Side,
                            text: &Option<(u32, Arc<str>)>,
                            color: Hsla,
                            cx: &mut Context<Self>| {
                    let key = (ordinal, side, *line);
                    let on = open && ticked.is_some_and(|t| t.contains(&(side, *line)));
                    let id = ordinal * 1_000_000 + line;
                    let (row_id, box_id) = match side {
                        Side::Mine => ("pick-mine-line", "pick-mine-box"),
                        Side::Other => ("pick-other-line", "pick-other-box"),
                    };
                    let half = div()
                        .id((row_id, id))
                        .h_flex()
                        .flex_1()
                        .min_w_0()
                        .h_full()
                        .pr_3()
                        .gap_2()
                        .bg(color.opacity(if on { 0.22 } else { 0.07 }))
                        .child(number(text.as_ref().map(|(line, _)| *line)));
                    match text {
                        // A side with fewer lines leaves the rest of the row empty.
                        None => half,
                        Some((_, text)) => half
                            .when(open, |half| {
                                half.cursor_pointer()
                                    .on_click(cx.listener(move |this, _, window, cx| {
                                        this.toggle_line(key, window, cx)
                                    }))
                                    .child(checkbox((box_id, id), on, false, cx))
                            })
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_color(foreground)
                                    .child(expand_tabs(text)),
                            ),
                    }
                };
                row.when(!open, |row| row.opacity(0.45))
                    .child(half(Side::Mine, mine, colors.0, cx))
                    .child(div().w(px(1.)).h_full().flex_none().bg(border))
                    .child(half(Side::Other, other, colors.1, cx))
            }
        }
    }
}

/// A conflicted file laid out for the list, built once in the background: both versions
/// of the whole file next to each other, the lines they agree on in both, and each
/// conflict as a header and then its two sides' lines.
pub(in crate::view) struct FileView {
    rows: Vec<Row>,
    /// Where each conflict is in `rows`: its header's row, and how many rows with it.
    conflicts: Vec<(usize, usize)>,
    /// How many lines both sides agree on before each conflict, for lining the result up.
    agreed: Vec<usize>,
}

enum Row {
    /// A line both versions have, with its number in each.
    Context {
        mine: u32,
        other: u32,
        text: Arc<str>,
    },
    Header {
        ordinal: usize,
        line: u32,
    },
    /// The `at`th line of both sides of a conflict. A side with fewer lines has none.
    Pair {
        ordinal: usize,
        at: usize,
        mine: Option<(u32, Arc<str>)>,
        other: Option<(u32, Arc<str>)>,
    },
}

impl FileView {
    fn new(pieces: &[Piece]) -> Self {
        let line_of = |line: &str| -> Arc<str> { line.trim_end_matches('\n').into() };
        let mut rows = Vec::new();
        let mut conflicts = Vec::new();
        let mut agreed = Vec::new();
        let (mut mine_number, mut other_number) = (1u32, 1u32);
        let (mut ordinal, mut since) = (0usize, 0usize);
        for piece in pieces {
            match piece {
                Piece::Text(text) => {
                    for line in conflict::lines_of(text) {
                        rows.push(Row::Context {
                            mine: mine_number,
                            other: other_number,
                            text: line_of(line),
                        });
                        mine_number += 1;
                        other_number += 1;
                        since += 1;
                    }
                }
                Piece::Conflict { mine, other } => {
                    let (mine, other) = (conflict::lines_of(mine), conflict::lines_of(other));
                    let start = rows.len();
                    rows.push(Row::Header {
                        ordinal,
                        line: mine_number,
                    });
                    for at in 0..mine.len().max(other.len()) {
                        let side = |lines: &[&str], first: u32| {
                            let line = lines.get(at)?;
                            Some((first + at as u32, line_of(line)))
                        };
                        rows.push(Row::Pair {
                            ordinal,
                            at,
                            mine: side(&mine, mine_number),
                            other: side(&other, other_number),
                        });
                    }
                    conflicts.push((start, rows.len() - start));
                    agreed.push(since);
                    mine_number += mine.len() as u32;
                    other_number += other.len() as u32;
                    (ordinal, since) = (ordinal + 1, 0);
                }
            }
        }
        Self {
            rows,
            conflicts,
            agreed,
        }
    }

    /// The row of a conflict's header.
    fn header_row(&self, ordinal: usize) -> usize {
        self.conflicts[ordinal].0
    }

    /// How many conflicts have their header above `row`, or at it too when `at_too`.
    fn conflicts_above(&self, row: usize, at_too: bool) -> usize {
        let above = |header: usize| header < row || (at_too && header == row);
        self.conflicts.partition_point(|&(start, _)| above(start))
    }

    fn len(&self) -> usize {
        self.rows.len()
    }

    /// How many lines both sides agree on before each conflict.
    pub(super) fn agreed(&self) -> &[usize] {
        &self.agreed
    }

    /// Pairs of places that match in the list and in the result, as rows and lines: the
    /// start and end of each conflict. Between them, lines match one to one.
    pub(super) fn anchors(&self, places: &[Range<usize>]) -> Vec<(f32, f32)> {
        let mut anchors = vec![(0., 0.)];
        for (&(start, rows), place) in self.conflicts.iter().zip(places) {
            anchors.push((start as f32, place.start as f32));
            anchors.push(((start + rows) as f32, place.end as f32));
        }
        anchors
    }
}

#[cfg(test)]
mod tests;
