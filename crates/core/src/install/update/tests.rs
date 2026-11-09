use super::*;
use std::collections::HashMap;
use std::io::Cursor;

fn v(text: &str) -> Version {
    Version::parse(text).unwrap()
}

fn linux() -> Target {
    Target::of("linux", "x86_64").unwrap()
}

fn windows() -> Target {
    Target::of("windows", "x86_64").unwrap()
}

/// The network, as a table of what each URL answers. A URL not in it is a failure.
#[derive(Default)]
struct Fake {
    answers: HashMap<String, Vec<u8>>,
}

impl Fake {
    fn with(mut self, url: &str, body: impl Into<Vec<u8>>) -> Self {
        self.answers.insert(url.to_owned(), body.into());
        self
    }
}

impl Http for Fake {
    fn get(&self, url: &str, into: &mut dyn Write, limit: u64) -> Result<(), String> {
        let body = self
            .answers
            .get(url)
            .ok_or(format!("no answer for {url}"))?;
        if body.len() as u64 > limit {
            return Err("too large".to_owned());
        }
        into.write_all(body).map_err(|error| error.to_string())
    }
}

fn releases() -> String {
    HOST.releases_url()
}

fn stable(http: &dyn Http, current: &str, target: Target) -> Result<Option<Available>, String> {
    check(http, &HOST, Track::Stable, &Build::of(v(current)), target)
}

/// What `read_stable` makes of one release, as the host lists it.
fn read_release(
    json: &[u8],
    current: Version,
    target: Target,
) -> Result<Option<Available>, String> {
    let list = [b"[", json, b"]"].concat();
    read_stable(&list, &HOST, &Build::of(current), target)
}

fn nightly_json(commit: &str, target: Target) -> String {
    let archive = target.archive_name("nightly");
    format!(
        r#"{{"tag_name":"nightly","draft":false,"prerelease":true,
            "body":"Built from {commit}. Replaced on every push to main.",
            "assets":[
            {{"name":"{archive}","browser_download_url":"{}"}},
            {{"name":"SHA256SUMS.txt","browser_download_url":"{}"}}]}}"#,
        download(&archive),
        download("SHA256SUMS.txt"),
    )
}

fn build_at(commit: Option<&str>) -> Build {
    Build {
        version: v("0.1.0"),
        commit: commit.map(str::to_owned),
    }
}

const COMMIT_A: &str = "1a2b3c4d5e6f708192a3b4c5d6e7f8091a2b3c4d";
const COMMIT_B: &str = "ffffffffffffffffffffffffffffffffffffffff";

fn download(name: &str) -> String {
    format!("https://git.example.com/example/gitclient/releases/download/v0.2.0/{name}")
}

fn release_json(target: Target, tag: &str) -> String {
    let archive = target.archive_name(tag);
    format!(
        r#"[{{"tag_name":"{tag}","draft":false,"prerelease":false,"assets":[
            {{"name":"{archive}","browser_download_url":"{}"}},
            {{"name":"SHA256SUMS.txt","browser_download_url":"{}"}}]}}]"#,
        download(&archive),
        download("SHA256SUMS.txt"),
    )
}

fn tar_gz(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
        Vec::new(),
        flate2::Compression::default(),
    ));
    for (name, data) in files {
        let mut header = tar::Header::new_gnu();
        header.set_size(data.len() as u64);
        header.set_mode(0o644);
        header.set_cksum();
        builder.append_data(&mut header, name, *data).unwrap();
    }
    builder.into_inner().unwrap().finish().unwrap()
}

fn zipped(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, data) in files {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(data).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn sum_of(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// A release for `target` holding `archive`, with a checksums file that says `sums`.
fn release(target: Target, archive: &[u8], sums: Option<String>) -> (Available, Fake) {
    let name = target.archive_name("v0.2.0");
    let sums = sums.unwrap_or_else(|| format!("{}  {name}\n", sum_of(archive)));
    let http = Fake::default()
        .with(&releases(), release_json(target, "v0.2.0"))
        .with(&download(&name), archive)
        .with(&download("SHA256SUMS.txt"), sums);
    let found = stable(&http, "0.1.0", target).unwrap().unwrap();
    (found, http)
}

/// A folder holding an old program, which an update replaces.
fn old_program() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("gitclient");
    std::fs::write(&exe, "old").unwrap();
    (dir, exe)
}

fn names_in(dir: &Path) -> Vec<String> {
    let mut names: Vec<String> = std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

#[test]
fn versions_read_with_or_without_a_v_and_compare_by_number() {
    assert_eq!(v("1.2.3"), v("v1.2.3"));
    assert_eq!(v("v0.10.0").to_string(), "0.10.0");
    // By number, not by text: 0.10.0 is newer than 0.9.0.
    assert!(v("0.10.0") > v("0.9.0"));
    assert!(v("1.0.0") > v("0.99.99"));
    assert_eq!(v("1.2.3-rc.1"), v("1.2.3"));
    for bad in ["", "1.2", "1.2.3.4", "one.two.three", "v"] {
        assert_eq!(Version::parse(bad), None, "{bad:?}");
    }
    // The core crate's version is the app's, which release-please keeps in step.
    assert_eq!(Version::current().to_string(), env!("CARGO_PKG_VERSION"));
}

#[test]
fn each_system_has_the_archive_the_release_workflow_names() {
    assert_eq!(
        linux().archive_name("v0.2.0"),
        "gitclient-v0.2.0-x86_64-unknown-linux-gnu.tar.gz"
    );
    assert_eq!(
        windows().archive_name("v0.2.0"),
        "gitclient-v0.2.0-x86_64-pc-windows-msvc.zip"
    );
    assert_eq!(
        Target::of("macos", "aarch64")
            .unwrap()
            .archive_name("v1.0.0"),
        "gitclient-v1.0.0-aarch64-apple-darwin.tar.gz"
    );
    assert_eq!(Target::of("linux", "riscv64"), None);
}

#[test]
fn a_newer_release_is_found_with_its_archive_and_checksums() {
    let (found, _) = release(linux(), b"unused", None);
    assert_eq!(found.name, "0.2.0");
    assert_eq!(found.tag, "v0.2.0");
    assert_eq!(
        found.archive.name,
        "gitclient-v0.2.0-x86_64-unknown-linux-gnu.tar.gz"
    );
    assert_eq!(found.checksums.name, "SHA256SUMS.txt");
    assert_eq!(
        found.page,
        "git.example.com/example/gitclient/releases/tag/v0.2.0"
    );
    assert_eq!(
        found.back,
        "To go back, download 0.1.0 from git.example.com/example/gitclient/releases/tag/v0.1.0."
    );
}

#[test]
fn the_same_or_an_older_release_is_not_an_update() {
    let http = Fake::default().with(&releases(), release_json(linux(), "v0.2.0"));
    assert_eq!(stable(&http, "0.2.0", linux()), Ok(None));
    assert_eq!(stable(&http, "0.3.0", linux()), Ok(None));
}

#[test]
fn drafts_and_suffixed_tags_are_not_offered() {
    let json = br#"{"tag_name":"v9.0.0","draft":true,"assets":[]}"#;
    assert_eq!(read_release(json, v("0.1.0"), linux()), Ok(None));
    for tag in ["nightly", "v9.0.0-rc1", "v9.0.0+build"] {
        let json = format!(r#"{{"tag_name":"{tag}","assets":[]}}"#);
        assert_eq!(read_release(json.as_bytes(), v("0.1.0"), linux()), Ok(None));
    }
}

#[test]
fn a_tagged_release_counts_even_when_the_host_flags_it_as_a_pre_release() {
    // The Gitea workflow publishes tagged releases with the flag set.
    let (found, _) = release(linux(), b"unused", None);
    let json =
        release_json(linux(), "v0.2.0").replace(r#""prerelease":false"#, r#""prerelease":true"#);
    let http = Fake::default().with(&releases(), json);
    assert_eq!(stable(&http, "0.1.0", linux()), Ok(Some(found)));
}

#[test]
fn the_newest_full_release_wins_whatever_order_the_host_lists_them_in() {
    let one = |tag: &str| {
        let json = release_json(linux(), tag);
        json[1..json.len() - 1].to_owned()
    };
    let json = format!(
        r#"[{{"tag_name":"nightly","assets":[]}},{},{}]"#,
        one("v0.9.0"),
        one("v0.10.0")
    );
    let found = read_stable(json.as_bytes(), &HOST, &Build::of(v("0.1.0")), linux())
        .unwrap()
        .unwrap();
    assert_eq!(found.name, "0.10.0");
}

#[test]
fn a_nightly_built_from_another_commit_is_offered_and_the_same_one_is_not() {
    let http = Fake::default().with(&HOST.tag_url("nightly"), nightly_json(COMMIT_A, linux()));
    let ask = |commit| check(&http, &HOST, Track::Nightly, &build_at(commit), linux());

    let found = ask(Some(COMMIT_B)).unwrap().unwrap();
    assert_eq!(found.name, "nightly 1a2b3c4");
    assert_eq!(found.tag, "nightly");
    assert_eq!(
        found.archive.name,
        "gitclient-nightly-x86_64-unknown-linux-gnu.tar.gz"
    );
    assert_eq!(ask(Some(COMMIT_A)), Ok(None), "this is that build");
    let upper = COMMIT_A.to_uppercase();
    assert_eq!(ask(Some(&upper)), Ok(None));
    // A copy that doesn't know its commit is offered the nightly rather than none.
    assert!(ask(None).unwrap().is_some());
}

#[test]
fn a_nightly_installs_like_any_release() {
    let archive = tar_gz(&[("gitclient", b"nightly program")]);
    let name = linux().archive_name("nightly");
    let http = Fake::default()
        .with(&HOST.tag_url("nightly"), nightly_json(COMMIT_A, linux()))
        .with(&download(&name), archive.clone())
        .with(
            &download("SHA256SUMS.txt"),
            format!("{}  {name}\n", sum_of(&archive)),
        );
    let found = check(&http, &HOST, Track::Nightly, &build_at(None), linux())
        .unwrap()
        .unwrap();
    let (_dir, exe) = old_program();
    install(&http, &found, &exe).unwrap();
    assert_eq!(std::fs::read(&exe).unwrap(), b"nightly program");
}

#[test]
fn a_nightly_that_names_no_commit_says_so() {
    let json = nightly_json(COMMIT_A, linux()).replace(COMMIT_A, "nothing");
    let error = read_nightly(json.as_bytes(), &HOST, &build_at(None), linux()).unwrap_err();
    assert!(error.contains("which commit"), "{error}");
    // The release's own commit is the fallback.
    let json = json.replace(
        r#""draft":false"#,
        &format!(r#""draft":false,"target_commitish":"{COMMIT_A}""#),
    );
    assert!(
        read_nightly(json.as_bytes(), &HOST, &build_at(None), linux())
            .unwrap()
            .is_some()
    );
}

#[test]
fn a_draft_nightly_is_not_offered() {
    let json = nightly_json(COMMIT_A, linux()).replace(r#""draft":false"#, r#""draft":true"#);
    assert_eq!(
        read_nightly(json.as_bytes(), &HOST, &build_at(None), linux()),
        Ok(None)
    );
}

#[test]
fn the_host_decides_where_to_ask_and_what_to_trust() {
    let gitea = Host::gitea("https://git.example.org", "me/app");
    assert_eq!(
        gitea.releases_url(),
        "https://git.example.org/api/v1/repos/me/app/releases?draft=false&limit=50"
    );
    assert_eq!(
        gitea.tag_url("nightly"),
        "https://git.example.org/api/v1/repos/me/app/releases/tags/nightly"
    );
    assert_eq!(
        gitea.page("v1.0.0"),
        "git.example.org/me/app/releases/tag/v1.0.0"
    );
    assert_eq!(gitea.name(), "git.example.org");
    assert!(gitea.trusts("https://git.example.org/me/app/releases/download/v1/a.zip"));
    assert!(!gitea.trusts("https://git.example.org.evil.test/a.zip"));
    assert!(!gitea.trusts("http://git.example.org/a.zip"));
    assert!(!gitea.trusts("https://github.com/me/app/a.zip"));

    let github = Host::github("me/app");
    assert_eq!(
        github.releases_url(),
        "https://api.github.com/repos/me/app/releases?per_page=50"
    );
    assert_eq!(
        github.tag_url("nightly"),
        "https://api.github.com/repos/me/app/releases/tags/nightly"
    );
    assert_eq!(
        github.page("v1.0.0"),
        "github.com/me/app/releases/tag/v1.0.0"
    );
    assert_eq!(github.name(), "GitHub");
    assert!(github.trusts("https://github.com/o/r/releases/download/v1/a.zip"));
    assert!(github.trusts("https://objects.githubusercontent.com/x"));
    assert!(!github.trusts("https://evilgithub.com/x"));
    assert!(!github.trusts("http://github.com/x"));
}

#[test]
fn the_track_is_kept_as_a_word_and_a_mangled_one_is_the_full_releases() {
    for track in Track::ALL {
        assert_eq!(Track::from_key(track.key()), track);
    }
    assert_eq!(Track::from_key("  nightly\n"), Track::Nightly);
    assert_eq!(Track::from_key("beta"), Track::Stable);
    assert_eq!(Track::default(), Track::Stable);
}

#[test]
fn a_build_says_its_commit_only_when_it_is_a_real_one() {
    assert_eq!(Build::current(None).commit, None);
    assert_eq!(Build::current(Some("not a commit")).commit, None);
    assert_eq!(build_at(Some(COMMIT_A)).label(), "0.1.0 (1a2b3c4)");
    assert_eq!(build_at(None).label(), "0.1.0");
}

#[test]
fn a_release_without_this_systems_build_says_so_and_offers_nothing() {
    let json = r#"{"tag_name":"v0.2.0","assets":[
        {"name":"SHA256SUMS.txt","browser_download_url":"https://git.example.com/x/SHA256SUMS.txt"}]}"#;
    let error = read_release(json.as_bytes(), v("0.1.0"), linux()).unwrap_err();
    assert!(
        error.contains("no gitclient-v0.2.0-x86_64-unknown-linux-gnu.tar.gz"),
        "{error}"
    );

    let archive = linux().archive_name("v0.2.0");
    let json = format!(
        r#"{{"tag_name":"v0.2.0","assets":[{{"name":"{archive}","browser_download_url":"https://git.example.com/x/a"}}]}}"#
    );
    let error = read_release(json.as_bytes(), v("0.1.0"), linux()).unwrap_err();
    assert!(error.contains("no SHA256SUMS.txt"), "{error}");
}

#[test]
fn nonsense_from_the_network_is_an_error_and_not_a_crash() {
    assert!(read_release(b"<html>", v("0.1.0"), linux()).is_err());
    assert!(read_stable(b"{}", &HOST, &Build::of(v("0.1.0")), linux()).is_err());
    assert!(read_nightly(b"<html>", &HOST, &Build::of(v("0.1.0")), linux()).is_err());
    assert!(read_nightly(b"[]", &HOST, &Build::of(v("0.1.0")), linux()).is_err());
}

#[test]
fn only_https_downloads_from_the_host_are_followed() {
    let trusted = |url| HOST.trusts(url);
    assert!(trusted(
        "https://git.example.com/o/r/releases/download/v1/a.zip"
    ));
    assert!(!trusted("http://git.example.com/o/r/a.zip"));
    assert!(!trusted("https://github.com/o/r/a.zip"));
    assert!(!trusted("https://evil.example/git.example.com/a.zip"));
    assert!(!trusted("https://git.example.com.evil.example/a.zip"));
    assert!(!trusted("https://user@git.example.com.evil.example/a.zip"));
    assert!(!trusted("file:///etc/passwd"));

    let archive = linux().archive_name("v0.2.0");
    let json = format!(
        r#"{{"tag_name":"v0.2.0","assets":[
        {{"name":"{archive}","browser_download_url":"https://evil.example/a"}},
        {{"name":"SHA256SUMS.txt","browser_download_url":"https://github.com/s"}}]}}"#
    );
    let error = read_release(json.as_bytes(), v("0.1.0"), linux()).unwrap_err();
    assert!(error.contains("won't download from"), "{error}");
}

#[test]
fn checksums_are_read_from_sha256sum_output() {
    let sums = "AAA111  gitclient-a.tar.gz\nbbb222 *gitclient-b.zip\n\nnot a line\n";
    assert_eq!(checksum_for(sums, "gitclient-a.tar.gz").unwrap(), "aaa111");
    assert_eq!(checksum_for(sums, "gitclient-b.zip").unwrap(), "bbb222");
    assert!(checksum_for(sums, "gitclient-c.zip").is_err());
    // A name that only ends the same way is not a match.
    assert!(checksum_for("abc  xgitclient-a.tar.gz\n", "gitclient-a.tar.gz").is_err());
}

#[test]
fn an_update_replaces_the_program_from_a_tar_gz_and_leaves_nothing_behind() {
    let archive = tar_gz(&[("README", b"hi"), ("./gitclient", b"new program")]);
    let (found, http) = release(linux(), &archive, None);
    let (dir, exe) = old_program();

    install(&http, &found, &exe).unwrap();

    assert_eq!(std::fs::read(&exe).unwrap(), b"new program");
    // The staging folder and the download are gone.
    assert_eq!(names_in(dir.path()), ["gitclient"]);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&exe).unwrap().permissions().mode();
        assert_eq!(mode & 0o111, 0o111, "the new program can be run");
    }
}

#[test]
fn an_update_replaces_the_program_from_a_zip() {
    let archive = zipped(&[("LICENSE", b"mit"), ("gitclient.exe", b"new exe")]);
    let (found, http) = release(windows(), &archive, None);
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("gitclient.exe");
    std::fs::write(&exe, "old").unwrap();

    install(&http, &found, &exe).unwrap();

    assert_eq!(std::fs::read(&exe).unwrap(), b"new exe");
    assert_eq!(names_in(dir.path()), ["gitclient.exe"]);
}

#[test]
fn a_download_that_does_not_match_its_checksum_is_never_installed() {
    let archive = tar_gz(&[("gitclient", b"tampered")]);
    let name = linux().archive_name("v0.2.0");
    let wrong = format!("{}  {name}\n", sum_of(b"something else"));
    let (found, http) = release(linux(), &archive, Some(wrong));
    let (dir, exe) = old_program();

    let error = install(&http, &found, &exe).unwrap_err();

    assert!(error.contains("doesn't match its checksum"), "{error}");
    assert_eq!(std::fs::read(&exe).unwrap(), b"old");
    assert_eq!(names_in(dir.path()), ["gitclient"]);
}

#[test]
fn a_release_without_a_checksum_for_the_archive_is_never_installed() {
    let archive = tar_gz(&[("gitclient", b"new")]);
    let (found, http) = release(linux(), &archive, Some("abc  other.tar.gz\n".to_owned()));
    let (_dir, exe) = old_program();

    let error = install(&http, &found, &exe).unwrap_err();

    assert!(error.contains("has no checksum"), "{error}");
    assert_eq!(std::fs::read(&exe).unwrap(), b"old");
}

#[test]
fn an_archive_without_the_program_leaves_the_old_one_in_place() {
    let archive = tar_gz(&[("README", b"nothing to run")]);
    let (found, http) = release(linux(), &archive, None);
    let (dir, exe) = old_program();

    let error = install(&http, &found, &exe).unwrap_err();

    assert!(error.contains("no gitclient inside"), "{error}");
    assert_eq!(std::fs::read(&exe).unwrap(), b"old");
    assert_eq!(names_in(dir.path()), ["gitclient"]);
}

#[test]
fn a_failed_download_leaves_the_old_one_in_place() {
    let archive = tar_gz(&[("gitclient", b"new")]);
    let (found, _) = release(linux(), &archive, None);
    let (dir, exe) = old_program();

    let error = install(&Fake::default(), &found, &exe).unwrap_err();

    assert!(error.contains("no answer"), "{error}");
    assert_eq!(std::fs::read(&exe).unwrap(), b"old");
    assert_eq!(names_in(dir.path()), ["gitclient"]);
}

#[test]
fn a_folder_that_cannot_be_written_to_says_where_to_download_instead() {
    let archive = tar_gz(&[("gitclient", b"new")]);
    let (found, http) = release(linux(), &archive, None);
    let missing = Path::new("/nonexistent-gitclient-folder/gitclient");

    let error = install(&http, &found, missing).unwrap_err();

    assert!(error.contains("can't update itself"), "{error}");
    assert!(
        error.contains("git.example.com/example/gitclient/releases/tag/v0.2.0"),
        "{error}"
    );
}

#[test]
fn swapping_on_unix_renames_over_the_program() {
    let dir = tempfile::tempdir().unwrap();
    let (exe, new) = (dir.path().join("gitclient"), dir.path().join("next"));
    std::fs::write(&exe, "old").unwrap();
    std::fs::write(&new, "new").unwrap();

    swap(&new, &exe, false).unwrap();

    assert_eq!(std::fs::read(&exe).unwrap(), b"new");
    assert_eq!(names_in(dir.path()), ["gitclient"]);
}

#[test]
fn swapping_on_windows_steps_the_old_program_aside_and_the_next_start_removes_it() {
    let dir = tempfile::tempdir().unwrap();
    let (exe, new) = (dir.path().join("gitclient.exe"), dir.path().join("next"));
    std::fs::write(&exe, "old").unwrap();
    std::fs::write(&new, "new").unwrap();
    // A leftover from an earlier update must not stand in the way.
    std::fs::write(dir.path().join("gitclient.old"), "older").unwrap();

    swap(&new, &exe, true).unwrap();

    assert_eq!(std::fs::read(&exe).unwrap(), b"new");
    assert_eq!(std::fs::read(old_copy(&exe)).unwrap(), b"old");
    assert_eq!(old_copy(&exe), dir.path().join("gitclient.old"));
}

#[test]
fn a_swap_that_fails_halfway_puts_the_old_program_back() {
    let dir = tempfile::tempdir().unwrap();
    let exe = dir.path().join("gitclient.exe");
    std::fs::write(&exe, "old").unwrap();

    // The new file isn't there, so the second step fails.
    assert!(swap(&dir.path().join("missing"), &exe, true).is_err());

    assert_eq!(std::fs::read(&exe).unwrap(), b"old");
    assert!(!old_copy(&exe).exists());
}

#[test]
fn packaged_copies_and_unknown_systems_cannot_update_and_say_why() {
    let why = |channel, target| match support(channel, target) {
        Support::No(why) => why,
        Support::Yes(_) => "yes".to_owned(),
    };
    assert_eq!(
        why(Channel::Packaged { by: "Flatpak" }, Some(linux())),
        "gitclient was installed with Flatpak, which updates it."
    );
    assert!(why(Channel::Portable, None).contains("no build of gitclient"));
    assert_eq!(why(Channel::Portable, Some(linux())), "yes");
    // A package manager wins even where there is a build.
    assert_ne!(why(Channel::Packaged { by: "Nix" }, Some(linux())), "yes");
}

#[test]
fn the_band_offers_the_next_step_and_stays_quiet_otherwise() {
    let (found, _) = release(linux(), b"x", None);
    let band = Status::Available(found.clone()).band().unwrap();
    assert_eq!(band.message, "gitclient 0.2.0 is available.");
    assert_eq!(band.action.as_deref(), Some("Update to 0.2.0"));
    assert!(!band.warning);

    let band = Status::Installing(found.clone()).band().unwrap();
    assert_eq!(band.action, None);

    let ready = Status::installed(&found);
    let band = ready.band().unwrap();
    assert!(band.message.starts_with("gitclient 0.2.0 is installed"));
    // It says how to undo the step.
    assert!(band.message.contains(
        "To go back, download 0.1.0 from git.example.com/example/gitclient/releases/tag/v0.1.0."
    ));
    assert_eq!(band.action.as_deref(), Some("Restart now"));

    let band = Status::InstallFailed(found, "No room.".into())
        .band()
        .unwrap();
    assert_eq!((band.message.as_str(), band.warning), ("No room.", true));
    assert_eq!(band.action.as_deref(), Some("Try again"));

    for quiet in [
        Status::Idle,
        Status::Checking,
        Status::UpToDate,
        Status::CheckFailed("offline".into()),
    ] {
        assert_eq!(quiet.band(), None, "{quiet:?}");
    }
}

#[test]
fn the_settings_sentence_and_button_follow_the_status() {
    let current = &build_at(None);
    let stable = Track::Stable;
    let (found, _) = release(linux(), b"x", None);
    assert_eq!(
        Status::Idle.sentence(current, stable),
        "This is gitclient 0.1.0."
    );
    assert_eq!(Status::Idle.button().as_deref(), Some("Check now"));
    assert_eq!(
        Status::UpToDate.sentence(current, stable),
        "gitclient 0.1.0 is the newest version."
    );
    assert_eq!(
        Status::UpToDate.sentence(&build_at(Some(COMMIT_A)), Track::Nightly),
        "gitclient 0.1.0 (1a2b3c4) is the newest nightly build."
    );
    assert_eq!(
        Status::Available(found.clone()).sentence(current, stable),
        "gitclient 0.2.0 is available. This is 0.1.0."
    );
    assert_eq!(
        Status::Available(found.clone()).button().as_deref(),
        Some("Update to 0.2.0")
    );
    assert_eq!(Status::Checking.button(), None);
    assert_eq!(Status::Installing(found).button(), None);
    assert_eq!(
        Status::CheckFailed("The host is limiting requests.".into()).sentence(current, stable),
        "Couldn't look for a newer version. The host is limiting requests."
    );
    // After a failed look the user can look again.
    assert_eq!(
        Status::CheckFailed("x".into()).button().as_deref(),
        Some("Check now")
    );
    assert_eq!(
        check_saved_message(true),
        "Saved. gitclient will look for a newer version when it starts."
    );
    assert!(check_sentence(false).contains("Check now"));
}

/// The real network, which the tests above stand in for. Run with
/// `cargo test -p gitclient-core -- --ignored web_`.
#[test]
#[ignore = "needs the network"]
fn web_reads_a_real_release_and_stops_a_body_that_is_too_long() {
    let url = "https://api.github.com/repos/rust-lang/rust/releases/latest";
    let mut body = Vec::new();
    Web.get(url, &mut body, MAX_TEXT).unwrap();
    let release: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert!(Version::parse(release["tag_name"].as_str().unwrap()).is_some());

    let mut small = Vec::new();
    let error = Web.get(url, &mut small, 100).unwrap_err();
    assert!(error.contains("larger than expected"), "{error}");
    assert!(small.len() <= 100 + 64 * 1024);
}

#[test]
#[ignore = "needs the network"]
fn web_says_what_it_found_for_this_project() {
    let target = Target::of("linux", "x86_64").unwrap();
    let result = check(&Web, &HOST, Track::Stable, &Build::current(None), target);
    eprintln!("this project's latest release: {result:?}");
    // Either there is nothing newer, a release to offer, or a plain-words reason.
    if let Err(error) = result {
        assert!(!error.is_empty());
    }
}
