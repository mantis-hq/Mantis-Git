//! The bar across the top of the graph.

use super::{GraphView, Status, repository_name};
use gpui_kit::component::button::*;
use gpui_kit::component::spinner::Spinner;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;

impl GraphView {
    pub(super) fn render_toolbar(&self, cx: &mut Context<Self>) -> impl IntoElement {
        let theme = cx.theme();
        let (muted, border, header) = (theme.muted_foreground, theme.border, theme.title_bar);
        let name = self.repo.as_deref().map(repository_name);
        let path = self.repo.as_ref().map(|path| path.display().to_string());
        let loading = self.status == Status::Loading || self.busy.is_some();

        div()
            .h_flex()
            .h(px(52.))
            .flex_none()
            .px_3()
            .gap_2()
            .bg(header)
            .border_b_1()
            .border_color(border)
            .when_some(name, |bar, name| {
                bar.child(div().font_semibold().child(name))
            })
            .when_some(path, |bar, path| {
                bar.child(
                    div()
                        .min_w_0()
                        .truncate()
                        .text_sm()
                        .text_color(muted)
                        .child(path),
                )
            })
            .child(div().flex_1())
            .when(loading, |bar| bar.child(Spinner::new().small()))
            .when(!self.entries.is_empty(), |bar| {
                bar.child(
                    div()
                        .flex_none()
                        .text_sm()
                        .text_color(muted)
                        .child(format!("{} commits", self.entries.len())),
                )
            })
            .children(self.render_sidebar_toggle(cx))
            .when(self.repo.is_some(), |bar| {
                bar.child(
                    Button::new("refresh")
                        .ghost()
                        .small()
                        .label("Refresh")
                        .on_click(cx.listener(|this, _, _, cx| this.reload(cx))),
                )
            })
            .child(
                Button::new("open")
                    .ghost()
                    .small()
                    .label("Open…")
                    .on_click(cx.listener(|this, _, _, cx| this.prompt_for_repository(cx))),
            )
    }
}
