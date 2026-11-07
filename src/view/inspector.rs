//! The panel beside the graph that shows the selected commit. Not built yet.

use super::GraphView;
use gpui_kit::*;

#[derive(Clone, Copy, Default)]
pub(super) struct State;

impl GraphView {
    pub(super) fn render_inspector(&self, _: &mut Context<Self>) -> Option<AnyElement> {
        let State = self.inspector;
        None
    }

    /// Runs whenever a different commit becomes selected.
    pub(super) fn selection_changed(&mut self, _: &mut Context<Self>) {}
}
