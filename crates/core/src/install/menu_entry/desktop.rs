//! The launcher on Linux: a `.desktop` file and an icon, in the user's own data folder,
//! so that no root is needed and a package manager's files are never touched.
//!
//! The file says it is ours with a marker line. One without it is somebody else's, and
//! is left alone.

use super::Status;
use std::path::{Path, PathBuf};

/// A line in the file that says Mantis Git wrote it, and so may rewrite or remove it.
pub const MARKER: &str = "X-Mantisgit-Managed=true";
const ENTRY_NAME: &str = "mantisgit.desktop";
const ICON_NAME: &str = "mantisgit.svg";
const ICON: &str = include_str!("mantisgit.svg");

/// Where the two files go.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Places {
    pub entry: PathBuf,
    pub icon: PathBuf,
}

impl Places {
    /// The places under a data folder such as `~/.local/share`.
    pub fn in_data_home(data_home: &Path) -> Self {
        Self {
            entry: data_home.join("applications").join(ENTRY_NAME),
            icon: data_home
                .join("icons")
                .join("hicolor")
                .join("scalable")
                .join("apps")
                .join(ICON_NAME),
        }
    }

    /// The user's data folder: `$XDG_DATA_HOME` if it is an absolute path, else
    /// `~/.local/share`.
    pub fn this_system() -> Option<Self> {
        Self::from_env(|name| std::env::var_os(name).map(PathBuf::from))
    }

    fn from_env(env: impl Fn(&str) -> Option<PathBuf>) -> Option<Self> {
        let set = |name: &str| env(name).filter(|path| !path.as_os_str().is_empty());
        let data_home = set("XDG_DATA_HOME")
            .filter(|path| path.is_absolute())
            .or_else(|| set("HOME").map(|home| home.join(".local").join("share")))?;
        Some(Self::in_data_home(&data_home))
    }

    /// The folder the launcher goes in, for saying so.
    pub fn folder(&self) -> &Path {
        self.entry.parent().unwrap_or(&self.entry)
    }
}

/// The text of the launcher for the program at `exe`.
pub fn render(exe: &Path, icon: &Path) -> Result<String, String> {
    let exe = exe
        .to_str()
        .filter(|exe| !exe.contains('\n'))
        .ok_or("Mantis Git's path isn't plain text, which a launcher can't hold.")?;
    let icon = icon.to_string_lossy().replace('\n', "");
    Ok(format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Mantis Git\n\
         GenericName=Git client\n\
         Comment=See where you are in a repository, and what to do next\n\
         Exec={exec}\n\
         Icon={icon}\n\
         Terminal=false\n\
         Categories=Development;RevisionControl;\n\
         Keywords=git;version control;repository;\n\
         StartupWMClass=mantisgit\n\
         {MARKER}\n",
        exec = exec_value(exe),
    ))
}

/// A program path as the `Exec` key wants it. The spec quotes an argument in double quotes
/// with `"`, `` ` ``, `$` and `\` escaped, escapes `%` as `%%`, and then escapes every
/// backslash again because the whole value is a string.
fn exec_value(path: &str) -> String {
    let mut quoted = String::from("\"");
    for ch in path.chars() {
        match ch {
            '"' | '`' | '$' | '\\' => {
                quoted.push('\\');
                quoted.push(ch);
            }
            '%' => quoted.push_str("%%"),
            _ => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted.replace('\\', "\\\\")
}

pub fn status(places: &Places, exe: &Path) -> Status {
    let Ok(text) = std::fs::read_to_string(&places.entry) else {
        return if places.entry.exists() {
            Status::Foreign
        } else {
            Status::Absent
        };
    };
    if !text.lines().any(|line| line == MARKER) {
        return Status::Foreign;
    }
    let up_to_date = render(exe, &places.icon).is_ok_and(|wanted| wanted == text);
    Status::Present { up_to_date }
}

fn foreign(places: &Places) -> String {
    format!(
        "There is already a mantisgit.desktop in {} that Mantis Git didn't write, so it was \
         left alone.",
        places.folder().display()
    )
}

/// Writes the launcher and its icon, or says why it didn't.
pub fn add(places: &Places, exe: &Path) -> Result<(), String> {
    if status(places, exe) == Status::Foreign {
        return Err(foreign(places));
    }
    let text = render(exe, &places.icon)?;
    write(&places.icon, ICON)?;
    write(&places.entry, &text)
}

/// Takes the launcher and its icon away.
pub fn remove(places: &Places) -> Result<(), String> {
    match status(places, Path::new("")) {
        Status::Foreign => return Err(foreign(places)),
        Status::Absent => return Ok(()),
        _ => {}
    }
    for file in [&places.entry, &places.icon] {
        match std::fs::remove_file(file) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Couldn't remove {}: {error}", file.display())),
        }
    }
    Ok(())
}

/// Writes a file in one step, so that a crash can't leave half of it for the desktop to read.
fn write(file: &Path, text: &str) -> Result<(), String> {
    let fail = |error: std::io::Error| format!("Couldn't write {}: {error}", file.display());
    if let Some(folder) = file.parent() {
        std::fs::create_dir_all(folder).map_err(fail)?;
    }
    let partial = file.with_extension("tmp");
    std::fs::write(&partial, text).map_err(fail)?;
    std::fs::rename(&partial, file).map_err(fail)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn places() -> (tempfile::TempDir, Places) {
        let dir = tempfile::tempdir().unwrap();
        let places = Places::in_data_home(dir.path());
        (dir, places)
    }

    #[test]
    fn the_places_follow_the_xdg_folders() {
        let at = |home: Option<&str>, data: Option<&str>| {
            Places::from_env(|name| match name {
                "HOME" => home.map(PathBuf::from),
                "XDG_DATA_HOME" => data.map(PathBuf::from),
                _ => None,
            })
        };
        assert_eq!(
            at(Some("/home/me"), None).unwrap().entry,
            Path::new("/home/me/.local/share/applications/mantisgit.desktop")
        );
        assert_eq!(
            at(Some("/home/me"), Some("/data")).unwrap().icon,
            Path::new("/data/icons/hicolor/scalable/apps/mantisgit.svg")
        );
        // A relative XDG_DATA_HOME must be ignored, as the spec says.
        assert_eq!(
            at(Some("/home/me"), Some("data")).unwrap().folder(),
            Path::new("/home/me/.local/share/applications")
        );
        assert!(at(None, None).is_none());
    }

    #[test]
    fn the_exec_line_survives_awkward_paths() {
        assert_eq!(exec_value("/a/mantisgit"), "\"/a/mantisgit\"");
        assert_eq!(exec_value("/my apps/git client"), "\"/my apps/git client\"");
        // `$` and `"` are escaped for the quoting, then every backslash again.
        assert_eq!(exec_value("/a$b"), "\"/a\\\\$b\"");
        assert_eq!(exec_value("/a\"b"), "\"/a\\\\\"b\"");
        assert_eq!(exec_value("/a\\b"), "\"/a\\\\\\\\b\"");
        assert_eq!(exec_value("/100%/g"), "\"/100%%/g\"");
    }

    #[test]
    fn the_launcher_names_the_program_the_icon_and_who_wrote_it() {
        let text = render(Path::new("/h/mantisgit"), Path::new("/i/mantisgit.svg")).unwrap();
        for line in [
            "[Desktop Entry]",
            "Type=Application",
            "Name=Mantis Git",
            "Exec=\"/h/mantisgit\"",
            "Icon=/i/mantisgit.svg",
            "Terminal=false",
            "StartupWMClass=mantisgit",
            MARKER,
        ] {
            assert!(text.lines().any(|l| l == line), "missing {line}\n{text}");
        }
        // No MimeType, which would make Mantis Git the handler for every folder.
        assert!(!text.contains("MimeType"));
    }

    #[test]
    fn adding_writes_both_files_and_removing_takes_both_away() {
        let (_dir, places) = places();
        let exe = Path::new("/h/mantisgit");
        assert_eq!(status(&places, exe), Status::Absent);

        add(&places, exe).unwrap();
        assert_eq!(status(&places, exe), Status::Present { up_to_date: true });
        assert!(
            std::fs::read_to_string(&places.icon)
                .unwrap()
                .contains("<svg")
        );
        assert!(
            std::fs::read_to_string(&places.entry)
                .unwrap()
                .contains("Exec=\"/h/mantisgit\"")
        );

        remove(&places).unwrap();
        assert_eq!(status(&places, exe), Status::Absent);
        assert!(!places.icon.exists());
        // Taking away what isn't there is fine.
        remove(&places).unwrap();
    }

    #[test]
    fn a_launcher_for_another_folder_is_stale_and_adding_again_mends_it() {
        let (_dir, places) = places();
        add(&places, Path::new("/old/mantisgit")).unwrap();
        let now = Path::new("/new/mantisgit");
        assert_eq!(status(&places, now), Status::Present { up_to_date: false });
        add(&places, now).unwrap();
        assert_eq!(status(&places, now), Status::Present { up_to_date: true });
    }

    #[test]
    fn a_launcher_somebody_else_wrote_is_never_replaced_or_removed() {
        let (_dir, places) = places();
        std::fs::create_dir_all(places.folder()).unwrap();
        std::fs::write(&places.entry, "[Desktop Entry]\nName=Mine\n").unwrap();
        let exe = Path::new("/h/mantisgit");

        assert_eq!(status(&places, exe), Status::Foreign);
        assert!(add(&places, exe).unwrap_err().contains("left alone"));
        assert!(remove(&places).unwrap_err().contains("left alone"));
        assert_eq!(
            std::fs::read_to_string(&places.entry).unwrap(),
            "[Desktop Entry]\nName=Mine\n"
        );
    }
}
