//! Actual Linux Session regressions. These require the parent's native engine test build.
use super::*;
use crate::rename::{MoveFs, RealFs};
use serde_json::json;
use std::sync::{
    Arc, Condvar, Mutex,
    atomic::{AtomicBool, AtomicUsize, Ordering},
    mpsc,
};
use std::time::Duration;

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Fixture(PathBuf);
impl Fixture {
    fn new(tag: &str) -> TestResult<Self> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/agent-import-run2-fixes/fixtures").join(format!(
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

fn png(width: usize, seed: u8) -> TestResult<Vec<u8>> {
    let image = lightcraft_raster::Rgba8::from_fn(width, 1, |_, _| [seed, 64, 128, 255]);
    let mut bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&image), &Default::default())?;
    // Equal-sized, valid PNGs: an ancillary tEXt chunk before IEND, including a real CRC.
    let iend = bytes.split_off(bytes.len().checked_sub(12).ok_or("missing IEND")?);
    let length = 4096usize.checked_sub(bytes.len() + iend.len() + 12).ok_or("fixture too large")?;
    let mut chunk = b"tEXtComment\0".to_vec();
    chunk.resize(length + 4, b'x');
    let mut crc = !0u32;
    for byte in &chunk {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ if crc & 1 == 1 { 0xedb88320 } else { 0 };
        }
    }
    bytes.extend(u32::try_from(length)?.to_be_bytes());
    bytes.extend(chunk);
    bytes.extend((!crc).to_be_bytes());
    bytes.extend(iend);
    Ok(bytes)
}

fn write_png(path: &Path, seed: u8) -> TestResult {
    std::fs::create_dir_all(path.parent().ok_or("missing fixture parent")?)?;
    std::fs::write(path, png(1, seed)?)?;
    Ok(())
}

fn session(root: &Path) -> TestResult<Session> {
    let mut s = Session::new().with_fs();
    s.clock = Box::new(|| "2026-01-14T05:58:48".into());
    s.open_library(root.join("library"), false)?;
    Ok(s)
}

fn local(s: &mut Session, path: &Path) -> TestResult<PhotoId> {
    let report = import_with(s, &[path.to_string_lossy().into()], &ImportOptions { local: true, ..Default::default() })?;
    Ok(PhotoId(*report.imported.first().ok_or("Local fixture not imported")?))
}

fn source(s: &Session, id: PhotoId) -> PathBuf {
    let Source::File { path } = &s.catalog.photo(id).unwrap().source else { panic!("expected file") };
    path.into()
}

fn transfer(mode: ImportMode, root: &Path) -> ImportOptions {
    ImportOptions {
        mode,
        destination: Some(root.join("output").to_string_lossy().into()),
        organize: Organize::Flat,
        rename: Some("Trip_{seq:3}".into()),
        ..Default::default()
    }
}

fn preserve_edits(before: &Photo, after: &Photo) {
    assert_eq!(after.id, before.id);
    assert_eq!(after.develop, before.develop);
    assert_eq!(after.meta, before.meta);
    assert_eq!(after.history, before.history);
    assert_eq!(after.versions, before.versions);
    assert_eq!(after.rating, before.rating);
    assert_eq!(after.content_hash, before.content_hash);
}

#[test]
fn local_copy_and_move_retain_ids_virtual_edits_stacks_and_one_alias_transfer() -> TestResult {
    for mode in [ImportMode::Copy, ImportMode::Move] {
        let fixture = Fixture::new(&format!("retained-{mode:?}"))?;
        let root = &fixture.0;
        let path = root.join("source/photo.png");
        write_png(&path, 16)?;
        std::fs::copy(&path, root.join("source/independent.png"))?;
        std::os::unix::fs::symlink(root.join("source"), root.join("alias"))?;
        let mut s = session(root)?;
        let master = local(&mut s, &path)?;
        let independent = local(&mut s, &root.join("source/independent.png"))?;
        s.execute("library.select", &json!({"ids":[master.0]}))?;
        let made = s.execute("photo.virtualCopy", &json!({"ids":[master.0]}))?;
        let copy = PhotoId(made["ids"][0].as_u64().ok_or("missing virtual copy")?);
        s.commit("master rating", Op::SetRating { id: master, rating: 2 })?;
        s.commit("copy rating", Op::SetRating { id: copy, rating: 5 })?;
        s.execute("library.select", &json!({"ids":[copy.0]}))?;
        s.execute("develop.set", &json!({"values":{"light.exposure":1.25}}))?;
        let mut meta = s.catalog.photo(copy).unwrap().meta.clone();
        meta.keywords = vec!["Independent edit".into()];
        s.commit("copy keywords", Op::SetMeta { id: copy, meta: Box::new(meta) })?;
        s.execute("library.browse", &json!({"path":root.join("alias")}))?;
        s.filter.rating = 5;
        assert_eq!(s.visible(), &[copy]);
        s.filter.rating = 0;
        let before =
            (s.catalog.photo(master).unwrap().clone(), s.catalog.photo(copy).unwrap().clone(), s.catalog.photo(independent).unwrap().clone());
        let stack = s.catalog.stack_of(copy).unwrap().clone();
        // Both sidecar naming conventions accompany Move. Catalog edits must win over their rating.
        std::fs::write(path.with_extension("xmp"), "retained stem XMP")?;
        std::fs::write(PathBuf::from(format!("{}.xmp", path.display())), "retained full XMP")?;
        std::fs::create_dir_all(root.join("output"))?;
        std::fs::write(root.join("output/Trip_001.png"), b"existing destination")?;
        let opts = transfer(mode, root);
        let paths = vec![path.to_string_lossy().into(), root.join("alias/photo.png").to_string_lossy().into()];
        let mut job = ImportJob::new(&mut s, opts.clone())?;
        let prepared = job.prepare(&paths, &AtomicBool::new(false));
        // An edit and a new virtual copy during worker preparation are retained, not overwritten.
        s.commit("late copy rating", Op::SetRating { id: copy, rating: 4 })?;
        s.execute("library.select", &json!({"ids":[master.0]}))?;
        let late = s.execute("photo.virtualCopy", &json!({"ids":[master.0]}))?;
        let late = PhotoId(late["ids"][0].as_u64().ok_or("missing late copy")?);
        let copy_before_commit = s.catalog.photo(copy).unwrap().clone();
        let late_before_commit = s.catalog.photo(late).unwrap().clone();
        let report = commit_prepared(&mut s, &opts, job.now(), prepared)?;
        assert!(report.failed.is_empty(), "{:?}", report.failed);
        assert_eq!(s.catalog.len(), 4, "no replacement IDs allocated");
        let mut imported = report.imported.clone();
        imported.sort();
        let mut expected = vec![master.0, copy.0, late.0];
        expected.sort();
        assert_eq!(imported, expected);
        assert_eq!(report.duplicates.len(), 1);
        assert_eq!(report.moved.len(), usize::from(mode == ImportMode::Move));
        let placed = root.join("output/Trip_001-1.png");
        assert_eq!(source(&s, master), placed);
        assert_eq!(source(&s, copy), placed);
        assert_eq!(source(&s, late), placed);
        assert_eq!(source(&s, independent), root.join("source/independent.png"));
        assert!(s.catalog.photo(independent).unwrap().local);
        preserve_edits(&before.0, s.catalog.photo(master).unwrap());
        preserve_edits(&copy_before_commit, s.catalog.photo(copy).unwrap());
        preserve_edits(&late_before_commit, s.catalog.photo(late).unwrap());
        preserve_edits(&before.2, s.catalog.photo(independent).unwrap());
        assert_eq!(s.catalog.stack_of(copy).unwrap().id, stack.id);
        assert_eq!(std::fs::read(root.join("output/Trip_001.png"))?, b"existing destination");
        assert_eq!(std::fs::read(&placed)?, png(1, 16)?);
        assert_eq!(path.exists(), mode == ImportMode::Copy);
        if mode == ImportMode::Move {
            assert_eq!(report.moved[0].sidecars.len(), 2);
            assert_eq!(std::fs::read(placed.with_extension("xmp"))?, b"retained stem XMP");
            assert!(!path.with_extension("xmp").exists());
        }
        s.undo_step()?;
        for id in [master, copy, late] {
            assert!(s.catalog.photo(id).unwrap().local);
            assert!(source(&s, id).is_file(), "Undo must not strand a retained record");
            if mode == ImportMode::Move {
                assert_eq!(source(&s, id), placed);
            }
        }
        s.redo_step()?;
        for id in [master, copy, late] {
            assert!(!s.catalog.photo(id).unwrap().local);
        }
        s.close_library()?;
        s.open_library(root.join("library"), false)?;
        assert_eq!(source(&s, copy), placed);
        preserve_edits(&copy_before_commit, s.catalog.photo(copy).unwrap());
        drop(s);
    }
    Ok(())
}

#[test]
fn local_add_stays_in_place_and_retains_the_existing_record() -> TestResult {
    let fixture = Fixture::new("add")?;
    let path = fixture.0.join("source/photo.png");
    write_png(&path, 16)?;
    let mut s = session(&fixture.0)?;
    let id = local(&mut s, &path)?;
    s.commit("rating", Op::SetRating { id, rating: 5 })?;
    s.execute("library.select", &json!({"ids":[id.0]}))?;
    let made = s.execute("photo.virtualCopy", &json!({"ids":[id.0]}))?;
    let copy = PhotoId(made["ids"][0].as_u64().ok_or("missing copy")?);
    s.commit("independent copy rating", Op::SetRating { id: copy, rating: 3 })?;
    let before = s.catalog.photo(id).unwrap().clone();
    let copy_before = s.catalog.photo(copy).unwrap().clone();
    let report = import_with(&mut s, &[path.to_string_lossy().into()], &transfer(ImportMode::Add, &fixture.0))?;
    let mut imported = report.imported.clone();
    imported.sort();
    assert_eq!(imported, [id.0, copy.0]);
    assert!(report.moved.is_empty() && report.failed.is_empty());
    assert_eq!(source(&s, id), path);
    assert_eq!(source(&s, copy), path);
    assert!(!s.catalog.photo(copy).unwrap().local);
    preserve_edits(&copy_before, s.catalog.photo(copy).unwrap());
    assert!(!fixture.0.join("output").exists());
    preserve_edits(&before, s.catalog.photo(id).unwrap());
    s.undo_step()?;
    assert!(s.catalog.photo(id).unwrap().local);
    assert!(s.catalog.photo(copy).unwrap().local);
    drop(s);
    Ok(())
}

#[test]
fn changed_local_inode_is_refused_in_all_modes_then_reload_allows_accurate_promotion() -> TestResult {
    use std::os::unix::fs::MetadataExt;
    for mode in [ImportMode::Add, ImportMode::Copy, ImportMode::Move] {
        for rescan in [false, true] {
            let fixture = Fixture::new(&format!("fresh-{mode:?}-{rescan}"))?;
            let path = fixture.0.join("source/photo.png");
            write_png(&path, 16)?;
            let mut s = session(&fixture.0)?;
            let id = local(&mut s, &path)?;
            s.commit("rating", Op::SetRating { id, rating: 5 })?;
            let before = s.catalog.photo(id).unwrap().clone();
            let paths = vec![path.to_string_lossy().into_owned()];
            let reviewed = scan(&mut s, &paths);
            assert!(reviewed[0].error.is_none());
            let old_stamp = s.import_probes.get(&paths[0]).unwrap().clone();
            let old_meta = std::fs::metadata(&path)?;
            let next = path.with_extension("replacement");
            let bytes = png(2, 48)?;
            assert_eq!(bytes.len() as u64, old_meta.len());
            std::fs::write(&next, &bytes)?;
            std::fs::File::options().write(true).open(&next)?.set_times(std::fs::FileTimes::new().set_modified(old_meta.modified()?))?;
            std::fs::rename(next, &path)?;
            let current = std::fs::metadata(&path)?;
            assert_eq!(current.len(), old_meta.len());
            assert_eq!(current.modified()?, old_meta.modified()?);
            assert_ne!(current.ino(), old_meta.ino());
            assert!(!old_stamp.cache_is_current(&paths[0]));
            if rescan {
                let review = scan(&mut s, &paths);
                assert_eq!((review[0].width, review[0].height), (2, 1));
                assert!(review[0].error.as_deref().is_some_and(|e| e.contains("Reload")));
            }
            let opts = transfer(mode, &fixture.0);
            let refused = import_with(&mut s, &paths, &opts)?;
            assert!(refused.imported.is_empty());
            assert_eq!(refused.failed.len(), 1);
            assert_eq!(s.catalog.len(), 1);
            assert!(s.catalog.photo(id).unwrap().local);
            preserve_edits(&before, s.catalog.photo(id).unwrap());
            assert_eq!(std::fs::read(&path)?, bytes);
            assert!(!fixture.0.join("output").exists());
            s.execute("library.select", &json!({"ids":[id.0]}))?;
            s.execute("photo.reload", &json!({"ids":[id.0]}))?;
            let reload = s.catalog.photo(id).unwrap().clone();
            assert_eq!((reload.width, reload.height), (2, 1));
            assert_ne!(reload.content_hash, before.content_hash);
            assert_eq!(reload.develop, before.develop);
            assert_eq!(reload.rating, before.rating);
            let report = import_with(&mut s, &paths, &opts)?;
            assert_eq!(report.imported, [id.0]);
            preserve_edits(&reload, s.catalog.photo(id).unwrap());
            assert_eq!(std::fs::read(source(&s, id))?, bytes);
            drop(s);
        }
    }
    Ok(())
}

// Retain real production repair work using an injected filesystem error, rather than
// constructing an otherwise private repair descriptor or editing history-worker modules.
struct PartialRename {
    sidecar: PathBuf,
    rollback: PathBuf,
}
impl MoveFs for PartialRename {
    fn exists(&self, p: &Path) -> bool {
        RealFs.exists(p)
    }
    fn is_file(&self, p: &Path) -> bool {
        RealFs.is_file(p)
    }
    fn same_file(&self, a: &Path, b: &Path) -> bool {
        RealFs.same_file(a, b)
    }
    fn rename_no_replace(&self, a: &Path, b: &Path) -> std::io::Result<()> {
        if a == self.sidecar || a == self.rollback {
            Err(std::io::Error::new(std::io::ErrorKind::PermissionDenied, "transient test fault"))
        } else {
            RealFs.rename_no_replace(a, b)
        }
    }
    fn copy_no_replace(&self, a: &Path, b: &Path) -> std::io::Result<()> {
        RealFs.copy_no_replace(a, b)
    }
    fn remove_file(&self, p: &Path) -> std::io::Result<()> {
        RealFs.remove_file(p)
    }
    fn stem_sibling(&self, p: &Path) -> Option<String> {
        RealFs.stem_sibling(p)
    }
}

// A real Linux store with a controllable journal-append failure. Atomic-write faults do
// not cover FsStore's append handle, so inject at the Store API actually used by persist.
struct FailingAppend {
    store: lightcraft_catalog::FsStore,
    fail: Arc<AtomicBool>,
}
impl lightcraft_catalog::Store for FailingAppend {
    fn read(&mut self, name: &str) -> std::io::Result<Option<Vec<u8>>> {
        lightcraft_catalog::Store::read(&mut self.store, name)
    }
    fn write_atomic(&mut self, name: &str, data: &[u8]) -> std::io::Result<()> {
        lightcraft_catalog::Store::write_atomic(&mut self.store, name, data)
    }
    fn append(&mut self, name: &str, data: &[u8]) -> std::io::Result<()> {
        if self.fail.load(Ordering::Relaxed) {
            Err(std::io::Error::other("injected journal failure"))
        } else {
            lightcraft_catalog::Store::append(&mut self.store, name, data)
        }
    }
    fn truncate(&mut self, name: &str, len: u64) -> std::io::Result<()> {
        lightcraft_catalog::Store::truncate(&mut self.store, name, len)
    }
    fn describe(&self) -> String {
        lightcraft_catalog::Store::describe(&self.store)
    }
}

#[test]
fn local_move_persistence_failure_keeps_verified_destination_and_original() -> TestResult {
    let fixture = Fixture::new("save-failure")?;
    let path = fixture.0.join("source/photo.png");
    write_png(&path, 16)?;
    std::fs::write(path.with_extension("xmp"), b"retained XMP")?;
    let fail = Arc::new(AtomicBool::new(false));
    let library = fixture.0.join("library");
    let mut s = Session::new().with_fs();
    s.open_library_in(
        crate::library::LibraryStores {
            dir: library.clone(),
            catalog: Box::new(FailingAppend { store: lightcraft_catalog::FsStore::open(&library)?, fail: fail.clone() }),
            files: Box::new(lightcraft_catalog::FsStore::open(&library)?),
            on_disk: true,
        },
        false,
    )?;
    let id = local(&mut s, &path)?;
    s.persist()?;
    let before = s.catalog.photo(id).unwrap().clone();
    fail.store(true, Ordering::Relaxed);
    let report = import_with(&mut s, &[path.to_string_lossy().into()], &transfer(ImportMode::Move, &fixture.0))?;
    fail.store(false, Ordering::Relaxed);
    assert_eq!(report.imported, [id.0]);
    assert!(report.moved.is_empty());
    assert_eq!(report.kept.len(), 1);
    assert!(path.is_file() && path.with_extension("xmp").is_file());
    assert_eq!(source(&s, id), fixture.0.join("output/Trip_001.png"));
    assert_eq!(std::fs::read(source(&s, id))?, std::fs::read(&path)?);
    preserve_edits(&before, s.catalog.photo(id).unwrap());
    s.persist()?;
    s.close_library()?;
    s.open_library(fixture.0.join("library"), false)?;
    assert_eq!(source(&s, id), fixture.0.join("output/Trip_001.png"));
    assert!(path.is_file(), "a later save does not silently delete the retained source");
    drop(s);
    Ok(())
}

#[test]
fn local_move_rollback_and_owner_fences_keep_source_sidecars_and_edits() -> TestResult {
    for fault in [crate::import_move::Fault::FailWrite, crate::import_move::Fault::Corrupt] {
        let fixture = Fixture::new("failed-placement")?;
        let path = fixture.0.join("source/photo.png");
        write_png(&path, 16)?;
        std::fs::write(path.with_extension("xmp"), b"retained XMP")?;
        let mut s = session(&fixture.0)?;
        let id = local(&mut s, &path)?;
        let before = s.catalog.to_snapshot();
        crate::import_move::inject(fault);
        let result = import_with(&mut s, &[path.to_string_lossy().into()], &transfer(ImportMode::Move, &fixture.0));
        crate::import_move::inject(crate::import_move::Fault::None);
        assert_eq!(result?.failed.len(), 1);
        assert_eq!(s.catalog.to_snapshot(), before);
        assert!(path.is_file() && path.with_extension("xmp").is_file());
        assert!(s.catalog.photo(id).unwrap().local);
        assert!(!fixture.0.join("output/Trip_001.png").exists());
        drop(s);
    }
    for fence in ["library", "source", "repair", "remove"] {
        let fixture = Fixture::new(fence)?;
        let path = fixture.0.join("source/photo.png");
        write_png(&path, 16)?;
        std::fs::write(path.with_extension("xmp"), b"retained XMP")?;
        let mut s = session(&fixture.0)?;
        let id = local(&mut s, &path)?;
        let opts = transfer(ImportMode::Move, &fixture.0);
        let mut job = ImportJob::new(&mut s, opts.clone())?;
        let prepared = job.prepare(&[path.to_string_lossy().into()], &AtomicBool::new(false));
        assert!(fixture.0.join("output/Trip_001.png").is_file());
        match fence {
            "library" => {
                s.open_library(fixture.0.join("other-library"), false)?;
            }
            "source" => s.commit(
                "relink",
                Op::Relink {
                    id,
                    file_name: "other.png".into(),
                    source: Source::File { path: fixture.0.join("other.png").to_string_lossy().into() },
                    format: None,
                },
            )?,
            "repair" => {
                let other = fixture.0.join("unrelated/a.png");
                write_png(&other, 64)?;
                std::fs::write(other.with_extension("xmp"), b"unrelated retained XMP")?;
                let other_id = local(&mut s, &other)?;
                let fault = PartialRename { sidecar: other.with_extension("xmp"), rollback: other.with_file_name("Trip.png") };
                let plans = s.plan_rename_with(&fault, &[other_id], "Trip", 1);
                assert!(s.apply_rename_with(&fault, &plans).is_err());
                assert!(!s.pending_sidecar_repairs.is_empty() || s.undo.iter().any(|e| !e.retry_sidecars.is_empty()));
            }
            _ => s.commit("remove", Op::RemovePhoto { id })?,
        }
        let before = s.catalog.to_snapshot();
        let report = commit_prepared(&mut s, &opts, job.now(), prepared)?;
        assert_eq!(report.failed.len(), 1);
        assert!(report.imported.is_empty());
        assert_eq!(s.catalog.to_snapshot(), before);
        assert!(path.is_file() && path.with_extension("xmp").is_file());
        assert!(!fixture.0.join("output/Trip_001.png").exists());
        assert!(!fixture.0.join("output/Trip_001.xmp").exists());
        drop(s);
    }
    Ok(())
}

#[test]
fn cancelled_import_stops_new_probe_waves_and_preserves_a_committed_batch() -> TestResult {
    let fixture = Fixture::new("cancel-owner")?;
    let workers = workers(usize::MAX);
    let first = fixture.0.join("first.png");
    write_png(&first, 1)?;
    let files: Vec<String> = (0..workers * 2)
        .map(|i| {
            let path = fixture.0.join(format!("source/{i}.png"));
            write_png(&path, u8::try_from(i + 16).unwrap()).unwrap();
            path.to_string_lossy().into()
        })
        .collect();
    let mut s = session(&fixture.0)?;
    let mut first_job = ImportJob::new(&mut s, Default::default())?;
    let committed = first_job.prepare(&[first.to_string_lossy().into()], &AtomicBool::new(false));
    let report = commit_prepared(&mut s, &Default::default(), first_job.now(), committed)?;
    let retained = PhotoId(report.imported[0]);
    let native = s.media.file_probe.clone().ok_or("missing native probe")?;
    let calls = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new((Mutex::new(false), Condvar::new()));
    let (started, starts) = mpsc::channel();
    s.media.file_probe = Some(Arc::new({
        let calls = calls.clone();
        let gate = gate.clone();
        move |path| {
            calls.fetch_add(1, Ordering::Relaxed);
            started.send(()).unwrap();
            let released = gate.1.wait_timeout_while(gate.0.lock().unwrap(), Duration::from_secs(20), |released| !*released).unwrap();
            assert!(*released.0, "probe gate timed out");
            native(path)
        }
    }));
    let mut job = ImportJob::new(&mut s, Default::default())?;
    let cancel = Arc::new(AtomicBool::new(false));
    let owner = cancel.clone();
    let join = std::thread::spawn(move || job.prepare_files(files, &owner));
    for _ in 0..workers {
        starts.recv_timeout(Duration::from_secs(10))?;
    }
    cancel.store(true, Ordering::Relaxed);
    *gate.0.lock().unwrap() = true;
    gate.1.notify_all();
    let prepared = join.join().unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), workers, "no subsequent probe wave after Cancel");
    assert!(prepared.is_empty(), "unplanned canceled probes do not promote anything");
    assert_eq!(s.catalog.len(), 1);
    assert!(s.catalog.photo(retained).is_some());
    assert!(first.is_file());
    let mut pre_cancel = ImportJob::new(&mut s, Default::default())?;
    let stopped = pre_cancel.prepare(&[fixture.0.to_string_lossy().into()], &AtomicBool::new(true));
    assert_eq!(stopped.scanned, 0, "initial scoped walker uses the owner's cancellation");
    assert_eq!(pre_cancel.total.load(Ordering::Relaxed), 0);
    assert_eq!(calls.load(Ordering::Relaxed), workers);
    drop(s);
    Ok(())
}

#[test]
fn local_move_uses_existing_repair_barrier_at_sources_and_destination_before_probing() -> TestResult {
    for full in [false, true] {
        for queue in ["detached", "undo", "redo"] {
            let fixture = Fixture::new(&format!("local-barrier-{full}-{queue}"))?;
            let path = fixture.0.join("source/a.png");
            write_png(&path, 16)?;
            let sidecar = if full { PathBuf::from(format!("{}.xmp", path.display())) } else { path.with_extension("xmp") };
            std::fs::write(&sidecar, b"retained XMP")?;
            let mut s = session(&fixture.0)?;
            let id = local(&mut s, &path)?;
            let moved = path.with_file_name("Trip.png");
            let fault = PartialRename { sidecar: sidecar.clone(), rollback: moved.clone() };
            let plans = s.plan_rename_with(&fault, &[id], "Trip", 1);
            assert!(s.apply_rename_with(&fault, &plans).is_err());
            let repairs: Vec<_> =
                s.pending_sidecar_repairs.iter().chain(s.undo.iter().chain(&s.redo).flat_map(|e| &e.retry_sidecars)).cloned().collect();
            assert_eq!(repairs.len(), 1);
            // Keep the exact descriptor made by rename; move it among its real retry owners.
            s.pending_sidecar_repairs.clear();
            for entry in s.undo.iter_mut().chain(&mut s.redo) {
                entry.retry_sidecars.clear();
            }
            s.commit("unrelated rating", Op::SetRating { id, rating: 4 })?;
            if queue == "redo" {
                s.undo_step()?;
            }
            match queue {
                "undo" => s.undo.last_mut().unwrap().retry_sidecars = repairs.clone(),
                "redo" => s.redo.last_mut().unwrap().retry_sidecars = repairs.clone(),
                _ => s.pending_sidecar_repairs = repairs.clone(),
            }
            std::fs::copy(&moved, &path)?;
            std::os::unix::fs::symlink(fixture.0.join("source"), fixture.0.join("alias"))?;
            let calls = Arc::new(AtomicUsize::new(0));
            let native = s.media.file_probe.clone().ok_or("missing native probe")?;
            s.media.file_probe = Some(Arc::new({
                let calls = calls.clone();
                move |p| {
                    calls.fetch_add(1, Ordering::Relaxed);
                    native(p)
                }
            }));
            let snapshot = s.catalog.to_snapshot();
            for incoming in [&path, &moved, &fixture.0.join("alias/Trip.png")] {
                let opts = transfer(ImportMode::Move, &fixture.0);
                let report = import_with(&mut s, &[incoming.to_string_lossy().into()], &opts)?;
                assert_eq!(report.failed.len(), 1);
                assert!(report.failed[0].1.contains("unfinished XMP repair"));
                assert_eq!(s.catalog.to_snapshot(), snapshot);
                assert_eq!(calls.load(Ordering::Relaxed), 0);
                assert!(path.is_file() && moved.is_file() && sidecar.is_file());
            }
            let other = fixture.0.join("fresh.png");
            write_png(&other, 48)?;
            let opts =
                ImportOptions { mode: ImportMode::Move, destination: Some(fixture.0.join("alias").to_string_lossy().into()), ..Default::default() };
            assert_eq!(import_with(&mut s, &[other.to_string_lossy().into()], &opts)?.failed.len(), 1);
            assert_eq!(calls.load(Ordering::Relaxed), 0);
            let mut cancelled = ImportJob::new(&mut s, opts)?;
            assert!(cancelled.prepare(&[moved.to_string_lossy().into()], &AtomicBool::new(true)).is_empty());
            assert!(cancelled.move_repair_paths.is_none());
            assert_eq!(retained_sidecars(&s), repairs.iter().map(|r| (r.from.clone(), r.to.clone())).collect::<Vec<_>>());
            drop(s);
        }
    }
    Ok(())
}

#[test]
fn unchanged_local_review_is_cached_and_cancelled_prepared_transfers_rollback_only_new_files() -> TestResult {
    for mode in [ImportMode::Copy, ImportMode::Move] {
        let fixture = Fixture::new(&format!("cached-rollback-{mode:?}"))?;
        let path = fixture.0.join("source/photo.png");
        write_png(&path, 16)?;
        std::fs::write(path.with_extension("xmp"), b"retained XMP")?;
        let mut s = session(&fixture.0)?;
        let id = local(&mut s, &path)?;
        let native = s.media.file_probe.clone().ok_or("missing native probe")?;
        let calls = Arc::new(AtomicUsize::new(0));
        s.media.file_probe = Some(Arc::new({
            let calls = calls.clone();
            move |p| {
                calls.fetch_add(1, Ordering::Relaxed);
                native(p)
            }
        }));
        let paths = vec![path.to_string_lossy().into_owned()];
        assert!(scan(&mut s, &paths)[0].error.is_none());
        assert!(scan(&mut s, &paths)[0].error.is_none());
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        let before = s.catalog.to_snapshot();
        let mut job = ImportJob::new(&mut s, transfer(mode, &fixture.0))?;
        let prepared = job.prepare(&paths, &AtomicBool::new(false));
        assert_eq!(calls.load(Ordering::Relaxed), 1, "confirmation reuses validated native facts");
        assert!(fixture.0.join("output/Trip_001.png").is_file());
        prepared.rollback();
        assert_eq!(s.catalog.to_snapshot(), before);
        assert!(s.catalog.photo(id).unwrap().local);
        assert!(!fixture.0.join("output/Trip_001.png").exists());
        assert!(!fixture.0.join("output/Trip_001.xmp").exists());
        assert!(path.is_file() && path.with_extension("xmp").is_file());
        drop(s);
    }
    Ok(())
}

#[test]
fn retained_local_copy_is_promotable_when_a_saved_master_already_belongs_to_the_library() -> TestResult {
    for mode in [ImportMode::Copy, ImportMode::Move] {
        let fixture = Fixture::new(&format!("partial-add-{mode:?}"))?;
        let path = fixture.0.join("source/photo.png");
        write_png(&path, 16)?;
        let mut s = session(&fixture.0)?;
        let master = local(&mut s, &path)?;
        s.execute("library.select", &json!({"ids":[master.0]}))?;
        let made = s.execute("photo.virtualCopy", &json!({"ids":[master.0]}))?;
        let copy = PhotoId(made["ids"][0].as_u64().ok_or("missing copy")?);
        s.commit("copy rating", Op::SetRating { id: copy, rating: 5 })?;
        // Exact catalog state saved by C0 Add: the master was promoted, its copy stayed Local.
        s.commit("prior Add", Op::SetLocal { id: master, local: false })?;
        s.close_library()?;
        s.open_library(fixture.0.join("library"), false)?;
        let before = (s.catalog.photo(master).unwrap().clone(), s.catalog.photo(copy).unwrap().clone());
        std::os::unix::fs::symlink(fixture.0.join("source"), fixture.0.join("alias"))?;
        let paths = vec![fixture.0.join("alias/photo.png").to_string_lossy().into()];
        let review = scan(&mut s, &paths);
        assert_eq!(review[0].existing, Some(copy.0));
        assert!(review[0].duplicate.is_none() && review[0].error.is_none());
        let report = import_with(&mut s, &paths, &transfer(mode, &fixture.0))?;
        assert_eq!(report.imported, [copy.0]);
        assert!(report.failed.is_empty());
        assert_eq!(source(&s, master), fixture.0.join("output/Trip_001.png"));
        assert_eq!(source(&s, copy), source(&s, master));
        assert!(!s.catalog.photo(master).unwrap().local && !s.catalog.photo(copy).unwrap().local);
        preserve_edits(&before.0, s.catalog.photo(master).unwrap());
        preserve_edits(&before.1, s.catalog.photo(copy).unwrap());
        assert_eq!(s.catalog.len(), 2);
        drop(s);
    }
    Ok(())
}

#[test]
fn local_move_rejects_new_alias_source_between_prepare_and_commit() -> TestResult {
    let fixture = Fixture::new("late-alias-source")?;
    let path = fixture.0.join("source/photo.png");
    write_png(&path, 32)?;
    std::os::unix::fs::symlink(fixture.0.join("source"), fixture.0.join("alias"))?;
    let alias = fixture.0.join("alias/photo.png");
    let mut s = session(&fixture.0)?;
    let master = local(&mut s, &path)?;
    let opts = transfer(ImportMode::Move, &fixture.0);
    let mut job = ImportJob::new(&mut s, opts.clone())?;
    let prepared = job.prepare(&[path.to_string_lossy().into()], &AtomicBool::new(false));
    s.execute("library.select", &json!({"ids":[master.0]}))?;
    let made = s.execute("photo.virtualCopy", &json!({"ids":[master.0]}))?;
    let late = PhotoId(made["ids"][0].as_u64().ok_or("missing late virtual copy")?);
    s.execute("photo.relink", &json!({"id":late.0,"path":alias}))?;
    let before = s.catalog.to_snapshot();
    let report = commit_prepared(&mut s, &opts, job.now(), prepared)?;
    assert_eq!(report.failed.len(), 1, "new alias must force fresh worker identity review");
    assert!(report.imported.is_empty() && report.moved.is_empty());
    assert_eq!(s.catalog.to_snapshot(), before);
    assert!(path.is_file() && source(&s, late).is_file());
    assert!(!fixture.0.join("output/Trip_001.png").exists());
    Ok(())
}
