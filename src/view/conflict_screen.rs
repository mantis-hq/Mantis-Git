//! The task screen for resolving conflicts. Not built yet.

use super::GraphView;
use gpui_kit::*;

#[derive(Clone, Copy, Default)]
pub(super) struct State;

impl GraphView {
    pub(super) fn render_conflict_screen(
        &mut self,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> AnyElement {
        let State = self.conflict;
        Empty.into_any_element()
    }
}
