//! Keyword no-op AutoWrite and virtual-copy control regression.

use crate::sidecar::{SidecarNaming, sidecar_path};
use crate::{PhotoId, Session};
use lightcraft_catalog::{Op, Photo, Source};
use serde_json::json;
use std::path::Path;

const FOREIGN: &[u8] = br#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#"><rdf:Description rdf:about="" xmlns:xmp="http://ns.adobe.com/xap/1.0/" xmlns:fixture="urn:lightcraft:repair-test" xmp:Rating="2" fixture:Marker="keep-me"/></rdf:RDF></x:xmpmeta>"#;

fn png(path: &Path) {
    let image = lightcraft_raster::Rgba8::from_fn(4, 3, |_, _| [40, 80, 120, 255]);
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&image), &Default::default()).unwrap();
    std::fs::write(path, bytes).unwrap();
}

#[test]
fn no_op_keyword_actions_skip_autowrite_and_virtual_copy_never_writes_original_packet() {
    let root = crate::tests_xmp::temp_dir("repair-save-controls");
    let original = root.join("a.png");
    png(&original);
    let packet_path = sidecar_path(&original.to_string_lossy(), SidecarNaming::Stem);
    std::fs::write(&packet_path, FOREIGN).unwrap();
    let mut s = Session::new();
    let id = PhotoId(1);
    let mut photo = Photo::new(id, Source::File { path: original.to_string_lossy().into_owned() }, "a.png", "PNG", 4, 3, "");
    photo.meta.keywords = vec!["CAFÉ".into()];
    s.catalog.apply(Op::AddPhoto { photo: Box::new(photo) }).unwrap();
    s.selection = crate::Selection::single(id);
    s.xmp.auto_write = true;
    for params in [json!({"addKeywords": ["café"]}), json!({"removeKeywords": ["absent"]})] {
        assert_eq!(s.execute("photo.setMeta", &params).unwrap()["changed"], 0);
        assert!(s.undo.is_empty() && s.redo.is_empty() && s.pending_log.is_empty());
        assert_eq!(std::fs::read(&packet_path).unwrap(), FOREIGN);
    }
    s.execute("photo.setMeta", &json!({"removeKeywords": ["café"]})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    let before = std::fs::read(&packet_path).unwrap();
    let history = (s.undo.len(), s.redo.len(), s.pending_log.len());
    assert_eq!(s.execute("photo.setMeta", &json!({"addKeywords": ["café"]})).unwrap()["changed"], 0);
    assert_eq!((s.undo.len(), s.redo.len(), s.pending_log.len()), history);
    assert_eq!(std::fs::read(&packet_path).unwrap(), before);
    s.execute("edit.redo", &json!({})).unwrap();
    assert!(s.catalog.photo(id).unwrap().meta.keywords.is_empty());

    let mut copy = (**s.catalog.photo(id).unwrap()).clone();
    copy.id = PhotoId(2);
    copy.copy_of = Some(id);
    s.catalog.apply(Op::AddPhoto { photo: Box::new(copy) }).unwrap();
    s.selection = crate::Selection::single(PhotoId(2));
    let before = std::fs::read(&packet_path).unwrap();
    assert_eq!(s.execute("photo.setMeta", &json!({"addKeywords": ["café"]})).unwrap()["changed"], 1);
    assert_eq!(std::fs::read(&packet_path).unwrap(), before);
    let saved = s.execute("photo.saveMetadataToFile", &json!({})).unwrap();
    assert!(saved["written"].as_array().unwrap().is_empty());
    assert!(saved["failed"][0]["error"].as_str().unwrap().contains("virtual copy"));
    assert_eq!(std::fs::read(&packet_path).unwrap(), before);
    drop(s);
    std::fs::remove_dir_all(root).unwrap();
}
