//! The task screen for writing a commit. Not built yet.

use super::GraphView;
use gpui_kit::*;

#[derive(Clone, Copy, Default)]
pub(super) struct State;

impl GraphView {
    pub(super) fn render_commit_screen(
        &mut self,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> AnyElement {
        let State = self.commit;
        Empty.into_any_element()
    }
}
