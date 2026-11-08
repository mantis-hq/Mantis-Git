//! The inspector's lists of files: a commit's, or the unstaged and staged working
//! changes, as a list or a tree, with the tick boxes that stage them.

use super::INDENT;
use crate::theme::tokens;
use crate::view::parts::TEXT_CHIP;
use crate::view::parts::{
    checkbox, chip, file_path, panel_row, section_label, status_color, working_note,
};
use crate::view::{GraphView, Notice};
use gitclient_core::commit::details::{CommitDetails, FileChange};
use gitclient_core::menu;
use gitclient_core::text::count;
use gitclient_core::tree::{self, FileView, TreeRow};
use gitclient_core::working::{self, Side};
use gpui_kit::component::button::*;
use gpui_kit::component::scroll::{ScrollableElement as _, ScrollbarAxis};
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::collections::HashSet;
use std::ops::Range;
use std::sync::Arc;

/// What the Unstaged list says while git is still walking the tree for untracked files.
const SCANNING: &str = "Looking for untracked files…";

/// One list of files the inspector can draw.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Pane {
    /// A commit's files.
    Commit,
    /// Working changes the index doesn't have.
    Unstaged,
    /// Working changes in the index: what the next commit holds.
    Staged,
}

impl Pane {
    fn of(side: Side) -> Self {
        match side {
            Side::Unstaged => Self::Unstaged,
            Side::Staged => Self::Staged,
        }
    }

    /// The list of working changes this is, if it is one.
    fn side(self) -> Option<Side> {
        match self {
            Self::Commit => None,
            Self::Unstaged => Some(Side::Unstaged),
            Self::Staged => Some(Side::Staged),
        }
    }

    /// Ids that keep one list's rows, boxes and animations apart from the others'.
    fn list_id(self) -> &'static str {
        match self {
            Self::Commit => "inspector-files",
            Self::Unstaged => "inspector-unstaged",
            Self::Staged => "inspector-staged",
        }
    }

    fn row_id(self) -> &'static str {
        match self {
            Self::Commit => "file",
            Self::Unstaged => "unstaged-file",
            Self::Staged => "staged-file",
        }
    }

    fn tree_id(self) -> &'static str {
        match self {
            Self::Commit => "tree-row",
            Self::Unstaged => "unstaged-tree-row",
            Self::Staged => "staged-tree-row",
        }
    }

    fn tick_id(self, folder: bool) -> &'static str {
        match (self, folder) {
            (Self::Staged, false) => "tick-staged-file",
            (Self::Staged, true) => "tick-staged-folder",
            (_, false) => "tick-unstaged-file",
            (_, true) => "tick-unstaged-folder",
        }
    }
}

impl GraphView {
    /// Puts files into the index, or takes them out, then reads the changes again so the
    /// lists show where they stand.
    pub fn stage_paths(&mut self, paths: Vec<String>, unstage: bool, cx: &mut Context<Self>) {
        let Some(repo) = self.workdir() else {
            return;
        };
        if paths.is_empty() || self.inspector.staging {
            return;
        }
        self.inspector.staging = true;
        Self::spawn_blocking(
            move || {
                if unstage {
                    working::unstage_files(&repo, &paths)
                } else {
                    working::stage_files(&repo, &paths)
                }
            },
            move |this, result, cx| {
                this.inspector.staging = false;
                if let Err(message) = result {
                    this.notice = Some(Notice::Error(message.into()));
                }
                this.reload_working(cx);
                cx.notify();
            },
            cx,
        );
    }

    fn closed_folders_mut(&mut self, pane: Pane) -> &mut HashSet<String> {
        match pane {
            Pane::Staged => &mut self.inspector.closed_staged,
            _ => &mut self.inspector.closed_folders,
        }
    }

    fn scroll_of(&self, pane: Pane) -> UniformListScrollHandle {
        match pane {
            Pane::Staged => self.inspector.staged_scroll.clone(),
            _ => self.inspector.files_scroll.clone(),
        }
    }

    /// The Tree and List chips, which every list in the inspector shares.
    pub(super) fn render_view_chips(&self, cx: &mut Context<Self>) -> Div {
        div().h_flex().children(FileView::ALL.map(|view| {
            chip(
                ("inspector-files-view", view as usize),
                view.label(),
                view == self.commit.files_view,
                cx,
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                this.set_files_view(view, cx);
            }))
        }))
    }

    /// A commit's files, under a line that counts them.
    pub(super) fn render_commit_files(
        &self,
        is_merge: bool,
        details: &Arc<CommitDetails>,
        cx: &mut Context<Self>,
    ) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, added, removed) = (theme.muted_foreground, theme.success, theme.danger);
        let total = details.files.len();
        let (additions, deletions) = details.totals();
        let merge_note = tokens(cx.theme().is_dark()).subtext0;

        div()
            .v_flex()
            .flex_1()
            .min_h_0()
            .gap(px(2.))
            // Rows run edge to edge, past the panel's padding.
            .mx(px(-8.))
            .child(
                section_label(muted)
                    .h_flex()
                    .h(px(24.))
                    .px_2()
                    .justify_between()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .child(format!("{} changed", count(total, "file"))),
                    )
                    .child(
                        div()
                            .h_flex()
                            .gap_1()
                            .text_size(TEXT_CHIP)
                            .child(div().text_color(added).child(format!("+{additions}")))
                            .child(div().text_color(removed).child(format!("−{deletions}"))),
                    )
                    .child(self.render_view_chips(cx)),
            )
            .child(self.render_file_pane(Pane::Commit, details, cx))
            .when(is_merge && total == 0, |list| {
                list.child(
                    div()
                        .px_2()
                        .text_color(merge_note)
                        .child("This merge has no changes of its own."),
                )
            })
    }

    /// One of the two lists of working changes. Its box stages everything in it, or
    /// unstages everything, and a list with nothing in it says so.
    pub(super) fn render_working_list(
        &self,
        side: Side,
        details: &Arc<CommitDetails>,
        cx: &mut Context<Self>,
    ) -> Div {
        let pane = Pane::of(side);
        let muted = cx.theme().muted_foreground;
        let paths: Vec<String> = self.working_changes().map_or_else(Vec::new, |changes| {
            changes.side(side).iter().map(|f| f.path.clone()).collect()
        });
        let empty = paths.is_empty();
        let scanning = side == Side::Unstaged && self.inspector.scanning_untracked;
        let unstage = side.unstages();
        let discard_all = (side == Side::Unstaged && !empty).then(|| {
            let what = format!("all {}", count(paths.len(), "file"));
            menu::discard_command(paths.clone(), &what)
        });
        let heading = if empty {
            side.heading().to_owned()
        } else {
            format!("{} · {}", side.heading(), count(paths.len(), "file"))
        };

        div()
            .v_flex()
            .min_h_0()
            .gap(px(2.))
            .mx(px(-8.))
            .when(empty, |list| list.flex_none())
            .when(!empty, |list| list.flex_1().min_h(px(110.)))
            .child(
                section_label(muted)
                    .h_flex()
                    .h(px(24.))
                    .px_2()
                    .gap_2()
                    .when(!empty, |header| {
                        header.child(
                            checkbox(("tick-all", side as usize), unstage, false, cx).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.stage_paths(paths.clone(), unstage, cx)
                                }),
                            ),
                        )
                    })
                    .child(div().flex_1().min_w_0().truncate().child(heading))
                    .when(scanning && !empty, |header| {
                        header.child(working_note(SCANNING, muted))
                    })
                    .children(discard_all.map(|command| {
                        Button::new("discard-all")
                            .ghost()
                            .small()
                            .label("Discard all")
                            .disabled(self.busy.is_some())
                            .on_click(
                                cx.listener(move |this, _, _, cx| this.choose(command.clone(), cx)),
                            )
                    })),
            )
            // "Nothing left to stage" isn't known until the untracked files are found.
            .when(empty && scanning, |list| {
                list.child(div().px_2().child(working_note(SCANNING, muted)))
            })
            .when(empty && !scanning, |list| {
                list.child(div().px_2().text_color(muted).child(side.empty()))
            })
            .when(!empty, |list| {
                list.child(self.render_file_pane(pane, details, cx))
            })
    }

    /// The rows of one list, in the chosen way of listing, drawing only those in view.
    fn render_file_pane(
        &self,
        pane: Pane,
        details: &Arc<CommitDetails>,
        cx: &mut Context<Self>,
    ) -> Div {
        let unstaged = self
            .working_changes()
            .map_or(0, |changes| changes.unstaged.len());
        let total = details.files.len();
        let range = match pane {
            Pane::Commit => 0..total,
            Pane::Unstaged => 0..unstaged.min(total),
            Pane::Staged => unstaged.min(total)..total,
        };
        let offset = range.start;
        let tree_view = self.commit.files_view == FileView::Tree;
        let closed = match pane {
            Pane::Staged => &self.inspector.closed_staged,
            _ => &self.inspector.closed_folders,
        };
        let rows: Arc<Vec<TreeRow>> = Arc::new(if tree_view {
            let paths: Vec<&str> = details.files[range.clone()]
                .iter()
                .map(|file| &*file.path)
                .collect();
            tree::rows(&paths, closed)
        } else {
            Vec::new()
        });
        let listed_rows = if tree_view { rows.len() } else { range.len() };
        let listed = details.clone();
        let scroll = self.scroll_of(pane);

        let list = div().relative().flex_1().min_h_0().child(
            uniform_list(
                pane.list_id(),
                listed_rows,
                cx.processor(move |this, rows_in_view: Range<usize>, _, cx| {
                    if tree_view {
                        return rows_in_view
                            .filter_map(|at| rows.get(at).map(|row| (at, row)))
                            .map(|(at, row)| {
                                this.render_details_tree_row(pane, offset, at, row, &listed, cx)
                            })
                            .collect::<Vec<_>>();
                    }
                    rows_in_view
                        .filter_map(|at| {
                            let file = listed.files.get(offset + at)?;
                            Some(this.render_file(pane, offset + at, file, cx))
                        })
                        .collect::<Vec<_>>()
                }),
            )
            .track_scroll(&scroll)
            .size_full(),
        );
        // A scrollbar takes its identity from the line that adds it, so each list adds its
        // own here. One shared line would make all of them answer to the same bar.
        match pane {
            Pane::Commit => list.scrollbar(&scroll, ScrollbarAxis::Vertical),
            Pane::Unstaged => list.scrollbar(&scroll, ScrollbarAxis::Vertical),
            Pane::Staged => list.scrollbar(&scroll, ScrollbarAxis::Vertical),
        }
    }

    /// The box at the start of a working row. Ticked means in the index, so an unticked
    /// file is staged by ticking it and a ticked one is unstaged by clearing it.
    fn tick_box(
        &self,
        pane: Pane,
        folder: bool,
        key: usize,
        paths: Vec<String>,
        cx: &mut Context<Self>,
    ) -> Option<Stateful<Div>> {
        let side = pane.side()?;
        let unstage = side.unstages();
        Some(
            checkbox((pane.tick_id(folder), key), unstage, false, cx).on_click(cx.listener(
                move |this, _, _, cx| {
                    // The row under the box would otherwise open the diff.
                    cx.stop_propagation();
                    this.stage_paths(paths.clone(), unstage, cx);
                },
            )),
        )
    }

    /// A line of the tree: a folder that opens and closes, or a file stepped in under its
    /// folders. `offset` is where this list starts in the details, and `index` in a file
    /// row counts from there.
    fn render_details_tree_row(
        &self,
        pane: Pane,
        offset: usize,
        at: usize,
        row: &TreeRow,
        details: &CommitDetails,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let muted = cx.theme().muted_foreground;
        match row {
            TreeRow::Folder {
                path,
                name,
                depth,
                open,
                files,
            } => {
                let toggled = path.clone();
                let paths: Vec<String> = files
                    .iter()
                    .filter_map(|index| details.files.get(offset + index))
                    .map(|file| file.path.to_string())
                    .collect();
                let menu_path = path.clone();
                let menu_paths = paths.clone();
                panel_row((pane.tree_id(), at), false, cx)
                    .h(px(28.))
                    .pl(px(8. + INDENT * *depth as f32))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        let closed = this.closed_folders_mut(pane);
                        if !closed.remove(&toggled) {
                            closed.insert(toggled.clone());
                        }
                        cx.notify();
                    }))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            let paths = menu_paths.clone();
                            this.open_folder_menu(
                                &menu_path,
                                paths,
                                pane.side(),
                                event.position,
                                cx,
                            );
                        }),
                    )
                    .child(
                        div()
                            .w(px(12.))
                            .flex_none()
                            .text_size(TEXT_CHIP)
                            .text_color(muted)
                            .child(if *open { "▾" } else { "▸" }),
                    )
                    .children(self.tick_box(pane, true, at, paths, cx))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_color(muted)
                            .child(format!("{name}/")),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(TEXT_CHIP)
                            .text_color(muted)
                            .child(files.len().to_string()),
                    )
            }
            TreeRow::File { index, name, depth } => {
                let index = offset + *index;
                let Some(file) = details.files.get(index) else {
                    return panel_row((pane.tree_id(), at), false, cx);
                };
                let shown = self.shown_file() == Some(index);
                let count = if file.binary {
                    "binary".to_owned()
                } else {
                    format!("+{} −{}", file.additions, file.deletions)
                };
                let paths = vec![file.path.to_string()];
                panel_row((pane.tree_id(), at), shown, cx)
                    .h(px(28.))
                    .pl(px(8. + 20. + INDENT * *depth as f32))
                    .on_click(cx.listener(move |this, _, _, cx| this.show_file(index, cx)))
                    .on_mouse_down(
                        MouseButton::Right,
                        cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                            cx.stop_propagation();
                            this.open_file_menu(index, event.position, cx);
                        }),
                    )
                    .children(self.tick_box(pane, false, index, paths, cx))
                    .child(
                        div()
                            .w(px(12.))
                            .flex_none()
                            .text_size(TEXT_CHIP)
                            .font_medium()
                            .text_color(status_color(file.status, cx))
                            .child(file.status.to_string()),
                    )
                    .child(div().flex_1().min_w_0().truncate().child(name.clone()))
                    .child(
                        div()
                            .flex_none()
                            .text_size(TEXT_CHIP)
                            .text_color(muted)
                            .child(count),
                    )
            }
        }
    }

    /// A file's row. `index` is its place in the details, which is what a click opens.
    fn render_file(
        &self,
        pane: Pane,
        index: usize,
        file: &FileChange,
        cx: &mut Context<Self>,
    ) -> Stateful<Div> {
        let muted = cx.theme().muted_foreground;
        let shown = self.shown_file() == Some(index);
        let count = if file.binary {
            "binary".to_owned()
        } else {
            format!("+{} −{}", file.additions, file.deletions)
        };
        panel_row((pane.row_id(), index), shown, cx)
            .h(px(28.))
            .on_click(cx.listener(move |this, _, _, cx| this.show_file(index, cx)))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(move |this, event: &MouseDownEvent, _, cx| {
                    cx.stop_propagation();
                    this.open_file_menu(index, event.position, cx);
                }),
            )
            .children(self.tick_box(pane, false, index, vec![file.path.to_string()], cx))
            .child(
                div()
                    .w(px(12.))
                    .flex_none()
                    .text_size(TEXT_CHIP)
                    .font_medium()
                    .text_color(status_color(file.status, cx))
                    .child(file.status.to_string()),
            )
            .child(file_path(&file.path, muted))
            .child(
                div()
                    .flex_none()
                    .text_size(TEXT_CHIP)
                    .text_color(muted)
                    .child(count),
            )
    }
}
