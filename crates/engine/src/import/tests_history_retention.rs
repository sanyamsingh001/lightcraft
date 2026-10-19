//! Actual Linux Session/filesystem regressions; run in the parent's native engine suite.
use super::*;
use serde_json::{Value, json};
use std::sync::atomic::{AtomicUsize, Ordering};

type TestResult<T = ()> = std::result::Result<T, Box<dyn std::error::Error>>;

struct Fixture(PathBuf);
impl Fixture {
    fn new(tag: &str) -> TestResult<Self> {
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/agent-import-history-retention/fixtures").join(format!(
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

fn packet(marker: &str, keyword: bool) -> String {
    let subject = if keyword { "<dc:subject><rdf:Bag><rdf:li>CAFÉ</rdf:li></rdf:Bag></dc:subject>" } else { "" };
    format!(
        r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description xmlns:dc="http://purl.org/dc/elements/1.1/" xmlns:foreign="urn:lightcraft:history-retention-test" foreign:marker="{marker}">{subject}</rdf:Description></rdf:RDF></x:xmpmeta>"#
    )
}

fn full_sidecar(path: &Path) -> PathBuf {
    PathBuf::from(format!("{}.xmp", path.display()))
}

fn write_photo(path: &Path, seed: u8, keyword: bool) -> TestResult {
    std::fs::create_dir_all(path.parent().ok_or("missing parent")?)?;
    let image = lightcraft_raster::Rgba8::from_fn(4, 3, |_, _| [seed, 64, 128, 255]);
    let png = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&image), &Default::default())?;
    std::fs::write(path, png)?;
    std::fs::write(path.with_extension("xmp"), packet("stem-foreign", keyword))?;
    std::fs::write(full_sidecar(path), packet("full-foreign", false))?;
    Ok(())
}

fn session(root: &Path) -> TestResult<Session> {
    let mut s = Session::new().with_fs();
    s.clock = Box::new(|| "2026-01-14T05:58:48".into());
    s.open_library(root.join("library"), false)?;
    assert!(!s.xmp.auto_write);
    Ok(s)
}

fn imported(report: &Value) -> TestResult<PhotoId> {
    assert!(report["failed"].as_array().ok_or("missing failures")?.is_empty(), "{report}");
    let ids = report["imported"].as_array().ok_or("missing imports")?;
    assert_eq!(ids.len(), 1, "{report}");
    Ok(PhotoId(ids[0].as_u64().ok_or("missing imported ID")?))
}

fn source(s: &Session, id: PhotoId) -> TestResult<PathBuf> {
    let p = s.catalog.photo(id).ok_or("missing photo")?;
    let Source::File { path } = &p.source else { return Err("expected native source".into()) };
    Ok(path.into())
}

fn assert_source(s: &Session, id: PhotoId, path: &Path, bytes: &[u8]) -> TestResult {
    assert_eq!(source(s, id)?, path);
    let actual = std::fs::read(path)?;
    assert_eq!(actual, bytes);
    lightcraft_codecs::decode(&actual, Default::default())?;
    Ok(())
}

fn assert_packets(path: &Path) -> TestResult {
    assert_eq!(std::fs::read(path.with_extension("xmp"))?, packet("stem-foreign", false).as_bytes());
    assert_eq!(std::fs::read(full_sidecar(path))?, packet("full-foreign", false).as_bytes());
    Ok(())
}

fn assert_kept(report: &Value, path: &Path) -> TestResult {
    assert!(report["moved"].as_array().is_none_or(|items| items.is_empty()), "{report}");
    assert!(
        report["kept"].as_array().ok_or("missing kept report")?.iter().any(|k| {
            k["path"].as_str() == path.to_str()
                && k["reason"].as_str().is_some_and(|reason| reason.contains("history") && reason.contains("sidecars"))
        }),
        "{report}"
    );
    Ok(())
}

fn edited_deleted(s: &mut Session, original: &Path) -> TestResult<PhotoId> {
    let id = imported(&s.execute("library.import", &json!({"paths":[original]}))?)?;
    s.execute("photo.setMeta", &json!({"ids":[id.0],"title":"Retained edits","keywords":["history"]}))?;
    s.execute("photo.rate", &json!({"ids":[id.0],"rating":4,"advance":false}))?;
    s.execute("library.select", &json!({"ids":[id.0]}))?;
    s.execute("develop.set", &json!({"values":{"light.exposure":0.75}}))?;
    s.execute("photo.delete", &json!({"ids":[id.0]}))?;
    Ok(id)
}

#[test]
fn fresh_move_retains_deleted_snapshot_and_sidecars_for_two_undo_redo_cycles() -> TestResult {
    let fixture = Fixture::new("fresh")?;
    let original = fixture.0.join("source/photo.png");
    write_photo(&original, 16, false)?;
    let bytes = std::fs::read(&original)?;
    let mut s = session(&fixture.0)?;
    let old_id = edited_deleted(&mut s, &original)?;
    let old = s.catalog.photo(old_id).ok_or("missing deleted snapshot")?.clone();
    assert!(old.deleted && !old.local);
    let report = s.execute(
        "library.import",
        &json!({"paths":[original],"mode":"move","destination":fixture.0.join("output"),"organize":"flat","onDeleted":"fresh"}),
    )?;
    let new_id = imported(&report)?;
    assert_ne!(old_id, new_id);
    assert!(s.catalog.photo(old_id).is_none());
    let destination = source(&s, new_id)?;
    assert_ne!(destination, original);
    assert_kept(&report, &original)?;
    assert_packets(&original)?;
    assert_packets(&destination)?;
    for _ in 0..2 {
        s.execute("edit.undo", &json!({}))?;
        assert!(s.catalog.photo(new_id).is_none());
        assert_eq!(s.catalog.photo(old_id).ok_or("Fresh Undo lost snapshot")?, &old);
        assert_source(&s, old_id, &original, &bytes)?;
        s.execute("edit.undo", &json!({}))?;
        let live = s.catalog.photo(old_id).ok_or("Delete Undo lost photo")?;
        assert!(!live.deleted);
        assert_eq!(live.meta, old.meta);
        assert_eq!(live.develop, old.develop);
        assert_eq!(live.history, old.history);
        assert_source(&s, old_id, &original, &bytes)?;
        s.execute("edit.redo", &json!({}))?;
        assert!(s.catalog.photo(old_id).ok_or("Delete Redo lost photo")?.deleted);
        s.execute("edit.redo", &json!({}))?;
        assert!(s.catalog.photo(old_id).is_none());
        assert_source(&s, new_id, &destination, &bytes)?;
        assert_packets(&original)?;
        assert_packets(&destination)?;
    }
    s.close_library()?;
    s.open_library(fixture.0.join("library"), false)?;
    assert_source(&s, new_id, &destination, &bytes)?;
    assert_eq!(std::fs::read(&original)?, bytes);
    assert_packets(&original)?;
    drop(s);
    Ok(())
}

#[test]
fn move_after_empty_retains_older_snapshot_with_or_without_hash_for_two_cycles() -> TestResult {
    for (tag, without_hash, alias) in
        [("empty-hash", false, false), ("empty-path-only", true, false), ("empty-hash-alias", false, true), ("empty-path-only-alias", true, true)]
    {
        let fixture = Fixture::new(tag)?;
        let original = fixture.0.join("source/photo.png");
        write_photo(&original, 32, false)?;
        let bytes = std::fs::read(&original)?;
        let mut s = session(&fixture.0)?;
        let old_id = edited_deleted(&mut s, &original)?;
        if without_hash {
            let old = s.catalog.photo(old_id).ok_or("missing legacy photo")?;
            let op = Op::SetContent {
                id: old_id,
                width: old.width,
                height: old.height,
                file_size: old.file_size,
                content_hash: None,
                preview_only: old.preview_only.clone(),
            };
            s.commit("Legacy snapshot without hash", op)?;
        }
        let old = s.catalog.photo(old_id).ok_or("missing old photo")?.clone();
        s.execute("library.emptyRecentlyDeleted", &json!({}))?;
        assert!(s.catalog.photo(old_id).is_none());
        let input = if alias {
            std::os::unix::fs::symlink(fixture.0.join("source"), fixture.0.join("alias"))?;
            fixture.0.join("alias/photo.png")
        } else {
            original.clone()
        };
        let report = s.execute("library.import", &json!({"paths":[input],"mode":"move","destination":fixture.0.join("output"),"organize":"flat"}))?;
        let new_id = imported(&report)?;
        let destination = source(&s, new_id)?;
        assert_kept(&report, &input)?;
        for _ in 0..2 {
            s.execute("edit.undo", &json!({}))?;
            assert!(s.catalog.photo(new_id).is_none());
            s.execute("edit.undo", &json!({}))?;
            assert_eq!(s.catalog.photo(old_id).ok_or("Empty Undo lost snapshot")?, &old);
            assert_source(&s, old_id, &original, &bytes)?;
            s.execute("edit.redo", &json!({}))?;
            assert!(s.catalog.photo(old_id).is_none());
            s.execute("edit.redo", &json!({}))?;
            assert_source(&s, new_id, &destination, &bytes)?;
            assert_eq!(std::fs::read(&original)?, bytes);
            assert_packets(&original)?;
            assert_packets(&destination)?;
        }
        drop(s);
    }
    Ok(())
}

#[test]
fn ordinary_move_keeps_an_older_relink_source_with_different_snapshot_bytes() -> TestResult {
    let fixture = Fixture::new("older-relink")?;
    let original = fixture.0.join("source/photo.png");
    let replacement = fixture.0.join("other/replacement.png");
    write_photo(&original, 40, false)?;
    write_photo(&replacement, 41, false)?;
    let bytes = std::fs::read(&original)?;
    let mut s = session(&fixture.0)?;
    let old_id = imported(&s.execute("library.import", &json!({"paths":[original]}))?)?;
    let probe = s.media.file_probe.as_ref().ok_or("missing native probe")?(&replacement.to_string_lossy())?;
    s.commit(
        "Replace source and file facts",
        Op::Batch {
            ops: vec![
                Op::Relink {
                    id: old_id,
                    file_name: "replacement.png".into(),
                    source: Source::File { path: replacement.to_string_lossy().into_owned() },
                    format: None,
                },
                Op::SetContent {
                    id: old_id,
                    width: probe.width,
                    height: probe.height,
                    file_size: probe.file_size,
                    content_hash: probe.content_hash.clone(),
                    preview_only: probe.preview_only.clone(),
                },
            ],
        },
    )?;
    s.execute("photo.delete", &json!({"ids":[old_id.0]}))?;
    s.execute("library.emptyRecentlyDeleted", &json!({}))?;
    // The AddPhoto snapshot names replacement and its different hash; older Relink names original.
    let report = s.execute("library.import", &json!({"paths":[original],"mode":"move","destination":fixture.0.join("output"),"organize":"flat"}))?;
    imported(&report)?;
    assert_kept(&report, &original)?;
    for _ in 0..2 {
        for _ in 0..4 {
            s.execute("edit.undo", &json!({}))?;
        }
        assert_source(&s, old_id, &original, &bytes)?;
        for _ in 0..4 {
            s.execute("edit.redo", &json!({}))?;
        }
        assert_eq!(std::fs::read(&original)?, bytes);
        assert_packets(&original)?;
    }
    drop(s);
    Ok(())
}

#[test]
fn ordinary_move_captures_a_retained_redo_add_photo_snapshot() -> TestResult {
    let fixture = Fixture::new("redo-snapshot")?;
    let original = fixture.0.join("source/photo.png");
    write_photo(&original, 48, false)?;
    let bytes = std::fs::read(&original)?;
    let mut s = session(&fixture.0)?;
    let old_id = imported(&s.execute("library.import", &json!({"paths":[original]}))?)?;
    s.execute("edit.undo", &json!({}))?;
    assert!(s.catalog.photo(old_id).is_none());
    assert!(!s.redo.is_empty());
    let report = s.execute("library.import", &json!({"paths":[original],"mode":"move","destination":fixture.0.join("output"),"organize":"flat"}))?;
    let new_id = imported(&report)?;
    let destination = source(&s, new_id)?;
    assert!(s.redo.is_empty(), "the new import retires Redo after its source requirements are captured");
    assert_kept(&report, &original)?;
    assert_eq!(std::fs::read(&original)?, bytes);
    assert_source(&s, new_id, &destination, &bytes)?;
    assert_packets(&original)?;
    assert_packets(&destination)?;
    drop(s);
    Ok(())
}

#[test]
fn ordinary_unreferenced_move_removes_source_and_both_sidecars() -> TestResult {
    let fixture = Fixture::new("ordinary-negative-control")?;
    let original = fixture.0.join("source/photo.png");
    write_photo(&original, 64, false)?;
    let bytes = std::fs::read(&original)?;
    let mut s = session(&fixture.0)?;
    let report = s.execute("library.import", &json!({"paths":[original],"mode":"move","destination":fixture.0.join("output"),"organize":"flat"}))?;
    let id = imported(&report)?;
    let destination = source(&s, id)?;
    assert_eq!(report["moved"].as_array().ok_or("missing moved report")?.len(), 1, "{report}");
    assert!(report["kept"].as_array().is_none_or(|items| items.is_empty()), "{report}");
    for _ in 0..2 {
        assert!(!original.exists());
        assert!(!original.with_extension("xmp").exists());
        assert!(!full_sidecar(&original).exists());
        assert_source(&s, id, &destination, &bytes)?;
        assert_packets(&destination)?;
        s.execute("edit.undo", &json!({}))?;
        assert!(s.catalog.photo(id).is_none());
        assert_eq!(std::fs::read(&destination)?, bytes);
        s.execute("edit.redo", &json!({}))?;
    }
    drop(s);
    Ok(())
}

#[test]
fn import_unicode_keyword_identity_matches_set_meta_for_add_copy_and_move() -> TestResult {
    let variants = vec!["CAFÉ", "café", "STRAẞE", "straße", "İ", "i\u{307}", "Travel|CAFÉ", "travel|café"];
    let expected = vec!["CAFÉ", "STRAẞE", "İ", "Travel|CAFÉ"];
    for mode in ["add", "copy", "move"] {
        for sidecar_keyword in [false, true] {
            let fixture = Fixture::new(&format!("unicode-{mode}-{sidecar_keyword}"))?;
            let original = fixture.0.join("source/photo.png");
            write_photo(&original, 80, sidecar_keyword)?;
            let mut s = session(&fixture.0)?;
            let report = s.execute(
                "library.import",
                &json!({"paths":[original],"mode":mode,"destination":fixture.0.join("output"),"organize":"flat","keywords":variants}),
            )?;
            let id = imported(&report)?;
            assert_eq!(s.catalog.photo(id).ok_or("missing imported photo")?.meta.keywords, expected);
            for _ in 0..2 {
                s.execute("edit.undo", &json!({}))?;
                assert!(s.catalog.photo(id).is_none());
                s.execute("edit.redo", &json!({}))?;
                assert_eq!(s.catalog.photo(id).ok_or("Redo lost imported photo")?.meta.keywords, expected);
            }
            let control_path = fixture.0.join("source/control.png");
            write_photo(&control_path, 96, false)?;
            let control = imported(&s.execute("library.import", &json!({"paths":[control_path]}))?)?;
            s.execute("photo.setMeta", &json!({"ids":[control.0],"addKeywords":variants}))?;
            assert_eq!(s.catalog.photo(control).ok_or("missing setMeta control")?.meta.keywords, expected);
            drop(s);
        }
    }
    Ok(())
}

#[test]
fn hashless_history_aliases_keep_the_stored_path_and_unrelated_move_still_removes_its_source() -> TestResult {
    for kind in ["relative", "dotdot", "file-symlink", "unrelated"] {
        let fixture = Fixture::new(kind)?;
        let original = fixture.0.join("source/photo.png");
        write_photo(&original, 112, false)?;
        let bytes = std::fs::read(&original)?;
        let (retained, input) = match kind {
            "relative" => {
                // Keep the process cwd unchanged; express this absolute fixture relative to it.
                let mut relative = PathBuf::new();
                for component in std::env::current_dir()?.components() {
                    if matches!(component, std::path::Component::Normal(_)) {
                        relative.push("..");
                    }
                }
                relative.push(original.strip_prefix("/")?);
                (relative, original.clone())
            }
            "dotdot" => {
                std::fs::create_dir_all(fixture.0.join("source/nested"))?;
                (original.clone(), fixture.0.join("source/nested/../photo.png"))
            }
            "file-symlink" => {
                let link = fixture.0.join("saved-pointer.png");
                std::os::unix::fs::symlink(&original, &link)?;
                (link, original.clone())
            }
            _ => {
                let other = fixture.0.join("other/photo.png");
                write_photo(&other, 113, false)?;
                (other, original.clone())
            }
        };
        let retained_bytes = std::fs::read(&retained)?;
        let mut s = session(&fixture.0)?;
        let old_id = edited_deleted(&mut s, &retained)?;
        let old = s.catalog.photo(old_id).ok_or("missing legacy photo")?;
        let op = Op::SetContent {
            id: old_id,
            width: old.width,
            height: old.height,
            file_size: old.file_size,
            content_hash: None,
            preview_only: old.preview_only.clone(),
        };
        s.commit("Legacy snapshot without hash", op)?;
        let snapshot = s.catalog.photo(old_id).ok_or("missing legacy snapshot")?.clone();
        s.execute("library.emptyRecentlyDeleted", &json!({}))?;
        let report = s.execute("library.import", &json!({"paths":[input],"mode":"move","destination":fixture.0.join("output"),"organize":"flat"}))?;
        let new_id = imported(&report)?;
        let destination = source(&s, new_id)?;
        if kind == "unrelated" {
            assert_eq!(report["moved"].as_array().ok_or("missing moved report")?.len(), 1, "{report}");
            assert!(report["kept"].as_array().is_none_or(|items| items.is_empty()), "{report}");
            assert!(!original.exists() && !original.with_extension("xmp").exists() && !full_sidecar(&original).exists());
        } else {
            assert_kept(&report, &input)?;
            assert_packets(&original)?;
        }
        for _ in 0..2 {
            s.execute("edit.undo", &json!({}))?;
            s.execute("edit.undo", &json!({}))?;
            assert_eq!(s.catalog.photo(old_id).ok_or("Empty Undo lost legacy snapshot")?, &snapshot);
            assert_source(&s, old_id, &retained, &retained_bytes)?;
            s.execute("edit.redo", &json!({}))?;
            s.execute("edit.redo", &json!({}))?;
            assert_source(&s, new_id, &destination, &bytes)?;
            assert_packets(&destination)?;
            if kind != "unrelated" {
                assert_eq!(std::fs::read(&original)?, bytes);
                assert_packets(&original)?;
            }
        }
        drop(s);
    }
    Ok(())
}

#[test]
fn import_legacy_hierarchy_xmp_keeps_first_spelling_and_unrelated_children() -> TestResult {
    let legacy = vec!["Travel | | CAFÉ", "Travel||CAFÉ | Street", "Travel | Rome", "Food | | Cake", "İ | | Places"];
    let requested = vec!["travel|café", "travel|café|street", "food|cake", "i\u{307}|places", "Travel | | New"];
    let mut expected = legacy.clone();
    expected.push("Travel|New");
    let subject = format!(
        "<dc:subject><rdf:Bag>{}</rdf:Bag></dc:subject>",
        legacy.iter().map(|keyword| format!("<rdf:li>{keyword}</rdf:li>")).collect::<String>()
    );
    let stem_packet = packet("stem-foreign", false).replace("</rdf:Description>", &format!("{subject}</rdf:Description>"));
    for mode in ["add", "copy", "move"] {
        let fixture = Fixture::new(&format!("legacy-hierarchy-{mode}"))?;
        let original = fixture.0.join("source/photo.png");
        write_photo(&original, 120, false)?;
        std::fs::write(original.with_extension("xmp"), &stem_packet)?;
        let mut s = session(&fixture.0)?;
        let report = s.execute(
            "library.import",
            &json!({"paths":[original],"mode":mode,"destination":fixture.0.join("output"),"organize":"flat","keywords":requested}),
        )?;
        let id = imported(&report)?;
        assert_eq!(report["sidecars"].as_u64(), Some(1), "legacy keywords must come from the real XMP parser: {report}");
        let stored = source(&s, id)?;
        // Copy reads metadata into the catalog and leaves the source packets in place;
        // Move carries the companion files to the destination.
        let packets = if mode == "copy" { &original } else { &stored };
        for _ in 0..2 {
            assert_eq!(s.catalog.photo(id).ok_or("missing imported photo")?.meta.keywords, expected);
            assert_eq!(std::fs::read(packets.with_extension("xmp"))?, stem_packet.as_bytes());
            assert_eq!(std::fs::read(full_sidecar(packets))?, packet("full-foreign", false).as_bytes());
            s.execute("edit.undo", &json!({}))?;
            assert!(s.catalog.photo(id).is_none());
            s.execute("edit.redo", &json!({}))?;
        }
        assert_eq!(s.catalog.photo(id).ok_or("Redo lost legacy keywords")?.meta.keywords, expected);
        let control_path = fixture.0.join("source/control.png");
        write_photo(&control_path, 121, false)?;
        let control = imported(&s.execute("library.import", &json!({"paths":[control_path]}))?)?;
        s.execute("photo.setMeta", &json!({"ids":[control.0],"keywords":legacy}))?;
        let normalized: Vec<String> = legacy.iter().map(|k| lightcraft_catalog::keywords::clean(k)).collect();
        assert_eq!(s.catalog.photo(control).ok_or("missing normalized replacement")?.meta.keywords, normalized);
        // Existing imported/journal metadata may retain raw spelling. Adding keywords preserves it.
        let mut meta = s.catalog.photo(control).ok_or("missing legacy control")?.meta.clone();
        meta.keywords = legacy.iter().map(|k| k.to_string()).collect();
        s.commit("Legacy metadata", Op::SetMeta { id: control, meta: Box::new(meta) })?;
        s.execute("photo.setMeta", &json!({"ids":[control.0],"addKeywords":requested}))?;
        assert_eq!(s.catalog.photo(control).ok_or("missing setMeta control")?.meta.keywords, expected);
        drop(s);
    }
    Ok(())
}
