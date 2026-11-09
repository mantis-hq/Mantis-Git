// Windows takes the window, taskbar and file icon from the program's own resources, and gpui
// asks for the one with ID 1. Without this the window has a blank icon.
fn main() {
    // The commit this is built from. Nightly builds share one version number, so the updater
    // tells them apart by it. CI says it in GITHUB_SHA (Gitea sets it too); a local build asks git.
    println!("cargo:rerun-if-env-changed=GITCLIENT_COMMIT");
    println!("cargo:rerun-if-env-changed=GITHUB_SHA");
    println!("cargo:rerun-if-changed=.git/HEAD");
    let commit = ["GITCLIENT_COMMIT", "GITHUB_SHA"]
        .iter()
        .find_map(|key| std::env::var(key).ok())
        .or_else(|| {
            let out = std::process::Command::new("git")
                .args(["rev-parse", "HEAD"])
                .output()
                .ok()?;
            out.status
                .success()
                .then(|| String::from_utf8_lossy(&out.stdout).trim().to_owned())
        });
    if let Some(commit) = commit.filter(|commit| !commit.is_empty()) {
        println!("cargo:rustc-env=GITCLIENT_COMMIT={commit}");
    }

    println!("cargo:rerun-if-changed=assets/gitclient.rc");
    println!("cargo:rerun-if-changed=assets/gitclient.ico");
    #[cfg(windows)]
    embed_resource::compile("assets/gitclient.rc", embed_resource::NONE)
        .manifest_optional()
        .unwrap();
}
