//! The Repository page: what belongs to the open repository alone, apart from its git
//! config. For now that is its commit graph, which says whether it lists every commit
//! and offers to write what is missing.

use super::*;
use gitclient_core::repo::commit_graph::{self, Report};
use gitclient_core::repo::forge::{self, Forge};

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

    /// Reads which forge the repository is on, off the UI thread.
    pub(super) fn load_forge_choice(&mut self, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let asked = repo.clone();
        cx.spawn(async move |this, cx| {
            let choice = cx
                .background_spawn(async move { forge::choice::read(&repo) })
                .await;
            this.update(cx, |this, cx| {
                if this.repo.as_ref() == Some(&asked) {
                    this.forge_choice = Some(choice);
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    /// Chooses the forge the repository is on, or with `None` leaves it to gitclient.
    pub(super) fn choose_forge(&mut self, forge: Option<Forge>, cx: &mut Context<Self>) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let asked = repo.clone();
        cx.spawn(async move |this, cx| {
            let saved = cx
                .background_spawn(async move {
                    forge::choice::set(&repo, forge).map(|()| forge::choice::read(&repo))
                })
                .await;
            this.update(cx, |this, cx| {
                if this.repo.as_ref() != Some(&asked) {
                    return;
                }
                match saved {
                    Ok(choice) => {
                        this.feedback = Feedback::Saved(match forge {
                            Some(forge) => format!(
                                "Pull and merge requests are now made for {}.",
                                forge.name()
                            ),
                            None => "gitclient tells which forge it is by itself again.".to_owned(),
                        });
                        this.forge_choice = Some(choice);
                    }
                    Err(message) => this.feedback = Feedback::Failed(message),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    /// The "Hosting" card: which forge the repository's remote is on, which decides the
    /// pages that start and show pull and merge requests.
    fn render_hosting_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let muted = cx.theme().muted_foreground;
        let (note, chosen) = match &self.forge_choice {
            Some(choice) => (choice.sentence(), choice.chosen),
            None => ("Checking the remote…".to_owned(), None),
        };
        let automatic = chip("forge-automatic", "Automatic", chosen.is_none(), cx)
            .on_click(cx.listener(|this, _, _, cx| this.choose_forge(None, cx)));
        let chips = Forge::ALL.into_iter().enumerate().map(|(index, forge)| {
            chip(("forge", index), forge.name(), chosen == Some(forge), cx)
                .on_click(cx.listener(move |this, _, _, cx| this.choose_forge(Some(forge), cx)))
        });
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
                            .child("Forge"),
                    )
                    .child(
                        div()
                            .text_size(TEXT_SMALL)
                            .line_height(px(17.))
                            .text_color(muted)
                            .child(note),
                    )
                    .child(
                        div()
                            .mt_1()
                            .text_size(TEXT_SMALL)
                            .line_height(px(17.))
                            .text_color(muted)
                            .child(
                                "Choose one when the address doesn't say, such as a server \
                                 your company runs. It is kept in this repository's own git \
                                 config as gitclient.forge.",
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_1()
                    .child(automatic)
                    .children(chips),
            )
            .into_any_element();
        self.render_group("Hosting", vec![row], cx)
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
            .child(self.render_hosting_group(cx))
            .child(self.render_commit_graph_group(cx))
            .into_any_element()
    }
}
