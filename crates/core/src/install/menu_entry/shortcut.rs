//! The launcher on Windows: one shortcut in the user's own Start menu folder, which needs
//! no admin rights. There is no icon file to write, since the shortcut takes the program's
//! own icon.

use super::Status;
use std::path::{Path, PathBuf};

const SHORTCUT_NAME: &str = "gitclient.lnk";

/// Where the shortcut goes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Place {
    pub shortcut: PathBuf,
}

impl Place {
    /// The shortcut in a Start menu `Programs` folder.
    pub fn in_programs(programs: &Path) -> Self {
        Self {
            shortcut: programs.join(SHORTCUT_NAME),
        }
    }

    /// `%APPDATA%\Microsoft\Windows\Start Menu\Programs`.
    pub fn this_system() -> Option<Self> {
        Self::from_env(|name| std::env::var_os(name).map(PathBuf::from))
    }

    fn from_env(env: impl Fn(&str) -> Option<PathBuf>) -> Option<Self> {
        let appdata = env("APPDATA").filter(|path| !path.as_os_str().is_empty())?;
        Some(Self::in_programs(
            &appdata
                .join("Microsoft")
                .join("Windows")
                .join("Start Menu")
                .join("Programs"),
        ))
    }

    pub fn folder(&self) -> &Path {
        self.shortcut.parent().unwrap_or(&self.shortcut)
    }
}

/// Writes a shortcut to `exe` at `to`.
#[cfg(not(windows))]
fn write_link(_: &Path, _: &Path) -> Result<(), String> {
    Err("Start menu shortcuts are only for Windows.".to_owned())
}

/// Writes a shortcut to `exe` at `to`.
#[cfg(windows)]
fn write_link(exe: &Path, to: &Path) -> Result<(), String> {
    use mslnk::ShellLink;
    fn failed(to: &Path, error: impl std::fmt::Display) -> String {
        format!("Couldn't write {}: {error}", to.display())
    }
    let mut link = ShellLink::new(exe).map_err(|error| failed(to, error))?;
    link.set_name(Some("Mantis Git".to_owned()));
    if let Some(folder) = exe.parent() {
        link.set_working_dir(Some(folder.to_string_lossy().into_owned()));
    }
    if let Some(folder) = to.parent() {
        std::fs::create_dir_all(folder).map_err(|error| failed(to, error))?;
    }
    link.create_lnk(to).map_err(|error| failed(to, error))
}

/// What a shortcut to `exe` would hold, to compare with the one on disk. Windows can't be
/// asked where a shortcut points without its COM interface, so the file is built again.
fn expected(exe: &Path) -> Result<Vec<u8>, String> {
    let dir = tempfile::tempdir().map_err(|error| error.to_string())?;
    let file = dir.path().join(SHORTCUT_NAME);
    write_link(exe, &file)?;
    std::fs::read(&file).map_err(|error| error.to_string())
}

pub fn status(place: &Place, exe: &Path) -> Status {
    let Ok(held) = std::fs::read(&place.shortcut) else {
        return Status::Absent;
    };
    // If the new one can't be built, the old one is left alone rather than called stale.
    let up_to_date = expected(exe).map_or(true, |wanted| wanted == held);
    Status::Present { up_to_date }
}

pub fn add(place: &Place, exe: &Path) -> Result<(), String> {
    // Written beside the shortcut and moved into place, so that a crash can't leave half.
    let partial = place.shortcut.with_extension("tmp");
    write_link(exe, &partial)?;
    std::fs::rename(&partial, &place.shortcut)
        .map_err(|error| format!("Couldn't write {}: {error}", place.shortcut.display()))
}

pub fn remove(place: &Place) -> Result<(), String> {
    match std::fs::remove_file(&place.shortcut) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "Couldn't remove {}: {error}",
            place.shortcut.display()
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_place_is_in_the_users_start_menu() {
        let place = Place::from_env(|name| (name == "APPDATA").then(|| PathBuf::from("/a")));
        assert_eq!(
            place.unwrap().shortcut,
            Path::new("/a/Microsoft/Windows/Start Menu/Programs/gitclient.lnk")
        );
        assert!(Place::from_env(|_| None).is_none());
    }

    #[cfg(windows)]
    #[test]
    fn adding_writes_a_shortcut_and_removing_takes_it_away() {
        let dir = tempfile::tempdir().unwrap();
        let place = Place::in_programs(&dir.path().join("Programs"));
        // The target has to exist for a shortcut to point at it.
        let exe = dir.path().join("gitclient.exe");
        std::fs::write(&exe, "x").unwrap();

        assert_eq!(status(&place, &exe), Status::Absent);
        add(&place, &exe).unwrap();
        assert!(place.shortcut.exists());
        assert_eq!(status(&place, &exe), Status::Present { up_to_date: true });
        // The shortcut names where it points, in the bytes Windows reads.
        let bytes = std::fs::read(&place.shortcut).unwrap();
        let name = exe.file_name().unwrap().to_str().unwrap().as_bytes();
        assert!(bytes.windows(name.len()).any(|window| window == name));
        // No half-written file is left in the Start menu.
        assert!(!place.shortcut.with_extension("tmp").exists());

        remove(&place).unwrap();
        assert_eq!(status(&place, &exe), Status::Absent);
        remove(&place).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn a_shortcut_to_another_folder_is_stale() {
        let dir = tempfile::tempdir().unwrap();
        let place = Place::in_programs(dir.path());
        let (old, new) = (dir.path().join("old.exe"), dir.path().join("new.exe"));
        std::fs::write(&old, "x").unwrap();
        std::fs::write(&new, "x").unwrap();

        add(&place, &old).unwrap();
        assert_eq!(status(&place, &new), Status::Present { up_to_date: false });
        add(&place, &new).unwrap();
        assert_eq!(status(&place, &new), Status::Present { up_to_date: true });
    }
}
