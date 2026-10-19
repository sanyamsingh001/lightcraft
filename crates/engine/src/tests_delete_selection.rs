use crate::Session;
use lightcraft_catalog::PhotoId;
use serde_json::json;

fn select(s: &mut Session, ids: &[PhotoId], active: Option<PhotoId>) {
    s.execute("library.select", &json!({"ids": ids.iter().map(|p| p.0).collect::<Vec<_>>(), "active": active.map(|p| p.0)})).unwrap();
}

#[test]
fn delete_middle_follows_next_and_undo_redo_keeps_place() {
    let mut s = Session::with_demo();
    let visible = s.visible_cloned();
    let at = visible.len() / 2;
    let (deleted, next) = (visible[at], visible[at + 1]);
    select(&mut s, &[deleted], Some(deleted));
    s.execute("photo.delete", &json!({})).unwrap();
    assert_eq!(s.selection.ids, vec![next]);
    assert_eq!(s.active(), Some(next));
    for _ in 0..2 {
        s.execute("edit.undo", &json!({})).unwrap();
        assert!(!s.catalog.photo(deleted).unwrap().deleted);
        assert_eq!(s.active(), Some(next));
        s.execute("edit.redo", &json!({})).unwrap();
        assert!(s.catalog.photo(deleted).unwrap().deleted);
        assert_eq!(s.active(), Some(next));
    }
}

#[test]
fn delete_last_follows_previous() {
    let mut s = Session::with_demo();
    let visible = s.visible_cloned();
    let at = visible.len() - 1;
    select(&mut s, &[visible[at]], Some(visible[at]));
    s.execute("photo.delete", &json!({})).unwrap();
    assert_eq!(s.selection.ids, vec![visible[at - 1]]);
    assert_eq!(s.active(), Some(visible[at - 1]));
}

#[test]
fn delete_other_targets_preserves_surviving_selection() {
    let mut s = Session::with_demo();
    let visible = s.visible_cloned();
    let retained = [visible[3], visible[6]];
    select(&mut s, &retained, Some(retained[0]));
    s.execute("photo.delete", &json!({"ids": [visible[0].0]})).unwrap();
    assert_eq!(s.selection.ids, retained);
    assert_eq!(s.active(), Some(retained[0]));
}

#[test]
fn partial_delete_prefers_remaining_selected_photo() {
    let mut s = Session::with_demo();
    let visible = s.visible_cloned();
    select(&mut s, &[visible[3], visible[6]], Some(visible[3]));
    s.execute("photo.delete", &json!({"ids": [visible[3].0]})).unwrap();
    assert_eq!(s.selection.ids, vec![visible[6]]);
    assert_eq!(s.active(), Some(visible[6]));
}

#[test]
fn delete_all_clears_selection() {
    let mut s = Session::with_demo();
    let visible = s.visible_cloned();
    select(&mut s, &visible, Some(visible[0]));
    s.execute("photo.delete", &json!({})).unwrap();
    assert!(s.visible_cloned().is_empty());
    assert!(s.selection.ids.is_empty());
    assert_eq!(s.active(), None);
}

#[test]
fn permanently_delete_in_trash_follows_next_visible_photo() {
    let mut s = Session::with_demo();
    let visible = s.visible_cloned();
    select(&mut s, &[visible[3], visible[4], visible[5]], Some(visible[3]));
    s.execute("photo.delete", &json!({})).unwrap();
    s.execute("library.source", &json!({"kind": "recentlyDeleted"})).unwrap();
    let trash = s.visible_cloned();
    select(&mut s, &[trash[1]], Some(trash[1]));
    s.execute("photo.deletePermanently", &json!({})).unwrap();
    assert!(s.catalog.photo(trash[1]).is_none());
    assert_eq!(s.selection.ids, vec![trash[2]]);
    assert_eq!(s.active(), Some(trash[2]));
}

#[test]
fn empty_trash_preserves_live_selection_and_empty_selection() {
    for keep_selection in [true, false] {
        let mut s = Session::with_demo();
        let visible = s.visible_cloned();
        select(&mut s, &[visible[3]], Some(visible[3]));
        s.execute("photo.delete", &json!({"ids": [visible[0].0]})).unwrap();
        if !keep_selection {
            select(&mut s, &[], None);
        }
        s.execute("library.emptyRecentlyDeleted", &json!({})).unwrap();
        assert!(s.catalog.photo(visible[0]).is_none());
        assert_eq!(s.active(), keep_selection.then_some(visible[3]));
        assert_eq!(s.selection.ids, if keep_selection { vec![visible[3]] } else { vec![] });
    }
}
