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

#[test]
fn legacy_hierarchy_row_actions_and_ingress_share_identity_without_migration() {
    let raw = "  Travel || CAFÉ  ";
    let mut s = session("Travel|café", raw);
    let row = s.catalog.keyword_tree()[0].children[0].path.clone();
    let before = s.catalog.to_snapshot();
    assert_eq!(s.execute("photo.setMeta", &json!({"addKeywords": [row]})).unwrap()["changed"], 0);
    assert_eq!(s.catalog.to_snapshot(), before);
    assert!(s.undo.is_empty() && s.redo.is_empty() && s.pending_log.is_empty());
    assert_eq!(s.execute("photo.setMeta", &json!({"removeKeywords": [row]})).unwrap()["changed"], 1);
    assert!(keywords(&s).is_empty());
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(keywords(&s), [raw]);
    s.execute("keyword.saveSet", &json!({"name": "Test", "keywords": ["travel | café"]})).unwrap();
    assert_eq!(s.execute("keyword.toggleFromSet", &json!({"index": 1})).unwrap()["added"], false);
    assert!(keywords(&s).is_empty());
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(keywords(&s), [raw]);
    s.execute("photo.setMeta", &json!({"addKeywords": [" New | | Straße ", " | "]})).unwrap();
    assert_eq!(keywords(&s), [raw, "New|Straße"]);
    s.execute("photo.setMeta", &json!({"keywords": ["  New | | Straße "]})).unwrap();
    assert_eq!(keywords(&s), ["New|Straße"]);
    s.execute("library.filter", &json!({"keyword": " new | STRAẞE "})).unwrap();
    assert_eq!(s.visible(), &[PhotoId(2)]);
}

#[test]
fn absent_global_keyword_actions_preserve_metadata_filter_log_and_available_redo() {
    let mut s = session("Travel", "CAFÉ");
    let mut meta = s.catalog.photo(PhotoId(2)).unwrap().meta.clone();
    meta.keywords = vec!["CAFÉ".into(), " café ".into(), " | ".into(), "".into()];
    s.catalog.apply(Op::SetMeta { id: PhotoId(2), meta: Box::new(meta) }).unwrap();
    s.execute("photo.setMeta", &json!({"title": "future"})).unwrap();
    s.execute("edit.undo", &json!({})).unwrap();
    s.execute("library.filter", &json!({"keyword": "missing"})).unwrap();
    let before = s.catalog.to_snapshot();
    let selection = s.selection.clone();
    let filter = s.filter.clone();
    let history = (s.undo.len(), s.redo.len(), s.pending_log.len());
    assert_eq!(history.1, 1);
    for (cmd, params) in [
        ("keyword.delete", json!({"keyword": "missing"})),
        ("keyword.rename", json!({"from": "missing", "to": "new"})),
        ("keyword.merge", json!({"from": ["missing"], "into": "new"})),
    ] {
        assert_eq!(s.execute(cmd, &params).unwrap()["changed"], 0);
        assert_eq!(s.catalog.to_snapshot(), before);
        assert_eq!((s.undo.len(), s.redo.len(), s.pending_log.len()), history);
        assert_eq!(s.filter, filter);
        assert_eq!(s.selection, selection);
    }
    s.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(s.catalog.photo(PhotoId(2)).unwrap().meta.title, "future");
    assert_eq!(keywords(&s), ["CAFÉ", " café ", " | ", ""]);
}

#[test]
fn legacy_global_keyword_history_restores_raw_spellings_and_followed_filter() {
    for action in ["rename", "merge", "delete"] {
        let mut s = session("Other", "  Travel || CAFÉ | Child ");
        let mut meta = s.catalog.photo(PhotoId(2)).unwrap().meta.clone();
        meta.keywords.extend(["untouched ".into(), "UNTOUCHED".into(), "".into()]);
        s.catalog.apply(Op::SetMeta { id: PhotoId(2), meta: Box::new(meta) }).unwrap();
        s.execute("library.filter", &json!({"keyword": "travel | café"})).unwrap();
        let before = s.catalog.to_snapshot();
        let filter = s.filter.clone();
        let selection = s.selection.clone();
        let (cmd, params) = match action {
            "rename" => ("keyword.rename", json!({"from": "travel", "to": "Trips"})),
            "merge" => ("keyword.merge", json!({"from": ["travel"], "into": "Trips"})),
            _ => ("keyword.delete", json!({"keyword": "travel"})),
        };
        assert_eq!(s.execute(cmd, &params).unwrap()["changed"], 1);
        assert_eq!(s.filter.keyword.as_deref(), if action == "delete" { None } else { Some("Trips| café") });
        assert_eq!(keywords(&s).iter().rev().take(3).cloned().collect::<Vec<_>>(), ["", "UNTOUCHED", "untouched "]);
        let after = s.catalog.to_snapshot();
        let after_filter = s.filter.clone();
        s.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(s.catalog.to_snapshot(), before);
        assert_eq!(s.filter, filter);
        assert_eq!(s.selection, selection);
        s.execute("edit.redo", &json!({})).unwrap();
        assert_eq!(s.catalog.to_snapshot(), after);
        assert_eq!(s.filter, after_filter);
        assert_eq!(s.selection, selection);
    }
}

#[test]
fn suggestion_commands_aggregate_legacy_variants_and_exclude_assigned_identity() {
    let mut s = session(" café ", "CAFÉ");
    let mut p = Photo::new(PhotoId(3), Source::Demo { scene: 1 }, "third.jpg", "JPEG", 4, 3, "");
    p.meta.keywords = vec!["beach".into()];
    s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    let before = s.catalog.to_snapshot();
    assert_eq!(s.execute("keyword.suggest", &json!({"ids": [], "limit": 1})).unwrap(), json!(["CAFÉ"]));
    assert_eq!(s.execute("keyword.suggest", &json!({"ids": [], "prefix": " ca "})).unwrap(), json!(["CAFÉ"]));
    assert_eq!(s.execute("keyword.suggest", &json!({})).unwrap(), json!(["beach"]));
    assert_eq!(s.catalog.to_snapshot(), before);
    assert!(s.undo.is_empty() && s.redo.is_empty() && s.pending_log.is_empty());
}

#[test]
fn merging_a_unicode_destination_alias_keeps_its_filter_and_unrelated_legacy_duplicates() {
    let mut s = session("Other", "Old|Child");
    let mut meta = s.catalog.photo(PhotoId(2)).unwrap().meta.clone();
    meta.keywords.extend(["CAFÉ".into(), " café ".into()]);
    s.catalog.apply(Op::SetMeta { id: PhotoId(2), meta: Box::new(meta) }).unwrap();
    s.execute("library.filter", &json!({"keyword": "CAFÉ"})).unwrap();
    let before = s.catalog.to_snapshot();
    let filter = s.filter.clone();
    assert_eq!(s.execute("keyword.merge", &json!({"from": ["CAFÉ", "Old"], "into": "café"})).unwrap()["changed"], 1);
    assert_eq!(keywords(&s), ["café|Child", "CAFÉ", " café "]);
    assert_eq!(s.filter, filter, "an equivalent destination in from is ignored by metadata and filter follow alike");
    let after = s.catalog.to_snapshot();
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.catalog.to_snapshot(), before);
    assert_eq!(s.filter, filter);
    s.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(s.catalog.to_snapshot(), after);
    assert_eq!(s.filter, filter);
}
