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

// These regressions run against the real Native Session in the engine test suite.
mod history_regressions {
    use crate::Session;
    use lightcraft_catalog::{Op, Photo, PhotoId, Sort, SortKey, Source};
    use serde_json::json;

    fn seed(n: u64) -> Session {
        let mut s = Session::new();
        for id in 1..=n {
            let p = Photo::new(PhotoId(id), Source::Demo { scene: 0 }, &format!("photo-{id:02}.jpg"), "JPEG", 16, 16, "2026-10-09");
            s.commit("fixture", Op::AddPhoto { photo: Box::new(p) }).unwrap();
        }
        s.sort = Sort { key: SortKey::FileName, ascending: true, ..Default::default() };
        s.undo.clear();
        s.redo.clear();
        s.drain_log();
        s
    }
    fn select(s: &mut Session, ids: &[u64], active: u64) {
        s.execute("library.select", &json!({"ids":ids,"active":active})).unwrap();
    }
    fn ids(s: &Session) -> Vec<u64> {
        s.selection.ids.iter().map(|id| id.0).collect()
    }
    fn visible(s: &mut Session) -> Vec<u64> {
        s.visible_cloned().iter().map(|id| id.0).collect()
    }
    fn trace(label: &str, s: &mut Session) {
        let vis = visible(s);
        println!(
            "{label}: visible={vis:?}, ids={:?}, raw_active={:?}, active={:?}, interaction={:?}, undo={:?}, redo={:?}",
            ids(s),
            s.selection.active,
            s.active(),
            s.interaction.as_ref().map(|i| i.photo),
            s.undo.iter().map(|e| e.label.as_str()).collect::<Vec<_>>(),
            s.redo.iter().map(|e| e.label.as_str()).collect::<Vec<_>>()
        );
    }
    fn drag(s: &mut Session) {
        s.execute("develop.beginInteraction", &json!({"label":"Exposure drag"})).unwrap();
        s.execute("develop.set", &json!({"control":"light.exposure","value":1.0})).unwrap();
    }

    #[test]
    fn survivors_preserve_order_active_and_owner_state() {
        for command in ["photo.delete", "photo.deletePermanently"] {
            let mut s = seed(7);
            select(&mut s, &[6, 3, 5], 5);
            // Owner sentinels exercise helper clearing/preservation, not mask/spot geometry.
            s.active_mask = Some(7);
            s.active_spot = Some(0);
            s.execute(command, &json!({"ids":[3]})).unwrap();
            assert_eq!(ids(&s), [6, 5]);
            assert_eq!(s.active(), Some(PhotoId(5)));
            assert_eq!((s.active_mask, s.active_spot), (Some(7), Some(0)));
            s.execute(command, &json!({"ids":[5]})).unwrap();
            assert_eq!(ids(&s), [6]);
            assert_eq!(s.active(), Some(PhotoId(6)));
            assert_eq!((s.active_mask, s.active_spot), (None, None));
        }
    }
    #[test]
    fn filtered_and_album_middle_end_all_and_outside_targets() {
        for album in [false, true] {
            let mut s = seed(8);
            select(&mut s, &[2], 2);
            if album {
                let id = s.execute("album.create", &json!({"name":"Scoped"})).unwrap()["id"].as_u64().unwrap();
                s.execute("album.addPhotos", &json!({"id":id,"ids":[2,4,6]})).unwrap();
                s.execute("library.source", &json!({"kind":"album","id":id})).unwrap();
            } else {
                s.execute("library.filter", &json!({"only":[2,4,6]})).unwrap();
            }
            assert_eq!(visible(&mut s), [2, 4, 6]);
            select(&mut s, &[4], 4);
            s.execute("photo.delete", &json!({"ids":[1]})).unwrap();
            assert_eq!(ids(&s), [4]);
            assert_eq!(s.active(), Some(PhotoId(4)));
            s.execute("photo.delete", &json!({})).unwrap();
            assert_eq!(s.active(), Some(PhotoId(6)));
            s.execute("photo.delete", &json!({})).unwrap();
            assert_eq!(s.active(), Some(PhotoId(2)));
            s.execute("photo.delete", &json!({})).unwrap();
            assert!(visible(&mut s).is_empty());
            assert!(ids(&s).is_empty());
            assert_eq!(s.active(), None);
        }
    }
    #[test]
    fn trash_soft_noop_and_permanent_end_all_empty() {
        let mut s = seed(5);
        select(&mut s, &[2, 3, 4], 2);
        s.execute("photo.delete", &json!({})).unwrap();
        s.execute("library.source", &json!({"kind":"recentlyDeleted"})).unwrap();
        select(&mut s, &[3, 4], 4);
        s.execute("photo.delete", &json!({})).unwrap();
        assert_eq!(ids(&s), [3, 4]);
        assert_eq!(s.active(), Some(PhotoId(4)));
        s.execute("photo.deletePermanently", &json!({"ids":[4]})).unwrap();
        assert_eq!(ids(&s), [3]);
        assert_eq!(s.active(), Some(PhotoId(3)));
        s.execute("library.emptyRecentlyDeleted", &json!({})).unwrap();
        assert!(visible(&mut s).is_empty());
        assert!(ids(&s).is_empty());
        assert_eq!(s.active(), None);
        let undo = s.undo.len();
        assert_eq!(s.execute("library.emptyRecentlyDeleted", &json!({})).unwrap()["deleted"], 0);
        assert_eq!(s.undo.len(), undo);
    }
    #[test]
    fn no_selection_empty_trash_does_not_manufacture_selection() {
        let mut s = seed(5);
        select(&mut s, &[2], 2);
        s.execute("photo.delete", &json!({})).unwrap();
        s.execute("library.select", &json!({"ids":[]})).unwrap();
        s.execute("library.emptyRecentlyDeleted", &json!({})).unwrap();
        assert!(ids(&s).is_empty());
        assert_eq!(s.active(), None);
        assert_eq!(visible(&mut s), [1, 3, 4, 5]);
    }
    #[test]
    fn collapsed_stack_live_deletion_keeps_valid_owner() {
        let mut s = seed(4);
        select(&mut s, &[2, 3], 2);
        s.execute("stack.group", &json!({"ids":[2,3],"collapsed":true})).unwrap();
        assert_eq!(visible(&mut s), [1, 2, 4]);
        select(&mut s, &[2], 2);
        s.execute("photo.delete", &json!({})).unwrap();
        assert_eq!(visible(&mut s), [1, 3, 4]);
        let active = s.active().unwrap();
        assert!(s.visible_cloned().contains(&active));
    }
    #[test]
    fn outside_deletion_preserves_open_interaction_owner() {
        let mut s = seed(4);
        select(&mut s, &[2], 2);
        drag(&mut s);
        s.execute("photo.delete", &json!({"ids":[4]})).unwrap();
        assert_eq!(s.active(), Some(PhotoId(2)));
        assert_eq!(s.interaction.as_ref().map(|i| i.photo), Some(PhotoId(2)));
        s.execute("develop.endInteraction", &json!({})).unwrap();
        assert!(s.interaction.is_none());
    }

    // Intended invariants below fail on the frozen deletion source.
    #[test]
    fn regression_soft_delete_finishes_drag_before_delete_undo() {
        let mut s = seed(4);
        select(&mut s, &[2], 2);
        drag(&mut s);
        s.execute("photo.delete", &json!({})).unwrap();
        trace("after delete", &mut s);
        s.execute("edit.undo", &json!({})).unwrap();
        trace("after first undo", &mut s);
        assert!(
            !s.catalog.photo(PhotoId(2)).unwrap().deleted,
            "Undo after Delete must undo Delete; the departed-photo drag must already have finished"
        );
    }
    #[test]
    fn regression_permanent_delete_retires_interaction_before_cancel() {
        let mut s = seed(4);
        select(&mut s, &[2], 2);
        drag(&mut s);
        s.execute("photo.deletePermanently", &json!({})).unwrap();
        trace("after permanent", &mut s);
        let cancelled = s.execute("develop.cancelInteraction", &json!({}));
        println!("late cancel={cancelled:?}");
        assert!(cancelled.is_ok(), "deleting an interaction owner must not leave develop.cancel targeting a nonexistent photo");
    }
    #[test]
    fn regression_empty_trash_retires_removed_interaction_owner() {
        let mut s = seed(4);
        select(&mut s, &[2], 2);
        s.execute("photo.delete", &json!({})).unwrap();
        s.execute("library.source", &json!({"kind":"recentlyDeleted"})).unwrap();
        select(&mut s, &[2], 2);
        drag(&mut s);
        s.execute("library.emptyRecentlyDeleted", &json!({})).unwrap();
        trace("after empty", &mut s);
        assert!(s.interaction.is_none(), "empty trash removed the interaction photo");
    }
    #[test]
    fn regression_soft_redo_prunes_reselected_deleted_photo() {
        let mut s = seed(4);
        select(&mut s, &[2], 2);
        s.execute("photo.delete", &json!({})).unwrap();
        s.execute("edit.undo", &json!({})).unwrap();
        select(&mut s, &[2], 2);
        s.execute("edit.redo", &json!({})).unwrap();
        trace("after soft redo", &mut s);
        let vis = s.visible_cloned();
        assert!(s.selection.ids.iter().all(|id| vis.contains(id)), "redo left a deleted/hidden photo selected and enabled as the next bulk target");
    }
    #[test]
    fn regression_permanent_redo_prunes_reselected_removed_photo() {
        let mut s = seed(4);
        select(&mut s, &[2], 2);
        s.execute("photo.deletePermanently", &json!({})).unwrap();
        s.execute("edit.undo", &json!({})).unwrap();
        select(&mut s, &[2], 2);
        s.execute("edit.redo", &json!({})).unwrap();
        trace("after permanent redo", &mut s);
        let action = s.execute("photo.delete", &json!({}));
        println!("next Delete={action:?}");
        assert!(ids(&s).is_empty() || s.active().is_some(), "permanent redo left stale selection with no active photo");
    }
    #[test]
    fn regression_stack_undo_prunes_hidden_fallback_photo() {
        let mut s = seed(2);
        select(&mut s, &[1, 2], 1);
        s.execute("stack.group", &json!({"ids":[1,2],"collapsed":true})).unwrap();
        select(&mut s, &[1], 1);
        s.execute("photo.delete", &json!({})).unwrap();
        assert_eq!(s.active(), Some(PhotoId(2)));
        s.execute("edit.undo", &json!({})).unwrap();
        trace("after stack undo", &mut s);
        let vis = s.visible_cloned();
        assert!(
            s.selection.ids.iter().all(|id| vis.contains(id)),
            "restoring a collapsed stack top hid the selected fallback; a hidden photo remains a bulk target"
        );
    }

    #[test]
    fn rejected_deletion_keeps_drag_preview_history_and_redo() {
        for command in ["photo.delete", "photo.deletePermanently"] {
            let mut s = seed(4);
            select(&mut s, &[2], 2);
            s.execute("photo.rate", &json!({"rating": 3})).unwrap();
            s.execute("edit.undo", &json!({})).unwrap();
            drag(&mut s);
            let catalog = s.catalog.to_snapshot();
            let undo = s.undo.len();
            let redo = s.redo.len();
            s.drain_log();
            assert!(s.execute(command, &json!({"ids": [2, 999]})).is_err());
            assert_eq!(s.catalog.to_snapshot(), catalog);
            assert_eq!((s.undo.len(), s.redo.len()), (undo, redo));
            assert_eq!(s.interaction.as_ref().map(|i| i.photo), Some(PhotoId(2)));
            assert!(s.drain_log().is_empty());
        }
    }

    #[test]
    fn empty_targets_and_empty_trash_do_not_finish_a_drag() {
        for command in ["photo.delete", "photo.deletePermanently", "library.emptyRecentlyDeleted"] {
            let mut s = seed(4);
            select(&mut s, &[2], 2);
            drag(&mut s);
            let before = s.develop_of(PhotoId(2)).unwrap();
            let result = s.execute(command, &json!({"ids": []})).unwrap();
            assert_eq!(result[if command == "photo.delete" { "changed" } else { "deleted" }], 0);
            assert_eq!(s.interaction.as_ref().map(|i| i.photo), Some(PhotoId(2)));
            assert_eq!(s.develop_of(PhotoId(2)).unwrap(), before);
            // Keep the existing empty permanent batch's undo semantics.
            assert_eq!(s.undo.len(), usize::from(command == "photo.deletePermanently"));
        }
    }

    #[test]
    fn soft_delete_of_already_deleted_owner_keeps_its_drag() {
        let mut s = seed(4);
        select(&mut s, &[2], 2);
        s.execute("photo.delete", &json!({})).unwrap();
        s.execute("library.source", &json!({"kind": "recentlyDeleted"})).unwrap();
        select(&mut s, &[2], 2);
        drag(&mut s);
        assert_eq!(s.execute("photo.delete", &json!({})).unwrap()["changed"], 1);
        assert_eq!(s.interaction.as_ref().map(|i| i.photo), Some(PhotoId(2)));
    }

    #[test]
    fn unrelated_permanent_target_keeps_changed_drag_owner() {
        let mut s = seed(4);
        select(&mut s, &[2], 2);
        drag(&mut s);
        s.execute("photo.deletePermanently", &json!({"ids": [4]})).unwrap();
        assert_eq!(s.interaction.as_ref().map(|i| i.photo), Some(PhotoId(2)));
        assert_eq!(s.active(), Some(PhotoId(2)));
        assert_eq!(s.develop_of(PhotoId(2)).unwrap().light.exposure, 1.0);
    }

    #[test]
    fn deleting_owner_orders_changed_drag_and_drops_unchanged_drag() {
        for command in ["photo.delete", "photo.deletePermanently"] {
            for changed in [false, true] {
                let mut s = seed(4);
                select(&mut s, &[2], 2);
                let original = s.develop_of(PhotoId(2)).unwrap();
                s.execute("develop.beginInteraction", &json!({"label": "Exposure drag"})).unwrap();
                if changed {
                    s.execute("develop.set", &json!({"control": "light.exposure", "value": 1.0})).unwrap();
                }
                s.execute(command, &json!({})).unwrap();
                assert!(s.interaction.is_none());
                assert_eq!(s.undo.len(), 1 + usize::from(changed));
                s.execute("edit.undo", &json!({})).unwrap();
                assert!(!s.catalog.photo(PhotoId(2)).unwrap().deleted);
                if changed {
                    assert_eq!(s.develop_of(PhotoId(2)).unwrap().light.exposure, 1.0);
                    s.execute("edit.undo", &json!({})).unwrap();
                }
                assert_eq!(s.develop_of(PhotoId(2)).unwrap(), original);
            }
        }
    }

    #[test]
    fn redo_prunes_only_invalidated_companion_and_keeps_active_tools() {
        for command in ["photo.delete", "photo.deletePermanently"] {
            let mut s = seed(4);
            select(&mut s, &[2], 2);
            s.execute(command, &json!({})).unwrap();
            s.execute("edit.undo", &json!({})).unwrap();
            select(&mut s, &[2, 3], 3);
            s.active_mask = Some(7);
            s.active_spot = Some(0);
            s.execute("edit.redo", &json!({})).unwrap();
            assert_eq!(ids(&s), [3]);
            assert_eq!(s.active(), Some(PhotoId(3)));
            assert_eq!((s.active_mask, s.active_spot), (Some(7), Some(0)));
        }
    }

    #[test]
    fn deletion_redo_repairs_filtered_and_album_current_selection() {
        for album in [false, true] {
            for command in ["photo.delete", "photo.deletePermanently"] {
                let mut s = seed(6);
                select(&mut s, &[2], 2);
                if album {
                    let id = s.execute("album.create", &json!({"name": "Scope"})).unwrap()["id"].as_u64().unwrap();
                    s.execute("album.addPhotos", &json!({"id": id, "ids": [2, 4, 6]})).unwrap();
                    s.execute("library.source", &json!({"kind": "album", "id": id})).unwrap();
                } else {
                    s.execute("library.filter", &json!({"only": [2, 4, 6]})).unwrap();
                }
                s.execute(command, &json!({})).unwrap();
                s.execute("edit.undo", &json!({})).unwrap();
                select(&mut s, &[2, 6], 2);
                s.active_mask = Some(7);
                s.active_spot = Some(0);
                let source = s.source;
                let filter = s.filter.clone();
                s.execute("edit.redo", &json!({})).unwrap();
                assert_eq!(ids(&s), [6]);
                assert_eq!(s.active(), Some(PhotoId(6)));
                assert_eq!((s.active_mask, s.active_spot), (None, None));
                assert_eq!((s.source, s.filter.clone()), (source, filter));
            }
        }
    }

    #[test]
    fn stack_history_collapse_repairs_active_but_unrelated_history_keeps_hidden_selection() {
        let mut s = seed(4);
        select(&mut s, &[1, 2], 1);
        s.execute("stack.group", &json!({"ids": [1, 2], "collapsed": true})).unwrap();
        s.execute("edit.undo", &json!({})).unwrap();
        select(&mut s, &[2], 2);
        s.execute("edit.redo", &json!({})).unwrap();
        assert_eq!(s.active(), Some(PhotoId(3))); // old next visible neighbor
        assert_eq!(ids(&s), [3]);

        s.execute("photo.setMeta", &json!({"ids": [3, 4], "keywords": ["before"]})).unwrap();
        s.execute("keyword.rename", &json!({"from": "before", "to": "after"})).unwrap();
        s.execute("library.filter", &json!({"only": [3]})).unwrap();
        select(&mut s, &[2, 3], 2); // deliberately hidden existing active
        s.active_mask = Some(7);
        s.active_spot = Some(0);
        let selection = s.selection.clone();
        for command in ["edit.undo", "edit.redo"] {
            s.execute(command, &json!({})).unwrap();
            assert_eq!(s.selection, selection);
            assert_eq!((s.active_mask, s.active_spot), (Some(7), Some(0)));
            assert_eq!(visible(&mut s), [3]);
        }
    }

    #[test]
    fn single_photo_history_reveal_still_wins_over_current_album() {
        let mut s = seed(4);
        select(&mut s, &[1], 1);
        let album = s.execute("album.create", &json!({"name": "Other"})).unwrap()["id"].as_u64().unwrap();
        s.execute("album.addPhotos", &json!({"id": album, "ids": [2]})).unwrap();
        s.undo.clear();
        s.execute("develop.set", &json!({"control": "light.exposure", "value": 1.0})).unwrap();
        for command in ["edit.undo", "edit.redo"] {
            s.execute("library.source", &json!({"kind": "album", "id": album})).unwrap();
            select(&mut s, &[2], 2);
            s.execute(command, &json!({})).unwrap();
            assert_eq!(s.active(), Some(PhotoId(1)));
            assert_eq!(s.source, crate::LibrarySource::All);
            assert!(s.visible_cloned().contains(&PhotoId(1)));
        }
    }

    #[test]
    fn followed_keyword_history_keeps_hidden_companion_and_user_filter() {
        let mut s = seed(4);
        select(&mut s, &[1, 2], 2);
        s.execute("photo.setMeta", &json!({"keywords": ["before|child"]})).unwrap();
        s.undo.clear();
        s.execute("library.filter", &json!({"keyword": "before|child"})).unwrap();
        s.execute("keyword.rename", &json!({"from": "before", "to": "after"})).unwrap();
        s.execute("library.filter", &json!({"only": [2]})).unwrap();
        let selection = s.selection.clone();
        for (command, keyword) in [("edit.undo", "before|child"), ("edit.redo", "after|child")] {
            s.execute(command, &json!({})).unwrap();
            assert_eq!(s.filter.keyword.as_deref(), Some(keyword));
            assert_eq!(s.filter.only, [PhotoId(2)]);
            assert_eq!(s.selection, selection);
            assert_eq!(visible(&mut s), [2]);
        }
    }
    #[test]
    fn deletion_finishes_target_owner_after_source_switch_changes_selection() {
        let mut s = seed(4);
        select(&mut s, &[2], 2);
        let album = s.execute("album.create", &json!({"name": "Other"})).unwrap()["id"].as_u64().unwrap();
        s.execute("album.addPhotos", &json!({"id": album, "ids": [3]})).unwrap();
        s.undo.clear();
        drag(&mut s);
        s.execute("library.source", &json!({"kind": "album", "id": album})).unwrap();
        assert_eq!(s.active(), Some(PhotoId(3)));
        assert_eq!(s.interaction.as_ref().map(|i| i.photo), Some(PhotoId(2)));
        s.execute("photo.delete", &json!({"ids": [2]})).unwrap();
        assert!(s.interaction.is_none());
        assert_eq!(s.active(), Some(PhotoId(3)));
        assert_eq!(s.undo.len(), 2);
        s.execute("edit.undo", &json!({})).unwrap();
        assert!(!s.catalog.photo(PhotoId(2)).unwrap().deleted);
    }

    #[test]
    fn permanent_redo_repairs_an_existing_hidden_owner_when_its_record_is_removed() {
        let mut s = seed(4);
        select(&mut s, &[2], 2);
        s.execute("photo.deletePermanently", &json!({})).unwrap();
        s.execute("edit.undo", &json!({})).unwrap();
        s.execute("library.filter", &json!({"only": [3]})).unwrap();
        select(&mut s, &[2], 2);
        s.execute("edit.redo", &json!({})).unwrap();
        assert_eq!(s.active(), Some(PhotoId(3)));
        assert_eq!(ids(&s), [3]);
        assert_eq!(s.filter.only, [PhotoId(3)]);
    }

    #[test]
    fn failed_history_preserves_selection_and_tools() {
        let mut s = seed(4);
        select(&mut s, &[2], 2);
        s.execute("photo.delete", &json!({})).unwrap();
        s.execute("edit.undo", &json!({})).unwrap();
        select(&mut s, &[2, 3], 2);
        s.active_mask = Some(7);
        s.active_spot = Some(0);
        let selection = s.selection.clone();
        s.redo.last_mut().unwrap().op = Op::RemovePhoto { id: PhotoId(999) };
        assert!(s.execute("edit.redo", &json!({})).is_err());
        assert_eq!(s.selection, selection);
        assert_eq!((s.active_mask, s.active_spot), (Some(7), Some(0)));
        assert_eq!(s.redo.len(), 1);
    }
    fn deletion_redo_fixture(command: &str) -> Session {
        let mut s = seed(4);
        select(&mut s, &[2], 2);
        if command == "library.emptyRecentlyDeleted" {
            s.execute("photo.delete", &json!({})).unwrap();
            s.execute("library.source", &json!({"kind": "recentlyDeleted"})).unwrap();
        }
        s.execute(command, &json!({})).unwrap();
        s.execute("edit.undo", &json!({})).unwrap();
        select(&mut s, &[2], 2);
        s
    }

    fn assert_deletion_redone(s: &Session, command: &str, redone: bool) {
        let photo = s.catalog.photo(PhotoId(2));
        if command == "photo.delete" {
            assert_eq!(photo.unwrap().deleted, redone);
        } else {
            assert_eq!(photo.is_none(), redone);
            if let Some(photo) = photo {
                assert_eq!(photo.deleted, command == "library.emptyRecentlyDeleted");
            }
        }
    }

    fn deletion_label(command: &str) -> &str {
        match command {
            "photo.delete" => "Delete",
            "photo.deletePermanently" => "Delete Permanently",
            _ => "Empty Recently Deleted",
        }
    }

    #[test]
    fn deletion_redo_changed_owner_commits_drag_and_clears_old_branch() {
        for command in ["photo.delete", "photo.deletePermanently", "library.emptyRecentlyDeleted"] {
            let mut s = deletion_redo_fixture(command);
            let original = s.develop_of(PhotoId(2)).unwrap();
            let selection = s.selection.clone();
            let undo = s.undo.len();
            s.drain_log();
            drag(&mut s);
            assert_eq!(s.execute("edit.redo", &json!({})).unwrap(), json!({"redone": null}));
            assert!(s.interaction.is_none());
            assert!(s.redo.is_empty());
            assert_eq!(s.undo.len(), undo + 1);
            assert_eq!(s.undo.last().unwrap().label, "Exposure drag");
            assert!(!s.drain_log().is_empty());
            assert_eq!(s.selection, selection);
            assert_deletion_redone(&s, command, false);
            assert_eq!(s.develop_of(PhotoId(2)).unwrap().light.exposure, 1.0);
            // A later update must still edit the restored owner, never its old visible neighbour.
            s.execute("develop.set", &json!({"control": "light.exposure", "value": 2.0})).unwrap();
            assert_eq!(s.develop_of(PhotoId(3)).unwrap().light.exposure, 0.0);
            s.execute("edit.undo", &json!({})).unwrap();
            assert_eq!(s.develop_of(PhotoId(2)).unwrap().light.exposure, 1.0);
            s.execute("edit.undo", &json!({})).unwrap();
            assert_eq!(s.develop_of(PhotoId(2)).unwrap(), original);
            assert_deletion_redone(&s, command, false);
        }
    }

    #[test]
    fn deletion_redo_unchanged_owner_retires_drag_before_replaying() {
        for command in ["photo.delete", "photo.deletePermanently", "library.emptyRecentlyDeleted"] {
            let mut s = deletion_redo_fixture(command);
            let undo = s.undo.len();
            s.execute("develop.beginInteraction", &json!({"label": "Exposure drag"})).unwrap();
            assert_eq!(s.execute("edit.redo", &json!({})).unwrap(), json!({"redone": deletion_label(command)}));
            assert!(s.interaction.is_none());
            assert!(s.redo.is_empty());
            assert_eq!(s.undo.len(), undo + 1);
            assert_eq!(s.undo.last().unwrap().label, deletion_label(command));
            assert_deletion_redone(&s, command, true);
            let visible = s.visible_cloned();
            assert!(s.selection.ids.iter().all(|id| visible.contains(id)));
            assert!(s.selection.active.is_none_or(|id| visible.contains(&id)));
            assert!(s.execute("develop.cancelInteraction", &json!({})).is_ok());
        }
    }

    #[test]
    fn deletion_redo_finishes_unrelated_owner_with_the_same_new_edit_policy() {
        for command in ["photo.delete", "photo.deletePermanently", "library.emptyRecentlyDeleted"] {
            for changed in [false, true] {
                let mut s = deletion_redo_fixture(command);
                s.execute("library.source", &json!({"kind": "all"})).unwrap();
                select(&mut s, &[3], 3);
                let undo = s.undo.len();
                let selection = s.selection.clone();
                s.execute("develop.beginInteraction", &json!({"label": "Exposure drag"})).unwrap();
                if changed {
                    s.execute("develop.set", &json!({"control": "light.exposure", "value": 1.0})).unwrap();
                }
                let expected = if changed { json!({"redone": null}) } else { json!({"redone": deletion_label(command)}) };
                assert_eq!(s.execute("edit.redo", &json!({})).unwrap(), expected);
                assert!(s.interaction.is_none());
                assert!(s.redo.is_empty());
                assert_eq!(s.undo.len(), undo + 1);
                assert_eq!(s.selection, selection);
                assert_eq!(s.develop_of(PhotoId(3)).unwrap().light.exposure, if changed { 1.0 } else { 0.0 });
                assert_deletion_redone(&s, command, !changed);
                assert!(s.execute("develop.cancelInteraction", &json!({})).is_ok());
            }
        }
    }

    #[test]
    fn disabled_redo_with_empty_history_keeps_changed_and_unchanged_drag() {
        for changed in [false, true] {
            let mut s = seed(4);
            select(&mut s, &[2], 2);
            s.execute("develop.beginInteraction", &json!({"label": "Exposure drag"})).unwrap();
            if changed {
                s.execute("develop.set", &json!({"control": "light.exposure", "value": 1.0})).unwrap();
            }
            let catalog = s.catalog.to_snapshot();
            let selection = s.selection.clone();
            s.drain_log();
            assert!(matches!(s.execute("edit.redo", &json!({})), Err(crate::EngineError::Disabled(..))));
            assert_eq!(s.catalog.to_snapshot(), catalog);
            assert_eq!(s.selection, selection);
            assert_eq!(s.interaction.as_ref().map(|i| i.photo), Some(PhotoId(2)));
            assert!(s.undo.is_empty());
            assert!(s.redo.is_empty());
            assert!(s.drain_log().is_empty());
        }
    }

    #[test]
    fn redo_unchanged_drag_retires_owner_and_preserves_failed_history_selection() {
        let mut s = deletion_redo_fixture("photo.deletePermanently");
        select(&mut s, &[2, 3], 2);
        s.active_mask = Some(7);
        s.active_spot = Some(0);
        s.execute("develop.beginInteraction", &json!({"label": "Exposure drag"})).unwrap();
        s.redo.last_mut().unwrap().op = Op::RemovePhoto { id: PhotoId(999) };
        let catalog = s.catalog.to_snapshot();
        let selection = s.selection.clone();
        let history = (s.undo.len(), s.redo.len());
        s.drain_log();
        assert!(s.execute("edit.redo", &json!({})).is_err());
        assert!(s.interaction.is_none());
        assert_eq!(s.catalog.to_snapshot(), catalog);
        assert_eq!(s.selection, selection);
        assert_eq!((s.active_mask, s.active_spot), (Some(7), Some(0)));
        assert_eq!((s.undo.len(), s.redo.len()), history);
        assert!(s.drain_log().is_empty());
    }

    #[test]
    fn redo_changed_drag_discards_invalid_old_history_instead_of_applying_it() {
        let mut s = deletion_redo_fixture("photo.deletePermanently");
        s.redo.last_mut().unwrap().op = Op::RemovePhoto { id: PhotoId(999) };
        drag(&mut s);
        assert_eq!(s.execute("edit.redo", &json!({})).unwrap(), json!({"redone": null}));
        assert!(s.interaction.is_none());
        assert!(s.redo.is_empty());
        assert_eq!(s.undo.last().unwrap().label, "Exposure drag");
        assert_deletion_redone(&s, "photo.deletePermanently", false);
        assert_eq!(s.active(), Some(PhotoId(2)));
        assert_eq!(s.develop_of(PhotoId(2)).unwrap().light.exposure, 1.0);
    }
}
