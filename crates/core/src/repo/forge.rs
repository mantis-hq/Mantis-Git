//! Where a repository is hosted, when that is GitHub, GitLab or Gitea: the page that opens
//! a pull or merge request, and the requests already there.
//!
//! This uses git alone. Hosts publish each request as a ref (`refs/pull/42/head`,
//! `refs/merge-requests/42/head`), so `git ls-remote` lists them with no sign-in and no API.
//! What git can't say is a request's title or whether it is still open, so the list is the
//! newest few and says so.

use super::remote::Remote;
use crate::ops;
use crate::text::{count, short_oid};
use std::collections::HashMap;
use std::path::Path;

/// The requests listed. A busy repository has thousands, and the newest are the ones wanted.
pub const SHOWN: usize = 30;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Forge {
    GitHub,
    GitLab,
    Gitea,
}

impl Forge {
    /// What the host calls it.
    pub fn name(self) -> &'static str {
        match self {
            Self::GitHub => "GitHub",
            Self::GitLab => "GitLab",
            Self::Gitea => "Gitea",
        }
    }

    /// What the host calls a request to bring a branch in, lowercase.
    pub fn noun(self) -> &'static str {
        match self {
            Self::GitLab => "merge request",
            Self::GitHub | Self::Gitea => "pull request",
        }
    }

    /// The sidebar's heading.
    pub fn heading(self) -> &'static str {
        match self {
            Self::GitLab => "Merge requests",
            Self::GitHub | Self::Gitea => "Pull requests",
        }
    }

    /// The marker before a number: GitLab writes `!42`, the others `#42`.
    pub fn marker(self) -> char {
        match self {
            Self::GitLab => '!',
            Self::GitHub | Self::Gitea => '#',
        }
    }

    /// Which refs of the remote hold the requests' tips.
    fn ref_pattern(self) -> &'static str {
        match self {
            Self::GitLab => "refs/merge-requests/*/head",
            Self::GitHub | Self::Gitea => "refs/pull/*/head",
        }
    }

    /// The forge a host is, by its name. A host that doesn't say stays unknown.
    fn of_host(host: &str) -> Option<Self> {
        let host = host.split(':').next().unwrap_or(host);
        let starts = |prefix: &str| host.starts_with(prefix);
        if host == "github.com" || starts("github.") {
            Some(Self::GitHub)
        } else if host == "gitlab.com" || starts("gitlab.") {
            Some(Self::GitLab)
        } else if matches!(host, "gitea.com" | "codeberg.org")
            || starts("gitea.")
            || starts("forgejo.")
        {
            Some(Self::Gitea)
        } else {
            None
        }
    }
}

/// A repository on a forge's website.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Site {
    pub forge: Forge,
    /// `http` or `https`, which is how the website is reached.
    scheme: &'static str,
    /// The host, with its port when the address names one for the web.
    host: String,
    /// `owner/repo`, or `group/subgroup/repo`.
    path: String,
}

impl Site {
    /// The repository a remote is, or `None` when its address isn't on a known forge.
    pub fn of(remote: &Remote) -> Option<Self> {
        let (scheme, host, path) = split_address(&remote.url)?;
        Some(Self {
            forge: Forge::of_host(&host)?,
            scheme,
            host,
            path,
        })
    }

    /// The repository's home page.
    pub fn web(&self) -> String {
        format!("{}://{}/{}", self.scheme, self.host, self.path)
    }

    /// The page that starts a request to bring `head` into `base`, both branches of this
    /// repository.
    pub fn new_request_url(&self, base: &str, head: &str) -> String {
        let web = self.web();
        match self.forge {
            Forge::GitHub => format!(
                "{web}/compare/{}...{}?expand=1",
                encode(base, true),
                encode(head, true)
            ),
            Forge::GitLab => format!(
                "{web}/-/merge_requests/new?merge_request%5Bsource_branch%5D={}\
                 &merge_request%5Btarget_branch%5D={}",
                encode(head, false),
                encode(base, false)
            ),
            // Gitea's compare page has the button that opens it.
            Forge::Gitea => format!(
                "{web}/compare/{}...{}",
                encode(base, true),
                encode(head, true)
            ),
        }
    }

    /// The page of request `number`.
    pub fn request_url(&self, number: u64) -> String {
        let web = self.web();
        match self.forge {
            Forge::GitHub => format!("{web}/pull/{number}"),
            Forge::GitLab => format!("{web}/-/merge_requests/{number}"),
            Forge::Gitea => format!("{web}/pulls/{number}"),
        }
    }

    /// How a request is written in a sentence: `#42` or `!42`.
    pub fn label(&self, number: u64) -> String {
        format!("{}{number}", self.forge.marker())
    }
}

/// The remote whose requests to show: `origin` when it is on a known forge, else the first
/// that is.
pub fn pick(remotes: &[Remote]) -> Option<(&Remote, Site)> {
    let origin = remotes.iter().filter(|remote| remote.name == "origin");
    origin
        .chain(remotes.iter())
        .find_map(|remote| Some((remote, Site::of(remote)?)))
}

/// Splits a remote's address into how the website is reached, its host and the repository's
/// path on it. Local paths and addresses with no `owner/repo` give `None`.
fn split_address(url: &str) -> Option<(&'static str, String, String)> {
    let (scheme, authority, path) = match url.split_once("://") {
        Some((scheme, rest)) => {
            let (authority, path) = rest.split_once('/')?;
            (scheme.to_ascii_lowercase(), authority, path)
        }
        // `git@host:owner/repo.git`, where the colon starts the path.
        None => {
            let (authority, path) = url.split_once(':')?;
            if authority.len() < 2 || authority.contains('/') || path.starts_with('/') {
                return None;
            }
            ("ssh".to_owned(), authority, path)
        }
    };
    let web_scheme = match scheme.as_str() {
        "http" => "http",
        "https" | "ssh" | "git" | "git+ssh" | "ssh+git" => "https",
        _ => return None,
    };
    let host = authority.rsplit('@').next()?.to_ascii_lowercase();
    // A port on an ssh address is ssh's, and the website lives on the usual one.
    let host = if matches!(scheme.as_str(), "http" | "https") {
        host
    } else {
        host.split(':').next()?.to_owned()
    };
    let path = path.trim_matches('/');
    let path = path.strip_suffix(".git").unwrap_or(path);
    if host.is_empty() || !path.contains('/') || path.split('/').any(str::is_empty) {
        return None;
    }
    Some((web_scheme, host, path.to_owned()))
}

/// Percent-encodes what a URL can't carry as it is. A branch name keeps its slashes in a
/// path, where they separate nothing.
fn encode(text: &str, keep_slashes: bool) -> String {
    let mut out = String::with_capacity(text.len());
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            b'/' if keep_slashes => out.push('/'),
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// One request, as git knows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub number: u64,
    pub tip: String,
    /// The tip commit's subject, when that commit is already in this repository.
    pub subject: Option<String>,
}

impl Request {
    /// What to show beside the number: the subject if the commit is here, else its hash.
    pub fn detail(&self) -> String {
        match &self.subject {
            Some(subject) => subject.clone(),
            None => short_oid(&self.tip).to_owned(),
        }
    }
}

/// The newest requests of one remote.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Requests {
    /// Newest first.
    pub newest: Vec<Request>,
    /// How many the remote has, shown or not.
    pub total: usize,
}

impl Requests {
    /// A sentence under the list, saying what it leaves out.
    pub fn caveat(&self, forge: Forge) -> String {
        let noun = forge.noun();
        if self.total > self.newest.len() {
            format!(
                "The newest {} of {}. Git can't say which are still open.",
                self.newest.len(),
                count(self.total, noun)
            )
        } else {
            "Git can't say which are still open.".to_owned()
        }
    }
}

/// Reads `git ls-remote`'s answer: an object name, a tab and a ref. Anything that isn't a
/// request's ref is skipped. Newest (highest number) first.
pub fn parse_listing(listing: &str) -> Vec<(u64, String)> {
    let mut found: Vec<(u64, String)> = listing
        .lines()
        .filter_map(|line| {
            let (oid, name) = line.split_once('\t')?;
            let number = name
                .strip_suffix("/head")?
                .rsplit('/')
                .next()?
                .parse::<u64>()
                .ok()?;
            Some((number, oid.to_owned()))
        })
        .collect();
    found.sort_by_key(|(number, _)| std::cmp::Reverse(*number));
    found.dedup_by_key(|(number, _)| *number);
    found
}

/// Asks `remote` for its requests. This reaches over the network, so call it off the UI
/// thread.
pub fn list(repo: &Path, remote: &Remote, site: &Site) -> Result<Requests, String> {
    let pattern = site.forge.ref_pattern();
    let listing =
        ops::run(repo, &["ls-remote", "--refs", &remote.name, pattern]).map_err(|message| {
            let reason = message.lines().next().unwrap_or("").trim();
            format!("Couldn't ask {} for its requests. {reason}", remote.name)
        })?;
    let found = parse_listing(&listing);
    let total = found.len();
    let mut newest: Vec<Request> = found
        .into_iter()
        .take(SHOWN)
        .map(|(number, tip)| Request {
            number,
            tip,
            subject: None,
        })
        .collect();
    let subjects = subjects_of(repo, newest.iter().map(|request| request.tip.as_str()));
    for request in &mut newest {
        request.subject = subjects.get(&request.tip).cloned();
    }
    Ok(Requests { newest, total })
}

/// The subject of each commit that this repository has. A request from a fork usually
/// isn't one of them until it is fetched.
fn subjects_of<'a>(repo: &Path, oids: impl Iterator<Item = &'a str>) -> HashMap<String, String> {
    let oids: Vec<&str> = oids.collect();
    if oids.is_empty() {
        return HashMap::new();
    }
    let input = oids.join("\n") + "\n";
    let Ok(checked) = ops::run_with_input(repo, &["cat-file", "--batch-check"], input.as_bytes())
    else {
        return HashMap::new();
    };
    let here: Vec<&str> = checked
        .lines()
        .filter(|line| line.split(' ').nth(1) == Some("commit"))
        .filter_map(|line| line.split(' ').next())
        .collect();
    if here.is_empty() {
        return HashMap::new();
    }
    let mut args = vec!["log", "--no-walk=unsorted", "--format=%H%x09%s"];
    args.extend(&here);
    ops::run(repo, &args)
        .map(|out| {
            out.lines()
                .filter_map(|line| line.split_once('\t'))
                .map(|(oid, subject)| (oid.to_owned(), subject.to_owned()))
                .collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::Repo;

    fn remote(name: &str, url: &str) -> Remote {
        Remote {
            name: name.to_owned(),
            url: url.to_owned(),
            push_url: None,
        }
    }

    fn site(url: &str) -> Site {
        Site::of(&remote("origin", url)).unwrap()
    }

    #[test]
    fn addresses_in_every_shape_name_the_same_repository() {
        for url in [
            "https://github.com/octo/cat.git",
            "https://user:token@github.com/octo/cat",
            "git@github.com:octo/cat.git",
            "ssh://git@github.com:22/octo/cat.git",
            "git://github.com/octo/cat/",
        ] {
            let site = site(url);
            assert_eq!(site.forge, Forge::GitHub, "{url}");
            assert_eq!(site.web(), "https://github.com/octo/cat", "{url}");
        }
    }

    #[test]
    fn a_web_port_is_kept_and_an_ssh_port_is_not() {
        assert_eq!(
            site("http://gitea.lan:3000/me/app.git").web(),
            "http://gitea.lan:3000/me/app"
        );
        assert_eq!(
            site("ssh://git@gitlab.corp:2222/team/app.git").web(),
            "https://gitlab.corp/team/app"
        );
    }

    #[test]
    fn subgroups_stay_in_the_path() {
        let site = site("git@gitlab.com:group/sub/app.git");
        assert_eq!(site.web(), "https://gitlab.com/group/sub/app");
    }

    #[test]
    fn hosts_are_told_apart_by_name() {
        let forge =
            |host: &str| Site::of(&remote("o", &format!("https://{host}/a/b"))).map(|s| s.forge);
        assert_eq!(forge("github.com"), Some(Forge::GitHub));
        assert_eq!(forge("github.acme.com"), Some(Forge::GitHub));
        assert_eq!(forge("gitlab.com"), Some(Forge::GitLab));
        assert_eq!(forge("gitlab.acme.com"), Some(Forge::GitLab));
        assert_eq!(forge("codeberg.org"), Some(Forge::Gitea));
        assert_eq!(forge("gitea.acme.com"), Some(Forge::Gitea));
        assert_eq!(
            forge("git.acme.com"),
            None,
            "a host that doesn't say stays unknown"
        );
    }

    #[test]
    fn local_paths_and_bare_names_are_not_on_a_forge() {
        for url in [
            "/srv/git/cat.git",
            "../cat",
            "file:///srv/git/cat.git",
            "C:\\repos\\cat",
            "https://github.com/cat",
            "",
        ] {
            assert_eq!(Site::of(&remote("origin", url)), None, "{url:?}");
        }
    }

    #[test]
    fn each_forge_has_its_own_page_for_a_new_request() {
        assert_eq!(
            site("https://github.com/octo/cat").new_request_url("main", "feat/a b"),
            "https://github.com/octo/cat/compare/main...feat/a%20b?expand=1"
        );
        assert_eq!(
            site("https://gitlab.com/g/cat").new_request_url("main", "feat/x"),
            "https://gitlab.com/g/cat/-/merge_requests/new?merge_request%5Bsource_branch%5D=feat%2Fx\
             &merge_request%5Btarget_branch%5D=main"
        );
        assert_eq!(
            site("https://codeberg.org/me/cat").new_request_url("main", "feat/x"),
            "https://codeberg.org/me/cat/compare/main...feat/x"
        );
    }

    #[test]
    fn each_forge_has_its_own_page_for_a_request() {
        assert_eq!(
            site("https://github.com/o/r").request_url(7),
            "https://github.com/o/r/pull/7"
        );
        assert_eq!(
            site("https://gitlab.com/o/r").request_url(7),
            "https://gitlab.com/o/r/-/merge_requests/7"
        );
        assert_eq!(
            site("https://codeberg.org/o/r").request_url(7),
            "https://codeberg.org/o/r/pulls/7"
        );
        assert_eq!(site("https://gitlab.com/o/r").label(7), "!7");
        assert_eq!(site("https://github.com/o/r").label(7), "#7");
    }

    #[test]
    fn origin_is_preferred_when_it_is_on_a_forge() {
        let remotes = [
            remote("fork", "git@github.com:me/cat.git"),
            remote("origin", "git@gitlab.com:team/cat.git"),
        ];
        let (picked, site) = pick(&remotes).unwrap();
        assert_eq!(
            (picked.name.as_str(), site.forge),
            ("origin", Forge::GitLab)
        );

        let remotes = [
            remote("origin", "/srv/git/cat.git"),
            remote("fork", "git@github.com:me/cat.git"),
        ];
        assert_eq!(pick(&remotes).unwrap().0.name, "fork");
        assert!(pick(&[remote("origin", "/srv/git/cat.git")]).is_none());
    }

    #[test]
    fn a_listing_reads_newest_first_and_skips_other_refs() {
        let listing = "aaa\trefs/pull/9/head\n\
                       bbb\trefs/pull/10/head\n\
                       ccc\trefs/pull/10/merge\n\
                       ddd\trefs/heads/main\n\
                       eee\trefs/merge-requests/3/head\n";
        assert_eq!(
            parse_listing(listing),
            [
                (10, "bbb".to_owned()),
                (9, "aaa".to_owned()),
                (3, "eee".to_owned())
            ]
        );
    }

    #[test]
    fn the_caveat_says_what_the_list_leaves_out() {
        let some = |total| Requests {
            newest: vec![Request {
                number: 1,
                tip: "a".into(),
                subject: None,
            }],
            total,
        };
        assert_eq!(
            some(1).caveat(Forge::GitHub),
            "Git can't say which are still open."
        );
        assert_eq!(
            some(40).caveat(Forge::GitLab),
            "The newest 1 of 40 merge requests. Git can't say which are still open."
        );
    }

    #[test]
    fn requests_come_from_the_remote_with_a_subject_when_the_commit_is_here() {
        let upstream = Repo::new();
        upstream.write("a", "1");
        let tip = upstream.commit("fix: the cat");
        upstream.git(&["update-ref", "refs/pull/7/head", &tip]);
        upstream.git(&["update-ref", "refs/pull/12/head", &tip]);
        upstream.git(&["update-ref", "refs/pull/3/merge", &tip]);

        let local = Repo::new();
        let address = upstream.path().to_string_lossy().into_owned();
        local.git(&["remote", "add", "origin", &address]);
        let origin = remote("origin", &address);
        let site = Site {
            forge: Forge::GitHub,
            scheme: "https",
            host: "github.com".into(),
            path: "o/r".into(),
        };

        // The commit isn't here yet, so only its hash can be shown.
        let found = list(local.path(), &origin, &site).unwrap();
        assert_eq!(found.total, 2);
        assert_eq!(
            found.newest.iter().map(|r| r.number).collect::<Vec<_>>(),
            [12, 7]
        );
        assert_eq!(found.newest[0].subject, None);
        assert_eq!(found.newest[0].detail(), short_oid(&tip));

        local.git(&["fetch", "origin", "refs/pull/7/head"]);
        let found = list(local.path(), &origin, &site).unwrap();
        assert_eq!(found.newest[0].subject.as_deref(), Some("fix: the cat"));
        assert_eq!(found.newest[0].detail(), "fix: the cat");
    }

    #[test]
    fn a_remote_that_cannot_be_reached_says_so() {
        let local = Repo::new();
        local.git(&["remote", "add", "origin", "/no/such/place"]);
        let origin = remote("origin", "/no/such/place");
        let site = Site {
            forge: Forge::GitLab,
            scheme: "https",
            host: "gitlab.com".into(),
            path: "o/r".into(),
        };
        let error = list(local.path(), &origin, &site).unwrap_err();
        assert!(
            error.starts_with("Couldn't ask origin for its requests."),
            "{error}"
        );
    }
}
