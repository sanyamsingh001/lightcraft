use super::*;
use lightcraft_catalog::{Op, Photo, PhotoId, Source};
use std::sync::atomic::Ordering;

#[test]
fn completed_imports_and_watched_imports_trigger_the_command_but_local_browsing_does_not() {
    for (auto, browse, cancelled) in [(false, false, false), (true, false, false), (false, false, true), (false, true, false)] {
        let mut session = lightcraft_engine::Session::new();
        let p = Photo::new(PhotoId(1), Source::File { path: "imported-original.jpg".into() }, "original", "JPEG", 6000, 4000, "2026-01-01");
        session.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
        session.media.file_loader = Some(std::sync::Arc::new(|_, _| panic!("import completion must not open an original on the UI thread")));
        let mut app = LightcraftApp::new(session, crate::Services::default());
        let mut task = ImportTask::new(vec![], json!({}), 0, browse);
        task.thumbnail_library = Some(app.session.thumbnail_library());
        task.imported = 1;
        task.auto = auto;
        task.cancelled = cancelled;
        finish(&mut app, &egui::Context::default(), task);
        assert_eq!(app.session.thumbnail_build_running(), !browse);
        if !browse {
            let state = app.session.preview_build.as_ref().unwrap();
            assert_eq!(state.what, "thumbnails");
            assert_eq!(state.total, 1);
            assert!(!state.finished.load(Ordering::Relaxed));
        }
    }
}

#[test]
fn an_old_import_completion_cannot_trigger_thumbnails_in_a_replacement_memory_library() {
    let mut app = LightcraftApp::new(lightcraft_engine::Session::new(), crate::Services::default());
    let mut task = ImportTask::new(vec![], json!({}), 0, false);
    task.imported = 1;
    task.thumbnail_library = Some(app.session.thumbnail_library());
    let cache = app.session.media.rendered.clone();
    app.session
        .open_library_in(
            lightcraft_engine::library::LibraryStores {
                dir: "thumbnail-import-replacement".into(),
                catalog: Box::new(lightcraft_catalog::MemStore::default()),
                files: Box::new(lightcraft_catalog::MemStore::default()),
                on_disk: false,
            },
            false,
        )
        .unwrap();
    assert!(std::sync::Arc::ptr_eq(&cache, &app.session.media.rendered));
    finish(&mut app, &egui::Context::default(), task);
    assert!(!app.session.thumbnail_build_running());
    assert!(app.session.preview_build.is_none());
}

#[test]
fn an_import_finishing_after_a_successful_close_cannot_restart_thumbnail_work() {
    let mut app = LightcraftApp::new(lightcraft_engine::Session::new(), crate::Services::default());
    app.session
        .open_library_in(
            lightcraft_engine::library::LibraryStores {
                dir: "thumbnail-import-closed".into(),
                catalog: Box::new(lightcraft_catalog::MemStore::default()),
                files: Box::new(lightcraft_catalog::MemStore::default()),
                on_disk: false,
            },
            false,
        )
        .unwrap();
    let mut task = ImportTask::new(vec![], json!({}), 0, false);
    task.imported = 1;
    task.thumbnail_library = Some(app.session.thumbnail_library());
    let cache = app.session.media.rendered.clone();
    let revision = app.session.catalog.revision;
    app.session.close_library().unwrap();
    assert!(std::sync::Arc::ptr_eq(&cache, &app.session.media.rendered));
    assert_eq!(app.session.catalog.revision, revision);
    assert!(!task.thumbnail_library.as_ref().unwrap().matches(&app.session));
    finish(&mut app, &egui::Context::default(), task);
    assert!(!app.session.thumbnail_build_running());
    assert!(app.session.preview_build.is_none());
}
