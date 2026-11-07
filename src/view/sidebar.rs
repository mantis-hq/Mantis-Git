//! The branch sidebar. Not built yet.

use super::GraphView;
use gpui_kit::*;

#[derive(Clone, Copy, Default)]
pub(super) struct State;

impl GraphView {
    pub(super) fn render_sidebar(&self, _: &mut Context<Self>) -> Option<AnyElement> {
        let State = self.sidebar;
        None
    }
}
