//! Headless import scope and file-type regressions (issues #332 and #344).

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

fn types(h: &mut Headless, types: Value) {
    let result = h.request("engine.execute", json!({"command":"import.fileTypes", "params":{"types":types}}), T);
    assert_eq!(result["ok"], true, "{result}");
    draw(h);
}

#[test]
fn import_subfolder_toggle_rescans_and_preserves_options_and_checked_files() {
    let dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/agent-import-dialog/fixtures")
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
    types(&mut h, json!({"formats":["PNG"]}));
    click(&mut h, "check:importSubfolders", false, false);
    assert!(h.step_until(T, |h| h.app.scan.is_none() && matches!(h.app.ui.dialog, Some(Dialog::Import { .. }))));
    draw(&mut h);
    let d = dialog(&h);
    assert!(!d.subfolders);
    assert_eq!(d.candidates.len(), 1);
    assert!(d.candidates[0].path.ends_with("original.png"));
    assert_eq!(d.checked, vec![false]);
    assert_eq!((&d.new_album, &d.keywords), (&"Trip".to_string(), &"landscape".to_string()));
    assert_eq!(h.app.session.catalog.len(), 0, "changing the scan never imports files");
    click(&mut h, "check:importSubfolders", false, false);
    assert!(h.step_until(T, |h| h.app.scan.is_none() && matches!(h.app.ui.dialog, Some(Dialog::Import { .. }))));
    draw(&mut h);
    assert_eq!(dialog(&h).candidates.len(), 2);
    assert_eq!(serde_json::to_value(&dialog(&h).types).unwrap(), json!({"formats":["png"]}));
    assert!(dialog(&h).selected_paths().is_empty(), "the rejected PNG remains unchecked; hidden JPEG stays excluded");
    types(&mut h, json!({"group":"all"}));
    assert_eq!(dialog(&h).selected_paths().len(), 1, "the hidden JPEG choice and rejected PNG survive rescan");
    // Move must not silently reselect a rejected nested original after an off/on round trip.
    let Some(Dialog::Import { opts }) = &mut h.app.ui.dialog else { panic!("no import review") };
    opts.copy = true;
    opts.move_files = true;
    let original = opts.candidates.iter().position(|c| c.path.ends_with("original.png")).unwrap();
    let companion = opts.candidates.iter().position(|c| c.path.ends_with("companion.JPG")).unwrap();
    opts.checked[original] = true;
    opts.checked[companion] = false;
    // Redraw the direct fixture mutation, then hide the rejected JPEG during both rescans.
    types(&mut h, json!({"formats":["png"]}));
    for subfolders in [false, true] {
        click(&mut h, "check:importSubfolders", false, false);
        assert!(h.step_until(T, |h| h.app.scan.is_none() && matches!(h.app.ui.dialog, Some(Dialog::Import { .. }))));
        draw(&mut h);
        let d = dialog(&h);
        assert_eq!(d.subfolders, subfolders);
        assert!(d.copy && d.move_files, "Move mode must survive rescans");
        assert_eq!((&d.new_album, &d.keywords), (&"Trip".to_string(), &"landscape".to_string()));
        assert_eq!(d.selected_paths(), vec![dir.join("original.png").to_string_lossy().to_string()]);
    }
    types(&mut h, json!({"group":"all"}));
    assert_eq!(
        dialog(&h).selected_paths(),
        vec![dir.join("original.png").to_string_lossy().to_string()],
        "returning to All must not reselect the rejected nested JPEG"
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
    click(&mut h, "check:importSubfolders", false, false);
    assert!(h.step_until(T, |h| h.app.scan.is_none() && matches!(h.app.ui.dialog, Some(Dialog::Import { .. }))));
    assert_eq!(dialog(&h).candidates.len(), 1);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn file_type_controls_preserve_hidden_checkboxes_options_and_visible_ranges() {
    use lightcraft_engine::import::ImportCandidate;
    let mut h = headless();
    let mut opts = ImportDialog::new(
        ["original.NEF", "rendered.JPG", "original.DNG", "preview.PNG", "nested/other.NEF"]
            .into_iter()
            .map(|path| ImportCandidate { path: path.into(), name: path.into(), ..Default::default() })
            .collect(),
    );
    assert!(opts.types.is_all(), "existing reviews default to All");
    opts.checked = vec![false, true, true, false, true];
    opts.copy = true;
    opts.move_files = true;
    opts.subfolders = false;
    opts.destination = "chosen destination".into();
    opts.new_album = "Chosen album".into();
    opts.keywords = "keep, these".into();
    opts.last_clicked = Some(1);
    h.app.ui.dialog = Some(Dialog::Import { opts: Box::new(opts) });
    draw(&mut h);
    click(&mut h, "button:importTypesRaw", false, false);
    assert!(dialog(&h).types.is_raw());
    assert_eq!(dialog(&h).checked, [false, true, true, false, true]);
    assert_eq!(dialog(&h).selected_paths(), ["original.DNG", "nested/other.NEF"]);
    assert_eq!(dialog(&h).last_clicked, None, "a hidden range anchor must be discarded");
    assert!(h.app.scan.is_none(), "type filtering must not start filesystem work");
    click(&mut h, "import:0", false, false);
    click(&mut h, "import:2", true, false);
    assert_eq!(dialog(&h).checked, [false, true, false, false, true], "range toggles visible RAWs and leaves the hidden JPEG checked");
    click(&mut h, "button:importTypesAll", false, false);
    assert_eq!(dialog(&h).selected_paths(), ["rendered.JPG", "nested/other.NEF"]);
    types(&mut h, json!({"formats":["JPEG"]}));
    assert_eq!(dialog(&h).selected_paths(), ["rendered.JPG"]);
    click(&mut h, "button:importNone", false, false);
    assert_eq!(dialog(&h).checked, [false, false, false, false, true], "Uncheck All affects the visible type only");
    types(&mut h, json!({"formats":["PNG"]}));
    click(&mut h, "combo:importFileTypes", false, false);
    click(&mut h, "check:importType-jpg", false, false);
    assert_eq!(serde_json::to_value(&dialog(&h).types).unwrap(), json!({"formats":["jpg","png"]}));
    assert_eq!(dialog(&h).checked, [false, false, false, false, true], "adding a type cannot recheck its rejected photos");
    // Close the type dropdown before interacting with the candidate grid.
    if h.app.widgets.iter().any(|(id, _)| id == "check:importType-jpg") {
        click(&mut h, "combo:importFileTypes", false, false);
    }
    click(&mut h, "import:1", false, false);
    click(&mut h, "button:importTypesAll", false, false);
    let opts = dialog(&h);
    assert_eq!(opts.selected_paths(), ["rendered.JPG", "nested/other.NEF"]);
    assert!(opts.copy && opts.move_files && !opts.subfolders);
    assert_eq!(
        (&opts.destination, &opts.new_album, &opts.keywords),
        (&"chosen destination".to_string(), &"Chosen album".to_string(), &"keep, these".to_string())
    );
    assert!(h.app.scan.is_none() && h.app.import.is_none());
    assert_eq!(h.app.session.catalog.len(), 0);
}

#[test]
fn type_api_validation_preserves_the_draft_before_invoking_native_pickers() {
    use std::sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    };
    let mut h = headless();
    h.app.ui.dialog = Some(Dialog::Import { opts: Box::new(ImportDialog { keywords: "keep draft".into(), ..Default::default() }) });
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
    let before = dialog(&h).clone();
    for invalid in [json!("raw"), json!({"group":"unknown"}), json!({"formats":[false]}), json!({"group":"raw","formats":[]})] {
        for command in ["file.addPhotos", "file.addFolder", "import.fileTypes"] {
            let response = h.request("engine.execute", json!({"command":command,"params":{"types":invalid}}), T);
            assert_eq!(response["ok"], false, "{command}: {response}");
            assert_eq!(dialog(&h), &before);
            assert!(h.app.scan.is_none() && h.app.import.is_none());
            assert!(h.app.session.import_probes.contains_key("retained.png"));
            assert_eq!(calls.load(Ordering::Relaxed), 0, "validation must precede even opening a picker");
        }
    }
    types(&mut h, json!({"formats":["JPEG","JPG","TIFF","tif"]}));
    assert_eq!(serde_json::to_value(&dialog(&h).types).unwrap(), json!({"formats":["jpg","tif"]}));
    assert_eq!(dialog(&h).keywords, "keep draft");
}
