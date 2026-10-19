//! Per-photo Unicode equality, exact hierarchy boundaries, and no-op history.
use super::*;
use lightcraft_catalog::{Op, Photo, PhotoId, Source};

fn session(first: &str, alternate: &str) -> Session {
    let mut s = Session::new();
    for (id, keyword) in [(1, first), (2, alternate)] {
        let mut p = Photo::new(PhotoId(id), Source::Demo { scene: 1 }, &format!("{id}.jpg"), "JPEG", 4, 3, "");
        p.meta.keywords = vec![keyword.to_string()];
        s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    s.selection = crate::Selection::single(PhotoId(2));
    s
}
fn keywords(s: &Session) -> Vec<String> {
    s.catalog.photo(PhotoId(2)).unwrap().meta.keywords.clone()
}

#[test]
fn unicode_row_add_remove_and_set_toggle_share_catalog_keys_and_round_trip_history() {
    for (first, alternate) in [("café", "CAFÉ"), ("Target|Straße", "TARGET|STRAẞE"), ("Αθήνα", "ΑΘΉΝΑ"), ("travel", "TRAVEL")] {
        let mut s = session(first, alternate);
        let mut tree = s.catalog.keyword_tree();
        while !tree[0].children.is_empty() {
            tree = tree.remove(0).children;
        }
        let row = tree[0].path.clone();
        assert_eq!(tree[0].count, 2);
        let before = s.catalog.to_snapshot();
        assert_eq!(s.execute("photo.setMeta", &json!({"addKeywords": [row]})).unwrap()["changed"], 0);
        assert_eq!(s.catalog.to_snapshot(), before);
        assert!(s.undo.is_empty() && s.redo.is_empty() && s.pending_log.is_empty());
        assert_eq!(s.execute("photo.setMeta", &json!({"removeKeywords": [row]})).unwrap()["changed"], 1);
        assert!(keywords(&s).is_empty());
        assert_eq!(s.undo.len(), 1);
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(keywords(&s), [alternate]);
        // A no-op Add must preserve the available Redo as well as the catalog/log.
        let history = (s.undo.len(), s.redo.len(), s.pending_log.len());
        assert_eq!(s.execute("photo.setMeta", &json!({"addKeywords": [first]})).unwrap()["changed"], 0);
        assert_eq!((s.undo.len(), s.redo.len(), s.pending_log.len()), history);
        s.execute("edit.redo", &json!({})).unwrap();
        assert!(keywords(&s).is_empty());
        s.execute("edit.undo", &json!({})).unwrap();
        s.execute("keyword.saveSet", &json!({"name": "Test", "keywords": [first]})).unwrap();
        let toggled = s.execute("keyword.toggleFromSet", &json!({"index": 1})).unwrap();
        assert_eq!(toggled["added"], false);
        assert_eq!(toggled["changed"], 1);
        assert!(keywords(&s).is_empty());
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(keywords(&s), [alternate]);
        s.execute("edit.redo", &json!({})).unwrap();
        assert!(keywords(&s).is_empty());
        assert_eq!(s.execute("keyword.toggleFromSet", &json!({"index": 1})).unwrap()["added"], true);
        assert_eq!(keywords(&s), [first]);
        s.execute("edit.undo", &json!({})).unwrap();
        assert!(keywords(&s).is_empty());
        s.execute("edit.redo", &json!({})).unwrap();
        assert_eq!(keywords(&s), [first]);
    }
}

#[test]
fn exact_keyword_actions_preserve_descendants_boundaries_and_recent_unicode_identity() {
    let mut s = session("café", "CAFÉ|Street");
    assert_eq!(s.execute("photo.setMeta", &json!({"removeKeywords": ["café"]})).unwrap()["changed"], 0);
    assert!(s.undo.is_empty());
    s.execute("photo.setMeta", &json!({"addKeywords": ["café", "caféhouse"]})).unwrap();
    assert_eq!(s.execute("photo.setMeta", &json!({"removeKeywords": ["CAFÉ"]})).unwrap()["changed"], 1);
    assert_eq!(keywords(&s), ["CAFÉ|Street", "caféhouse"]);
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(keywords(&s), ["CAFÉ|Street", "café", "caféhouse"]);
    s.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(keywords(&s), ["CAFÉ|Street", "caféhouse"]);
    note_recent(&mut s, &["café".into(), "CAFÉ".into()]);
    assert_eq!(s.recent_keywords.iter().filter(|k| k.to_lowercase() == "café").count(), 1);
}
