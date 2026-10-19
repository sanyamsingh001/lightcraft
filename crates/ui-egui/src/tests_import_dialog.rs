//! Headless import scope and confirmation regressions (issue #332).

use crate::headless::Headless;
use crate::import::ImportDialog;
use crate::state::Dialog;
use crate::{LightcraftApp, Services};
use serde_json::{Value, json};
use std::time::Duration;

const T: Duration = Duration::from_secs(20);

fn headless() -> Headless {
    Headless::new(LightcraftApp::new(lightcraft_engine::Session::new(), Services { png: None, ..Default::default() }), [1600.0, 1200.0], 1.0)
}

fn dialog(h: &Headless) -> &ImportDialog {
    let Some(Dialog::Import { opts }) = &h.app.ui.dialog else { panic!("no import review") };
    opts
}

fn draw(h: &mut Headless) {
    for _ in 0..5 {
        h.step();
    }
}

fn click(h: &mut Headless, id: &str, shift: bool, ctrl: bool) {
    let r = h.request("ui.clickWidget", json!({"id": id, "shift": shift, "ctrl": ctrl}), T);
    assert_eq!(r["ok"], true, "{r}");
    draw(h);
}

#[test]
fn import_subfolder_toggle_rescans_and_preserves_options_and_checked_files() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/agent-import-residual-67/fixtures")
        .join(format!("ui-subfolders-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("exported")).unwrap();
    let image = lightcraft_raster::Rgba8::from_fn(24, 16, |x, y| [(x * 9) as u8, (y * 12) as u8, 80, 255]);
    let png = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&image), &Default::default()).unwrap();
    let jpeg = lightcraft_codecs::encode_jpeg(
        &lightcraft_codecs::EncodeImage::rgba8(&image),
        90,
        lightcraft_codecs::ChromaSubsampling::S444,
        &Default::default(),
    )
    .unwrap();
    std::fs::write(dir.join("original.png"), png).unwrap();
    std::fs::write(dir.join("exported/companion.JPG"), jpeg).unwrap();
    let mut h = headless();
    h.app.session = lightcraft_engine::Session::new().with_fs();
    let r = h.request("engine.execute", json!({"command": "file.addFolder", "params": {"path": dir.to_string_lossy()}}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(h.step_until(T, |h| h.app.scan.is_none() && matches!(h.app.ui.dialog, Some(Dialog::Import { .. }))));
    draw(&mut h);
    assert_eq!(dialog(&h).candidates.len(), 2);
    let Some(Dialog::Import { opts }) = &mut h.app.ui.dialog else { panic!("no import review") };
    opts.new_album = "Trip".into();
    opts.keywords = "landscape".into();
    let original = opts.candidates.iter().position(|c| c.path.ends_with("original.png")).unwrap();
    opts.checked[original] = false;
    click(&mut h, "button:importType:JPG", false, false);
    click(&mut h, "checkbox:importSubfolders", false, false);
    assert!(h.step_until(T, |h| h.app.scan.is_none() && matches!(h.app.ui.dialog, Some(Dialog::Import { .. }))));
    draw(&mut h);
    let d = dialog(&h);
    assert!(!d.subfolders);
    assert_eq!(d.candidates.len(), 1);
    assert!(d.candidates[0].path.ends_with("original.png"));
    assert_eq!(d.checked, vec![false]);
    assert_eq!((&d.new_album, &d.keywords), (&"Trip".to_string(), &"landscape".to_string()));
    assert_eq!(h.app.session.catalog.len(), 0, "changing the scan never imports files");
    click(&mut h, "checkbox:importSubfolders", false, false);
    assert!(h.step_until(T, |h| h.app.scan.is_none() && matches!(h.app.ui.dialog, Some(Dialog::Import { .. }))));
    draw(&mut h);
    assert_eq!(dialog(&h).candidates.len(), 2);
    assert!(!dialog(&h).type_checked("JPG"));
    assert!(dialog(&h).selected_paths().is_empty(), "the rejected PNG remains unchecked; nested JPEG stays unchecked");
    click(&mut h, "button:importType:JPG", false, false);
    assert_eq!(dialog(&h).selected_paths().len(), 1, "the deliberate JPEG recheck leaves the rejected PNG off");
    // Move must not silently reselect a rejected nested original after an off/on round trip.
    let Some(Dialog::Import { opts }) = &mut h.app.ui.dialog else { panic!("no import review") };
    opts.copy = true;
    opts.move_files = true;
    let original = opts.candidates.iter().position(|c| c.path.ends_with("original.png")).unwrap();
    let companion = opts.candidates.iter().position(|c| c.path.ends_with("companion.JPG")).unwrap();
    opts.checked[original] = true;
    opts.checked[companion] = false;
    // Redraw the direct fixture mutation; the upstream type choice follows its candidates.
    draw(&mut h);
    for subfolders in [false, true] {
        click(&mut h, "checkbox:importSubfolders", false, false);
        assert!(h.step_until(T, |h| h.app.scan.is_none() && matches!(h.app.ui.dialog, Some(Dialog::Import { .. }))));
        draw(&mut h);
        let d = dialog(&h);
        assert_eq!(d.subfolders, subfolders);
        assert!(d.copy && d.move_files, "Move mode must survive rescans");
        assert_eq!((&d.new_album, &d.keywords), (&"Trip".to_string(), &"landscape".to_string()));
        assert_eq!(d.selected_paths(), vec![dir.join("original.png").to_string_lossy().to_string()]);
    }
    assert!(!dialog(&h).type_checked("JPG"));
    assert_eq!(
        dialog(&h).selected_paths(),
        vec![dir.join("original.png").to_string_lossy().to_string()],
        "returning to recursive scope must not reselect the rejected nested JPEG"
    );
    assert_eq!(h.app.session.catalog.len(), 0, "rescanning a Move review never imports or moves files");
    assert!(dir.join("exported/companion.JPG").exists());
    // An initially nonrecursive scan also leaves its empty review open so nested photos can be included.
    std::fs::remove_file(dir.join("original.png")).unwrap();
    h.app.ui.dialog = None;
    let r = h.request("engine.execute", json!({"command": "file.addFolder", "params": {"path": dir.to_string_lossy(), "subfolders": false}}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(h.step_until(T, |h| h.app.scan.is_none() && matches!(h.app.ui.dialog, Some(Dialog::Import { .. }))));
    draw(&mut h);
    assert!(dialog(&h).candidates.is_empty());
    click(&mut h, "checkbox:importSubfolders", false, false);
    assert!(h.step_until(T, |h| h.app.scan.is_none() && matches!(h.app.ui.dialog, Some(Dialog::Import { .. }))));
    assert_eq!(dialog(&h).candidates.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn rejected_import_confirmation_preserves_draft_albums_undo_and_shared_probes() {
    use lightcraft_engine::import::{ImportCandidate, ScanInput};
    use lightcraft_engine::media::ProbeInfo;
    let mut h = headless();
    h.app.session.execute("album.create", &json!({"name":"Existing album"})).unwrap();
    let path = "virtual-confirm-options.png".to_string();
    h.app.session.accept_import_scan([(path.clone(), ProbeInfo { width: 24, height: 16, ..Default::default() })].into_iter().collect());
    // Cancellation leaves the review cache shared with its old scan owner.
    let (_pending_input, _) = ScanInput::new(&mut h.app.session, std::slice::from_ref(&path));
    let albums = serde_json::to_value(h.app.session.catalog.albums().collect::<Vec<_>>()).unwrap();
    let undo = h.app.session.undo.len();
    for error in ["template", "preset", "metadata", "album", "onDeleted", "destination"] {
        let mut draft = ImportDialog::new(vec![ImportCandidate { path: path.clone(), ..Default::default() }]);
        draft.new_album = "Do not create on error".into();
        draft.keywords = "preserve these options".into();
        match error {
            "template" => {
                draft.copy = true;
                draft.destination = "virtual-destination".into();
                draft.organize = "custom".into();
                draft.folder_template = "../Photos".into();
            }
            "preset" => draft.preset = "missing-preset".into(),
            "metadata" => draft.metadata_preset = "missing-metadata-preset".into(),
            "album" => draft.album = Some(u64::MAX),
            "onDeleted" => draft.on_deleted = "unknown-choice".into(),
            _ => draft.copy = true, // no library or destination
        }
        h.app.ui.dialog = Some(Dialog::Import { opts: Box::new(draft.clone()) });
        for _ in 0..2 {
            let response = h.request("ui.dialog.confirm", json!({}), T);
            assert_eq!(response["ok"], false, "{error}: {response}");
            assert_eq!(dialog(&h), &draft, "{error} consumed or changed the review");
            assert!(h.app.import.is_none(), "{error} queued an invalid import");
            assert_eq!(serde_json::to_value(h.app.session.catalog.albums().collect::<Vec<_>>()).unwrap(), albums);
            assert_eq!(h.app.session.undo.len(), undo);
            assert_eq!(h.app.session.catalog.len(), 0);
            assert_eq!(h.app.session.cached_import_probe(&path).unwrap().width, 24, "{error} consumed review probes");
        }
        assert_eq!(h.request("ui.dialog.cancel", json!({}), T)["ok"], true);
        assert_eq!(h.app.session.undo.len(), undo);
    }
}

#[test]
fn new_import_album_is_created_once_with_successful_batches_only() {
    use lightcraft_engine::import::ImportCandidate;
    use lightcraft_engine::media::ProbeInfo;
    use std::sync::Arc;
    let count = lightcraft_engine::import::batch_size() + 2;
    for succeeds in [false, true] {
        let mut h = headless();
        h.app.session.media.file_probe = Some(Arc::new(move |path| {
            if !succeeds {
                return Err("source is unavailable".into());
            }
            Ok(ProbeInfo { width: 24, height: 16, format: "PNG".into(), content_hash: Some(path.into()), ..Default::default() })
        }));
        let mut draft =
            ImportDialog::new((0..count).map(|i| ImportCandidate { path: format!("virtual-batch-{i}.png"), ..Default::default() }).collect());
        draft.new_album = "Batch album".into();
        h.app.ui.dialog = Some(Dialog::Import { opts: Box::new(draft) });
        let (confirm, _) = crate::control::ControlRequest::new("ui.dialog.confirm", json!({}));
        let ctx = egui::Context::default();
        let crate::control::Outcome::Done(response) = crate::control::handle(&mut h.app, &ctx, &confirm) else {
            panic!("confirmation returned no result")
        };
        assert_eq!(response["ok"], true, "{response}");
        assert!(h.app.ui.dialog.is_none());
        assert!(h.app.import.is_some());
        assert_eq!(h.app.session.catalog.albums().count(), 0, "confirmation must defer album creation");
        assert_eq!(h.app.session.undo.len(), 0);
        let started = std::time::Instant::now();
        while h.app.import.is_some() {
            assert!(started.elapsed() < T, "import did not finish");
            crate::import::tick(&mut h.app, &ctx);
            std::thread::yield_now();
        }
        if succeeds {
            let albums: Vec<_> = h.app.session.catalog.albums().collect();
            assert_eq!(albums.len(), 1, "later batches must reuse the first album");
            assert_eq!(albums[0].name, "Batch album");
            assert_eq!(albums[0].photos.len(), count);
            assert_eq!(h.app.session.catalog.len(), count);
            assert_eq!(h.app.session.undo.len(), 1, "album and all batches share one undo step");
            h.app.session.execute("edit.undo", &json!({})).unwrap();
            assert_eq!(h.app.session.catalog.len(), 0);
            assert_eq!(h.app.session.catalog.albums().count(), 0);
        } else {
            assert_eq!(h.app.session.catalog.len(), 0);
            assert_eq!(h.app.session.catalog.albums().count(), 0, "failed files cannot create an empty album");
            assert_eq!(h.app.session.undo.len(), 0);
        }
    }
}

#[test]
fn import_review_promotes_checked_local_photos_but_blocks_imported_duplicates() {
    use lightcraft_engine::media::ProbeInfo;
    use std::sync::Arc;
    let mut h = headless();
    h.app.session.media.file_probe =
        Some(Arc::new(|path| Ok(ProbeInfo { width: 24, height: 16, format: "PNG".into(), content_hash: Some(path.into()), ..Default::default() })));
    let paths = ["virtual-ui-local.png", "virtual-ui-imported.png"];
    let local = h.app.session.execute("library.import", &json!({"paths":[paths[0]], "local":true})).unwrap()["imported"][0].as_u64().unwrap();
    let imported = h.app.session.execute("library.import", &json!({"paths":[paths[1]]})).unwrap()["imported"][0].as_u64().unwrap();
    let candidates = lightcraft_engine::import::scan(&mut h.app.session, &paths.iter().map(|p| p.to_string()).collect::<Vec<_>>());
    let mut draft = ImportDialog::new(candidates);
    assert_eq!(draft.selected_paths(), vec![paths[0].to_string()]);
    draft.click(1, false);
    assert_eq!(draft.selected_paths(), vec![paths[0].to_string()], "an imported duplicate cannot be checked");
    draft.click(0, false);
    assert!(draft.selected_paths().is_empty());
    draft.click(0, false);
    draft.new_album = "Promoted photos".into();
    h.app.ui.dialog = Some(Dialog::Import { opts: Box::new(draft) });
    assert_eq!(h.request("ui.dialog.confirm", json!({}), T)["ok"], true);
    assert!(h.step_until(T, |h| h.app.import.is_none()));
    assert!(!h.app.session.catalog.photo(lightcraft_catalog::PhotoId(local)).unwrap().local);
    assert!(!h.app.session.catalog.photo(lightcraft_catalog::PhotoId(imported)).unwrap().local);
    assert_eq!(h.app.session.catalog.len(), 2, "promotion must not make a second copy");
    let album = h.app.session.catalog.albums().find(|a| a.name == "Promoted photos").unwrap();
    assert_eq!(album.photos, vec![lightcraft_catalog::PhotoId(local)]);
}

#[test]
fn invalid_scope_is_rejected_before_picker_or_probe_capture() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let mut h = headless();
    let mut draft = ImportDialog::default();
    draft.keywords = "keep draft".into();
    h.app.ui.dialog = Some(Dialog::Import { opts: Box::new(draft.clone()) });
    h.app.session.import_probes.insert("retained.png".into(), Default::default());
    let calls = Arc::new(AtomicUsize::new(0));
    h.app.services.pick_files = Some(Box::new({
        let calls = calls.clone();
        move || {
            calls.fetch_add(1, Ordering::Relaxed);
            vec![]
        }
    }));
    h.app.services.pick_folder = Some(Box::new({
        let calls = calls.clone();
        move || {
            calls.fetch_add(1, Ordering::Relaxed);
            None
        }
    }));
    for value in [json!(null), json!("false"), json!(1), json!([])] {
        for command in ["file.addPhotos", "file.addFolder", "file.addFromDevice"] {
            let response = h.request("engine.execute", json!({"command":command,"params":{"subfolders":value}}), T);
            assert_eq!(response["ok"], false, "{command}: {response}");
            assert_eq!(dialog(&h), &draft);
            assert!(h.app.scan.is_none() && h.app.import.is_none());
            assert!(h.app.session.import_probes.contains_key("retained.png"));
            assert_eq!(calls.load(Ordering::Relaxed), 0);
        }
    }
}
