//! The resolver that opens over the graph: a conflicted file's conflicts side by side, to
//! take whole or line by line, and the file's result in an editor, saved with one button.

use super::*;

impl GraphView {
    /// Opens the resolver on a file, reading its result from the file as it stands and
    /// laying the file out for the list, which is work for another thread.
    pub fn show_conflict(&mut self, path: &str, cx: &mut Context<Self>) {
        // The file that was in the editor keeps what was typed into it.
        self.catch_up_draft(cx);
        self.conflict.shown = Some(path.to_owned());
        self.conflict.open = true;
        self.conflict.lines.clear();
        self.conflict.jumped = None;
        // The editor keeps a file it already holds, rather than loading it again.
        if self.conflict.editing.as_deref() != Some(path) {
            self.conflict.editing = None;
        }
        self.conflict.scroll.scroll_to_item(0, ScrollStrategy::Top);
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
                let view = FileView::new(&progress.pieces(&text));
                let open = standing_in(&progress, &path, &text);
                Ok::<_, String>((text, Arc::new(view), open))
            },
            move |this, loaded, cx| {
                match loaded {
                    Ok((text, view, open)) => {
                        this.conflict.views.insert(wanted.clone(), view);
                        if this.conflict.drafts.contains_key(&wanted) {
                            // A result from earlier at this stop wins over the file.
                            this.refresh_open(&wanted);
                        } else {
                            this.conflict.drafts.insert(wanted.clone(), text);
                            this.conflict.standing.insert(wanted.clone(), open);
                        }
                    }
                    Err(message) => this.notice = Some(Notice::Error(message.into())),
                }
                cx.notify();
            },
            cx,
        );
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

    pub(in crate::view) fn toggle_line(
        &mut self,
        key: (usize, Side, usize),
        cx: &mut Context<Self>,
    ) {
        if !self.conflict.lines.remove(&key) {
            self.conflict.lines.insert(key);
        }
        cx.notify();
    }

    /// The lines picked in one conflict: those of the first side, then the second's.
    fn picked_lines(&self, ordinal: usize, decision: &Decision) -> String {
        let mut picked = String::new();
        for side in [Side::Mine, Side::Other] {
            for (at, line) in conflict::lines_of(decision.side(side)).iter().enumerate() {
                if self.conflict.lines.contains(&(ordinal, side, at)) {
                    picked.push_str(line);
                    if !line.ends_with('\n') {
                        picked.push('\n');
                    }
                }
            }
        }
        picked
    }

    /// Puts a side, or the picked lines, of one conflict in the result in place of its
    /// markers.
    pub(in crate::view) fn use_chunk(
        &mut self,
        ordinal: usize,
        pick: Pick,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (Some(progress), Some(path)) =
            (self.conflict.progress.clone(), self.conflict.shown.clone())
        else {
            return;
        };
        self.catch_up_draft(cx);
        let Some(draft) = self.conflict.drafts.get(&path).cloned() else {
            return;
        };
        let open = progress.open_chunks(&path, &draft);
        let decision = self.file_decisions(&path).nth(ordinal).cloned();
        let (Some(decision), Some(Some(block))) = (decision, open.get(ordinal)) else {
            return;
        };
        let chosen = match pick {
            Pick::Mine => decision.mine.clone(),
            Pick::Other => decision.other.clone(),
            Pick::Lines => self.picked_lines(ordinal, &decision),
        };
        let text: Arc<str> = conflict::replace_block(&draft, block, &chosen).into();
        self.conflict.drafts.insert(path.clone(), text);
        // The other blocks are untouched, so they stand as they did.
        let standing = open
            .iter()
            .enumerate()
            .map(|(at, block)| block.is_some() && at != ordinal)
            .collect();
        self.conflict.standing.insert(path.clone(), standing);
        self.conflict.revision += 1;
        self.conflict.lines.retain(|(at, ..)| *at != ordinal);

        // The editor changes only where the conflict was, so it keeps its place and the
        // pick can be undone there, and a large file isn't loaded into it again.
        match &self.conflict.editor {
            // The block was found in the draft, which holds what the editor does.
            Some(editor)
                if self.conflict.editing.as_deref() == Some(path.as_str())
                    && editor.read(cx).text().len() == draft.len() =>
            {
                let replacement = block.replacement(&draft, &chosen);
                let range = block.start..block.end;
                editor.update(cx, |editor, cx| {
                    editor.set_selected_range(range, cx);
                    editor.replace(replacement, window, cx);
                });
            }
            // So that the editor reads the new result the next time it is drawn.
            _ => self.conflict.editing = None,
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
        let whole_file = self.diff.view.whole_file;
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
        let from_last = (self.conflict.jumped)
            .filter(|&last| last < count && view.header_row(whole_file, last) >= top);
        let at = match from_last {
            Some(last) => last + usize::from(forward),
            None => view.conflicts_above(whole_file, top, forward),
        };
        let Some(target) = conflict::next_standing(&standing, at, forward) else {
            return;
        };
        self.conflict.jumped = Some(target);
        let row = view.header_row(whole_file, target);
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
        let editor = match &self.conflict.editor {
            Some(editor) => editor.clone(),
            None => {
                let editor = cx.new(|cx| {
                    // Wrapping is worked out for every line of the file, which takes most
                    // of a second for a large one and again at every change of width.
                    TextareaState::new(window, cx)
                        .soft_wrap(false)
                        .placeholder("The file as it will be saved.")
                });
                let changes = cx.subscribe(&editor, |this, _, event, cx| {
                    if matches!(event, InputEvent::Change) {
                        this.editor_changed(cx);
                    }
                });
                self.conflict._editor_changes = Some(changes);
                self.conflict.editor = Some(editor.clone());
                editor
            }
        };
        if self.conflict.editing.as_deref() != Some(path.as_str()) {
            // The file that was in the editor keeps what was typed into it.
            self.catch_up_draft(cx);
            self.conflict.editing = None;
            editor.update(cx, |editor, cx| editor.set_value(draft, window, cx));
            self.conflict.editing = Some(path);
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
                .child(self.render_view_modes(cx))
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
        let (panel, border) = (theme.title_bar, theme.border);
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

        let editor = match (&self.conflict.editor, draft) {
            (Some(editor), Some(_)) if self.conflict.editing.as_deref() == Some(path) => div()
                .flex_1()
                .min_h_0()
                .child(Textarea::new(editor).h_full()),
            _ => div().flex_1().min_h_0().overflow_hidden().child(loading(
                "conflict-result-loading",
                "Reading the file…",
                8,
                cx,
            )),
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
        let whole_file = self.diff.view.whole_file;
        let len = view.len(whole_file);
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
        let marks = if whole_file {
            scroll_marks(
                view.conflicts
                    .iter()
                    .map(|&(start, rows)| (start, rows, peach)),
                view.whole.len(),
            )
        } else {
            Vec::new()
        };
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
                            .map(|at| {
                                this.render_conflict_row(
                                    &rows_view, &owner, whole_file, at, colors, cx,
                                )
                            })
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

    /// One row of the list, by its place in what is shown.
    fn render_conflict_row(
        &self,
        view: &FileView,
        path: &str,
        whole_file: bool,
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

        let index = match view.visible(whole_file, at) {
            Visible::Gap(skipped) => {
                return row
                    .justify_center()
                    .text_size(TEXT_CHIP)
                    .text_color(muted)
                    .child(format!("⋯ {}", count(skipped, "unchanged line")));
            }
            Visible::Row(index) => index,
        };
        let open_at = |ordinal: usize| {
            let open = self.conflict.standing.get(path);
            open.and_then(|open| open.get(ordinal))
                .copied()
                .unwrap_or(true)
        };

        match &view.whole[index] {
            Row::Context { number, text } => row
                .child(
                    div()
                        .w(NUMBER_WIDTH)
                        .flex_none()
                        .pr_3()
                        .text_right()
                        .text_color(overlay)
                        .child(number.to_string()),
                )
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .truncate()
                        .text_color(subtext)
                        .child(expand_tabs(text)),
                ),
            Row::Header { ordinal, line } => {
                let ordinal = *ordinal;
                let open = open_at(ordinal);
                let picked = self
                    .conflict
                    .lines
                    .iter()
                    .filter(|(chosen, ..)| *chosen == ordinal)
                    .count();
                let (mine, other) = self
                    .conflict
                    .progress
                    .as_ref()
                    .map_or_else(Default::default, |p| (p.mine.clone(), p.other.clone()));
                let button = |id: &'static str, label: String, pick: Pick| {
                    Button::new((id, ordinal))
                        .ghost()
                        .xsmall()
                        .label(label)
                        .on_click(cx.listener(move |this, _, window, cx| {
                            this.use_chunk(ordinal, pick, window, cx)
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
                    .when(!open, |row| {
                        row.child(div().text_color(muted).child("settled in the result"))
                    })
                    .when(open, |row| {
                        row.child(button("use-mine", format!("Use {mine}"), Pick::Mine))
                            .child(button("use-other", format!("Use {other}"), Pick::Other))
                            .when(picked > 0, |row| {
                                row.child(button(
                                    "use-lines",
                                    conflict::picked_lines_label(picked),
                                    Pick::Lines,
                                ))
                            })
                    })
            }
            Row::Pair {
                ordinal,
                at: line,
                mine,
                other,
            } => {
                let ordinal = *ordinal;
                let open = open_at(ordinal);
                let half =
                    |side: Side, text: &Option<Arc<str>>, color: Hsla, cx: &mut Context<Self>| {
                        let key = (ordinal, side, *line);
                        let on = open && self.conflict.lines.contains(&key);
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
                            .px_3()
                            .gap_2()
                            .bg(color.opacity(if on { 0.22 } else { 0.07 }));
                        match text {
                            // A side with fewer lines leaves the rest of the row empty.
                            None => half,
                            Some(text) => half
                                .when(open, |half| {
                                    half.cursor_pointer()
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.toggle_line(key, cx)
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

/// A conflicted file laid out for a list, built once in the background.
pub(in crate::view) struct FileView {
    /// Every line of the file: those both sides agree on, and each conflict as a header and
    /// then its lines side by side.
    whole: Vec<Row>,
    /// What to draw when only the changes are shown: the conflicts with a few lines of
    /// context around each, and a note for every stretch left out.
    changes: Vec<Visible>,
    /// Where each conflict is in `whole`: its first row, and how many rows.
    conflicts: Vec<(usize, usize)>,
    /// Where each conflict's header is in `changes`.
    headers_in_changes: Vec<usize>,
}

enum Row {
    Context {
        number: u32,
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
        mine: Option<Arc<str>>,
        other: Option<Arc<str>>,
    },
}

enum Visible {
    Row(usize),
    /// This many lines left out.
    Gap(usize),
}

impl FileView {
    fn new(pieces: &[Piece]) -> Self {
        let line_of = |line: &str| -> Arc<str> { line.trim_end_matches('\n').into() };
        let mut whole = Vec::new();
        let mut conflicts = Vec::new();
        let (mut number, mut ordinal) = (1u32, 0usize);
        for piece in pieces {
            match piece {
                Piece::Text(text) => {
                    for line in conflict::lines_of(text) {
                        whole.push(Row::Context {
                            number,
                            text: line_of(line),
                        });
                        number += 1;
                    }
                }
                Piece::Conflict { mine, other } => {
                    let (mine, other) = (conflict::lines_of(mine), conflict::lines_of(other));
                    let rows = mine.len().max(other.len());
                    let start = whole.len();
                    whole.push(Row::Header {
                        ordinal,
                        line: number,
                    });
                    for at in 0..rows {
                        whole.push(Row::Pair {
                            ordinal,
                            at,
                            mine: mine.get(at).map(|line| line_of(line)),
                            other: other.get(at).map(|line| line_of(line)),
                        });
                    }
                    conflicts.push((start, whole.len() - start));
                    number += rows as u32;
                    ordinal += 1;
                }
            }
        }

        let mut keep = vec![false; whole.len()];
        for &(start, rows) in &conflicts {
            let end = start + rows;
            let around =
                start.saturating_sub(CONTEXT_LINES)..(end + CONTEXT_LINES).min(whole.len());
            keep[around].fill(true);
        }
        let mut changes = Vec::new();
        let mut headers_in_changes = Vec::with_capacity(conflicts.len());
        let mut skipped = 0;
        for (index, keep) in keep.into_iter().enumerate() {
            if keep {
                if skipped > 0 {
                    changes.push(Visible::Gap(skipped));
                    skipped = 0;
                }
                if matches!(whole[index], Row::Header { .. }) {
                    headers_in_changes.push(changes.len());
                }
                changes.push(Visible::Row(index));
            } else {
                skipped += 1;
            }
        }
        if skipped > 0 {
            changes.push(Visible::Gap(skipped));
        }
        Self {
            whole,
            changes,
            conflicts,
            headers_in_changes,
        }
    }

    /// The row of a conflict's header in what is shown.
    fn header_row(&self, whole_file: bool, ordinal: usize) -> usize {
        if whole_file {
            self.conflicts[ordinal].0
        } else {
            self.headers_in_changes[ordinal]
        }
    }

    /// How many conflicts have their header above `row` in what is shown, or at it too when
    /// `at_too`.
    fn conflicts_above(&self, whole_file: bool, row: usize, at_too: bool) -> usize {
        let above = |header: usize| header < row || (at_too && header == row);
        if whole_file {
            self.conflicts.partition_point(|&(start, _)| above(start))
        } else {
            self.headers_in_changes
                .partition_point(|&header| above(header))
        }
    }

    /// How many rows there are to draw.
    fn len(&self, whole_file: bool) -> usize {
        if whole_file {
            self.whole.len()
        } else {
            self.changes.len()
        }
    }

    fn visible(&self, whole_file: bool, at: usize) -> Visible {
        if whole_file {
            Visible::Row(at)
        } else {
            match self.changes[at] {
                Visible::Row(index) => Visible::Row(index),
                Visible::Gap(skipped) => Visible::Gap(skipped),
            }
        }
    }
}

#[cfg(test)]
mod tests;
