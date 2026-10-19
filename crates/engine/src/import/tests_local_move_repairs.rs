//! Linux Session regressions for aliases, native review freshness and retained Move repairs.
use super::*;
use crate::rename::{MoveFs, RealFs, SidecarMove};
use crate::sidecar::{SidecarNaming, sidecar_path};
use lightcraft_catalog::Op;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Fixture(PathBuf);
impl Fixture {
    fn new(tag: &str) -> TestResult<Self> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = std::env::temp_dir().join(format!("lc-import-run1-{tag}-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::create_dir(&path)?;
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
    Ok(lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&image), &Default::default())?)
}

fn write_png(path: &Path, seed: u8) -> TestResult {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, png(2, seed)?)?;
    Ok(())
}

fn retained(s: &Session) -> Vec<SidecarMove> {
    s.pending_sidecar_repairs.iter().chain(s.undo.iter().chain(&s.redo).flat_map(|e| &e.retry_sidecars)).cloned().collect()
}

// Inject only the transient sidecar and photo rollback errors; all other operations use Linux files.
struct PartialRename {
    sidecar: PathBuf,
    photo: PathBuf,
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
        if a == self.sidecar.as_path() || a == self.photo.as_path() {
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

fn repair_session(root: &Path, naming: SidecarNaming, upper: bool, queue: &str) -> TestResult<Session> {
    let from = root.join("source/a.png");
    let to = root.join("source/Trip.png");
    write_png(&from, 16)?;
    let mut old_sidecar = sidecar_path(&from.to_string_lossy(), naming);
    if upper {
        old_sidecar = old_sidecar.with_extension("XMP");
    }
    let xmp = br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:crs="http://ns.adobe.com/camera-raw-settings/1.0/" xmp:Rating="4" crs:Exposure2012="1.25"/></rdf:RDF></x:xmpmeta>"#;
    std::fs::write(&old_sidecar, xmp)?;
    let mut s = Session::new().with_fs();
    s.open_library(root.join("library-a"), false)?;
    import_with(&mut s, &[from.to_string_lossy().into()], &Default::default())?;
    let id = s.catalog.photos().next().ok_or("missing fixture photo")?.id;
    let fs = PartialRename { sidecar: old_sidecar, photo: to.clone() };
    let plans = s.plan_rename_with(&fs, &[id], "Trip", 1);
    if s.apply_rename_with(&fs, &plans).is_ok() {
        return Err("partial rename fixture unexpectedly succeeded".into());
    }
    if retained(&s).len() != 1 || !to.is_file() {
        return Err("missing retained repair fixture".into());
    }
    s.persist()?;
    s.open_library(root.join("library-b"), false)?;
    if s.pending_sidecar_repairs.len() != 1 {
        return Err("library switch lost detached repair".into());
    }
    // The repair's original endpoint can still belong to a separate retained file.
    std::fs::copy(&to, &from)?;
    write_png(&root.join("sentinel.png"), 96)?;
    import_with(&mut s, &[root.join("sentinel.png").to_string_lossy().into()], &Default::default())?;
    let sentinel = s.catalog.photos().next().ok_or("missing sentinel photo")?.id;
    s.commit("sentinel rating", Op::SetRating { id: sentinel, rating: 3 })?;
    if queue == "redo" {
        s.undo_step()?;
    }
    if queue == "undo" || queue == "redo" {
        let pending = std::mem::take(&mut s.pending_sidecar_repairs);
        let entry = if queue == "redo" { s.redo.last_mut() } else { s.undo.last_mut() }.ok_or("missing history entry")?;
        entry.retry_sidecars = pending;
    }
    Ok(s)
}

#[test]
fn move_refuses_retained_stem_full_repairs_at_both_endpoints_across_library_switch() -> TestResult {
    for naming in [SidecarNaming::Stem, SidecarNaming::Full] {
        for upper in [false, true] {
            for queue in ["detached", "undo", "redo"] {
                let fixture = Fixture::new("move-repair")?;
                let mut s = repair_session(&fixture.0, naming, upper, queue)?;
                let calls = count_probes(&mut s)?;
                for name in ["a.png", "Trip.png"] {
                    let path = fixture.0.join("source").join(name);
                    let source_bytes = std::fs::read(&path)?;
                    let repairs = retained(&s);
                    let xmp = std::fs::read(&repairs[0].from)?;
                    let snapshot = s.catalog.to_snapshot();
                    let history = (s.undo.len(), s.redo.len());
                    let dest = fixture.0.join("output");
                    let result = s.execute(
                        "library.import",
                        &json!({"paths":[path],"mode":"move","destination":dest,"keywords":["must not apply"],"albumName":"must not create"}),
                    )?;
                    assert_eq!(result["imported"], json!([]));
                    assert_eq!(result["failed"].as_array().unwrap().len(), 1);
                    assert!(result["failed"][0][1].as_str().unwrap().contains("unfinished XMP repair"));
                    assert_eq!(calls.load(Ordering::Relaxed), 0, "refusal precedes probing");
                    assert_eq!(std::fs::read(&path)?, source_bytes);
                    assert_eq!(std::fs::read(&repairs[0].from)?, xmp);
                    assert!(!repairs[0].to.exists());
                    assert!(!dest.exists());
                    assert_eq!(s.catalog.to_snapshot(), snapshot);
                    assert_eq!((s.undo.len(), s.redo.len()), history);
                    assert_eq!(retained(&s), repairs);
                    assert_eq!(s.catalog.albums().count(), 0);
                }
                drop(s);
            }
        }
    }
    Ok(())
}

#[test]
fn move_preflight_covers_directory_aliases_and_output_repairs_but_allows_unrelated_moves() -> TestResult {
    let fixture = Fixture::new("move-scope")?;
    let mut s = repair_session(&fixture.0, SidecarNaming::Full, false, "detached")?;
    let calls = count_probes(&mut s)?;
    std::os::unix::fs::symlink(fixture.0.join("source"), fixture.0.join("alias"))?;
    let repairs = retained(&s);
    let opts = ImportOptions { mode: ImportMode::Move, destination: Some(fixture.0.join("output").to_string_lossy().into()), ..Default::default() };
    let mut cancelled = ImportJob::new(&mut s, opts.clone())?;
    let prepared =
        cancelled.prepare_files(vec![fixture.0.join("alias/Trip.png").to_string_lossy().into()], &std::sync::atomic::AtomicBool::new(true));
    assert!(prepared.is_empty());
    assert!(cancelled.move_repair_paths.is_none(), "cancelled preflight never installs a partial map");
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    drop(cancelled);
    let report = import_with(&mut s, &[fixture.0.join("alias/Trip.png").to_string_lossy().into()], &opts)?;
    assert_eq!(report.failed.len(), 1);
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    // Even an unrelated source cannot be placed under a root with an exact retained repair.
    let fresh = fixture.0.join("fresh/photo.png");
    write_png(&fresh, 48)?;
    let into_repair =
        ImportOptions { mode: ImportMode::Move, destination: Some(fixture.0.join("alias").to_string_lossy().into()), ..Default::default() };
    let before = s.catalog.to_snapshot();
    let history = (s.undo.len(), s.redo.len());
    let refused = import_with(&mut s, &[fresh.to_string_lossy().into()], &into_repair)?;
    assert_eq!(refused.failed.len(), 1);
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    assert!(fresh.exists());
    assert_eq!(s.catalog.to_snapshot(), before);
    assert_eq!((s.undo.len(), s.redo.len()), history);
    assert_eq!(retained(&s), repairs);
    // Retained work elsewhere does not turn Move into a global prohibition.
    let xmp=br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmp:Rating="4"/></rdf:RDF></x:xmpmeta>"#;
    std::fs::write(fresh.with_extension("xmp"), xmp)?;
    std::fs::write(sidecar_path(&fresh.to_string_lossy(), SidecarNaming::Full), xmp)?;
    let accepted = import_with(&mut s, &[fresh.to_string_lossy().into()], &opts)?;
    assert_eq!(accepted.imported.len(), 1);
    assert_eq!(accepted.moved.len(), 1);
    assert_eq!(accepted.moved[0].sidecars.len(), 2);
    assert!(!fresh.exists());
    assert_eq!(s.catalog.photo(PhotoId(accepted.imported[0])).unwrap().rating, 4);
    assert_eq!(retained(&s), repairs);
    drop(s);
    Ok(())
}

#[test]
fn move_can_retry_after_existing_history_barrier_finishes_detached_xmp() -> TestResult {
    for naming in [SidecarNaming::Stem, SidecarNaming::Full] {
        let fixture = Fixture::new("move-repaired-retry")?;
        let mut s = repair_session(&fixture.0, naming, false, "detached")?;
        let path = fixture.0.join("source/Trip.png");
        let opts =
            ImportOptions { mode: ImportMode::Move, destination: Some(fixture.0.join("output").to_string_lossy().into()), ..Default::default() };
        let refused = import_with(&mut s, &[path.to_string_lossy().into()], &opts)?;
        assert_eq!(refused.failed.len(), 1);
        assert!(path.exists());
        s.repair_sidecars_with(&RealFs)?;
        assert!(retained(&s).is_empty());
        let accepted = import_with(&mut s, &[path.to_string_lossy().into()], &opts)?;
        assert_eq!(accepted.imported.len(), 1);
        assert_eq!(accepted.moved.len(), 1);
        assert_eq!(accepted.moved[0].sidecars.len(), 1);
        let photo = s.catalog.photo(PhotoId(accepted.imported[0])).unwrap();
        assert_eq!(photo.rating, 4);
        assert_eq!(photo.develop.light.exposure, 1.25);
        assert!(!path.exists());
        drop(s);
    }
    Ok(())
}
