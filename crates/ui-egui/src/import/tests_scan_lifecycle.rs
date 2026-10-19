//! Pending scan ownership and dialog regressions; all filesystem fixtures stay in this checkout.
use super::*;
use crate::state::Dialog;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
    mpsc,
};
use std::time::Duration;

fn app() -> LightcraftApp {
    LightcraftApp::new(lightcraft_engine::Session::new().with_fs(), crate::Services { png: None, ..Default::default() })
}

fn pending(app: &mut LightcraftApp, previous: ImportDialog) -> (mpsc::Sender<ScanOutput>, Arc<ScanProgress>) {
    let progress = Arc::new(ScanProgress::default());
    let (sender, rx) = mpsc::channel();
    app.scan = Some(ScanTask {
        progress: progress.clone(),
        rx,
        copy: previous.copy,
        browse: false,
        sources: previous.sources.clone(),
        subfolders: false,
        review: Some(previous),
    });
    (sender, progress)
}

#[test]
fn newer_dialog_drafts_survive_scan_completion_and_failure() {
    for succeeds in [true, false] {
        let mut app = app();
        let previous = ImportDialog { sources: vec!["photos".into()], keywords: "preserved".into(), copy: true, ..Default::default() };
        let (sender, progress) = pending(&mut app, previous);
        app.run("dialog.newAlbum", json!({"name": "current draft"})).unwrap();
        if succeeds {
            sender
                .send(ScanOutput {
                    has_folders: true,
                    candidates: vec![ImportCandidate { path: "photo.png".into(), ..Default::default() }],
                    probes: Default::default(),
                })
                .unwrap();
        }
        drop(sender);
        let ctx = egui::Context::default();
        poll_scan(&mut app, &ctx);
        assert!(matches!(&app.ui.dialog, Some(Dialog::NewAlbum { name, .. }) if name == "current draft"));
        assert!(app.scan.is_some(), "the completed review waits for the newer dialog");
        assert!(!progress.cancel.load(Ordering::Relaxed));
        app.ui.dialog = None;
        poll_scan(&mut app, &ctx);
        assert!(app.scan.is_none());
        let Some(Dialog::Import { opts }) = &app.ui.dialog else { panic!("previous review was lost") };
        assert_eq!(opts.keywords, "preserved");
        assert!(opts.copy);
        assert_eq!(opts.subfolders, !succeeds);
    }
}

#[test]
fn switching_libraries_cancels_pending_scans_and_discards_old_album_options() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/agent-import-residual-67/fixtures")
        .join(format!("switch-scan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let first = root.join("first");
    let second = root.join("second");
    let mut app = app();
    app.session.open_library(&first, false).unwrap();
    let old_id = app.session.execute("album.create", &json!({"name": "old album"})).unwrap()["id"].as_u64().unwrap();
    {
        let mut other = lightcraft_engine::Session::new().with_fs();
        other.open_library(&second, false).unwrap();
        let new_id = other.execute("album.create", &json!({"name": "new album"})).unwrap()["id"].as_u64().unwrap();
        assert_eq!(old_id, new_id, "album ids may be reused in another library");
        other.close_library().unwrap();
    }
    let previous = ImportDialog { album: Some(old_id), copy: true, destination: "old destination".into(), ..Default::default() };
    app.ui.dialog = Some(Dialog::Import { opts: Box::new(previous.clone()) });
    let (sender, progress) = pending(&mut app, previous);
    app.run("app.openLibrary", json!({"path": second.to_string_lossy()})).unwrap();
    assert!(progress.cancel.load(Ordering::Relaxed));
    assert!(app.scan.is_none());
    assert!(app.ui.dialog.is_none());
    assert_eq!(app.session.catalog.album(lightcraft_catalog::AlbumId(old_id)).unwrap().name, "new album");
    assert!(sender.send(ScanOutput { has_folders: true, candidates: Vec::new(), probes: Default::default() }).is_err());
    poll_scan(&mut app, &egui::Context::default());
    assert!(app.ui.dialog.is_none(), "late results cannot target the replacement library");
    drop(app);
    std::fs::remove_dir_all(root).unwrap();
}

#[test]
fn dropping_the_scan_owner_stops_probing_after_in_flight_files() {
    use std::sync::{Condvar, Mutex};
    let mut app = app();
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8);
    let calls = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let (started, starts) = mpsc::channel();
    let (finished, finishes) = mpsc::channel();
    app.session.media.file_probe = Some(Arc::new({
        let calls = calls.clone();
        let gate = gate.clone();
        move |_| {
            calls.fetch_add(1, Ordering::Relaxed);
            started.send(()).unwrap();
            let _released = gate.1.wait_timeout_while(gate.0.lock().unwrap(), Duration::from_secs(20), |released| !*released).unwrap();
            finished.send(()).unwrap();
            Ok(lightcraft_engine::media::ProbeInfo::default())
        }
    }));
    let paths = (0..workers * 2).map(|i| format!("scan-owner-{i}.png")).collect();
    open(&mut app, paths).unwrap();
    let progress = app.scan.as_ref().unwrap().progress.clone();
    for _ in 0..workers {
        starts.recv_timeout(Duration::from_secs(10)).unwrap();
    }
    drop(app);
    assert!(progress.cancel.load(Ordering::Relaxed));
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    for _ in 0..workers {
        finishes.recv_timeout(Duration::from_secs(10)).unwrap();
    }
    assert!(
        matches!(finishes.recv_timeout(Duration::from_secs(10)), Err(mpsc::RecvTimeoutError::Disconnected)),
        "wait for the worker and its probe callback to drop"
    );
    assert_eq!(calls.load(Ordering::Relaxed), workers, "only files already in flight can finish after the owner is dropped");
}

#[test]
fn queued_move_batches_survive_second_review_confirmation_and_library_switch_attempts() {
    use lightcraft_engine::import::{ImportJob, ImportMode, ImportOptions, Organize};
    use std::sync::atomic::AtomicBool;
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/agent-import-residual-67/fixtures")
        .join(format!("queued-move-switch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    let source = root.join("photo.png");
    let image = lightcraft_raster::Rgba8::from_fn(12, 8, |x, y| [(x * 12) as u8, (y * 20) as u8, 120, 255]);
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&image), &Default::default()).unwrap();
    std::fs::write(&source, bytes).unwrap();
    let first = root.join("first");
    let second = root.join("second");
    let mut app = app();
    app.session.open_library(&first, false).unwrap();
    let paths = vec![source.to_string_lossy().to_string()];
    let mut job = ImportJob::new(&mut app.session, ImportOptions { mode: ImportMode::Move, organize: Organize::Flat, ..Default::default() }).unwrap();
    let prepared = job.prepare(&paths, &AtomicBool::new(false));
    let placed = first.join("Originals/photo.png");
    assert!(source.exists() && placed.exists(), "Move is prepared but not yet catalogued");
    let (sender, rx) = mpsc::channel();
    sender.send(prepared).unwrap();
    drop(sender);
    let cancel = Arc::new(AtomicBool::new(false));
    let mut task = ImportTask::new(paths, json!({"mode": "move"}), app.session.undo.len(), false);
    task.run = Some(ImportRun {
        runner: Runner::Thread(rx),
        cancel: cancel.clone(),
        total: job.total.clone(),
        done: job.done.clone(),
        opts: job.opts.clone(),
        now: job.now().to_string(),
        album: None,
        album_name: None,
    });
    app.import = Some(task);
    // A second review may finish while Move batches await their catalog commit.
    let second_source = root.join("second-review.png");
    let second_image = lightcraft_raster::Rgba8::from_fn(8, 4, |x, y| [(x * 20) as u8, 40, (y * 40) as u8, 255]);
    std::fs::write(
        &second_source,
        lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&second_image), &Default::default()).unwrap(),
    )
    .unwrap();
    app.run("file.addPhotos", json!({"paths": [second_source.to_string_lossy()]})).unwrap();
    let ctx = egui::Context::default();
    let started = std::time::Instant::now();
    while app.scan.is_some() {
        assert!(started.elapsed() < Duration::from_secs(10), "second review did not finish");
        poll_scan(&mut app, &ctx);
        std::thread::yield_now();
    }
    let Some(Dialog::Import { opts }) = &mut app.ui.dialog else { panic!("second review was lost") };
    opts.new_album = "second review album".into();
    opts.check_type("PNG", false);
    opts.check_type("PNG", true);
    let (confirm, _) = crate::control::ControlRequest::new("ui.dialog.confirm", json!({}));
    let crate::control::Outcome::Done(response) = crate::control::handle(&mut app, &ctx, &confirm) else { panic!("confirmation returned no result") };
    assert_eq!(response["ok"], false, "{response}");
    assert!(response["error"].as_str().unwrap().contains("import is running"), "{response}");
    assert!(matches!(&app.ui.dialog, Some(Dialog::Import { opts }) if opts.new_album == "second review album"));
    assert!(app.session.catalog.albums().all(|album| album.name != "second review album"), "rejected confirmation must not create an album");
    let task = app.import.as_ref().unwrap();
    assert_eq!(task.params["mode"], "move");
    assert!(Arc::ptr_eq(&task.run.as_ref().unwrap().cancel, &cancel), "confirmation cannot replace the active Move worker");
    assert!(source.exists() && placed.exists());
    let error = app.run("app.openLibrary", json!({"path": second.to_string_lossy()})).unwrap_err();
    assert!(error.contains("import is running"), "{error}");
    assert_eq!(app.session.library.as_ref().unwrap().dir, first);
    assert!(!second.exists());
    assert!(app.import.is_some() && !cancel.load(Ordering::Relaxed));
    tick(&mut app, &ctx);
    tick(&mut app, &ctx);
    assert!(app.import.is_none());
    assert_eq!(app.session.catalog.len(), 1);
    assert!(!source.exists() && placed.exists(), "the queued Move commits in its original library");
    let crate::control::Outcome::Done(response) = crate::control::handle(&mut app, &ctx, &confirm) else { panic!("confirmation returned no result") };
    assert_eq!(response["ok"], true, "{response}");
    assert!(app.ui.dialog.is_none());
    let started = std::time::Instant::now();
    while app.import.is_some() {
        assert!(started.elapsed() < Duration::from_secs(10), "second import did not finish");
        tick(&mut app, &ctx);
        std::thread::yield_now();
    }
    assert_eq!(app.session.catalog.len(), 2);
    assert!(second_source.exists(), "the second review still uses Add in place");
    assert!(app.session.catalog.albums().any(|album| album.name == "second review album"));
    drop(app);
    std::fs::remove_dir_all(root).unwrap();
}

/// The previous review stays usable after Cancel, but its native probes cannot describe
/// different bytes. Equal file lengths make this exercise modification time, too.
#[test]
fn confirming_a_canceled_rescan_checks_changed_native_sources() {
    use crate::headless::Headless;
    use lightcraft_engine::media::ProbeInfo;
    use std::path::Path;
    fn write_image(path: &Path, width: usize, modified_seconds: u64) {
        let image = lightcraft_raster::Rgba8::from_fn(width, 8, |x, y| [(x * 8) as u8, (y * 20) as u8, 100, 255]);
        let mut bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&image), &Default::default()).unwrap();
        assert!(bytes.len() < 4096);
        bytes.resize(4096, 0);
        std::fs::write(path, bytes).unwrap();
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(std::time::UNIX_EPOCH + Duration::from_secs(modified_seconds)))
            .unwrap();
    }
    for mode in ["add", "move", "copy"] {
        let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/agent-import-residual-67/fixtures")
            .join(format!("changed-after-cancel-{mode}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let source_dir = root.join("source");
        std::fs::create_dir_all(&source_dir).unwrap();
        let source = source_dir.join("photo.png");
        write_image(&source, 12, 1_000_000);
        let mut app = app();
        app.session.open_library(root.join("library"), false).unwrap();
        let calls = Arc::new(AtomicUsize::new(0));
        let probe = app.session.media.file_probe.clone().unwrap();
        app.session.media.file_probe = Some(Arc::new({
            let calls = calls.clone();
            move |path| -> Result<ProbeInfo, String> {
                calls.fetch_add(1, Ordering::Relaxed);
                probe(path)
            }
        }));
        let sources = vec![source_dir.to_string_lossy().to_string()];
        let mut previous = ImportDialog::new(lightcraft_engine::import::scan(&mut app.session, &sources));
        assert_eq!(previous.candidates.len(), 1);
        assert_eq!(previous.candidates[0].width, 12);
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        previous.sources = sources.clone();
        previous.has_folders = true;
        previous.subfolders = false;
        previous.copy = mode != "add";
        previous.move_files = mode == "move";
        previous.organize = "flat".into();
        // Hold the snapshot as a rescan worker would while waiting on another file.
        let (pending_input, _) = ScanInput::new(&mut app.session, &sources);
        let (_sender, progress) = pending(&mut app, previous);
        app.scan.as_mut().unwrap().subfolders = true;
        let mut h = Headless::new(app, [1200.0, 900.0], 1.0);
        for _ in 0..5 {
            h.step();
        }
        write_image(&source, 20, 1_000_002);
        assert_eq!(h.request("ui.clickWidget", json!({"id": "button:scanCancel"}), Duration::from_secs(10))["ok"], true);
        assert!(h.app.scan.is_none() && progress.cancel.load(Ordering::Relaxed));
        let Some(Dialog::Import { opts }) = &h.app.ui.dialog else { panic!("Cancel lost the previous review") };
        assert!(!opts.subfolders);
        assert_eq!(opts.candidates[0].width, 12, "the old review is restored until confirmation");
        assert_eq!(h.request("ui.dialog.confirm", json!({}), Duration::from_secs(10))["ok"], true);
        assert!(h.step_until(Duration::from_secs(20), |h| h.app.import.is_none()));
        let placed = root.join("library/Originals/photo.png");
        if mode == "copy" {
            assert_eq!(h.app.session.catalog.len(), 0, "Copy refuses sources changed since review");
            assert!(source.exists() && !placed.exists());
            assert_eq!(calls.load(Ordering::Relaxed), 1);
        } else {
            assert_eq!(h.app.session.catalog.len(), 1);
            let photo = h.app.session.catalog.photos().next().unwrap();
            assert_eq!((photo.width, photo.height), (20, 8), "{mode} imported stale dimensions");
            assert_eq!(calls.load(Ordering::Relaxed), 2, "{mode} must re-probe changed native bytes");
            let current =
                (h.app.session.media.file_probe.as_ref().unwrap())(if mode == "move" { placed.to_str().unwrap() } else { source.to_str().unwrap() })
                    .unwrap();
            assert_eq!(photo.content_hash, current.content_hash, "{mode} stored a stale hash");
            assert_eq!(source.exists(), mode == "add");
            assert_eq!(placed.exists(), mode == "move");
        }
        drop(pending_input);
        drop(h);
        std::fs::remove_dir_all(root).unwrap();
    }
}
