//! Actual UI-owned scan/import workers; native execution is delegated to the parent.
use super::*;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, atomic::AtomicUsize, mpsc};
use std::time::{Duration, Instant};

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;
struct Fixture(PathBuf);
impl Fixture {
    fn new(tag: &str) -> TestResult<Self> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/agent-import-run2-fixes/ui-fixtures").join(format!(
            "{tag}-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path)?;
        Ok(Self(path))
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
fn write_png(path: &Path, seed: u8) -> TestResult {
    std::fs::create_dir_all(path.parent().ok_or("missing parent")?)?;
    let image = lightcraft_raster::Rgba8::from_fn(2, 1, |_, _| [seed, 64, 128, 255]);
    std::fs::write(path, lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&image), &Default::default())?)?;
    Ok(())
}
fn app() -> LightcraftApp {
    LightcraftApp::new(lightcraft_engine::Session::new().with_fs(), crate::Services { png: None, ..Default::default() })
}

#[test]
fn local_browse_uses_the_review_walker_scope_and_keeps_type_choices() -> TestResult {
    let fixture = Fixture::new("local-scope")?;
    write_png(&fixture.0.join("source/direct.png"), 16)?;
    write_png(&fixture.0.join("source/nested/child.png"), 48)?;
    write_png(&fixture.0.join("source/.hidden.png"), 96)?;
    let mut app = app();
    let ctx = egui::Context::default();
    for (subfolders, expected) in [(false, 1), (true, 2)] {
        browse(&mut app, fixture.0.join("source").to_str().unwrap(), Some(subfolders))?;
        let started = Instant::now();
        while app.scan.is_some() || app.import.is_some() {
            assert!(started.elapsed() < Duration::from_secs(15));
            poll_scan(&mut app, &ctx);
            tick(&mut app, &ctx);
            std::thread::yield_now();
        }
        assert_eq!(app.session.visible().len(), expected);
        assert!(app.session.catalog.photos().all(|p| p.local && p.file_name != ".hidden.png"));
    }
    open_with_subfolders(&mut app, vec![fixture.0.join("source").to_string_lossy().into()], false)?;
    let started = Instant::now();
    while app.scan.is_some() {
        assert!(started.elapsed() < Duration::from_secs(15));
        poll_scan(&mut app, &ctx);
        std::thread::yield_now();
    }
    let Some(crate::state::Dialog::Import { opts }) = &mut app.ui.dialog else { panic!("missing review") };
    assert_eq!(opts.candidates.len(), 1);
    opts.check_type("PNG", false);
    assert!(opts.selected_paths().is_empty());
    opts.check_type("PNG", true);
    assert_eq!(opts.selected_paths().len(), 1);
    drop(app);
    Ok(())
}

#[test]
fn cancel_local_browse_stops_native_probe_wave_and_cannot_deliver_to_a_new_owner() -> TestResult {
    let fixture = Fixture::new("local-cancel")?;
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8);
    for i in 0..workers * 2 {
        write_png(&fixture.0.join(format!("source/{i}.png")), u8::try_from(i + 16)?)?;
    }
    let mut app = app();
    let native = app.session.media.file_probe.clone().ok_or("missing native probe")?;
    let calls = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let (started, starts) = mpsc::channel();
    let (finished, finishes) = mpsc::channel();
    app.session.media.file_probe = Some(Arc::new({
        let calls = calls.clone();
        let gate = gate.clone();
        move |path| {
            calls.fetch_add(1, Ordering::Relaxed);
            started.send(()).unwrap();
            let release = gate.1.wait_timeout_while(gate.0.lock().unwrap(), Duration::from_secs(20), |r| !*r).unwrap();
            assert!(*release.0);
            let result = native(path);
            finished.send(()).unwrap();
            result
        }
    }));
    browse(&mut app, fixture.0.join("source").to_str().unwrap(), Some(true))?;
    for _ in 0..workers {
        starts.recv_timeout(Duration::from_secs(10))?;
    }
    let task = app.scan.take().ok_or("missing scan owner")?;
    let progress = task.progress.clone();
    // ScanTask has Drop, so keep the receiver in its owner and set the same cancellation
    // flag as the progress-window Cancel. Wait for its actual result before asserting counts.
    progress.cancel.store(true, Ordering::Relaxed);
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    for _ in 0..workers {
        finishes.recv_timeout(Duration::from_secs(10))?;
    }
    let out = task.rx.recv_timeout(Duration::from_secs(10))?;
    assert!(out.candidates.is_empty());
    assert_eq!(calls.load(Ordering::Relaxed), workers);
    poll_scan(&mut app, &egui::Context::default());
    assert!(app.session.catalog.is_empty() && app.import.is_none());
    drop(app);
    Ok(())
}

#[test]
fn cancel_import_run_preserves_the_committed_batch_and_stops_new_native_reads() -> TestResult {
    let fixture = Fixture::new("run-cancel")?;
    let batch = lightcraft_engine::import::batch_size();
    let workers = std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8);
    let paths: Vec<String> = (0..batch * 2)
        .map(|i| {
            let path = fixture.0.join(format!("source/{i:03}.png"));
            write_png(&path, u8::try_from(i + 16).unwrap()).unwrap();
            path.to_string_lossy().into()
        })
        .collect();
    let mut s = lightcraft_engine::Session::new().with_fs();
    s.open_library(fixture.0.join("library"), false)?;
    let native = s.media.file_probe.clone().ok_or("missing native probe")?;
    let calls = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let (started, starts) = mpsc::channel();
    s.media.file_probe = Some(Arc::new({
        let calls = calls.clone();
        let gate = gate.clone();
        move |path| {
            if calls.fetch_add(1, Ordering::Relaxed) >= batch {
                started.send(()).unwrap();
                let release = gate.1.wait_timeout_while(gate.0.lock().unwrap(), Duration::from_secs(20), |r| !*r).unwrap();
                assert!(*release.0);
            }
            native(path)
        }
    }));
    let job = lightcraft_engine::import::ImportJob::new(&mut s, Default::default())?;
    let mut run = ImportRun::start(job, paths, None, None, &egui::Context::default())?;
    for _ in 0..workers {
        starts.recv_timeout(Duration::from_secs(10))?;
    }
    let batches = run.next_batches().ok_or("worker ended before delivering its first batch")?;
    assert_eq!(batches.len(), 1);
    let report = lightcraft_engine::import::commit_prepared(&mut s, &run.opts, &run.now, batches.into_iter().next().unwrap())?;
    assert_eq!(report.imported.len(), batch);
    run.cancel.store(true, Ordering::Relaxed);
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    let start = Instant::now();
    while let Some(batches) = run.next_batches() {
        assert!(start.elapsed() < Duration::from_secs(15));
        for prepared in batches {
            assert!(prepared.is_empty());
            prepared.rollback();
        }
        std::thread::yield_now();
    }
    assert_eq!(calls.load(Ordering::Relaxed), batch + workers);
    assert_eq!(s.catalog.len(), batch);
    assert!(s.catalog.photos().all(|p| matches!(&p.source, lightcraft_catalog::Source::File { path } if Path::new(path).is_file())));
    drop(run);
    drop(s);
    Ok(())
}
