//! The manual-install folder action uses the engine-configured directory and the normal host
//! opener. These tests never download or load a model, or start a native file manager.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::json;

use crate::state::Dialog;
use crate::{LightcraftApp, Services};

struct Fixture {
    app: LightcraftApp,
    root: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("lc-sam-folder-{name}-{}-{stamp}", std::process::id()));
        let mut app = LightcraftApp::new(lightcraft_engine::Session::new(), Services::default());
        app.session.segmenter.dir = Some(root.join("custom/models/sam3"));
        Self { app, root }
    }

    fn dir(&self) -> PathBuf {
        self.app.session.segmenter.dir.clone().unwrap()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

#[test]
fn folder_command_opens_the_configured_directory_and_preserves_files() {
    let mut f = Fixture::new("configured");
    let dir = f.dir();
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("local-note"), b"keep this file").unwrap();
    let calls = Arc::new(Mutex::new(Vec::new()));
    let log = calls.clone();
    f.app.services.open_with = Some(Box::new(move |path, editor| {
        assert!(std::path::Path::new(path).is_dir());
        log.lock().unwrap().push((path.to_string(), editor.to_string()));
        Ok(())
    }));
    f.app.services.reveal = Some(Box::new(|_| panic!("revealing a file would open its parent")));
    assert!(crate::menus::ui_commands().any(|c| c.0 == "app.openSamModelFolder"));
    assert!(crate::menus::ui_enabled(&f.app, "app.openSamModelFolder"));
    let reply = f.app.run("app.openSamModelFolder", json!({})).unwrap();
    assert_eq!(reply["path"], dir.to_str().unwrap());
    assert_eq!(*calls.lock().unwrap(), vec![(dir.to_str().unwrap().to_string(), String::new())]);
    assert_eq!(std::fs::read(dir.join("local-note")).unwrap(), b"keep this file");
    assert!(!f.app.session.segmenter.download_status().running);
}

#[test]
fn missing_folder_or_opener_is_rejected_before_creating_any_directory() {
    let mut f = Fixture::new("unavailable");
    assert!(!crate::menus::ui_enabled(&f.app, "app.openSamModelFolder"));
    assert!(f.app.run("app.openSamModelFolder", json!({})).unwrap_err().contains("not available"));
    assert!(!f.root.exists());
    f.app.services.open_with = Some(Box::new(|_, _| panic!("no directory is configured")));
    f.app.session.segmenter.dir = None;
    assert!(!crate::menus::ui_enabled(&f.app, "app.openSamModelFolder"));
    assert!(f.app.run("app.openSamModelFolder", json!({})).unwrap_err().contains("no folder"));
    assert!(!f.root.exists());
}

#[test]
fn folder_creation_failure_preserves_the_existing_file_and_skips_the_opener() {
    let mut f = Fixture::new("blocked");
    std::fs::create_dir_all(&f.root).unwrap();
    let blocked = f.root.join("not-a-directory");
    std::fs::write(&blocked, b"keep").unwrap();
    f.app.session.segmenter.dir = Some(blocked.join("sam3"));
    f.app.services.open_with = Some(Box::new(|_, _| panic!("creation failed before opening")));
    assert!(f.app.run("app.openSamModelFolder", json!({})).is_err());
    assert_eq!(std::fs::read(blocked).unwrap(), b"keep");
}

#[test]
fn folder_opener_failure_can_be_retried_without_dismissing_setup() {
    let mut f = Fixture::new("retry");
    f.app.ui.dialog = Some(Dialog::SamModel { then: Some(("object".into(), "new".into())), error: None });
    let pending = f.app.ui.dialog.clone();
    f.app.services.open_with = Some(Box::new(|_, _| Err("test opener failure".into())));
    assert_eq!(f.app.run("app.openSamModelFolder", json!({})).unwrap_err(), "test opener failure");
    assert_eq!(f.app.ui.dialog, pending);
    assert!(f.dir().is_dir());
    f.app.services.open_with = Some(Box::new(|_, _| Ok(())));
    assert!(f.app.run("app.openSamModelFolder", json!({})).is_ok());
    assert_eq!(f.app.ui.dialog, pending);
    assert!(!f.app.session.segmenter.download_status().running);
}
