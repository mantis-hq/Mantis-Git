//! The few pictures the interface draws: which forge a line is about, a question mark
//! when that isn't known, and a check and a broken cloud on branch rows, and a lock on a locked worktree, and the window buttons of the tab strip. They are plain shapes that take the colour of the text beside
//! them, so they add no colour of their own. GitHub's mark and the question mark are from
//! Simple Icons and Lucide (ISC); the other two are drawn here.

use gpui_kit::*;
use std::borrow::Cow;

/// A picture the interface can draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Icon {
    GitHub,
    GitLab,
    Gitea,
    Question,
    Check,
    CloudOff,
    Lock,
    ShieldCheck,
    ShieldAlert,
    ArrowUp,
    WindowMinimize,
    WindowMaximize,
    WindowClose,
    /// The app's mark: the mantis on its lane, in one colour, for the welcome panel.
    Mantis,
}

impl Icon {
    /// Where the picture is served from, which `svg().path(..)` asks for.
    pub fn path(self) -> &'static str {
        match self {
            Self::GitHub => "icons/github.svg",
            Self::GitLab => "icons/gitlab.svg",
            Self::Gitea => "icons/gitea.svg",
            Self::Question => "icons/question.svg",
            Self::Check => "icons/check.svg",
            Self::CloudOff => "icons/cloud-off.svg",
            Self::Lock => "icons/lock.svg",
            Self::ShieldCheck => "icons/shield-check.svg",
            Self::ShieldAlert => "icons/shield-alert.svg",
            Self::ArrowUp => "icons/arrow-up.svg",
            Self::WindowMinimize => "icons/window-minimize.svg",
            Self::WindowMaximize => "icons/window-maximize.svg",
            Self::WindowClose => "icons/window-close.svg",
            Self::Mantis => "icons/mantis-git.svg",
        }
    }

    fn bytes(path: &str) -> Option<&'static [u8]> {
        Some(match path {
            "icons/github.svg" => include_bytes!("icons/github.svg"),
            "icons/gitlab.svg" => include_bytes!("icons/gitlab.svg"),
            "icons/gitea.svg" => include_bytes!("icons/gitea.svg"),
            "icons/question.svg" => include_bytes!("icons/question.svg"),
            "icons/check.svg" => include_bytes!("icons/check.svg"),
            "icons/cloud-off.svg" => include_bytes!("icons/cloud-off.svg"),
            "icons/lock.svg" => include_bytes!("icons/lock.svg"),
            "icons/shield-check.svg" => include_bytes!("icons/shield-check.svg"),
            "icons/shield-alert.svg" => include_bytes!("icons/shield-alert.svg"),
            "icons/arrow-up.svg" => include_bytes!("icons/arrow-up.svg"),
            "icons/window-minimize.svg" => include_bytes!("icons/window-minimize.svg"),
            "icons/window-maximize.svg" => include_bytes!("icons/window-maximize.svg"),
            "icons/window-close.svg" => include_bytes!("icons/window-close.svg"),
            "icons/mantis-git.svg" => include_bytes!("icons/mantis-git.svg"),
            _ => return None,
        })
    }
}

/// Serves the pictures to the window. Register it with `Application::with_assets`.
pub struct Icons;

impl AssetSource for Icons {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(Icon::bytes(path).map(Cow::Borrowed))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        let all = [
            Icon::GitHub,
            Icon::GitLab,
            Icon::Gitea,
            Icon::Question,
            Icon::Check,
            Icon::CloudOff,
            Icon::Lock,
            Icon::ShieldCheck,
            Icon::ShieldAlert,
            Icon::ArrowUp,
            Icon::WindowMinimize,
            Icon::WindowMaximize,
            Icon::WindowClose,
            Icon::Mantis,
        ];
        Ok(all
            .into_iter()
            .map(Icon::path)
            .filter(|known| known.starts_with(path))
            .map(SharedString::from)
            .collect())
    }
}

#[cfg(test)]
mod tests {
    // Not `super::*`: that would bring in GPUI's `test` macro, which shadows Rust's.
    use super::{Icon, Icons};
    use gpui_kit::AssetSource;

    #[test]
    fn every_icon_is_served_and_is_an_svg() {
        for icon in [
            Icon::GitHub,
            Icon::GitLab,
            Icon::Gitea,
            Icon::Question,
            Icon::Check,
            Icon::CloudOff,
            Icon::Lock,
            Icon::ShieldCheck,
            Icon::ShieldAlert,
            Icon::ArrowUp,
            Icon::WindowMinimize,
            Icon::WindowMaximize,
            Icon::WindowClose,
            Icon::Mantis,
        ] {
            let bytes = Icons.load(icon.path()).unwrap().expect("served");
            assert!(bytes.starts_with(b"<svg"), "{}", icon.path());
        }
        assert!(Icons.load("icons/none.svg").unwrap().is_none());
        assert_eq!(Icons.list("icons/").unwrap().len(), 14);
    }
}
