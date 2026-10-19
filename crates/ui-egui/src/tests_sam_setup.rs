//! Issue #366: useful setup actions when SAM 3 has no download mirror.
//! No model is loaded or downloaded; the host actions are recorded by FakeServices.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::state::Dialog;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);

#[derive(Clone, Default)]
struct FakeServices {
    urls: Arc<Mutex<Vec<String>>>,
    opened: Arc<Mutex<Vec<(String, String)>>>,
}

impl FakeServices {
    fn services(&self) -> Services {
        let urls = self.urls.clone();
        let opened = self.opened.clone();
        Services {
            open_url: Some(Box::new(move |url| {
                urls.lock().unwrap().push(url.to_string());
                Ok(())
            })),
            open_with: Some(Box::new(move |path, app| {
                assert!(std::path::Path::new(path).is_dir(), "the model folder is created before opening");
                opened.lock().unwrap().push((path.to_string(), app.to_string()));
                Ok(())
            })),
            ..Default::default()
        }
    }
}

struct Fixture {
    h: Headless,
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str, services: Services) -> Self {
        let root = std::env::temp_dir().join(format!("lc-sam-help-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut session = lightcraft_engine::Session::new();
        session.segmenter.dir = Some(root.join("models/sam3"));
        session.segmenter.mirrors_file = Some(root.join("models/sam3-mirrors.txt"));
        let mut app = LightcraftApp::new(session, services);
        app.ui.settings.gpu = false;
        app.offer_sam_download(Some(("object", "new")));
        let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
        h.step();
        h.step();
        Self { h, root }
    }

    fn has(&self, id: &str) -> bool {
        self.h.app.widgets.iter().any(|(w, _)| w == id)
    }

    fn click(&mut self, id: &str) {
        self.h.step();
        self.h.step();
        let r = self.h.request("ui.clickWidget", json!({"id": id}), T);
        assert_eq!(r["ok"], true, "{id}: {r}");
        for _ in 0..4 {
            self.h.step();
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn empty_mirror_dialog_opens_the_guide_and_exact_model_folder_without_downloading() {
    let fake = FakeServices::default();
    let mut f = Fixture::new("actions", fake.services());
    assert!(f.h.app.session.segmenter.mirrors().is_empty(), "run this fixture without LIGHTCRAFT_SAM3_MIRRORS overrides");
    assert!(f.has("link:samModelHelp"));
    assert!(f.has("button:samModelFolder"));
    assert!(!f.has("button:dialogOk"), "Download remains unavailable without a location");
    assert!(!f.root.exists(), "showing help does not install anything");
    f.click("link:samModelHelp");
    assert_eq!(*fake.urls.lock().unwrap(), vec![crate::links::SAM_MODEL_GUIDE.to_string()]);
    assert!(!f.root.exists(), "opening the guide does not create model files");
    f.click("button:samModelFolder");
    let dir = f.h.app.session.segmenter.dir.as_ref().unwrap();
    assert_eq!(*fake.opened.lock().unwrap(), vec![(dir.to_str().unwrap().to_string(), String::new())]);
    assert_eq!(std::fs::read_dir(dir).unwrap().count(), 0);
    assert!(!f.h.app.session.segmenter.download_status().running);
    assert!(matches!(f.h.app.ui.dialog, Some(Dialog::SamModel { then: Some(_), .. })), "help preserves the pending mask action");
    f.click("button:dialogCancel");
    assert!(f.h.app.ui.dialog.is_none());
    assert!(f.h.app.session.catalog.is_empty());
}

#[test]
fn adding_a_configured_mirror_makes_download_available_but_still_requires_consent() {
    if !lightcraft_engine::segment::Segmenter::AVAILABLE {
        return; // The parent runs this with the desktop SAM feature enabled.
    }
    let mut f = Fixture::new("mirror", FakeServices::default().services());
    assert!(f.h.app.session.segmenter.mirrors().is_empty(), "run without LIGHTCRAFT_SAM3_MIRRORS overrides");
    assert!(!f.has("button:dialogOk"));
    let file = f.h.app.session.segmenter.mirrors_file.as_ref().unwrap();
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, "https://sam-test.invalid/model/\n").unwrap();
    f.h.step();
    f.h.step();
    assert!(f.has("button:dialogOk"), "the dialog reads the engine's configured mirror file again");
    assert!(f.has("link:samModelHelp"));
    assert!(!f.h.app.session.segmenter.download_status().running);
    let e = f.h.app.run("segment.model.download", json!({"acknowledged": false})).unwrap_err();
    assert!(e.contains("acknowledged") && e.contains("SAM License"), "{e}");
    assert!(!f.h.app.session.segmenter.download_status().running);
    // Never choose Download: availability and refusal without consent need no network request.
}

#[test]
fn missing_host_or_model_folder_is_an_actionable_error_without_creating_files() {
    let mut f = Fixture::new("no-host", Services::default());
    assert!(!crate::menus::ui_enabled(&f.h.app, "app.samModelHelp"));
    assert!(!crate::menus::ui_enabled(&f.h.app, "app.openSamModelFolder"));
    assert!(f.h.app.run("app.samModelHelp", json!({})).unwrap_err().contains("open links"));
    assert!(f.h.app.run("app.openSamModelFolder", json!({})).unwrap_err().contains("not available"));
    assert!(!f.root.exists());
    f.h.app.services = FakeServices::default().services();
    f.h.app.session.segmenter.dir = None;
    assert!(!crate::menus::ui_enabled(&f.h.app, "app.openSamModelFolder"));
    assert!(f.h.app.run("app.openSamModelFolder", json!({})).unwrap_err().contains("no folder"));
}

#[test]
fn failed_folder_open_is_visible_and_keeps_the_setup_dialog_available() {
    let mut services = FakeServices::default().services();
    services.open_with = Some(Box::new(|_, _| Err("test folder opener failed".into())));
    let mut f = Fixture::new("failed-open", services);
    f.click("button:samModelFolder");
    assert!(f.h.app.ui.toast.as_ref().unwrap().0.contains("test folder opener failed"));
    assert!(matches!(f.h.app.ui.dialog, Some(Dialog::SamModel { .. })));
    assert!(!f.h.app.session.segmenter.download_status().running);
}
