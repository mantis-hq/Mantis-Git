//! The author's mark on a commit's dot: a picture, or their initials on a colour. What is
//! drawn is decided in `mantisgit_core::history::avatar`; this reads the setting, loads the
//! pictures off the UI thread and draws the result.

use super::GraphView;
use crate::theme::lane_color;
use gpui_kit::component::*;
use gpui_kit::prelude::*;
use gpui_kit::*;
use mantisgit_core::history::Commit;
use mantisgit_core::history::avatar::{self, Asked, Gravatar, Kind, Style};
use mantisgit_core::history::lanes::LaneColor;
use mantisgit_core::settings::state::AppState;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::SystemTime;

/// The most requests to gravatar.com out at once.
const MAX_IN_FLIGHT: usize = 4;

/// How wide the mark is. Wider than a lane, but a row holds one commit and is taller, so
/// no two marks touch, and the neighbouring lanes' lines pass clear of it.
pub(super) const SIZE: f32 = 20.;
const RING: f32 = 2.;

/// What one commit's dot shows.
pub(super) enum Face {
    /// A plain dot.
    None,
    Initials {
        text: String,
        tint: u32,
    },
    Picture(Arc<Image>),
    /// A picture still to be read from this file, for Settings' preview.
    File(PathBuf),
}

/// What the view remembers about avatars: the setting as last read, and the pictures
/// loaded so far. Gravatar is asked once for each address.
#[derive(Default)]
pub(super) struct State {
    /// The settings file's time when `prefs` was read, so a change in the Settings
    /// window shows without reading the file on every frame.
    read_at: Option<SystemTime>,
    style: Style,
    own_file: Option<PathBuf>,
    /// The user's own picture, once loaded, and the file it came from.
    own: Option<(PathBuf, Arc<Image>)>,
    own_requested: Option<PathBuf>,
    /// The repository the user's email was read for, and the email.
    own_email: Option<(PathBuf, Option<String>)>,
    /// Shared by every tab, so an author is asked about once per session.
    asked: Arc<Asked>,
    /// Addresses waiting for a turn, and how many requests are out. A long scroll can show
    /// dozens of new authors at once, which gravatar.com is asked about a few at a time.
    queue: VecDeque<String>,
    in_flight: usize,
    /// Whether a failure to reach Gravatar has been said this session.
    failure_shown: bool,
    /// Gravatars that arrived, by `avatar::key`.
    gravatars: HashMap<String, Arc<Image>>,
}

impl State {
    /// The state of a new tab, which shares the one record of who was asked.
    pub(super) fn new() -> Self {
        Self {
            asked: avatar::asked(),
            ..Self::default()
        }
    }
}

fn image_of(kind: Kind, bytes: Vec<u8>) -> Arc<Image> {
    let format = match kind {
        Kind::Png => ImageFormat::Png,
        Kind::Jpeg => ImageFormat::Jpeg,
        Kind::Gif => ImageFormat::Gif,
        Kind::Webp => ImageFormat::Webp,
        Kind::Bmp => ImageFormat::Bmp,
    };
    Arc::new(Image::from_bytes(format, bytes))
}

impl GraphView {
    /// Reads the setting, and starts loading what the rows about to draw need. Called
    /// before a batch of rows is built, so that building them only looks things up.
    pub(super) fn prepare_avatars(&mut self, cx: &mut Context<Self>) {
        self.refresh_avatar_prefs();
        if self.avatars.style == Style::Off {
            return;
        }
        if let Some(repo) = self.repo.clone()
            && self.avatars.own_email.as_ref().map(|(r, _)| r) != Some(&repo)
        {
            // Claimed now, so a slow git isn't asked again by every frame.
            self.avatars.own_email = Some((repo.clone(), None));
            Self::spawn_blocking(
                {
                    let repo = repo.clone();
                    move || avatar::own_email(&repo)
                },
                move |this, email, cx| {
                    if this.repo.as_deref() == Some(repo.as_path()) {
                        this.avatars.own_email = Some((repo, email));
                        cx.notify();
                    }
                },
                cx,
            );
        }
        if let Some(file) = self.avatars.own_file.clone()
            && self.avatars.own_requested.as_ref() != Some(&file)
        {
            self.avatars.own_requested = Some(file.clone());
            Self::spawn_blocking(
                {
                    let file = file.clone();
                    move || avatar::read_own(&file)
                },
                move |this, read, cx| {
                    if this.avatars.own_file.as_ref() != Some(&file) {
                        return;
                    }
                    match read {
                        Ok((kind, bytes)) => this.avatars.own = Some((file, image_of(kind, bytes))),
                        Err(error) => {
                            this.notice = Some(super::notice::Notice::Error(error.into()));
                        }
                    }
                    cx.notify();
                },
                cx,
            );
        }
    }

    /// Says once why Gravatar couldn't be asked. The address isn't asked again until the
    /// app restarts, so a failing connection isn't hammered.
    fn gravatar_failed(&mut self, error: String, cx: &mut Context<Self>) {
        if !self.avatars.failure_shown {
            self.avatars.failure_shown = true;
            self.notice = Some(super::notice::Notice::Error(error.into()));
            cx.notify();
        }
    }

    fn refresh_avatar_prefs(&mut self) {
        let Some(file) = self.state_file().map(std::path::Path::to_path_buf) else {
            return;
        };
        let modified = std::fs::metadata(&file)
            .and_then(|meta| meta.modified())
            .ok();
        if self.avatars.read_at.is_some() && self.avatars.read_at == modified {
            return;
        }
        let state = AppState::load(&file);
        self.avatars.read_at = modified;
        self.avatars.style = state.avatars;
        if self.avatars.own_file != state.own_avatar {
            // A different picture, or none: forget the old one.
            self.avatars.own = None;
            self.avatars.own_requested = None;
            self.avatars.own_file = state.own_avatar;
        }
    }

    /// Starts as many queued lookups as there is room for. A lookup is the disk cache first,
    /// which holds pictures and "none" answers from earlier runs, and gravatar.com only for
    /// an address nobody has asked about lately.
    fn start_gravatar_requests(&mut self, cx: &mut Context<Self>) {
        while self.avatars.in_flight < MAX_IN_FLIGHT
            && let Some(email) = self.avatars.queue.pop_front()
        {
            self.avatars.in_flight += 1;
            let caches = self.caches.clone();
            Self::spawn_blocking(
                {
                    let email = email.clone();
                    move || -> Result<Option<Vec<u8>>, String> {
                        match caches.cached_avatar(&email).as_deref() {
                            Some(Gravatar::Picture(bytes)) => return Ok(Some(bytes.clone())),
                            Some(known @ Gravatar::Missing { .. }) if !known.is_stale() => {
                                return Ok(None);
                            }
                            _ => {}
                        }
                        let answer = avatar::fetch(&email)?;
                        match &answer {
                            Some(bytes) => caches.store_avatar(&email, bytes.clone()),
                            None => caches.store_no_avatar(&email),
                        }
                        Ok(answer)
                    }
                },
                move |this, answer, cx| {
                    this.avatars.in_flight -= 1;
                    match answer {
                        Ok(Some(bytes)) => {
                            if let Some(kind) = avatar::sniff(&bytes) {
                                this.avatars
                                    .gravatars
                                    .insert(avatar::key(&email), image_of(kind, bytes));
                                cx.notify();
                            }
                        }
                        // Final: this address has no Gravatar, so it keeps its initials.
                        Ok(None) => {}
                        Err(error) => this.gravatar_failed(error, cx),
                    }
                    this.start_gravatar_requests(cx);
                },
                cx,
            );
        }
    }

    /// Whether the Gravatar for `email` has arrived. For tests, which can't see the state.
    #[cfg(test)]
    pub(super) fn has_gravatar(&self, email: &str) -> bool {
        self.avatars.gravatars.contains_key(&avatar::key(email))
    }

    /// What `commit`'s dot shows. Starts the Gravatar download the first time an address
    /// is seen, and shows initials until it arrives or if there is none.
    pub(super) fn avatar_face(&mut self, commit: &Commit, cx: &mut Context<Self>) -> Face {
        let style = self.avatars.style;
        if style == Style::Off || commit.oid.is_empty() || !commit.wears_avatar() {
            return Face::None;
        }
        let mine = self
            .avatars
            .own_email
            .as_ref()
            .and_then(|(_, email)| email.as_deref())
            .is_some_and(|own| own == commit.email.trim().to_lowercase());
        if mine && let Some((_, image)) = &self.avatars.own {
            return Face::Picture(image.clone());
        }
        if style == Style::Gravatar {
            if let Some(image) = self.avatars.gravatars.get(&avatar::key(&commit.email)) {
                return Face::Picture(image.clone());
            }
            if self.avatars.asked.claim(&commit.email) {
                self.avatars.queue.push_back(commit.email.to_string());
                self.start_gravatar_requests(cx);
            }
        }
        Face::Initials {
            text: avatar::initials(&commit.author),
            tint: avatar::tint(&commit.email, &commit.author),
        }
    }
}

/// A round avatar `size` px across with a `ring_width` ring: the picture, or the initials on
/// their colour. Used on the graph and, larger, in Settings.
pub(super) fn disc(
    face: Face,
    size: f32,
    ring_width: f32,
    ring: Hsla,
    is_dark: bool,
    on_tint: Hsla,
) -> Div {
    let inside = match face {
        Face::None => div().size_full().into_any_element(),
        Face::Picture(image) => img(image).size_full().rounded_full().into_any_element(),
        Face::File(file) => img(file).size_full().rounded_full().into_any_element(),
        Face::Initials { text, tint } => div()
            .size_full()
            .flex()
            .items_center()
            .justify_center()
            .rounded_full()
            .bg(lane_color(LaneColor::Branch(tint), is_dark))
            .text_size(px(size * 0.45))
            .font_semibold()
            .text_color(on_tint)
            .child(text)
            .into_any_element(),
    };
    div()
        .flex_none()
        .size(px(size))
        .rounded_full()
        .border(px(ring_width))
        .border_color(ring)
        .overflow_hidden()
        .child(inside)
}

/// The mark on a commit's dot, in the lane's colour (blue for HEAD). `left` and `top` are
/// where its corner goes in the graph cell.
pub(super) fn avatar_mark(
    face: Face,
    left: f32,
    top: f32,
    ring: Hsla,
    is_dark: bool,
    on_tint: Hsla,
) -> AnyElement {
    if matches!(face, Face::None) {
        return div().into_any_element();
    }
    disc(face, SIZE, RING, ring, is_dark, on_tint)
        .absolute()
        .left(px(left))
        .top(px(top))
        .into_any_element()
}
