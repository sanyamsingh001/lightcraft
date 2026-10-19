//! Saved Local views reconnect on native app startup and library switches.

use super::*;

fn saved_alias_library(tag: &str) -> (lightcraft_engine::Session, std::path::PathBuf, lightcraft_catalog::PhotoId) {
    let dir = std::env::temp_dir().join(format!("lc-ui-saved-alias-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("real")).unwrap();
    let image = lightcraft_raster::Rgba8::from_fn(2, 2, |_, _| [32, 64, 128, 255]);
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&image), &Default::default()).unwrap();
    std::fs::write(dir.join("real/photo.png"), bytes).unwrap();
    std::os::unix::fs::symlink(dir.join("real"), dir.join("alias")).unwrap();
    let mut session = lightcraft_engine::Session::new().with_fs();
    session.open_library(dir.join("library-a"), false).unwrap();
    let report = session.execute("library.import", &json!({"paths": [dir.join("real/photo.png")]})).unwrap();
    let id = lightcraft_catalog::PhotoId(report["imported"][0].as_u64().unwrap());
    session.execute("develop.set", &json!({"values": {"light.exposure": 1.25}})).unwrap();
    session.source = lightcraft_engine::LibrarySource::Folder;
    session.browse = Some(lightcraft_engine::Browse { path: dir.join("alias").to_string_lossy().into_owned(), subfolders: false });
    session.close_library().unwrap();
    session.open_library(dir.join("library-a"), false).unwrap();
    (session, dir, id)
}

fn wait_for_aliases(app: &mut LightcraftApp) {
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.alias_restore.is_some() {
        poll_browse_aliases(app, &ctx);
        assert!(std::time::Instant::now() < deadline, "Local alias worker did not finish");
        std::thread::yield_now();
    }
}

#[test]
fn startup_and_library_switch_reconnect_the_saved_view_without_importing() {
    let (session, dir, id) = saved_alias_library("reopen");
    let before = session.catalog.photo(id).unwrap().clone();
    let mut app = LightcraftApp::new(session, crate::Services::default());
    assert!(app.alias_restore.is_some(), "startup schedules the saved view's reconstruction");
    wait_for_aliases(&mut app);
    assert_eq!(app.session.visible(), &[id]);
    assert!(app.scan.is_none() && app.import.is_none());
    crate::panels::settings::open_library(&mut app, &json!({"path": dir.join("library-b")})).unwrap();
    crate::panels::settings::open_library(&mut app, &json!({"path": dir.join("library-a")})).unwrap();
    assert!(app.alias_restore.is_some(), "library opening schedules reconstruction too");
    wait_for_aliases(&mut app);
    assert_eq!(app.session.visible(), &[id]);
    assert_eq!(app.session.source_total(), Some(1));
    let after = app.session.catalog.photo(id).unwrap();
    assert_eq!(after.source, before.source);
    assert_eq!(after.develop, before.develop);
    assert_eq!(after.history, before.history);
    assert_eq!(app.session.catalog.len(), 1);
    for (command, params) in [("photo.rate", json!({"ids": [id], "rating": 4})), ("develop.set", json!({"values": {"light.exposure": 1.5}}))] {
        app.session.execute(command, &params).unwrap();
        for _ in 0..32 {
            poll_browse_aliases(&mut app, &egui::Context::default());
            assert!(app.alias_restore.is_none(), "edits and frames do not reschedule reconstruction");
            assert_eq!(app.session.visible(), &[id]);
        }
    }
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn changing_the_view_or_dropping_the_task_cancels_the_old_reconstruction() {
    let (session, dir, _) = saved_alias_library("cancel");
    let mut app = LightcraftApp::new(session, crate::Services::default());
    let cancel = app.alias_restore.as_ref().unwrap().cancel.clone();
    app.session.source = lightcraft_engine::LibrarySource::All;
    poll_browse_aliases(&mut app, &egui::Context::default());
    assert!(app.alias_restore.is_none() && cancel.load(Ordering::Relaxed));
    app.session.source = lightcraft_engine::LibrarySource::Folder;
    restore_browse_aliases(&mut app);
    let cancel = app.alias_restore.as_ref().unwrap().cancel.clone();
    app.alias_restore = None;
    assert!(cancel.load(Ordering::Relaxed));
    drop(app);
    std::fs::remove_dir_all(dir).unwrap();
}
