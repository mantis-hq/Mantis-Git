//! Updating a portable copy from the project's GitHub releases.
//!
//! The steps: ask GitHub for the latest release, and if it is newer, download the archive
//! built for this system, check it against the release's `SHA256SUMS.txt`, take the program
//! out of it, and swap it in for the running one. The new version starts the next time the
//! app does. Nothing is installed without the checksum matching, and nothing at all happens
//! for a copy a package manager installed.
//!
//! The network sits behind [`Http`], so every step is tested without one.

use super::Channel;
use sha2::{Digest, Sha256};
use std::fmt;
use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// The GitHub project releases are read from, as `owner/name`.
pub const REPOSITORY: &str = "pretext8335/gitclient";

/// The most a release's archive may be, so a wrong answer can't fill the disk.
const MAX_ARCHIVE: u64 = 256 * 1024 * 1024;
/// The most the program may be once it is out of the archive.
const MAX_PROGRAM: u64 = 512 * 1024 * 1024;
const MAX_TEXT: u64 = 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Version {
    pub major: u32,
    pub minor: u32,
    pub patch: u32,
}

impl Version {
    /// This app's own version.
    pub fn current() -> Self {
        Self::parse(env!("CARGO_PKG_VERSION")).unwrap_or(Self {
            major: 0,
            minor: 0,
            patch: 0,
        })
    }

    /// Reads `1.2.3` or `v1.2.3`. Anything after a `-` or `+` is ignored.
    pub fn parse(text: &str) -> Option<Self> {
        let text = text.trim().trim_start_matches('v');
        let core = text.split(['-', '+']).next()?;
        let mut parts = core.split('.');
        let mut next = || parts.next()?.parse().ok();
        let version = Self {
            major: next()?,
            minor: next()?,
            patch: next()?,
        };
        parts.next().is_none().then_some(version)
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// The build a release has for one kind of system, named as the release workflow names it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Target {
    triple: &'static str,
    zip: bool,
    program: &'static str,
}

impl Target {
    /// The build for the system this runs on, if releases have one.
    pub fn this_system() -> Option<Self> {
        Self::of(std::env::consts::OS, std::env::consts::ARCH)
    }

    /// The build for the system named as `std::env::consts` names them.
    pub fn of(os: &str, arch: &str) -> Option<Self> {
        let (triple, zip, program) = match (os, arch) {
            ("linux", "x86_64") => ("x86_64-unknown-linux-gnu", false, "gitclient"),
            ("macos", "aarch64") => ("aarch64-apple-darwin", false, "gitclient"),
            ("windows", "x86_64") => ("x86_64-pc-windows-msvc", true, "gitclient.exe"),
            _ => return None,
        };
        Some(Self {
            triple,
            zip,
            program,
        })
    }

    /// `gitclient-v0.2.0-x86_64-unknown-linux-gnu.tar.gz`
    pub fn archive_name(&self, tag: &str) -> String {
        let extension = if self.zip { "zip" } else { "tar.gz" };
        format!("gitclient-{tag}-{}.{extension}", self.triple)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Asset {
    pub name: String,
    pub url: String,
}

/// A release newer than this copy, with what it takes to install it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Available {
    pub version: Version,
    pub tag: String,
    archive: Asset,
    checksums: Asset,
    target: Target,
}

impl Available {
    /// Where the release is described, for telling the user how to go back.
    pub fn page(&self) -> String {
        format!("github.com/{REPOSITORY}/releases/tag/{}", self.tag)
    }
}

/// Whether this copy can update itself, and if not, why in words.
pub enum Support {
    Yes(Target),
    No(String),
}

pub fn support(channel: Channel, target: Option<Target>) -> Support {
    match (channel, target) {
        (Channel::Packaged { by }, _) => Support::No(format!(
            "gitclient was installed with {by}, which updates it."
        )),
        (Channel::Portable, None) => {
            Support::No("There is no build of gitclient for this system to update to.".to_owned())
        }
        (Channel::Portable, Some(target)) => Support::Yes(target),
    }
}

// ---- the network ----

/// Fetching a URL, which is all the updater asks of the network.
pub trait Http {
    /// Writes the body of `url` to `into`, failing if it is longer than `limit` bytes.
    fn get(&self, url: &str, into: &mut dyn Write, limit: u64) -> Result<(), String>;
}

/// The real network.
pub struct Web;

impl Http for Web {
    fn get(&self, url: &str, into: &mut dyn Write, limit: u64) -> Result<(), String> {
        let agent: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(Duration::from_secs(600)))
            .timeout_connect(Some(Duration::from_secs(15)))
            .user_agent(format!("gitclient/{}", Version::current()))
            .build()
            .into();
        let mut response = agent
            .get(url)
            .header(
                "Accept",
                "application/vnd.github+json, application/octet-stream",
            )
            .call()
            .map_err(describe)?;
        let mut body = response.body_mut().as_reader();
        let mut seen = 0u64;
        let mut chunk = [0u8; 64 * 1024];
        loop {
            let read = body
                .read(&mut chunk)
                .map_err(|error| format!("The download broke off: {error}"))?;
            if read == 0 {
                return Ok(());
            }
            seen += read as u64;
            if seen > limit {
                return Err("The download is larger than expected, so it was stopped.".into());
            }
            into.write_all(&chunk[..read])
                .map_err(|error| format!("Couldn't save the download: {error}"))?;
        }
    }
}

fn describe(error: ureq::Error) -> String {
    match error {
        ureq::Error::StatusCode(404) => "GitHub has no published release to update to.".to_owned(),
        ureq::Error::StatusCode(403 | 429) => {
            "GitHub is limiting requests from this network. Try again later.".to_owned()
        }
        ureq::Error::StatusCode(code) => format!("GitHub answered with an error ({code})."),
        other => format!("Couldn't reach GitHub: {other}."),
    }
}

// ---- looking ----

/// Asks GitHub for the latest release, and returns it if it is newer than `current`.
pub fn check(
    http: &dyn Http,
    current: Version,
    target: Target,
) -> Result<Option<Available>, String> {
    let url = format!("https://api.github.com/repos/{REPOSITORY}/releases/latest");
    let mut body = Vec::new();
    http.get(&url, &mut body, MAX_TEXT)?;
    read_release(&body, current, target)
}

/// What GitHub's answer says about the latest release.
fn read_release(
    json: &[u8],
    current: Version,
    target: Target,
) -> Result<Option<Available>, String> {
    let release: serde_json::Value = serde_json::from_slice(json)
        .map_err(|_| "GitHub's answer wasn't what gitclient expected.".to_owned())?;
    let flag = |key: &str| release[key].as_bool().unwrap_or(false);
    if flag("draft") || flag("prerelease") {
        return Ok(None);
    }
    let tag = release["tag_name"].as_str().unwrap_or_default();
    let version = Version::parse(tag)
        .ok_or_else(|| format!("The latest release is called {tag:?}, which isn't a version."))?;
    if version <= current {
        return Ok(None);
    }
    let asset = |name: &str| -> Result<Asset, String> {
        let found = release["assets"].as_array().and_then(|assets| {
            assets
                .iter()
                .find(|asset| asset["name"].as_str() == Some(name))
        });
        let url = found
            .and_then(|asset| asset["browser_download_url"].as_str())
            .ok_or_else(|| format!("Release {tag} has no {name}."))?;
        if !trusted(url) {
            return Err(format!(
                "Release {tag} points {name} somewhere gitclient won't download from."
            ));
        }
        Ok(Asset {
            name: name.to_owned(),
            url: url.to_owned(),
        })
    };
    Ok(Some(Available {
        version,
        tag: tag.to_owned(),
        archive: asset(&target.archive_name(tag))?,
        checksums: asset("SHA256SUMS.txt")?,
        target,
    }))
}

/// Downloads only over HTTPS from GitHub, whatever a release says.
fn trusted(url: &str) -> bool {
    let Some(rest) = url.strip_prefix("https://") else {
        return false;
    };
    let host = rest.split(['/', '?', '#']).next().unwrap_or_default();
    host == "github.com"
        || host.ends_with(".github.com")
        || host.ends_with(".githubusercontent.com")
}

// ---- installing ----

/// Installs `available` over the program at `exe`. The new version is ready the next time
/// the app starts.
pub fn install(http: &dyn Http, available: &Available, exe: &Path) -> Result<(), String> {
    let folder = exe
        .parent()
        .ok_or("gitclient doesn't know which folder it is in.")?;
    // Staged beside the program, so the swap is a rename on one disk. If the folder can't be
    // written to, an update can't be installed there, which is better said before downloading.
    let staging = tempfile::Builder::new()
        .prefix(".gitclient-update-")
        .tempdir_in(folder)
        .map_err(|error| {
            format!(
                "gitclient's folder ({}) can't be written to, so it can't update itself: {error}. \
                 Download {} from {} instead.",
                folder.display(),
                available.version,
                available.page()
            )
        })?;

    let archive = staging.path().join(&available.archive.name);
    let mut file = File::create(&archive).map_err(|error| error.to_string())?;
    http.get(&available.archive.url, &mut file, MAX_ARCHIVE)?;
    drop(file);

    let mut sums = Vec::new();
    http.get(&available.checksums.url, &mut sums, MAX_TEXT)?;
    let expected = checksum_for(&String::from_utf8_lossy(&sums), &available.archive.name)?;
    if sha256(&archive)? != expected {
        return Err(
            "The download doesn't match its checksum, so nothing was installed.".to_owned(),
        );
    }

    let program = staging.path().join(available.target.program);
    extract(&archive, available.target, &program)?;
    make_executable(&program, exe)?;
    swap(&program, exe, cfg!(windows))
        .map_err(|error| format!("Couldn't replace {}: {error}", exe.display()))
}

/// The checksum a `sha256sum`-style file gives for `name`.
fn checksum_for(sums: &str, name: &str) -> Result<String, String> {
    sums.lines()
        .filter_map(|line| {
            let (sum, file) = line.split_once(char::is_whitespace)?;
            // `sha256sum` marks a binary file with a `*` before its name.
            let file = file.trim().trim_start_matches('*');
            (file == name).then(|| sum.to_lowercase())
        })
        .next()
        .ok_or_else(|| format!("SHA256SUMS.txt has no checksum for {name}."))
}

fn sha256(file: &Path) -> Result<String, String> {
    let mut input = File::open(file).map_err(|error| error.to_string())?;
    let mut hash = Sha256::new();
    let mut chunk = [0u8; 64 * 1024];
    loop {
        let read = input.read(&mut chunk).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        hash.update(&chunk[..read]);
    }
    Ok(hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

/// Writes the program found in `archive` to `to`. Only the entry named for the program is
/// read, wherever it sits, and it is written to a path chosen here, so nothing in an archive
/// can place a file anywhere else.
fn extract(archive: &Path, target: Target, to: &Path) -> Result<(), String> {
    let missing = || format!("The download has no {} inside.", target.program);
    let fail = |error: std::io::Error| format!("Couldn't unpack the download: {error}");
    let mut out = File::create(to).map_err(fail)?;
    let input = File::open(archive).map_err(fail)?;
    if target.zip {
        let mut zip = zip::ZipArchive::new(input)
            .map_err(|error| format!("Couldn't unpack the download: {error}"))?;
        for index in 0..zip.len() {
            let entry = zip
                .by_index(index)
                .map_err(|error| format!("Couldn't unpack the download: {error}"))?;
            let named = Path::new(entry.name()).file_name().and_then(|n| n.to_str());
            if entry.is_file() && named == Some(target.program) {
                std::io::copy(&mut entry.take(MAX_PROGRAM), &mut out).map_err(fail)?;
                return Ok(());
            }
        }
    } else {
        let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(input));
        for entry in tar.entries().map_err(fail)? {
            let entry = entry.map_err(fail)?;
            let named = entry
                .path()
                .ok()
                .and_then(|path| path.file_name().map(|name| name.to_os_string()));
            if entry.header().entry_type().is_file()
                && named.as_deref() == Some(std::ffi::OsStr::new(target.program))
            {
                std::io::copy(&mut entry.take(MAX_PROGRAM), &mut out).map_err(fail)?;
                return Ok(());
            }
        }
    }
    Err(missing())
}

/// Gives the new program the permissions of the one it replaces, and makes sure it can run.
#[cfg(unix)]
fn make_executable(program: &Path, like: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let mode = std::fs::metadata(like)
        .map(|meta| meta.permissions().mode())
        .unwrap_or(0o755);
    std::fs::set_permissions(program, std::fs::Permissions::from_mode(mode | 0o111))
        .map_err(|error| format!("Couldn't make the new version runnable: {error}"))
}

#[cfg(not(unix))]
fn make_executable(_: &Path, _: &Path) -> Result<(), String> {
    Ok(())
}

/// Puts `new` where `exe` is. Unix lets a running program's file be replaced by a rename.
/// Windows doesn't, but lets it be renamed, so the old one steps aside first and is cleaned
/// up by the next start ([`clean_up`]). If the second step fails, the first is undone.
fn swap(new: &Path, exe: &Path, step_aside: bool) -> std::io::Result<()> {
    if !step_aside {
        return std::fs::rename(new, exe);
    }
    let old = old_copy(exe);
    let _ = std::fs::remove_file(&old);
    std::fs::rename(exe, &old)?;
    std::fs::rename(new, exe).inspect_err(|_| {
        let _ = std::fs::rename(&old, exe);
    })
}

/// Where Windows keeps the replaced program until the next start.
fn old_copy(exe: &Path) -> PathBuf {
    exe.with_extension("old")
}

/// Removes what the last update left behind. Call it when the app starts.
pub fn clean_up(exe: &Path) {
    if cfg!(windows) {
        let _ = std::fs::remove_file(old_copy(exe));
    }
}

/// Starts the program at `exe` again with the arguments this run was given, so the user is
/// in the new version. The caller quits afterwards.
pub fn restart(exe: &Path) -> Result<(), String> {
    std::process::Command::new(exe)
        .args(std::env::args_os().skip(1))
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Couldn't start {}: {error}", exe.display()))
}

// ---- what to say ----

/// Where an update stands, and the words for it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Status {
    /// Nothing has been looked for yet.
    Idle,
    Checking,
    UpToDate,
    Available(Available),
    Installing(Available),
    /// Installed. The new version starts the next time the app does.
    Ready {
        version: Version,
        back: String,
    },
    CheckFailed(String),
    InstallFailed(Available, String),
}

/// A line over the window that offers the next step.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Band {
    pub message: String,
    /// The button, named for its result, if there is a next step.
    pub action: Option<String>,
    /// Peach rather than plain text: something needs a decision or went wrong.
    pub warning: bool,
}

impl Status {
    /// What the band over the window says, if anything is worth interrupting for. Looking
    /// and finding nothing, or failing to look, is only for the Settings page.
    pub fn band(&self) -> Option<Band> {
        let (message, action, warning) = match self {
            Self::Available(found) => (
                format!("gitclient {} is available.", found.version),
                Some(format!("Update to {}", found.version)),
                false,
            ),
            Self::Installing(found) => (
                format!("Downloading gitclient {}…", found.version),
                None,
                false,
            ),
            Self::Ready { version, back } => (
                format!(
                    "gitclient {version} is installed and starts the next time you open it. {back}"
                ),
                Some("Restart now".to_owned()),
                false,
            ),
            Self::InstallFailed(_, error) => (error.clone(), Some("Try again".to_owned()), true),
            Self::Idle | Self::Checking | Self::UpToDate | Self::CheckFailed(_) => return None,
        };
        Some(Band {
            message,
            action,
            warning,
        })
    }

    /// The sentence under "Check for updates" on the Settings page.
    pub fn sentence(&self, current: Version) -> String {
        match self {
            Self::Idle => format!("This is gitclient {current}."),
            Self::Checking => "Looking for a newer version…".to_owned(),
            Self::UpToDate => format!("gitclient {current} is the newest version."),
            Self::Available(found) => {
                format!(
                    "gitclient {} is available. This is {current}.",
                    found.version
                )
            }
            Self::Installing(found) => format!("Downloading gitclient {}…", found.version),
            Self::Ready { version, back } => {
                format!(
                    "gitclient {version} is installed and starts the next time you open it. {back}"
                )
            }
            Self::CheckFailed(error) => format!("Couldn't look for a newer version. {error}"),
            Self::InstallFailed(_, error) => error.clone(),
        }
    }

    /// The Settings button that acts on this status, named for its result.
    pub fn button(&self) -> Option<String> {
        match self {
            Self::Available(found) | Self::InstallFailed(found, _) => {
                Some(format!("Update to {}", found.version))
            }
            Self::Ready { .. } => Some("Restart now".to_owned()),
            Self::Idle | Self::UpToDate | Self::CheckFailed(_) => Some("Check now".to_owned()),
            Self::Checking | Self::Installing(_) => None,
        }
    }

    /// The status once `found` has been installed, with how to go back.
    pub fn installed(found: &Available, current: Version) -> Self {
        Self::Ready {
            version: found.version,
            back: format!("To go back, download {current} from {}.", old_page(current)),
        }
    }
}

fn old_page(version: Version) -> String {
    format!("github.com/{REPOSITORY}/releases/tag/v{version}")
}

/// The sentence under "Check for updates when gitclient starts".
pub fn check_sentence(on: bool) -> &'static str {
    if on {
        "gitclient looks for a newer version when it starts, and asks before installing it."
    } else {
        "gitclient only looks for a newer version when you press Check now."
    }
}

/// What the header says after that switch is flipped.
pub fn check_saved_message(on: bool) -> &'static str {
    if on {
        "Saved. gitclient will look for a newer version when it starts."
    } else {
        "Saved. gitclient won't look for newer versions by itself."
    }
}

#[cfg(test)]
mod tests;
