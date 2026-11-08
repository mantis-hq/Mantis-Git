//! Whether this copy is portable, or put there by a package manager.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// What a package recipe sets at build time (`GITCLIENT_INSTALL_CHANNEL=pacman cargo build`)
/// to say that the build is installed by a package manager. Release archives leave it unset.
const BUILT_FOR: Option<&str> = option_env!("GITCLIENT_INSTALL_CHANNEL");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    /// Unpacked from a release archive, so it is the app's own to launch and update.
    Portable,
    /// Installed by something else that keeps it up to date. `by` completes a sentence
    /// such as "gitclient was installed with Flatpak".
    Packaged { by: &'static str },
}

impl Channel {
    /// How this copy was installed.
    pub fn detect() -> Self {
        classify(
            &exe_path(),
            BUILT_FOR,
            |name| std::env::var_os(name),
            |path| path.exists(),
        )
    }

    pub fn is_portable(self) -> bool {
        self == Self::Portable
    }
}

/// The decision behind [`Channel::detect`], with the system it asks about passed in.
fn classify(
    exe: &Path,
    built_for: Option<&str>,
    env: impl Fn(&str) -> Option<OsString>,
    exists: impl Fn(&Path) -> bool,
) -> Channel {
    if built_for.is_some_and(|name| !name.is_empty() && name != "portable") {
        return Channel::Packaged {
            by: "a package manager",
        };
    }
    let set = |name: &str| env(name).is_some_and(|value| !value.is_empty());
    if set("FLATPAK_ID") || exists(Path::new("/.flatpak-info")) {
        return Channel::Packaged { by: "Flatpak" };
    }
    // An AppImage runs from a read-only mount, and its file is what an update would replace.
    if set("APPIMAGE") {
        return Channel::Packaged { by: "an AppImage" };
    }
    // Compared in one spelling, so Windows and Unix paths are tested the same way.
    let path = exe.to_string_lossy().replace('\\', "/").to_lowercase();
    let has = |part: &str| path.contains(part);
    if has("/scoop/apps/") {
        return Channel::Packaged { by: "Scoop" };
    }
    if has("/chocolatey/") {
        return Channel::Packaged { by: "Chocolatey" };
    }
    if has("/windowsapps/") {
        return Channel::Packaged { by: "the Store" };
    }
    if has("/cellar/") || has("/caskroom/") {
        return Channel::Packaged { by: "Homebrew" };
    }
    if has("/nix/store/") {
        return Channel::Packaged { by: "Nix" };
    }
    if path.starts_with("/usr/") && !path.starts_with("/usr/local/") {
        return Channel::Packaged {
            by: "a package manager",
        };
    }
    Channel::Portable
}

/// The path of the running program, as it was when the app started. Linux reports a
/// program that has since been replaced as `… (deleted)`, which is no use for starting it
/// again, so an update never changes the answer.
pub fn exe_path() -> PathBuf {
    static EXE: OnceLock<PathBuf> = OnceLock::new();
    EXE.get_or_init(|| {
        clean(std::env::current_exe().unwrap_or_else(|_| PathBuf::from("gitclient")))
    })
    .clone()
}

fn clean(path: PathBuf) -> PathBuf {
    const DELETED: &str = " (deleted)";
    match path.to_str().and_then(|text| text.strip_suffix(DELETED)) {
        Some(text) => PathBuf::from(text),
        None => path,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn detect(exe: &str, built_for: Option<&str>, env: &[(&str, &str)], flatpak: bool) -> Channel {
        classify(
            Path::new(exe),
            built_for,
            |name| {
                env.iter()
                    .find(|(key, _)| *key == name)
                    .map(|(_, value)| OsString::from(value))
            },
            |path| flatpak && path == Path::new("/.flatpak-info"),
        )
    }

    #[test]
    fn a_folder_the_user_unpacked_into_is_portable() {
        for exe in [
            "/home/me/Apps/gitclient/gitclient",
            "/opt/gitclient/gitclient",
            "/usr/local/bin/gitclient",
            "C:\\Users\\me\\Apps\\gitclient\\gitclient.exe",
        ] {
            assert_eq!(detect(exe, None, &[], false), Channel::Portable, "{exe}");
        }
    }

    #[test]
    fn a_build_made_for_a_package_is_never_portable() {
        assert!(!detect("/home/me/gitclient", Some("pacman"), &[], false).is_portable());
        // The release archives name themselves portable, or say nothing.
        assert!(detect("/home/me/gitclient", Some("portable"), &[], false).is_portable());
        assert!(detect("/home/me/gitclient", Some(""), &[], false).is_portable());
    }

    #[test]
    fn a_package_managers_folders_are_recognised() {
        let by = |exe| match detect(exe, None, &[], false) {
            Channel::Packaged { by } => by,
            Channel::Portable => "portable",
        };
        assert_eq!(by("/usr/bin/gitclient"), "a package manager");
        assert_eq!(by("/nix/store/abc-gitclient/bin/gitclient"), "Nix");
        assert_eq!(
            by("/opt/homebrew/Cellar/gitclient/1/bin/gitclient"),
            "Homebrew"
        );
        assert_eq!(
            by("C:\\Users\\me\\scoop\\apps\\gitclient\\current\\gitclient.exe"),
            "Scoop"
        );
    }

    #[test]
    fn flatpak_and_appimage_are_told_by_their_environment() {
        let flatpak = detect("/app/bin/gitclient", None, &[], true);
        assert_eq!(flatpak, Channel::Packaged { by: "Flatpak" });
        let id = detect(
            "/app/bin/gitclient",
            None,
            &[("FLATPAK_ID", "dev.x")],
            false,
        );
        assert_eq!(id, Channel::Packaged { by: "Flatpak" });
        let image = detect(
            "/tmp/.mount_x/gitclient",
            None,
            &[("APPIMAGE", "/h/g.AppImage")],
            false,
        );
        assert_eq!(image, Channel::Packaged { by: "an AppImage" });
        // An empty variable is not set.
        assert!(detect("/h/gitclient", None, &[("APPIMAGE", "")], false).is_portable());
    }

    #[test]
    fn a_replaced_program_keeps_its_path() {
        assert_eq!(
            clean(PathBuf::from("/h/gitclient (deleted)")),
            PathBuf::from("/h/gitclient")
        );
        assert_eq!(
            clean(PathBuf::from("/h/gitclient")),
            PathBuf::from("/h/gitclient")
        );
    }
}
