//! The About page: which build this is, where its source is, and the licenses of everything
//! in it. What it says lives in `gitclient_core::about`; this only draws it.

use super::*;
use gitclient_core::about::{self, License};
use gitclient_core::install::update::Build;

impl Settings {
    /// Opens a license's text, or closes it when it is the one open.
    pub(super) fn toggle_license(&mut self, index: usize, cx: &mut Context<Self>) {
        self.open_license = (self.open_license != Some(index)).then_some(index);
        cx.notify();
    }

    /// The "Version" card: the build, what gitclient is licensed under, and a link to the
    /// source.
    fn render_version_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let build = Build::current(option_env!("GITCLIENT_COMMIT"));
        let row = div()
            .px_4()
            .py_3()
            .gap_3()
            .h_flex()
            .child(self.row_words(&about::version_line(&build), &about::license_sentence(), cx))
            .child(
                Button::new("open-source")
                    .ghost()
                    .small()
                    .label("Open the source on GitHub")
                    .on_click(|_, _, cx| cx.open_url(about::SOURCE_URL)),
            )
            .into_any_element();
        self.render_group("Version", vec![row], cx)
    }

    /// One license: its name and what is under it, with its text under that when open.
    fn render_license(
        &self,
        index: usize,
        license: &License,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let (muted, base, hover) = (theme.muted_foreground, theme.background, theme.list_hover);
        let open = self.open_license == Some(index);
        let head = div()
            .id(("license", index))
            .v_flex()
            .gap_0p5()
            .px_4()
            .py_2()
            .cursor_pointer()
            .hover(|row| row.bg(hover))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_license(index, cx)))
            .child(
                div()
                    .h_flex()
                    .gap_2()
                    .child(div().w(px(12.)).text_color(muted).child(if open {
                        "▾"
                    } else {
                        "▸"
                    }))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .truncate()
                            .text_size(TEXT_BODY)
                            .line_height(px(20.))
                            .child(license.name.clone()),
                    )
                    .child(
                        div()
                            .flex_none()
                            .text_size(TEXT_SMALL)
                            .text_color(muted)
                            .child(license.projects()),
                    ),
            )
            .child(
                div()
                    .pl_5()
                    .truncate()
                    .text_size(TEXT_SMALL)
                    .line_height(px(17.))
                    .text_color(muted)
                    .child(license.used_by_line()),
            );
        div()
            .v_flex()
            .child(head)
            .when(open, |row| {
                row.child(
                    div().px_4().pb_3().child(
                        div()
                            .p_3()
                            .rounded(px(6.))
                            .bg(base)
                            .text_size(TEXT_SMALL)
                            .line_height(px(20.))
                            .child(license.text.clone()),
                    ),
                )
            })
            .into_any_element()
    }

    /// The "Licenses" card, one row for each license text.
    fn render_licenses_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let rows = about::licenses()
            .iter()
            .enumerate()
            .map(|(index, license)| self.render_license(index, license, cx))
            .collect();
        self.render_group("Licenses", rows, cx)
    }

    pub(super) fn render_about(&self, cx: &mut Context<Self>) -> AnyElement {
        div()
            .v_flex()
            .gap_6()
            .w_full()
            .max_w(PAGE_WIDTH)
            .child(self.page_head("About", &about::page_sentence(), cx))
            .child(self.render_version_group(cx))
            .child(self.render_licenses_group(cx))
            .into_any_element()
    }
}
