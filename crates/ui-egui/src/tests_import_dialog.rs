//! Headless import scope and confirmation regressions (issue #332).

use crate::headless::Headless;
use crate::import::ImportDialog;
use crate::state::Dialog;
use crate::{LightcraftApp, Services};
use serde_json::json;
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
    let dir = std::env::temp_dir().join("lc-import-review-scan").join(format!("ui-subfolders-{}", std::process::id()));
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
