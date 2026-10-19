//! Import scope changes keep the completed review's choices and reject stale workers.
use super::*;
use crate::Services;
use crate::headless::Headless;
use crate::state::Dialog;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::time::Duration;

const T: Duration = Duration::from_secs(20);

struct Fixture(PathBuf);

impl Fixture {
    fn new() -> Self {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let dir = std::env::temp_dir().join(format!("lc-import-scope-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir_all(&dir).unwrap();
        Self(dir)
    }

    fn image(&self, name: &str, seed: u8) -> String {
        let path = self.0.join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let image = lightcraft_raster::Rgba8::from_fn(24, 16, |_, _| [seed, 64, 128, 255]);
        let bytes = lightcraft_engine::export::encode_image(
            &image,
            &lightcraft_engine::export::ExportOptions { format: lightcraft_engine::export::ExportFormat::Png, ..Default::default() },
        )
        .unwrap();
        std::fs::write(&path, bytes).unwrap();
        path.to_string_lossy().into_owned()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn app() -> LightcraftApp {
    LightcraftApp::new(lightcraft_engine::Session::new().with_fs(), Services { png: None, ..Default::default() })
}

fn review(app: &LightcraftApp) -> &ImportDialog {
    let Some(Dialog::Import { opts }) = &app.ui.dialog else { panic!("missing import review") };
    opts
}

fn wait_review(h: &mut Headless) {
    assert!(h.step_until(T, |h| h.app.scan.is_none() && matches!(h.app.ui.dialog, Some(Dialog::Import { .. }))), "scan completed");
    h.settle(T);
}

fn click(h: &mut Headless, id: &str) {
    let result = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(result["ok"], true, "{result}");
    h.step();
}

fn open_folder(h: &mut Headless, path: &Path) {
    h.step();
    h.step();
    let result = h.request("engine.execute", json!({"command": "file.addFolder", "params": {"path": path.to_string_lossy()}}), T);
    assert_eq!(result["ok"], true, "{result}");
    wait_review(h);
}

// Compare every retained serialized option, including future options, while scan results and
// the deliberately changed scope are asserted separately against fixed fixture expectations.
fn retained_options(d: &ImportDialog) -> serde_json::Value {
    let mut value = serde_json::to_value(d).unwrap();
    let fields = value.as_object_mut().unwrap();
    for key in ["subfolders", "hasFolders", "candidates", "checked", "trashed"] {
        assert!(fields.remove(key).is_some(), "scan field {key} exists");
    }
    value
}

fn assert_review_paths(d: &ImportDialog, source: &Path, paths: &[&str]) {
    assert!(source.is_absolute());
    assert!(d.has_folders);
    assert_eq!(d.sources, [source.to_string_lossy().into_owned()]);
    assert!(paths.iter().all(|path| Path::new(path).is_absolute()));
    assert_eq!(d.candidates.iter().map(|c| c.path.as_str()).collect::<Vec<_>>(), paths);
}

#[test]
fn scope_toggle_and_rescan_preserve_checked_files_and_import_only_selection() {
    let fixture = Fixture::new();
    let root = fixture.image("root.png", 16);
    let child = fixture.image("nested/child.png", 48);
    let mut h = Headless::new(app(), [1200.0, 900.0], 1.0);
    open_folder(&mut h, &fixture.0);
    assert_review_paths(review(&h.app), &fixture.0, &[&child, &root]);
    assert!(review(&h.app).subfolders, "recursive remains the default");
    assert_eq!(review(&h.app).candidates.len(), 2);
    click(&mut h, "button:importNone");
    if let Some(Dialog::Import { opts }) = &mut h.app.ui.dialog {
        opts.keywords = "scope-kept".into();
    }
    let options = retained_options(review(&h.app));

    click(&mut h, "check:importSubfolders");
    wait_review(&mut h);
    assert!(!review(&h.app).subfolders);
    assert_eq!(review(&h.app).candidates.iter().map(|c| &c.path).collect::<Vec<_>>(), [&root]);
    assert_eq!(review(&h.app).checked, [false]);
    assert_eq!(review(&h.app).keywords, "scope-kept");
    assert_review_paths(review(&h.app), &fixture.0, &[&root]);
    assert_eq!(retained_options(review(&h.app)), options);

    let added = fixture.image("added.png", 96);
    click(&mut h, "button:importRescan");
    wait_review(&mut h);
    assert!(!review(&h.app).subfolders, "Rescan keeps scope");
    assert_review_paths(review(&h.app), &fixture.0, &[&added, &root]);
    assert_eq!(retained_options(review(&h.app)), options);
    assert_eq!(review(&h.app).selected_paths(), [added.clone()], "new files checked, old unchecked file preserved after reordering");

    click(&mut h, "check:importSubfolders");
    wait_review(&mut h);
    let d = review(&h.app);
    assert!(d.subfolders);
    assert_eq!(d.candidates.len(), 3);
    assert!(d.candidates.iter().any(|c| c.path == child));
    assert_review_paths(d, &fixture.0, &[&added, &child, &root]);
    assert_eq!(retained_options(d), options);
    assert_eq!(d.selected_paths(), [added.clone()], "hidden child remembers its unchecked choice");
    assert!(h.app.session.catalog.is_empty(), "reviews never import");
    assert!(h.app.ui.local_roots.is_empty(), "review does not add a Local root");
    let result = h.request("ui.dialog.confirm", json!({}), T);
    assert_eq!(result["ok"], true, "{result}");
    assert!(h.step_until(T, |h| h.app.import.is_none()));
    assert_eq!(h.app.session.catalog.len(), 1);
    assert!(h.app.session.catalog.photos().all(|p| matches!(&p.source, lightcraft_catalog::Source::File { path } if path == &added)));
}

#[test]
fn empty_direct_scope_keeps_the_review_and_can_restore_children() {
    let fixture = Fixture::new();
    let child = fixture.image("nested/child.png", 32);
    let storage = Fixture::new();
    let destination = storage.0.join("destination");
    std::fs::create_dir_all(&destination).unwrap();
    let mut ui_app = app();
    ui_app.session.open_library(storage.0.join("library"), false).unwrap();
    assert!(ui_app.session.library.as_ref().unwrap().on_disk);
    ui_app.run("metadata.savePreset", json!({"name": "Retained metadata", "fields": {"creator": "Scope fixture"}})).unwrap();
    let preset = ui_app.session.presets.first().expect("a built-in preset is available").id.clone();
    let mut h = Headless::new(ui_app, [1200.0, 900.0], 1.0);
    open_folder(&mut h, &fixture.0);
    assert_review_paths(review(&h.app), &fixture.0, &[&child]);
    if let Some(Dialog::Import { opts }) = &mut h.app.ui.dialog {
        opts.copy = true;
        opts.move_files = true;
        opts.new_album = "Retained scope album".into();
        opts.preset = preset;
        opts.keywords = "scope, retained".into();
        opts.destination = destination.to_string_lossy().into_owned();
        opts.organize = "custom".into();
        opts.folder_template = "{date:%Y}/{date:%Y%m%d}".into();
        opts.rename = "{date}_{seq:3}".into();
        opts.metadata_preset = "Retained metadata".into();
        opts.dng = true; // The inactive Copy preference must also survive while Move is selected.
        opts.on_deleted = "fresh".into();
    }
    let seeded_options = retained_options(review(&h.app));
    h.step();
    h.step();
    assert_eq!(retained_options(review(&h.app)), seeded_options, "the intended seed survives a normal draw");
    let options = retained_options(review(&h.app));
    click(&mut h, "check:importSubfolders");
    wait_review(&mut h);
    assert!(review(&h.app).has_folders);
    assert!(review(&h.app).candidates.is_empty());
    assert!(!review(&h.app).subfolders);
    assert_review_paths(review(&h.app), &fixture.0, &[]);
    assert_eq!(retained_options(review(&h.app)), options);
    click(&mut h, "check:importSubfolders");
    wait_review(&mut h);
    assert_review_paths(review(&h.app), &fixture.0, &[&child]);
    assert_eq!(retained_options(review(&h.app)), options);
    click(&mut h, "button:importRescan");
    wait_review(&mut h);
    assert_review_paths(review(&h.app), &fixture.0, &[&child]);
    assert_eq!(retained_options(review(&h.app)), options);
    for (command, locale) in [("app.language.german", crate::i18n::Locale::De), ("app.language.english", crate::i18n::Locale::En)] {
        let result = h.request("engine.execute", json!({"command": command, "params": {}}), T);
        assert_eq!(result["ok"], true, "{result}");
        h.step();
        h.step();
        assert_eq!(h.app.ui.language, locale);
        assert_review_paths(review(&h.app), &fixture.0, &[&child]);
        assert_eq!(retained_options(review(&h.app)), options);
    }
    assert!(h.app.session.catalog.is_empty(), "scope, refresh and locale changes never import");
    assert_eq!(review(&h.app).selected_paths(), [child]);
}

fn candidate(path: &str) -> ImportCandidate {
    ImportCandidate { path: path.into(), selection_identity: path.into(), name: path.into(), ..Default::default() }
}

fn output(path: &str) -> ScanOutput {
    ScanOutput {
        has_folders: true,
        candidates: vec![candidate(path)],
        probes: [(path.into(), lightcraft_engine::media::ProbeInfo { width: 99, ..Default::default() })].into(),
    }
}

/// A controlled worker channel makes completion/cancellation ordering deterministic.
fn pending(app: &mut LightcraftApp, review: ImportDialog) -> (std::sync::mpsc::Sender<ScanOutput>, Arc<ScanProgress>) {
    let progress = Arc::new(ScanProgress::default());
    let (tx, rx) = std::sync::mpsc::channel();
    let guard = scan_guard(app, &progress, false);
    app.scan = Some(ScanTask {
        progress: progress.clone(),
        rx,
        copy: review.copy,
        browse: false,
        sources: review.sources.clone(),
        subfolders: false,
        library_generation: app.session.library_generation(),
        review: Some(review),
        guard,
    });
    (tx, progress)
}

#[test]
fn cancelled_rescan_restores_review_and_late_results_cannot_replace_a_new_scan() {
    let mut app = app();
    let ctx = egui::Context::default();
    let mut previous = ImportDialog::new(vec![candidate("old.png")]);
    previous.sources = vec!["source".into()];
    previous.checked[0] = false;
    previous.keywords = "kept".into();
    app.session.import_probes.insert("old.png".into(), Default::default());
    let (_snapshot, _) = ScanInput::new(&mut app.session, &previous.sources);
    assert!(app.session.import_probes.is_empty(), "the pending worker owns a shared snapshot");
    let (old_tx, progress) = pending(&mut app, previous.clone());
    assert!(old_tx.send(output("cancelled.png")).is_ok());
    progress.cancel.store(true, Ordering::Relaxed);
    poll_scan(&mut app, &ctx);
    assert_eq!(review(&app), &previous, "Cancel restores the completed scope and choices");
    assert!(app.session.cached_import_probe("old.png").is_some(), "the restored review can still use its probes");
    assert!(!app.session.import_probes.contains_key("cancelled.png"));
    assert!(old_tx.send(output("late.png")).is_err(), "old receiver was dropped");

    app.ui.dialog = None;
    let (new_tx, _) = pending(&mut app, previous);
    assert!(new_tx.send(output("new.png")).is_ok());
    poll_scan(&mut app, &ctx);
    assert!(!review(&app).subfolders);
    assert!(app.session.cached_import_probe("old.png").is_none(), "completed rescan releases the previous review cache");
    assert_eq!(review(&app).selected_paths(), ["new.png"]);
    assert!(old_tx.send(output("later.png")).is_err());
    poll_scan(&mut app, &ctx);
    assert_eq!(review(&app).selected_paths(), ["new.png"]);
}

#[test]
fn rescan_results_neither_cross_libraries_nor_replace_another_dialog() {
    for change_library in [true, false] {
        let mut app = app();
        let ctx = egui::Context::default();
        let (tx, progress) = pending(&mut app, ImportDialog::new(vec![candidate("old.png")]));
        assert!(tx.send(output("stale.png")).is_ok());
        if change_library {
            app.session = lightcraft_engine::Session::new().with_fs();
        } else {
            app.ui.dialog = Some(Dialog::About);
        }
        poll_scan(&mut app, &ctx);
        assert!(app.scan.is_none());
        assert!(progress.cancel.load(Ordering::Relaxed));
        assert!(app.session.import_probes.is_empty());
        assert!(app.session.catalog.is_empty());
        if change_library {
            assert!(app.ui.dialog.is_none());
        } else {
            assert!(matches!(app.ui.dialog, Some(Dialog::About)));
        }
    }
}

#[test]
fn failed_rescan_restores_review_without_accepting_partial_probes() {
    let mut app = app();
    let ctx = egui::Context::default();
    let previous = ImportDialog::new(vec![candidate("old.png")]);
    let (tx, _) = pending(&mut app, previous.clone());
    drop(tx);
    poll_scan(&mut app, &ctx);
    assert_eq!(review(&app), &previous);
    assert!(app.session.import_probes.is_empty());
}

#[test]
fn rescan_rechecks_importability_and_source_identity_without_resetting_options() {
    let mut d = ImportDialog::new(vec![candidate("old.png"), candidate("duplicate.png")]);
    d.checked = vec![false, true];
    d.copy = true;
    d.destination = "destination".into();
    d.album = Some(7);
    let options = retained_options(&d);
    let mut retargeted = candidate("old.png");
    retargeted.selection_identity = "different-file.png".into();
    let mut duplicate = candidate("duplicate.png");
    duplicate.duplicate = Some("content".into());
    d.apply_scan(vec![duplicate, retargeted], true, vec![false, false]);
    assert_eq!(d.checked, [false, true], "duplicates remain unselectable; a different source gets the new-file default");
    assert!(d.copy);
    assert_eq!(d.destination, "destination");
    assert_eq!(retained_options(&d), options);
    let old: ImportDialog = serde_json::from_value(json!({"sources": ["source"]})).unwrap();
    assert!(old.subfolders, "old serialized dialogs retain recursive scope");
}
