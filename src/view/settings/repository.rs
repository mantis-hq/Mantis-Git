//! The Repository page: what belongs to the open repository alone, apart from its git
//! config. For now that is its commit graph, which says whether it lists every commit
//! and offers to write what is missing.

use super::*;
use gitclient_core::repo::commit_graph::{self, Report};

/// How the open repository's commit graph stands, as the page last read it.
#[derive(Default)]
pub(super) enum GraphState {
    /// Not read yet, or being read.
    #[default]
    Checking,
    Read(Result<Report, String>),
}

impl Settings {
    /// Reads how the commit graph stands, off the UI thread. An answer about a repository
    /// the window has since moved away from is dropped.
    pub(super) fn load_commit_graph(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let asked = repo.clone();
        cx.spawn(async move |this, cx| {
            let report = cx
                .background_spawn(async move { commit_graph::check(&repo) })
                .await;
            this.update(cx, |this, cx| {
                if this.repo.as_ref() == Some(&asked) {
                    this.commit_graph = GraphState::Read(report);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Writes what the commit graph is missing, then shows how it stands.
    pub(super) fn write_commit_graph(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        if self.writing_graph {
            return;
        }
        self.writing_graph = true;
        cx.notify();
        let asked = repo.clone();
        cx.spawn(async move |this, cx| {
            let written = cx
                .background_spawn(async move { commit_graph::write(&repo) })
                .await;
            this.update(cx, |this, cx| {
                this.writing_graph = false;
                if this.repo.as_ref() != Some(&asked) {
                    return;
                }
                match written {
                    Ok(report) => {
                        this.feedback = Feedback::Saved(report.written_message());
                        this.commit_graph = GraphState::Read(Ok(report));
                    }
                    Err(message) => {
                        this.feedback = Feedback::Failed(message);
                        // The write may have got part of the way, so read it again.
                        this.load_commit_graph(cx);
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn render_commit_graph_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (muted, danger) = (theme.muted_foreground, theme.danger);
        let (note, color, action) = match &self.commit_graph {
            GraphState::Checking => ("Checking the commit graph…".to_owned(), muted, None),
            GraphState::Read(Ok(report)) => (report.sentence(), muted, report.action()),
            GraphState::Read(Err(message)) => (message.clone(), danger, None),
        };
        let button = action.map(|label| {
            let label = if self.writing_graph {
                "Writing the commit graph…".to_owned()
            } else {
                label
            };
            Button::new("write-commit-graph")
                .ghost()
                .small()
                .label(label)
                .disabled(self.writing_graph)
                .on_click(cx.listener(|this, _, _, cx| this.write_commit_graph(cx)))
        });
        let row = div()
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
                    .child(
                        div()
                            .text_size(TEXT_BODY)
                            .line_height(px(20.))
                            .child("Commit graph"),
                    )
                    .child(
                        div()
                            .text_size(TEXT_SMALL)
                            .line_height(px(17.))
                            .text_color(color)
                            .child(note),
                    )
                    .child(
                        div()
                            .mt_1()
                            .text_size(TEXT_SMALL)
                            .line_height(px(17.))
                            .text_color(muted)
                            .child(
                                "Git keeps it beside the repository's objects to walk history \
                                 faster. Loading the graph and counting ahead and behind use it.",
                            ),
                    ),
            )
            .children(button)
            .into_any_element();
        self.render_group("History", vec![row], cx)
    }

    pub(super) fn render_repository(&self, cx: &mut Context<Self>) -> AnyElement {
        let Some(repo) = &self.repo else {
            return div().into_any_element();
        };
        let sentence = format!(
            "Settings that belong to {} alone, at {}. Its git config is on the Git config page.",
            repository_name(repo),
            repo.display()
        );
        div()
            .v_flex()
            .gap_6()
            .w_full()
            .max_w(PAGE_WIDTH)
            .child(self.page_head("Repository", &sentence, cx))
            .child(self.render_commit_graph_group(cx))
            .into_any_element()
    }
}
