//! Browse alias identity and virtual-copy ownership regressions.
use super::*;
use lightcraft_catalog::Op;
use serde_json::json;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

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

#[test]
fn live_and_saved_alias_views_keep_virtual_copy_edits_filters_and_physical_copies() -> TestResult {
    let fixture = Fixture::new("virtual-copies")?;
    let root = &fixture.0;
    write_png(&root.join("real/photo.png"), 32)?;
    std::fs::copy(root.join("real/photo.png"), root.join("real/independent.png"))?;
    std::os::unix::fs::symlink(root.join("real"), root.join("alias"))?;
    let mut s = Session::new().with_fs();
    s.clock = Box::new(|| "2026-10-09T00:00:00".into());
    s.open_library(root.join("library"), false)?;
    s.execute("library.browse", &json!({"path":root.join("real")}))?;
    let master = s.catalog.photos().find(|p| p.file_name == "photo.png").unwrap().id;
    let physical = s.catalog.photos().find(|p| p.file_name == "independent.png").unwrap().id;
    s.commit("master", Op::SetRating { id: master, rating: 2 })?;
    s.commit("physical", Op::SetRating { id: physical, rating: 3 })?;
    let originals = [master, physical].map(|id| s.catalog.photo(id).unwrap().clone());
    s.execute("library.select", &json!({"ids":[master.0]}))?;
    let made = s.execute("photo.virtualCopy", &json!({"ids":[master.0]}))?;
    let copy = PhotoId(made["ids"][0].as_u64().unwrap());
    s.execute("library.select", &json!({"ids":[copy.0]}))?;
    s.execute("develop.set", &json!({"values":{"light.exposure":0.5}}))?;
    s.execute("version.create", &json!({"name":"Saved copy edit"}))?;
    s.execute("develop.set", &json!({"values":{"light.exposure":1.25}}))?;
    s.commit("copy rating", Op::SetRating { id: copy, rating: 5 })?;
    let mut meta = s.catalog.photo(copy).unwrap().meta.clone();
    meta.keywords = vec!["Independent copy edit".into()];
    s.commit("copy metadata", Op::SetMeta { id: copy, meta: Box::new(meta) })?;
    let before = s.catalog.photo(copy).unwrap().clone();
    assert_eq!(before.copy_of, Some(master));
    assert_eq!(before.versions.len(), 1);
    assert!(!before.versions[0].auto);
    assert_eq!(before.versions[0].settings.light.exposure, 0.5);
    let browse = s.execute("library.browse", &json!({"path":root.join("alias")}))?;
    assert_eq!(browse["new"], 0);
    assert_eq!(s.visible().len(), 3);
    assert!(s.visible().contains(&master) && s.visible().contains(&copy) && s.visible().contains(&physical));
    assert_eq!(s.source_total(), Some(3));
    s.filter.rating = 5;
    assert_eq!(s.visible(), &[copy]);
    // A copy created after alias discovery must inherit visibility without another scan.
    s.filter.rating = 0;
    let undo = s.undo.len();
    s.execute("photo.virtualCopy", &json!({"ids":[master.0]}))?;
    assert_eq!(s.undo.len(), undo + 1, "the navigation checkpoint is not an Undo step");
    let mut checkpointed = before.clone();
    let created = (s.clock)();
    Arc::make_mut(&mut checkpointed).versions.push(lightcraft_catalog::Version {
        name: lightcraft_catalog::dates::display_time(&created),
        created,
        settings: before.develop.clone(),
        auto: true,
    });
    assert_eq!(s.catalog.photo(copy).unwrap(), &checkpointed, "leaving the edited copy adds exactly one Auto version");
    assert_eq!(s.visible().len(), 4);
    assert_eq!(s.source_total(), Some(4));
    s.close_library()?;
    s.open_library(root.join("library"), false)?;
    assert!(s.visible().is_empty());
    let input = BrowseAliasInput::new(&s).unwrap();
    let output = std::thread::spawn(move || input.restore(&std::sync::atomic::AtomicBool::new(false))).join().unwrap();
    assert!(output.apply(&mut s));
    assert_eq!(s.visible().len(), 4);
    assert_eq!(s.source_total(), Some(4));
    s.filter.rating = 5;
    assert_eq!(s.visible(), &[copy]);
    let after = s.catalog.photo(copy).unwrap();
    assert_eq!(after.source, before.source);
    assert_eq!(after.develop, before.develop);
    assert_eq!(after.meta, before.meta);
    assert_eq!(after.history, before.history);
    assert_eq!(after.rating, before.rating);
    assert_eq!(after.versions, checkpointed.versions);
    assert_eq!(after, &checkpointed, "saved alias restoration must retain the complete virtual copy");
    for original in &originals {
        assert_eq!(s.catalog.photo(original.id).unwrap(), original, "copy edits must not affect the original or the physical copy");
    }
    assert_eq!(s.catalog.len(), 4);
    // A relinked original must not discard the unchanged copy's reconstruction.
    s.filter.rating = 0;
    let output = BrowseAliasInput::new(&s).unwrap().restore(&std::sync::atomic::AtomicBool::new(false));
    s.commit(
        "relink original",
        Op::Relink {
            id: master,
            file_name: "other.png".into(),
            source: Source::File { path: root.join("other.png").to_string_lossy().into() },
            format: None,
        },
    )?;
    assert!(output.apply(&mut s));
    assert!(!s.visible().contains(&master));
    assert!(s.visible().contains(&copy) && s.visible().contains(&physical));
    // The copy's own stored-source change fences it independently too.
    s.commit(
        "relink copy",
        Op::Relink {
            id: copy,
            file_name: "copy-other.png".into(),
            source: Source::File { path: root.join("copy-other.png").to_string_lossy().into() },
            format: None,
        },
    )?;
    assert!(!s.visible().contains(&copy));
    drop(s);
    Ok(())
}
