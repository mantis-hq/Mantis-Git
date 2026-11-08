//! Remotes in the sidebar. The "+ Add" in their heading and "Edit" in a remote's menu open
//! the form in `view::dialog`; what a remote is and how it is saved lives in
//! `gitclient_core::repo::remote`.

use crate::view::GraphView;
use gitclient_core::menu;
use gpui_kit::*;

impl GraphView {
    pub(in crate::view) fn open_remote_menu(
        &mut self,
        name: &str,
        position: Point<Pixels>,
        cx: &mut Context<Self>,
    ) {
        let lines = menu::remote_menu(&self.refs, name);
        self.show_menu(position, lines, cx);
    }

    /// Whether `name` is a remote of the repository, as opposed to a folder of branches.
    pub(in crate::view) fn is_remote(&self, name: &str) -> bool {
        self.refs
            .remote_configs
            .iter()
            .any(|remote| remote.name == name)
    }
}
