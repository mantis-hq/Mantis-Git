//! The launcher that puts Mantis Git in the application menu (Linux) or the Start menu
//! (Windows), for a copy unpacked from an archive. The welcome panel offers it once, and
//! Settings can add or remove it at any time.
//!
//! What is on disk is the truth: there is no remembered "wanted" flag to fall out of step
//! with it. The only thing remembered is that the welcome has been shown.

mod desktop;
mod shortcut;

use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Absent,
    /// A launcher Mantis Git didn't write is in the way, and is left alone.
    Foreign,
    /// Ours. `up_to_date` is false when it starts a copy of Mantis Git that has since moved.
    Present {
        up_to_date: bool,
    },
}

/// The launcher as the welcome panel offers it: a tick box with a title, and under it what
/// ticking it does, in words.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Offer {
    /// "Add Mantis Git to the application menu".
    pub title: String,
    /// What will be written, and how to take it away again.
    pub detail: String,
    /// The panel's button while the box is ticked, named for its result.
    pub button: &'static str,
}

/// The launcher this system has, if it has one.
#[derive(Clone)]
enum Kind {
    Desktop(desktop::Places),
    Shortcut(shortcut::Place),
}

impl Kind {
    fn this_system() -> Option<Self> {
        if cfg!(windows) {
            shortcut::Place::this_system().map(Self::Shortcut)
        } else if cfg!(target_os = "linux") {
            desktop::Places::this_system().map(Self::Desktop)
        } else {
            None
        }
    }

    /// What the menu is called here.
    fn menu(&self) -> &'static str {
        match self {
            Self::Desktop(_) => "application menu",
            Self::Shortcut(_) => "Start menu",
        }
    }

    fn status(&self, exe: &Path) -> Status {
        match self {
            Self::Desktop(places) => desktop::status(places, exe),
            Self::Shortcut(place) => shortcut::status(place, exe),
        }
    }

    fn add(&self, exe: &Path) -> Result<(), String> {
        match self {
            Self::Desktop(places) => desktop::add(places, exe),
            Self::Shortcut(place) => shortcut::add(place, exe),
        }
    }

    fn remove(&self) -> Result<(), String> {
        match self {
            Self::Desktop(places) => desktop::remove(places),
            Self::Shortcut(place) => shortcut::remove(place),
        }
    }

    fn offer(&self, exe: &Path) -> Offer {
        let detail = match self {
            Self::Desktop(places) => format!(
                "Writes a launcher to {} and an icon beside it, so you can start Mantis Git \
                 without opening its folder. Settings → App takes them away again.",
                places.folder().display()
            ),
            Self::Shortcut(place) => format!(
                "Puts a shortcut to {} in {}, so you can start Mantis Git without opening its \
                 folder. Settings → App takes it away again.",
                exe.display(),
                place.folder().display()
            ),
        };
        Offer {
            title: format!("Add Mantis Git to the {}", self.menu()),
            detail,
            button: "Add to menu and start",
        }
    }

    fn sentence(&self, status: Status, exe: &Path) -> String {
        let menu = self.menu();
        match status {
            Status::Absent => {
                format!(
                    "Mantis Git isn't in your {menu}. It starts from {}.",
                    exe.display()
                )
            }
            Status::Present { up_to_date: true } => {
                format!(
                    "Mantis Git is in your {menu}, and starts from {}.",
                    exe.display()
                )
            }
            Status::Present { up_to_date: false } => format!(
                "Mantis Git is in your {menu}, but it starts a copy that has moved. It is mended \
                 the next time Mantis Git starts."
            ),
            Status::Foreign => match self {
                Self::Desktop(places) => format!(
                    "A gitclient.desktop that Mantis Git didn't write is in {}, so it stays as \
                     it is.",
                    places.folder().display()
                ),
                Self::Shortcut(_) => format!("Something else is in your {menu}."),
            },
        }
    }
}

/// The launcher of this system, which can be added, mended and taken away.
#[derive(Clone)]
pub struct Launcher(Kind);

impl Launcher {
    /// The launcher this system has, or `None` where there is no menu to add Mantis Git to.
    pub fn this_system() -> Option<Self> {
        Kind::this_system().map(Self)
    }

    /// A launcher under `data_home` as on Linux, so tests never touch the real one.
    #[cfg(any(test, feature = "testing"))]
    pub fn in_data_home(data_home: &Path) -> Self {
        Self(Kind::Desktop(desktop::Places::in_data_home(data_home)))
    }

    pub fn status(&self, exe: &Path) -> Status {
        self.0.status(exe)
    }

    /// Adds the launcher, or mends one that starts a copy that has moved.
    pub fn add(&self, exe: &Path) -> Result<(), String> {
        self.0.add(exe)
    }

    pub fn remove(&self) -> Result<(), String> {
        self.0.remove()
    }

    /// Mends a launcher of ours that starts a copy which has moved. One that was never
    /// added, was taken away by hand, or isn't ours stays as it is. Returns whether it was
    /// rewritten.
    pub fn refresh(&self, exe: &Path) -> Result<bool, String> {
        if self.0.status(exe) != (Status::Present { up_to_date: false }) {
            return Ok(false);
        }
        self.0.add(exe)?;
        Ok(true)
    }

    /// What the welcome panel offers.
    pub fn offer(&self, exe: &Path) -> Offer {
        self.0.offer(exe)
    }

    /// The title of the Settings row, such as "Add Mantis Git to the application menu".
    pub fn title(&self) -> String {
        format!("Add Mantis Git to the {}", self.0.menu())
    }

    /// The sentence under the Settings row.
    pub fn sentence(&self, exe: &Path) -> String {
        self.0.sentence(self.0.status(exe), exe)
    }

    /// What the header says after the launcher was added.
    pub fn added_message(&self) -> String {
        format!(
            "Added Mantis Git to your {}. Settings → App takes it away again.",
            self.0.menu()
        )
    }

    /// What the header says after the launcher was removed.
    pub fn removed_message(&self) -> String {
        format!("Took Mantis Git out of your {}.", self.0.menu())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn linux() -> Kind {
        Kind::Desktop(desktop::Places::in_data_home(Path::new("/d")))
    }

    fn windows() -> Kind {
        Kind::Shortcut(shortcut::Place::in_programs(Path::new("C:/Programs")))
    }

    #[test]
    fn the_offer_says_what_will_be_written_and_how_to_undo_it() {
        let exe = Path::new("/h/gitclient");
        let offer = linux().offer(exe);
        assert_eq!(offer.title, "Add Mantis Git to the application menu");
        assert!(offer.detail.contains("/d/applications"));
        assert!(offer.detail.contains("Settings → App"));
        assert_eq!(offer.button, "Add to menu and start");

        let offer = windows().offer(Path::new("C:/g/gitclient.exe"));
        assert_eq!(offer.title, "Add Mantis Git to the Start menu");
        assert!(offer.detail.contains("C:/g/gitclient.exe"));
        assert!(offer.detail.contains("C:/Programs"));
    }

    #[test]
    fn the_settings_sentence_says_how_the_launcher_stands() {
        let exe = Path::new("/h/gitclient");
        let say = |status| linux().sentence(status, exe);
        assert_eq!(
            say(Status::Absent),
            "Mantis Git isn't in your application menu. It starts from /h/gitclient."
        );
        assert!(say(Status::Present { up_to_date: true }).starts_with("Mantis Git is in your"));
        assert!(say(Status::Present { up_to_date: false }).contains("has moved"));
        assert!(say(Status::Foreign).contains("didn't write"));
        assert!(
            windows()
                .sentence(Status::Absent, exe)
                .contains("Start menu")
        );
    }

    #[test]
    fn refreshing_only_mends_a_launcher_that_is_ours_and_stale() {
        let dir = tempfile::tempdir().unwrap();
        let places = desktop::Places::in_data_home(dir.path());
        let kind = Kind::Desktop(places.clone());
        let launcher = Launcher(Kind::Desktop(places.clone()));
        let (old, new): (PathBuf, PathBuf) = ("/old/gitclient".into(), "/new/gitclient".into());

        // Never added: refreshing doesn't add it.
        assert!(!launcher.refresh(&new).unwrap());
        assert!(!places.entry.exists());

        // Added for a copy that has since moved: refreshing points it at this one.
        kind.add(&old).unwrap();
        assert!(launcher.refresh(&new).unwrap());
        assert_eq!(kind.status(&new), Status::Present { up_to_date: true });
        // Nothing left to mend.
        assert!(!launcher.refresh(&new).unwrap());

        // Taken away by hand: it stays away.
        std::fs::remove_file(&places.entry).unwrap();
        assert!(!launcher.refresh(&old).unwrap());
        assert!(!places.entry.exists());

        // Somebody else's launcher is never touched.
        std::fs::write(&places.entry, "[Desktop Entry]\nName=Mine\n").unwrap();
        assert!(!launcher.refresh(&new).unwrap());
        assert!(
            std::fs::read_to_string(&places.entry)
                .unwrap()
                .contains("Mine")
        );
    }
}
