//! The log tab of the dock: the last commands git ran, one picked, and its output as it
//! arrives.

use super::*;

impl Dock {
    /// The command on show: the one picked, else the newest.
    fn shown(&self) -> Option<u64> {
        log::read(|entries| {
            let picked = self
                .selected
                .filter(|id| entries.iter().any(|entry| entry.id == *id));
            picked.or_else(|| entries.last().map(|entry| entry.id))
        })
    }

    /// Shows a command's output. Picking the newest goes back to following it.
    fn select(&mut self, id: u64, cx: &mut Context<Self>) {
        let newest = log::read(|entries| entries.last().map(|entry| entry.id));
        self.selected = (newest != Some(id)).then_some(id);
        cx.notify();
    }

    /// Moves through the sidebar, newest at the top: negative goes toward the newest.
    pub(super) fn move_selection(&mut self, delta: isize, cx: &mut Context<Self>) {
        let ids: Vec<u64> = log::read(|entries| entries.iter().rev().map(|e| e.id).collect());
        let Some(current) = self
            .shown()
            .and_then(|shown| ids.iter().position(|id| *id == shown))
        else {
            return;
        };
        let target = current
            .saturating_add_signed(delta)
            .min(ids.len().saturating_sub(1));
        self.commands_scroll
            .scroll_to_item(target, ScrollStrategy::Nearest);
        self.select(ids[target], cx);
    }

    pub(super) fn render_log(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let Some(shown) = self.shown() else {
            return div()
                .flex_1()
                .p_4()
                .text_size(TEXT_BODY)
                .text_color(muted)
                .child(
                    "Every git command the app runs is listed here, with its output as it arrives.",
                )
                .into_any_element();
        };
        let commands = log::read(|entries| entries.len());
        let rows = log::read(|entries| {
            entries
                .iter()
                .find(|entry| entry.id == shown)
                .map_or(0, output_rows)
        });

        // A command opens at its top. While it runs, the view stays on the newest line
        // unless the reader has scrolled up to look at something.
        let (seen, seen_rows) = self.output_seen;
        if seen != Some(shown) {
            self.output_scroll.scroll_to_item(0, ScrollStrategy::Top);
        } else if rows != seen_rows && rows > 0 {
            let at_end = {
                let scroll = self.output_scroll.0.borrow();
                let base = &scroll.base_handle;
                base.offset().y + base.max_offset().y <= px(ROW_HEIGHT)
            };
            if at_end {
                self.output_scroll
                    .scroll_to_item(rows - 1, ScrollStrategy::Bottom);
            }
        }
        self.output_seen = (Some(shown), rows);

        let border = cx.theme().border;
        let sidebar = div()
            .relative()
            .v_flex()
            .w(px(self.log_width.unwrap_or(SIDEBAR_WIDTH)))
            // A row of columns centres them, and a column with no height shows nothing.
            .h_full()
            .flex_none()
            .border_r_1()
            .border_color(border)
            .child(
                uniform_list(
                    "log-commands",
                    commands,
                    cx.processor(move |this, range: Range<usize>, _, cx| {
                        this.command_rows(range, shown, cx)
                    }),
                )
                .track_scroll(&self.commands_scroll)
                .size_full(),
            )
            .scrollbar(&self.commands_scroll, ScrollbarAxis::Vertical);

        let output = div()
            .v_flex()
            .flex_1()
            .h_full()
            .min_w_0()
            .child(output_header(shown, cx))
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h_0()
                    .child(
                        uniform_list(
                            "log-output",
                            rows,
                            cx.processor(move |_, range: Range<usize>, _, cx| {
                                output_lines(shown, range, cx)
                            }),
                        )
                        .track_scroll(&self.output_scroll)
                        .size_full(),
                    )
                    .scrollbar(&self.output_scroll, ScrollbarAxis::Vertical),
            );

        let row = div()
            .h_flex()
            .flex_1()
            .min_h_0()
            .child(sidebar)
            .child(grip(Edge::Log, true, cx))
            .child(output);
        on_resize(row, Edge::Log, cx, |this, pointer, bounds| {
            let max = f32::from(bounds.size.width) - 240.;
            this.log_width = Some(fit(pointer.x - bounds.left(), 160., max));
        })
        .into_any_element()
    }

    /// The sidebar's rows, newest first.
    fn command_rows(
        &self,
        range: Range<usize>,
        shown: u64,
        cx: &mut Context<Self>,
    ) -> Vec<Stateful<Div>> {
        let theme = cx.theme();
        let (muted, danger) = (theme.muted_foreground, theme.danger);
        let found: Vec<_> = log::read(|entries| {
            range
                .filter_map(|index| {
                    let entry = entries.get(entries.len().checked_sub(index + 1)?)?;
                    let failed = matches!(entry.state, State::Done { success: false, .. });
                    Some((entry.id, entry.title.clone(), entry.brief(), failed))
                })
                .collect()
        });
        found
            .into_iter()
            .map(|(id, title, brief, failed)| {
                parts::panel_row(("log-command", id as usize), id == shown, cx)
                    .h(px(28.))
                    .w_full()
                    .text_size(TEXT_SMALL)
                    .whitespace_nowrap()
                    .on_click(cx.listener(move |this, _, _, cx| this.select(id, cx)))
                    .child(div().flex_1().min_w_0().truncate().child(title))
                    .child(
                        div()
                            .flex_none()
                            .text_color(if failed { danger } else { muted })
                            .child(brief),
                    )
            })
            .collect()
    }
}

/// The rows of one command's output: its lines, and a note if lines were dropped.
fn output_rows(entry: &Entry) -> usize {
    entry.lines.len() + (entry.dropped > 0) as usize
}

/// The command as it ran and how it stands, above its output.
fn output_header(id: u64, cx: &App) -> Div {
    let theme = cx.theme();
    let (muted, text, border) = (theme.muted_foreground, theme.foreground, theme.border);
    let (success, danger) = (theme.success, theme.danger);
    let (command, summary, color) = log::read(|entries| {
        entries
            .iter()
            .find(|entry| entry.id == id)
            .map_or_else(Default::default, |entry| {
                let color = match entry.state {
                    State::Running => muted,
                    State::Done { success: true, .. } => success,
                    State::Done { .. } => danger,
                };
                (entry.command.clone(), entry.summary(), color)
            })
    });
    div()
        .h_flex()
        .h(px(ROW_HEIGHT + 8.))
        .flex_none()
        .px_3()
        .gap_3()
        .items_center()
        .text_size(TEXT_SMALL)
        .whitespace_nowrap()
        .border_b_1()
        .border_color(border)
        .child(div().flex_none().text_color(muted).child("$"))
        .child(
            div()
                .flex_1()
                .min_w_0()
                .truncate()
                .font_semibold()
                .text_color(text)
                .child(command.clone()),
        )
        .child(div().flex_none().text_color(color).child(summary))
        .child(
            Button::new("copy-command")
                .ghost()
                .small()
                .label("Copy")
                .tooltip("Copy the command to the clipboard")
                .on_click(move |_, _, cx| {
                    cx.write_to_clipboard(ClipboardItem::new_string(command.to_string()))
                }),
        )
}

fn output_lines(id: u64, range: Range<usize>, cx: &mut App) -> Vec<Div> {
    let theme = cx.theme();
    let (muted, subtext) = (theme.muted_foreground, tokens(theme.is_dark()).subtext0);
    let danger = theme.danger;
    let row = || {
        div()
            .h_flex()
            .h(px(ROW_HEIGHT))
            .px_3()
            .text_size(TEXT_SMALL)
            .whitespace_nowrap()
    };

    log::read(|entries| {
        let Some(entry) = entries.iter().find(|entry| entry.id == id) else {
            return Vec::new();
        };
        let failed = matches!(entry.state, State::Done { success: false, .. });
        range
            .map(|index| match entry.lines.get(index) {
                Some(line) => {
                    let color = match line.stream {
                        Stream::Out => subtext,
                        Stream::Err if failed => danger,
                        Stream::Err | Stream::Note => muted,
                    };
                    row()
                        .text_color(color)
                        .child(parts::expand_tabs(&line.text))
                }
                None => row()
                    .text_color(muted)
                    .child(format!("… {} more lines not shown", entry.dropped)),
            })
            .collect()
    })
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that would bring in GPUI's `test` macro, which shadows Rust's.
    use super::Dock;
    use gitclient_core::log;
    use gpui_kit::{TestAppContext, VisualTestContext};
    use std::process::Command;

    fn command(name: &str) -> log::Handle {
        let mut command = Command::new("git");
        command.args(["-C", "/dock-test", name]);
        log::start(&command)
    }

    fn id_of(title: &str) -> u64 {
        log::read(|entries| {
            entries
                .iter()
                .rev()
                .find(|entry| entry.title == title)
                .expect("the command is logged")
                .id
        })
    }

    #[gpui_kit::test]
    fn the_log_shows_one_command_and_a_pick_in_the_sidebar_changes_which(cx: &mut TestAppContext) {
        // The log is shared by every test, so another test's command can wake the dock.
        cx.executor().allow_parking();
        cx.update(gpui_kit::init);
        let window = cx.add_window(|_, cx| Dock::new(cx));
        let dock = window.root(cx).unwrap();
        let cx = VisualTestContext::from_window(window.into(), cx);

        command("dock-first").finish(true, "exit 0");
        command("dock-second").finish(true, "exit 0");
        let (first, second) = (id_of("dock-first"), id_of("dock-second"));

        dock.read_with(&cx, |dock, _| {
            // Other tests log commands too, so the newest may be later than `second`.
            assert!(dock.shown() >= Some(second), "follows the newest");
        });
        dock.update(&mut cx.clone(), |dock, cx| dock.select(first, cx));
        dock.read_with(&cx, |dock, _| {
            assert_eq!(dock.shown(), Some(first), "a pick holds, whatever arrives");
        });
        command("dock-third");
        dock.read_with(&cx, |dock, _| assert_eq!(dock.shown(), Some(first)));

        // Moving toward the newest, one step at a time.
        dock.update(&mut cx.clone(), |dock, cx| dock.move_selection(-1, cx));
        dock.read_with(&cx, |dock, _| {
            let shown = dock.shown().unwrap();
            assert!(shown > first, "moved to a newer command");
        });
    }
}
