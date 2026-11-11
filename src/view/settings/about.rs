//! The About page: which build this is, how it updates, where its source is, and the licenses
//! that have to be shown for everything in it. What it says lives in `gitclient_core::about`;
//! this only draws it.

use super::*;
use gitclient_core::about::{self, License};
use gitclient_core::install::update::Build;
use gpui_kit::component::scroll::ScrollableElement as _;
use std::ops::Range;

/// The height of one license in the list. Every row shares it, so the list can draw only
/// the rows in view.
const LICENSE_ROW: Pixels = px(48.);
/// How much of the list shows at once.
const LICENSES_SHOWN: Pixels = px(288.);
/// How much of a license's text shows at once before it scrolls.
const TEXT_SHOWN: Pixels = px(320.);

impl Settings {
    /// Picks a license to read, or puts it down when it is the one picked.
    pub(super) fn pick_license(&mut self, index: usize, cx: &mut Context<Self>) {
        self.picked_license = (self.picked_license != Some(index)).then_some(index);
        cx.notify();
    }

    /// The "Version" card: the build and where its source is, then how it updates.
    fn render_version_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let build = Build::current(option_env!("GITCLIENT_COMMIT"));
        let version = div()
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
        let mut rows = vec![version];
        rows.extend(self.git_row(cx));
        rows.extend(self.update_rows(cx));
        self.render_group("Version", rows, cx)
    }

    /// Which git this is, with a way to get a newer one when it is older than Mantis Git
    /// is made for. Not drawn until git has been asked.
    fn git_row(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let row = about::git_row(self.git.as_ref()?);
        Some(
            div()
                .px_4()
                .py_3()
                .gap_3()
                .h_flex()
                .child(self.row_words(&row.title, &row.sentence, cx))
                .children(row.update.then(|| {
                    Button::new("get-newer-git")
                        .ghost()
                        .small()
                        .label("Get a newer git")
                        .on_click(|_, _, cx| cx.open_url(about::GIT_DOWNLOAD_URL))
                }))
                .into_any_element(),
        )
    }

    /// The rows of the list in `range`: a license's name, what is under it, and how many.
    fn render_license_rows(&self, range: Range<usize>, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = cx.theme();
        let (muted, border, picked, hover) = (
            theme.muted_foreground,
            theme.border,
            selected_row(theme),
            theme.list_hover,
        );
        let all = about::licenses();
        range
            .filter_map(|index| Some((index, all.get(index)?)))
            .map(|(index, license)| {
                let on = self.picked_license == Some(index);
                div()
                    .id(("license", index))
                    .v_flex()
                    .w_full()
                    .justify_center()
                    .h(LICENSE_ROW)
                    .px_4()
                    .gap_0p5()
                    .border_b_1()
                    .border_color(border)
                    .cursor_pointer()
                    .when(on, |row| row.bg(picked))
                    .when(!on, |row| row.hover(|row| row.bg(hover)))
                    .on_click(cx.listener(move |this, _, _, cx| this.pick_license(index, cx)))
                    .child(
                        div()
                            .h_flex()
                            .gap_2()
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .truncate()
                                    .text_size(TEXT_BODY)
                                    .line_height(px(20.))
                                    .when(on, |name| name.font_semibold())
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
                            .truncate()
                            .text_size(TEXT_SMALL)
                            .line_height(px(17.))
                            .text_color(muted)
                            .child(license.used_by_line()),
                    )
                    .into_any_element()
            })
            .collect()
    }

    /// The "Licenses" card: a list that draws only the rows in view, and the text of the
    /// license picked from it under that.
    fn render_licenses_group(&self, cx: &mut Context<Self>) -> AnyElement {
        let theme = cx.theme();
        let (panel, border, muted, base) = (
            theme.list_head,
            theme.border,
            theme.muted_foreground,
            theme.background,
        );
        let all = about::licenses();
        let list = div()
            .relative()
            .h(LICENSES_SHOWN)
            .child(
                uniform_list(
                    "licenses",
                    all.len(),
                    cx.processor(|this, range: Range<usize>, _, cx| {
                        this.render_license_rows(range, cx)
                    }),
                )
                .track_scroll(&self.licenses_scroll)
                .size_full(),
            )
            .vertical_scrollbar(&self.licenses_scroll);
        let text = self
            .picked_license
            .and_then(|index| all.get(index))
            .map(|license: &License| {
                // A license worded more than one way shows each wording, with who used it.
                let several = license.texts.len() > 1;
                let blocks = license.texts.iter().enumerate().map(|(index, text)| {
                    // Each copyright line with the projects it came with, so that nobody's
                    // year and name is taken for another's. When all of a wording's
                    // projects share the lines, saying so again would only repeat it.
                    let notices = text.notices.iter().map(|notice| {
                        let lines = notice.copyrights.join("\n");
                        div().v_flex().child(lines).when(
                            notice.used_by != text.used_by,
                            |notice_box| {
                                notice_box.child(
                                    div()
                                        .text_color(muted)
                                        .child(format!("— {}", notice.used_by_line())),
                                )
                            },
                        )
                    });
                    div()
                        .v_flex()
                        .gap_3()
                        .when(index > 0, |block| {
                            block.pt_3().border_t_1().border_color(border)
                        })
                        .when(several, |block| {
                            block.child(
                                div()
                                    .text_color(muted)
                                    .child(format!("Used by {}", text.used_by_line())),
                            )
                        })
                        .child(div().v_flex().gap_2().children(notices))
                        .child(text.body.clone())
                });
                div()
                    .id("license-text")
                    .max_h(TEXT_SHOWN)
                    .overflow_y_scroll()
                    .m_3()
                    .p_3()
                    .rounded(px(6.))
                    .bg(base)
                    .v_flex()
                    .gap_3()
                    .text_size(TEXT_SMALL)
                    .line_height(px(20.))
                    .children(blocks)
            });
        let hint = text.is_none().then(|| {
            div()
                .p_3()
                .text_size(TEXT_SMALL)
                .text_color(muted)
                .child("Pick a license to read it.")
        });
        div()
            .v_flex()
            .gap_2()
            .child(section_label(muted).child("Licenses"))
            .child(
                div()
                    .v_flex()
                    .rounded(px(8.))
                    .border_1()
                    .border_color(border)
                    .bg(panel)
                    .overflow_hidden()
                    .child(list)
                    .children(text)
                    .children(hint),
            )
            .into_any_element()
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
