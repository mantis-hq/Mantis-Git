use super::{AppUpdates, Arc, Channel, Http, Status, Target, Updates, update};
use gitclient_core::install::update::Version;
use gpui_kit::{AppContext as _, Entity, TestAppContext, VisualTestContext};
use std::io::Write;
use std::sync::Mutex;

/// The network: answers the release lookup with `tag`, and nothing else. Every request is
/// remembered, so a test can say that none was made.
struct Releases {
    tag: String,
    asked: Mutex<Vec<String>>,
    broken: bool,
}

impl Releases {
    fn at(tag: &str) -> Arc<Self> {
        Arc::new(Self {
            tag: tag.to_owned(),
            asked: Mutex::default(),
            broken: false,
        })
    }

    fn offline() -> Arc<Self> {
        Arc::new(Self {
            tag: String::new(),
            asked: Mutex::default(),
            broken: true,
        })
    }

    fn requests(&self) -> usize {
        self.asked.lock().unwrap().len()
    }
}

fn linux() -> Target {
    Target::of("linux", "x86_64").unwrap()
}

impl Http for Releases {
    fn get(&self, url: &str, into: &mut dyn Write, _: u64) -> Result<(), String> {
        self.asked.lock().unwrap().push(url.to_owned());
        if self.broken {
            return Err("Couldn't reach GitHub: offline.".to_owned());
        }
        if !url.ends_with("/releases/latest") {
            return Err(format!("no answer for {url}"));
        }
        let archive = linux().archive_name(&self.tag);
        let json = format!(
            r#"{{"tag_name":"{tag}","assets":[
                {{"name":"{archive}","browser_download_url":"https://github.com/o/r/{archive}"}},
                {{"name":"SHA256SUMS.txt","browser_download_url":"https://github.com/o/r/SHA256SUMS.txt"}}]}}"#,
            tag = self.tag
        );
        into.write_all(json.as_bytes()).map_err(|e| e.to_string())
    }
}

fn updates(
    channel: Channel,
    http: Arc<Releases>,
    cx: &mut TestAppContext,
) -> (Entity<Updates>, VisualTestContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });
    let window = cx.add_window(|_, _| gpui_kit::Empty);
    let model = cx.new(|_| {
        Updates::new(
            channel,
            "/apps/gitclient/gitclient".into(),
            Some(linux()),
            http,
        )
    });
    let cx = VisualTestContext::from_window(window.into(), cx);
    (model, cx)
}

fn look(model: &Entity<Updates>, cx: &mut VisualTestContext) {
    model.update(cx, |updates, cx| updates.check(cx));
    cx.run_until_parked();
}

fn band_of(model: &Entity<Updates>, cx: &VisualTestContext) -> Option<update::Band> {
    model.read_with(cx, |updates, _| updates.band())
}

fn status_of(model: &Entity<Updates>, cx: &VisualTestContext) -> Status {
    model.read_with(cx, |updates, _| updates.status().clone())
}

#[gpui_kit::test]
fn a_newer_release_is_offered_and_later_hides_the_offer_until_a_newer_one(cx: &mut TestAppContext) {
    let (model, mut cx) = updates(Channel::Portable, Releases::at("v999.0.0"), cx);
    assert_eq!(status_of(&model, &cx), Status::Idle);
    assert_eq!(band_of(&model, &cx), None);

    look(&model, &mut cx);

    let band = band_of(&model, &cx).expect("an offer");
    assert_eq!(band.message, "gitclient 999.0.0 is available.");
    assert_eq!(band.action.as_deref(), Some("Update to 999.0.0"));

    // "Later" hides the band, and Settings still says it is available.
    model.update(&mut cx, |updates, cx| updates.put_off(cx));
    assert_eq!(band_of(&model, &cx), None);
    assert!(matches!(status_of(&model, &cx), Status::Available(_)));

    // Looking again and finding a release brings the offer back.
    look(&model, &mut cx);
    assert!(band_of(&model, &cx).is_some());
}

#[gpui_kit::test]
fn the_same_version_is_up_to_date_and_says_nothing_over_the_window(cx: &mut TestAppContext) {
    let current = Version::current().to_string();
    let (model, mut cx) = updates(Channel::Portable, Releases::at(&format!("v{current}")), cx);

    look(&model, &mut cx);

    assert_eq!(status_of(&model, &cx), Status::UpToDate);
    assert_eq!(band_of(&model, &cx), None);
}

#[gpui_kit::test]
fn a_failed_look_is_only_for_settings_and_can_be_repeated(cx: &mut TestAppContext) {
    let http = Releases::offline();
    let (model, mut cx) = updates(Channel::Portable, http.clone(), cx);

    look(&model, &mut cx);

    let status = status_of(&model, &cx);
    assert!(matches!(&status, Status::CheckFailed(error) if error.contains("offline")));
    assert_eq!(
        band_of(&model, &cx),
        None,
        "no interruption for a failed look"
    );
    assert_eq!(status.button().as_deref(), Some("Check now"));

    // The button looks again.
    model.update(&mut cx, |updates, cx| updates.act(cx));
    cx.run_until_parked();
    assert_eq!(http.requests(), 2);
}

#[gpui_kit::test]
fn a_failed_install_says_why_in_peach_and_offers_to_try_again(cx: &mut TestAppContext) {
    // The release is there, but its archive can't be fetched.
    let (model, mut cx) = updates(Channel::Portable, Releases::at("v999.0.0"), cx);
    look(&model, &mut cx);

    model.update(&mut cx, |updates, cx| updates.act(cx));
    cx.run_until_parked();

    let status = status_of(&model, &cx);
    assert!(matches!(status, Status::InstallFailed(..)), "{status:?}");
    let band = band_of(&model, &cx).expect("the failure is shown");
    assert!(band.warning);
    assert_eq!(band.action.as_deref(), Some("Try again"));
}

#[gpui_kit::test]
fn a_copy_a_package_manager_installed_never_looks_and_says_why(cx: &mut TestAppContext) {
    let http = Releases::at("v999.0.0");
    let (model, mut cx) = updates(Channel::Packaged { by: "Flatpak" }, http.clone(), cx);

    look(&model, &mut cx);
    model.update(&mut cx, |updates, cx| updates.install(cx));
    cx.run_until_parked();

    assert_eq!(http.requests(), 0, "not a single request");
    assert_eq!(status_of(&model, &cx), Status::Idle);
    assert_eq!(band_of(&model, &cx), None);
    let reason = model.read_with(&cx, |updates, _| updates.reason().map(str::to_owned));
    assert_eq!(
        reason.as_deref(),
        Some("gitclient was installed with Flatpak, which updates it.")
    );
}

#[gpui_kit::test]
fn restarting_into_a_program_that_cannot_start_says_so_and_keeps_the_app_open(
    cx: &mut TestAppContext,
) {
    let (model, mut cx) = updates(Channel::Portable, Releases::at("v999.0.0"), cx);
    model.update(&mut cx, |updates, _| {
        updates.status = Status::Ready {
            version: Version::parse("999.0.0").unwrap(),
            back: "To go back, download 0.1.0.".to_owned(),
        };
    });
    // The program is at a path that doesn't exist, so it can't be started.
    let band = band_of(&model, &cx).unwrap();
    assert_eq!(band.action.as_deref(), Some("Restart now"));

    model.update(&mut cx, |updates, cx| updates.restart(cx));

    let band = band_of(&model, &cx).unwrap();
    assert!(band.warning);
    assert!(band.message.contains("Couldn't start"), "{}", band.message);
    assert_eq!(band.action.as_deref(), Some("Try again"));
}

#[gpui_kit::test]
fn the_band_is_drawn_over_the_window_in_every_state_it_can_take(cx: &mut TestAppContext) {
    cx.update(|cx| {
        gpui_kit::init(cx);
        crate::theme::init(cx);
    });
    let model = cx.new(|_| {
        Updates::new(
            Channel::Portable,
            "/apps/gitclient/gitclient".into(),
            Some(linux()),
            Releases::at("v999.0.0"),
        )
    });
    cx.update(|cx| cx.set_global(AppUpdates(model.clone())));
    model.update(cx, |updates, cx| updates.check(cx));
    cx.run_until_parked();
    let found = match model.read_with(cx, |updates, _| updates.status().clone()) {
        Status::Available(found) => found,
        other => panic!("expected an offer, got {other:?}"),
    };

    let window = cx.add_window(crate::view::Workspace::new);
    let mut cx = VisualTestContext::from_window(window.into(), cx);
    let draw = |cx: &mut VisualTestContext| {
        cx.simulate_mouse_move(
            gpui_kit::point(gpui_kit::px(5.), gpui_kit::px(5.)),
            None,
            gpui_kit::Modifiers::none(),
        );
    };
    let states = [
        Status::Available(found.clone()),
        Status::Installing(found.clone()),
        Status::installed(&found, Version::current()),
        Status::InstallFailed(found.clone(), "No room.".to_owned()),
        Status::UpToDate,
    ];
    for status in states {
        model.update(&mut cx, |updates, cx| {
            updates.status = status;
            cx.notify();
        });
        draw(&mut cx);
    }
    // And with "Later" chosen, and with a restart that failed.
    model.update(&mut cx, |updates, cx| {
        updates.status = Status::Available(found.clone());
        updates.put_off(cx);
    });
    draw(&mut cx);
    model.update(&mut cx, |updates, cx| {
        updates.problem = Some("Couldn't start it.".to_owned());
        cx.notify();
    });
    draw(&mut cx);
}
