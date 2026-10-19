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
