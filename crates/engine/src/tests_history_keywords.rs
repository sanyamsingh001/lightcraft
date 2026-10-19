//! Library-wide keyword identity and conditional filter history.

use lightcraft_catalog::{Op, Photo, PhotoId, Source};
use serde_json::json;

use crate::{Selection, Session};

fn session() -> Session {
    let mut s = Session::new();
    for id in 1..=3 {
        let name = format!("synthetic-{id}.jpg");
        let photo = Photo::new(PhotoId(id), Source::File { path: name.clone() }, &name, "JPEG", 16, 16, "");
        s.catalog.apply(Op::AddPhoto { photo: Box::new(photo) }).unwrap();
    }
    s.selection = Selection::single(PhotoId(1));
    s
}

fn select(s: &mut Session, ids: &[u64], active: u64) {
    s.execute("library.select", &json!({"ids": ids, "active": active})).unwrap();
}

fn memory_library(store: &lightcraft_catalog::MemStore) -> Session {
    let mut s = Session::new();
    s.open_library_in(
        crate::library::LibraryStores {
            dir: "history-regression".into(),
            catalog: Box::new(store.clone()),
            files: Box::new(lightcraft_catalog::MemStore::new()),
            on_disk: false,
        },
        false,
    )
    .unwrap();
    s
}

fn assert_no_navigation(s: &mut Session) {
    select(s, &[3], 3);
    s.execute("library.filter", &json!({"rating": 2})).unwrap();
    let filter = s.filter.clone();
    let source = s.source;
    s.active_mask = Some(7);
    s.active_spot = Some(2);
    for command in ["edit.undo", "edit.redo"] {
        s.execute(command, &json!({})).unwrap();
        assert_eq!(s.selection, Selection::single(PhotoId(3)));
        assert_eq!(s.filter, filter);
        assert_eq!(s.source, source);
        assert_eq!((s.active_mask, s.active_spot), (Some(7), Some(2)));
    }
}

fn keyword_history_fixture() -> (Session, lightcraft_catalog::MemStore) {
    let store = lightcraft_catalog::MemStore::new();
    let mut s = memory_library(&store);
    for id in 1..=3 {
        let photo = Photo::new(PhotoId(id), Source::Demo { scene: 0 }, &format!("synthetic-{id}.jpg"), "JPEG", 16, 16, "");
        s.commit("Synthetic Photo", Op::AddPhoto { photo: Box::new(photo) }).unwrap();
    }
    select(&mut s, &[1, 2], 2);
    s.execute("photo.setMeta", &json!({"keywords": ["before|child"]})).unwrap();
    s.execute("photo.setMeta", &json!({"ids": [3], "keywords": ["other"]})).unwrap();
    s.execute("photo.rate", &json!({"rating": 5})).unwrap();
    s.undo.clear();
    (s, store)
}

#[test]
fn library_wide_keyword_edits_do_not_navigate_to_the_rewritten_photo_records() {
    let mut s = session();
    s.execute("photo.setMeta", &json!({"ids": [1], "keywords": ["before"]})).unwrap();
    for (command, params) in [
        ("keyword.rename", json!({"from": "before", "to": "after"})),
        ("keyword.merge", json!({"from": ["after"], "into": "merged"})),
        ("keyword.delete", json!({"keyword": "merged"})),
    ] {
        s.execute(command, &params).unwrap();
        assert_no_navigation(&mut s);
    }
}

#[test]
fn followed_keyword_filters_round_trip_without_global_photo_navigation_or_stale_journals() {
    for (command, params, keyword) in [
        ("keyword.rename", json!({"from": "before", "to": "after"}), Some("after|child")),
        ("keyword.merge", json!({"from": ["before"], "into": "after"}), Some("after|child")),
        ("keyword.delete", json!({"keyword": "before"}), None),
    ] {
        let (mut s, store) = keyword_history_fixture();
        s.execute("library.filter", &json!({"keyword": "before|child", "rating": 4, "text": "synthetic"})).unwrap();
        let selection = s.selection.clone();
        let before = s.catalog.to_snapshot();
        assert_eq!(s.execute(command, &params).unwrap()["changed"], 2);
        assert_eq!(s.filter.keyword.as_deref(), keyword);
        assert!(s.undo.last().unwrap().global, "global keywords must not acquire photo targets");
        let after = s.catalog.to_snapshot();
        // New filter fields are the user's choices, even when the followed keyword still matches.
        s.execute("library.filter", &json!({"rating": 5, "text": "synthetic-2", "only": [2]})).unwrap();
        let after_filter = s.filter.clone();
        let mut before_filter = after_filter.clone();
        before_filter.keyword = Some("before|child".into());
        for (step, expected_catalog, expected_filter) in
            [("edit.undo", &before, &before_filter), ("edit.redo", &after, &after_filter), ("edit.undo", &before, &before_filter)]
        {
            s.execute(step, &json!({})).unwrap();
            assert_eq!(&s.filter, expected_filter, "{command}: {step} must follow only the keyword field");
            assert_eq!(&s.catalog.to_snapshot(), expected_catalog);
            assert_eq!(s.selection, selection);
            assert_eq!(s.visible_cloned(), vec![PhotoId(2)], "the current photo must remain visible after {step}");
            assert_eq!(memory_library(&store).catalog.to_snapshot(), s.catalog.to_snapshot());
        }
    }
}

#[test]
fn keyword_history_preserves_an_unrelated_keyword_source_and_selection() {
    for keyword in [Some("other"), None] {
        let (mut s, store) = keyword_history_fixture();
        let album = s.execute("album.create", &json!({"name": "Other Photos"})).unwrap()["id"].as_u64().unwrap();
        s.execute("album.addPhotos", &json!({"id": album, "ids": [3]})).unwrap();
        s.undo.clear();
        s.execute("library.filter", &json!({"keyword": "before|child"})).unwrap();
        let before = s.catalog.to_snapshot();
        s.execute("keyword.rename", &json!({"from": "before", "to": "after"})).unwrap();
        let after = s.catalog.to_snapshot();
        s.execute("library.source", &json!({"kind": "album", "id": album})).unwrap();
        s.execute("library.filter", &json!({"keyword": keyword, "rating": 0, "text": "synthetic-3", "only": [3]})).unwrap();
        select(&mut s, &[3], 3);
        let filter = s.filter.clone();
        let source = s.source;
        for (step, expected_catalog) in [("edit.undo", &before), ("edit.redo", &after), ("edit.undo", &before)] {
            s.execute(step, &json!({})).unwrap();
            assert_eq!(s.filter, filter, "a different keyword or cleared filter must stay untouched");
            assert_eq!(s.source, source);
            assert_eq!(s.selection, Selection::single(PhotoId(3)));
            assert_eq!(s.visible_cloned(), vec![PhotoId(3)]);
            assert_eq!(&s.catalog.to_snapshot(), expected_catalog);
            assert_eq!(memory_library(&store).catalog.to_snapshot(), s.catalog.to_snapshot());
        }
    }
}

#[test]
fn merged_keyword_history_keeps_one_contiguous_filter_transition_and_replays() {
    let (mut s, store) = keyword_history_fixture();
    s.execute("library.filter", &json!({"keyword": "before|child", "text": "synthetic"})).unwrap();
    let selection = s.selection.clone();
    let before = s.catalog.to_snapshot();
    for (from, to) in [("before", "middle"), ("middle", "after")] {
        s.execute("keyword.rename", &json!({"from": from, "to": to})).unwrap();
    }
    let after = s.catalog.to_snapshot();
    s.merge_undo(2, "Rename Keyword Twice");
    assert_eq!(s.undo.len(), 1);
    let entry = s.undo.last().unwrap();
    assert!(entry.global);
    let transition = entry.keyword_filter.as_ref().unwrap();
    assert_eq!(transition.from.as_deref(), Some("after|child"));
    assert_eq!(transition.to.as_deref(), Some("before|child"));
    for (step, keyword, snapshot) in
        [("edit.undo", "before|child", &before), ("edit.redo", "after|child", &after), ("edit.undo", "before|child", &before)]
    {
        s.execute(step, &json!({})).unwrap();
        assert_eq!(s.filter.keyword.as_deref(), Some(keyword));
        assert_eq!(s.filter.text, "synthetic");
        assert_eq!(s.selection, selection);
        assert!(s.visible_cloned().contains(&PhotoId(2)));
        assert_eq!(&s.catalog.to_snapshot(), snapshot);
        assert_eq!(memory_library(&store).catalog.to_snapshot(), s.catalog.to_snapshot());
    }
}

#[test]
fn disconnected_keyword_filter_transitions_remain_separate_history_steps() {
    let (mut s, store) = keyword_history_fixture();
    s.execute("library.filter", &json!({"keyword": "before|child"})).unwrap();
    s.execute("keyword.rename", &json!({"from": "before", "to": "after"})).unwrap();
    s.execute("library.filter", &json!({"keyword": "other"})).unwrap();
    s.execute("keyword.rename", &json!({"from": "other", "to": "elsewhere"})).unwrap();
    s.merge_undo(2, "Different Keywords");
    assert_eq!(s.undo.len(), 2, "a user's intervening keyword choice is not a filter transition to merge");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.filter.keyword.as_deref(), Some("other"));
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.filter.keyword.as_deref(), Some("other"));
    for _ in 0..2 {
        s.execute("edit.redo", &json!({})).unwrap();
    }
    assert_eq!(s.filter.keyword.as_deref(), Some("elsewhere"));
    assert_eq!(memory_library(&store).catalog.to_snapshot(), s.catalog.to_snapshot());
}
