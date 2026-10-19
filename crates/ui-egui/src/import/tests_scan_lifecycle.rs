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
    let root = std::env::temp_dir().join("lc-import-review-scan").join(format!("switch-scan-{}", std::process::id()));
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
