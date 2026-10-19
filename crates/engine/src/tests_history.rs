//! Residual undo/redo gaps after upstream single-photo history navigation.

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

#[test]
fn permanent_batch_deletion_clears_cover_and_round_trips_collections_and_journal() {
    for stack_size in [2, 3, 4] {
        let store = lightcraft_catalog::MemStore::new();
        let mut s = memory_library(&store);
        for id in 1..=5 {
            let photo = Photo::new(PhotoId(id), Source::Demo { scene: 0 }, &format!("synthetic-{id}.jpg"), "JPEG", 16, 16, "");
            s.commit("Synthetic Photo", Op::AddPhoto { photo: Box::new(photo) }).unwrap();
        }
        select(&mut s, &[1], 1);
        let album = s.execute("album.create", &json!({"name": "Shared"})).unwrap()["id"].as_u64().unwrap();
        let album_id = lightcraft_catalog::AlbumId(album);
        s.execute("album.addPhotos", &json!({"id": album, "ids": [1, 2, 3, 4]})).unwrap();
        s.execute("album.setCover", &json!({"id": album, "photo": 1})).unwrap();
        s.execute("stack.group", &json!({"ids": (1..=stack_size).collect::<Vec<_>>(), "collapsed": false})).unwrap();
        s.execute("photo.delete", &json!({"ids": [1, 2]})).unwrap();
        s.undo.clear();
        let before = s.catalog.to_snapshot();
        let result = s.execute("photo.deletePermanently", &json!({"ids": [1, 2, 1]})).unwrap();
        assert_eq!(result["deleted"], 2, "duplicate targets must not remove a collection twice");
        assert!(s.catalog.photo(PhotoId(1)).is_none() && s.catalog.photo(PhotoId(2)).is_none());
        let a = s.catalog.album(album_id).unwrap();
        assert_eq!(a.photos, vec![PhotoId(3), PhotoId(4)]);
        assert_eq!(a.cover, None);
        if stack_size == 4 {
            assert_eq!(s.catalog.stack_of(PhotoId(3)).unwrap().photos, vec![PhotoId(3), PhotoId(4)]);
        } else {
            assert_eq!(s.catalog.stacks().count(), 0, "zero or one survivor must be unstacked");
        }
        let deleted = s.catalog.to_snapshot();
        assert_eq!(memory_library(&store).catalog.to_snapshot(), deleted);
        s.execute("library.source", &json!({"kind": "all"})).unwrap();
        select(&mut s, &[5], 5);
        for (command, expected) in [("edit.undo", &before), ("edit.redo", &deleted), ("edit.undo", &before)] {
            s.execute(command, &json!({})).unwrap();
            assert_eq!(&s.catalog.to_snapshot(), expected, "{command}: complete collections and cover must round trip");
            assert_eq!(memory_library(&store).catalog.to_snapshot(), s.catalog.to_snapshot());
            assert_eq!(s.selection, Selection::single(PhotoId(5)), "collection history must not manufacture photo navigation");
        }
        assert!(s.execute("photo.deletePermanently", &json!({"ids": [1, 999]})).is_err());
        assert_eq!(s.catalog.to_snapshot(), before, "an invalid target rolls the whole deletion back");
        assert_eq!(memory_library(&store).catalog.to_snapshot(), before);
        assert_eq!((s.undo.len(), s.redo.len()), (0, 1));
    }
}

#[test]
fn history_reveals_the_photo_after_an_unrelated_album_folder_or_restrictive_filter() {
    for context in ["album", "folder", "filter", "album+filter", "folder+filter"] {
        let mut s = session();
        let album = s.execute("album.create", &json!({"name": "Other Photos"})).unwrap()["id"].as_u64().unwrap();
        s.execute("album.addPhotos", &json!({"id": album, "ids": [2]})).unwrap();
        s.undo.clear();
        s.execute("develop.set", &json!({"control": "light.exposure", "value": 1.0})).unwrap();
        for (command, exposure) in [("edit.undo", 0.0), ("edit.redo", 1.0)] {
            select(&mut s, &[2], 2);
            if context.starts_with("album") {
                s.execute("library.source", &json!({"kind": "album", "id": album})).unwrap();
            } else if context.starts_with("folder") {
                // A cached folder view, with no files to import or filesystem access.
                s.source = crate::LibrarySource::Folder;
                s.browse = Some(crate::Browse { path: "unrelated-folder".into(), subfolders: true });
            }
            if context.contains("filter") {
                s.execute("library.filter", &json!({"only": [2], "text": "synthetic-2"})).unwrap();
            }
            assert!(!s.visible().contains(&PhotoId(1)), "{context}");
            s.execute(command, &json!({})).unwrap();
            assert_eq!(s.active(), Some(PhotoId(1)), "{command}: {context}");
            assert!(s.visible().contains(&PhotoId(1)), "{command}: {context}");
            assert_eq!(s.source, crate::LibrarySource::All);
            assert_eq!(s.filter, lightcraft_catalog::Filter::default());
            if context.starts_with("folder") {
                assert!(s.browse.is_none());
            }
            assert_eq!(s.develop_of(PhotoId(1)).unwrap().light.exposure, exposure);
            assert_eq!((s.undo.len(), s.redo.len()), if command == "edit.undo" { (0, 1) } else { (1, 0) });
        }
    }
}

#[test]
fn history_preserves_a_source_filter_and_browse_that_already_show_the_target() {
    for context in ["album", "folder"] {
        let mut s = session();
        s.catalog
            .apply(Op::Relink {
                id: PhotoId(1),
                file_name: "synthetic-1.jpg".into(),
                source: Source::File { path: "same-folder/synthetic-1.jpg".into() },
                format: None,
            })
            .unwrap();
        let album = s.execute("album.create", &json!({"name": "Edited Photos", "addSelected": true})).unwrap()["id"].as_u64().unwrap();
        s.undo.clear();
        s.execute("develop.set", &json!({"control": "light.exposure", "value": 1.0})).unwrap();
        if context == "album" {
            s.execute("library.source", &json!({"kind": "album", "id": album})).unwrap();
        } else {
            s.source = crate::LibrarySource::Folder;
            s.browse = Some(crate::Browse { path: "same-folder".into(), subfolders: true });
        }
        s.execute("library.filter", &json!({"text": "synthetic-1"})).unwrap();
        let (source, filter, browse) = (s.source, s.filter.clone(), s.browse.clone());
        for command in ["edit.undo", "edit.redo"] {
            select(&mut s, &[2], 2);
            assert!(s.visible().contains(&PhotoId(1)));
            s.execute(command, &json!({})).unwrap();
            assert_eq!(s.active(), Some(PhotoId(1)));
            assert!(s.visible().contains(&PhotoId(1)));
            assert_eq!((s.source, &s.filter, &s.browse), (source, &filter, &browse));
        }
    }
}

#[test]
fn history_keeps_matching_sources_when_only_the_filter_hides_the_edited_photo() {
    for context in ["album", "folder", "libraryFolder"] {
        let mut s = session();
        s.catalog
            .apply(Op::Relink {
                id: PhotoId(1),
                file_name: "synthetic-1.jpg".into(),
                source: Source::File { path: "same-folder/synthetic-1.jpg".into() },
                format: None,
            })
            .unwrap();
        let album = s.execute("album.create", &json!({"name": "Edited Photos", "addSelected": true})).unwrap()["id"].as_u64().unwrap();
        s.undo.clear();
        s.execute("develop.set", &json!({"control": "light.exposure", "value": 1.0})).unwrap();
        match context {
            "album" => {
                s.execute("library.source", &json!({"kind": "album", "id": album})).unwrap();
            }
            "folder" => {
                s.source = crate::LibrarySource::Folder;
                s.browse = Some(crate::Browse { path: "same-folder".into(), subfolders: false });
            }
            _ => {
                s.source = crate::LibrarySource::LibraryFolder;
                s.library_folder = Some("same-folder".into());
            }
        }
        let (source, browse, folder) = (s.source, s.browse.clone(), s.library_folder.clone());
        for command in ["edit.undo", "edit.redo"] {
            s.execute("library.filter", &json!({"only": [2]})).unwrap();
            select(&mut s, &[2], 2);
            assert!(!s.visible().contains(&PhotoId(1)));
            s.execute(command, &json!({})).unwrap();
            assert_eq!((s.source, &s.browse, &s.library_folder), (source, &browse, &folder), "{command}: {context}");
            assert_eq!(s.filter, lightcraft_catalog::Filter::default());
            assert_eq!(s.active(), Some(PhotoId(1)));
            assert!(s.visible().contains(&PhotoId(1)));
        }
    }
}

#[test]
fn history_keeps_a_matching_filter_when_switching_out_of_an_unrelated_source() {
    let mut s = session();
    let album = s.execute("album.create", &json!({"name": "Other Photos"})).unwrap()["id"].as_u64().unwrap();
    s.execute("album.addPhotos", &json!({"id": album, "ids": [2]})).unwrap();
    s.undo.clear();
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 1.0})).unwrap();
    s.execute("library.filter", &json!({"text": "synthetic"})).unwrap();
    let filter = s.filter.clone();
    for command in ["edit.undo", "edit.redo"] {
        s.execute("library.source", &json!({"kind": "album", "id": album})).unwrap();
        select(&mut s, &[2], 2);
        s.execute(command, &json!({})).unwrap();
        assert_eq!(s.source, crate::LibrarySource::All);
        assert_eq!(s.filter, filter);
        assert_eq!(s.active(), Some(PhotoId(1)));
        assert!(s.visible().contains(&PhotoId(1)));
    }
}

#[test]
fn history_reveals_an_already_active_photo_hidden_by_the_undone_rating() {
    let mut s = session();
    s.execute("photo.rate", &json!({"rating": 5})).unwrap();
    s.execute("library.filter", &json!({"rating": 5})).unwrap();
    assert!(s.visible().contains(&PhotoId(1)));
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.active(), Some(PhotoId(1)));
    assert!(s.visible().contains(&PhotoId(1)));
    assert_eq!(s.filter, lightcraft_catalog::Filter::default());
    s.execute("library.filter", &json!({"rating": 1, "ratingOp": "atMost"})).unwrap();
    s.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(s.active(), Some(PhotoId(1)));
    assert!(s.visible().contains(&PhotoId(1)));
    assert_eq!(s.filter, lightcraft_catalog::Filter::default());
}

#[test]
fn history_reveals_local_photos_in_their_folder_without_importing_or_scanning() {
    for (path, folder) in [
        ("/synthetic/history/synthetic-1.jpg", "/synthetic/history"),
        ("synthetic-1.jpg", "."),
        ("./synthetic-1.jpg", "."),
        ("1:synthetic-1.jpg", "."),
        ("./1:synthetic-1.jpg", "."),
        ("relative/synthetic-1.jpg", "relative"),
    ] {
        let mut s = session();
        s.catalog
            .apply(Op::Relink { id: PhotoId(1), file_name: "synthetic-1.jpg".into(), source: Source::File { path: path.into() }, format: None })
            .unwrap();
        s.catalog.apply(Op::SetLocal { id: PhotoId(1), local: true }).unwrap();
        s.execute("develop.set", &json!({"control": "light.exposure", "value": 1.0})).unwrap();
        for command in ["edit.undo", "edit.redo"] {
            s.source = crate::LibrarySource::Folder;
            s.browse = Some(crate::Browse { path: "/synthetic/unrelated".into(), subfolders: false });
            select(&mut s, &[2], 2);
            s.execute("library.filter", &json!({"only": [2]})).unwrap();
            s.execute(command, &json!({})).unwrap();
            assert_eq!(s.active(), Some(PhotoId(1)));
            assert!(s.visible().contains(&PhotoId(1)));
            assert_eq!(s.source, crate::LibrarySource::Folder);
            assert_eq!(s.browse.as_ref().map(|browse| browse.path.as_str()), Some(folder));
            assert_eq!(s.filter, lightcraft_catalog::Filter::default());
            assert_eq!(s.catalog.len(), 3);
            assert!(s.catalog.photo(PhotoId(1)).unwrap().local);
            assert_eq!(s.catalog.photo(PhotoId(1)).unwrap().source, Source::File { path: path.into() });
            assert_eq!((s.undo.len(), s.redo.len()), if command == "edit.undo" { (0, 1) } else { (1, 0) });
        }
    }
}

#[test]
fn history_reveals_a_collapsed_stack_member_without_losing_redo_or_matching_filters() {
    let mut s = session();
    s.execute("stack.group", &json!({"ids": [1, 2], "top": 2})).unwrap();
    s.undo.clear();
    select(&mut s, &[1], 1);
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 1.0})).unwrap();
    s.execute("library.filter", &json!({"text": "synthetic"})).unwrap();
    let filter = s.filter.clone();
    for command in ["edit.undo", "edit.redo"] {
        let stack = s.catalog.stack_of(PhotoId(1)).unwrap().id;
        for op in s.catalog.set_collapsed_ops(&[stack], true) {
            s.catalog.apply(op).unwrap();
        }
        select(&mut s, &[3], 3);
        assert!(!s.visible().contains(&PhotoId(1)));
        s.execute(command, &json!({})).unwrap();
        assert_eq!(s.active(), Some(PhotoId(1)));
        assert!(s.visible().contains(&PhotoId(1)));
        assert!(!s.catalog.stack(stack).unwrap().collapsed);
        assert_eq!(s.filter, filter);
        assert_eq!((s.undo.len(), s.redo.len()), if command == "edit.undo" { (0, 1) } else { (1, 0) });
    }
}

#[test]
fn redo_finishes_changed_drags_as_new_edits_and_preserves_unchanged_redo() {
    for photo in [1, 2] {
        for changed in [false, true] {
            let mut s = session();
            s.execute("photo.rate", &json!({"rating": 5})).unwrap();
            s.execute("edit.undo", &json!({})).unwrap();
            select(&mut s, &[photo], photo);
            s.execute("develop.beginInteraction", &json!({"label": "Targeted Adjustment"})).unwrap();
            if changed {
                s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.75})).unwrap();
            }
            let result = s.execute("edit.redo", &json!({}));
            assert!(s.interaction.is_none(), "photo={photo}, changed={changed}");
            if changed {
                assert!(result.unwrap()["redone"].is_null());
                assert_eq!(s.active(), Some(PhotoId(photo)));
                assert_eq!(s.catalog.photo(PhotoId(1)).unwrap().rating, 0);
                assert_eq!(s.develop_of(PhotoId(photo)).unwrap().light.exposure, 0.75);
                assert_eq!((s.undo.len(), s.redo.len()), (1, 0));
                s.execute("edit.undo", &json!({})).unwrap();
                assert_eq!(s.develop_of(PhotoId(photo)).unwrap().light.exposure, 0.0);
                s.execute("edit.redo", &json!({})).unwrap();
                assert_eq!(s.develop_of(PhotoId(photo)).unwrap().light.exposure, 0.75);
                assert_eq!(s.catalog.photo(PhotoId(1)).unwrap().rating, 0);
            } else {
                result.unwrap();
                assert_eq!(s.active(), Some(PhotoId(1)));
                assert_eq!(s.catalog.photo(PhotoId(1)).unwrap().rating, 5);
                assert_eq!((s.undo.len(), s.redo.len()), (1, 0));
            }
        }
    }
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

fn file_history_step(s: &mut Session, fs: &dyn crate::rename::MoveFs, redo: bool) -> crate::Result<serde_json::Value> {
    s.execute_fn(if redo { "edit.redo" } else { "edit.undo" }, |s| {
        let label = if redo { s.redo_step_with(fs)? } else { s.undo_step_with(fs)? };
        Ok(json!({"label": label}))
    })
}

fn assert_file_catalog_and_journal(s: &Session, fs: &crate::rename::tests::FakeFs, store: &lightcraft_catalog::MemStore) {
    let files = fs.listing();
    for photo in s.catalog.photos() {
        let Source::File { path } = &photo.source else { panic!("synthetic file fixture") };
        assert!(files.iter().any(|(file, _)| file == path), "photo {} points to missing {path}: {files:?}", photo.id.0);
        assert_eq!(std::path::Path::new(path).file_name().unwrap().to_string_lossy(), photo.file_name.as_str());
    }
    let replayed = memory_library(store);
    assert_eq!(replayed.catalog.to_snapshot(), s.catalog.to_snapshot());
}

#[test]
fn merged_renames_coalesce_physical_moves_and_keep_retry_inverses_and_journal() {
    use crate::rename::tests::FakeFs;
    use crate::sidecar::{SidecarNaming, sidecar_path};

    let store = lightcraft_catalog::MemStore::new();
    let mut s = memory_library(&store);
    let fs = FakeFs::new(false, &[("/p/a.jpg", "Photo"), ("/p/a.xmp", "Edits"), ("/p/other.jpg", "Other")]);
    for (id, path) in [(1, "/p/a.jpg"), (2, "/p/other.jpg")] {
        let name = std::path::Path::new(path).file_name().unwrap().to_string_lossy();
        let photo = Photo::new(PhotoId(id), Source::File { path: path.into() }, &name, "JPEG", 16, 16, "");
        s.commit("Synthetic File", Op::AddPhoto { photo: Box::new(photo) }).unwrap();
    }
    select(&mut s, &[1], 1);
    let copy = s.execute("photo.virtualCopy", &json!({})).unwrap()["ids"][0].as_u64().unwrap();
    s.undo.clear();
    let before = fs.listing();
    for name in ["middle", "final"] {
        let plans = s.plan_rename_with(&fs, &[PhotoId(1)], name, 1);
        s.execute_fn("photo.rename", |s| Ok(json!({"renamed": s.apply_rename_with(&fs, &plans)?}))).unwrap();
        assert_file_catalog_and_journal(&s, &fs, &store);
    }
    let renamed = fs.listing();
    s.merge_undo(2, "Rename Twice");
    assert_eq!(s.undo.len(), 1);
    assert_eq!(s.file_moves(&s.undo.last().unwrap().op), vec![("/p/final.jpg".into(), "/p/a.jpg".into())]);
    // First exercise the ordinary merged step, before any corrective paths exist.
    for redo in [false, true] {
        file_history_step(&mut s, &fs, redo).unwrap();
        assert_eq!(fs.listing(), if redo { renamed.clone() } else { before.clone() });
        assert_eq!(s.catalog.photo(PhotoId(1)).unwrap().source, s.catalog.photo(PhotoId(copy)).unwrap().source);
        assert_file_catalog_and_journal(&s, &fs, &store);
    }
    for redo in [false, true, false] {
        select(&mut s, &[2], 2);
        let entry = if redo { s.redo.last().unwrap() } else { s.undo.last().unwrap() };
        let moves = s.file_moves(&entry.op);
        assert_eq!(moves.len(), 1, "virtual copies must share the final physical move");
        let (from, to) = &moves[0];
        let sidecar = sidecar_path(from, SidecarNaming::Stem).to_string_lossy().into_owned();
        *fs.fail.borrow_mut() = vec![sidecar, to.clone()];
        let history = (s.undo.len(), s.redo.len());
        assert!(file_history_step(&mut s, &fs, redo).is_err());
        assert_eq!((s.undo.len(), s.redo.len()), history);
        assert_eq!(s.selection, Selection::single(PhotoId(2)));
        assert_file_catalog_and_journal(&s, &fs, &store);
        fs.fail.borrow_mut().clear();
        file_history_step(&mut s, &fs, redo).unwrap();
        assert_eq!(fs.listing(), if redo { renamed.clone() } else { before.clone() });
        assert_eq!(s.catalog.photo(PhotoId(1)).unwrap().source, s.catalog.photo(PhotoId(copy)).unwrap().source);
        assert_file_catalog_and_journal(&s, &fs, &store);
    }
}

#[test]
fn partial_file_history_failures_retry_without_losing_either_inverse_or_journal() {
    use crate::rename::tests::FakeFs;

    for redo in [false, true] {
        let store = lightcraft_catalog::MemStore::new();
        let mut s = memory_library(&store);
        let fs = FakeFs::new(
            false,
            &[
                ("/p/a.jpg", "A"),
                ("/p/a.xmp", "A edits"),
                ("/p/b.jpg", "B"),
                ("/p/b.xmp", "B edits"),
                ("/p/c.jpg", "C"),
                ("/p/c.xmp", "C edits"),
                ("/p/other.jpg", "Other"),
            ],
        );
        let ops = ["/p/a.jpg", "/p/b.jpg", "/p/c.jpg", "/p/other.jpg"]
            .iter()
            .enumerate()
            .map(|(i, path)| {
                let name = std::path::Path::new(path).file_name().unwrap().to_string_lossy();
                let photo = Photo::new(PhotoId(i as u64 + 1), Source::File { path: (*path).into() }, &name, "JPEG", 16, 16, "");
                Op::AddPhoto { photo: Box::new(photo) }
            })
            .collect();
        s.commit("Synthetic Files", Op::Batch { ops }).unwrap();
        // A virtual copy shares B's physical file and must follow every partial correction too.
        let mut copy = (**s.catalog.photo(PhotoId(2)).unwrap()).clone();
        copy.id = PhotoId(5);
        copy.copy_of = Some(PhotoId(2));
        s.commit("Synthetic Copy", Op::AddPhoto { photo: Box::new(copy) }).unwrap();
        s.persist().unwrap();
        s.undo.clear();
        select(&mut s, &[1, 2, 3], 1);
        let before = fs.listing();
        let plans = s.plan_rename_with(&fs, &[PhotoId(1), PhotoId(2), PhotoId(3)], "Trip-{seq}", 1);
        s.execute_fn("photo.rename", |s| Ok(json!({"renamed": s.apply_rename_with(&fs, &plans)?}))).unwrap();
        let renamed = fs.listing();
        if redo {
            file_history_step(&mut s, &fs, false).unwrap();
        }
        select(&mut s, &[4], 4);
        // Two bounded failures leave a different file moved on each attempt. A subsequent
        // failure with no moves must keep those corrections; a final retry round-trips them.
        for attempt in 0..2 {
            let entry = if redo { s.redo.last().unwrap() } else { s.undo.last().unwrap() };
            let moves = s.file_moves(&entry.op);
            assert_eq!(moves.len(), 3 - attempt);
            *fs.fail.borrow_mut() = vec![moves[1].0.clone(), moves[0].1.clone()];
            let history = (s.undo.len(), s.redo.len());
            let error = file_history_step(&mut s, &fs, redo).unwrap_err().to_string();
            assert!(error.contains("could not be moved back"), "{error}");
            assert_eq!((s.undo.len(), s.redo.len()), history);
            assert_eq!(s.selection, Selection::single(PhotoId(4)));
            assert_file_catalog_and_journal(&s, &fs, &store);
        }
        let entry = if redo { s.redo.last().unwrap() } else { s.undo.last().unwrap() };
        let moves = s.file_moves(&entry.op);
        assert_eq!(moves.len(), 1);
        *fs.fail.borrow_mut() = vec![moves[0].0.clone()];
        let unchanged = fs.listing();
        let history = (s.undo.len(), s.redo.len());
        assert!(file_history_step(&mut s, &fs, redo).is_err());
        assert_eq!(fs.listing(), unchanged);
        assert_eq!((s.undo.len(), s.redo.len()), history);
        assert_eq!(s.selection, Selection::single(PhotoId(4)));
        assert_file_catalog_and_journal(&s, &fs, &store);
        fs.fail.borrow_mut().clear();
        file_history_step(&mut s, &fs, redo).unwrap();
        assert_eq!(fs.listing(), if redo { renamed.clone() } else { before.clone() });
        assert_file_catalog_and_journal(&s, &fs, &store);
        file_history_step(&mut s, &fs, !redo).unwrap();
        assert_eq!(fs.listing(), if redo { before.clone() } else { renamed.clone() });
        assert_file_catalog_and_journal(&s, &fs, &store);
        file_history_step(&mut s, &fs, redo).unwrap();
        assert_eq!(fs.listing(), if redo { renamed } else { before });
        assert_file_catalog_and_journal(&s, &fs, &store);
    }
}

/// All four sidecar spellings carry distinct bytes so a partial move cannot hide lost work.
fn sidecar_history_fixture(redo: bool) -> (Session, crate::rename::tests::FakeFs, lightcraft_catalog::MemStore) {
    let store = lightcraft_catalog::MemStore::new();
    let mut s = memory_library(&store);
    let fs = crate::rename::tests::FakeFs::new(
        false,
        &[
            ("/p/a.jpg", "Photo"),
            ("/p/a.XMP", "Upper stem edits"),
            ("/p/a.xmp", "Lower stem edits"),
            ("/p/a.jpg.XMP", "Upper full-name edits"),
            ("/p/a.jpg.xmp", "Lower full-name edits"),
            ("/p/other.jpg", "Other"),
        ],
    );
    for (i, path) in ["/p/a.jpg", "/p/other.jpg"].iter().enumerate() {
        let name = std::path::Path::new(path).file_name().unwrap().to_string_lossy();
        let photo = Photo::new(PhotoId(i as u64 + 1), Source::File { path: (*path).into() }, &name, "JPEG", 16, 16, "");
        s.commit("Synthetic File", Op::AddPhoto { photo: Box::new(photo) }).unwrap();
    }
    s.persist().unwrap();
    s.undo.clear();
    select(&mut s, &[1], 1);
    let plans = s.plan_rename_with(&fs, &[PhotoId(1)], "Trip", 1);
    s.execute_fn("photo.rename", |s| Ok(json!({"renamed": s.apply_rename_with(&fs, &plans)?}))).unwrap();
    if redo {
        file_history_step(&mut s, &fs, false).unwrap();
    }
    select(&mut s, &[2], 2);
    (s, fs, store)
}

fn sidecar_history_entry(s: &Session, redo: bool) -> &crate::UndoEntry {
    if redo { s.redo.last().unwrap() } else { s.undo.last().unwrap() }
}

#[test]
fn history_retries_both_sidecar_styles_after_following_a_photo_with_failed_rollback() {
    use crate::rename::MoveFs;
    use crate::sidecar::{SidecarNaming, sidecar_path};

    for redo in [false, true] {
        for naming in [SidecarNaming::Stem, SidecarNaming::Full] {
            let (mut s, fs, store) = sidecar_history_fixture(redo);
            let moves = s.file_moves(&sidecar_history_entry(&s, redo).op);
            assert_eq!(moves.len(), 1);
            let (from, to) = &moves[0];
            let from = from.clone();
            let to = to.clone();
            let before = fs.listing();
            let failing = sidecar_path(&from, naming).with_extension("XMP");
            *fs.fail.borrow_mut() = vec![failing.to_string_lossy().into_owned(), to.clone()];
            let history = (s.undo.len(), s.redo.len());
            let error = file_history_step(&mut s, &fs, redo).unwrap_err().to_string();
            assert!(error.contains("could not be moved back"), "{error}");
            assert_eq!(s.catalog.photo(PhotoId(1)).unwrap().source, Source::File { path: to.clone() });
            assert!(s.file_moves(&sidecar_history_entry(&s, redo).op).is_empty(), "the photo already followed its destination");
            let expected_pending = if naming == SidecarNaming::Stem { 4 } else { 2 };
            assert_eq!(sidecar_history_entry(&s, redo).retry_sidecars.len(), expected_pending);
            assert_file_catalog_and_journal(&s, &fs, &store);

            // Repeating the same XMP failure must not turn the empty photo move list into success.
            *fs.fail.borrow_mut() = vec![failing.to_string_lossy().into_owned()];
            let partial = fs.listing();
            let error = file_history_step(&mut s, &fs, redo).unwrap_err().to_string();
            assert!(error.contains("injected failure"), "{error}");
            assert_eq!(fs.listing(), partial);
            assert_eq!((s.undo.len(), s.redo.len()), history);
            assert_eq!(s.selection, Selection::single(PhotoId(2)));
            assert_eq!(sidecar_history_entry(&s, redo).retry_sidecars.len(), expected_pending);
            assert_file_catalog_and_journal(&s, &fs, &store);

            // Make progress, then fail at the other full-name spelling. Completed sidecars
            // must not be retried (their old names no longer exist).
            let last = sidecar_history_entry(&s, redo).retry_sidecars.last().unwrap().clone();
            *fs.fail.borrow_mut() = vec![last.from.to_string_lossy().into_owned()];
            assert!(file_history_step(&mut s, &fs, redo).is_err());
            assert_eq!(sidecar_history_entry(&s, redo).retry_sidecars.len(), 1);
            assert_eq!((s.undo.len(), s.redo.len()), history);
            assert_eq!(s.selection, Selection::single(PhotoId(2)));
            assert_file_catalog_and_journal(&s, &fs, &store);

            // A new destination collision stays an error and never replaces unrelated bytes.
            fs.fail.borrow_mut().clear();
            fs.files.borrow_mut().push((last.to.to_string_lossy().into_owned(), b"Unrelated edits".to_vec()));
            let occupied = fs.listing();
            assert!(file_history_step(&mut s, &fs, redo).is_err());
            assert_eq!(fs.listing(), occupied);
            assert_eq!((s.undo.len(), s.redo.len()), history);
            assert_eq!(sidecar_history_entry(&s, redo).retry_sidecars.len(), 1);
            assert_file_catalog_and_journal(&s, &fs, &store);
            fs.remove_file(&last.to).unwrap();

            file_history_step(&mut s, &fs, redo).unwrap();
            let mut after = before
                .iter()
                .map(|(path, bytes)| (path.replace(from.trim_end_matches(".jpg"), to.trim_end_matches(".jpg")), bytes.clone()))
                .collect::<Vec<_>>();
            after.sort();
            assert_eq!(fs.listing(), after);
            assert_file_catalog_and_journal(&s, &fs, &store);
            file_history_step(&mut s, &fs, !redo).unwrap();
            assert_eq!(fs.listing(), before);
            assert_file_catalog_and_journal(&s, &fs, &store);
            file_history_step(&mut s, &fs, redo).unwrap();
            assert_eq!(fs.listing(), after);
            assert_file_catalog_and_journal(&s, &fs, &store);
        }
    }
}

#[test]
fn history_repairs_a_sidecar_whose_rollback_failed_before_retrying_the_photo() {
    use crate::sidecar::{SidecarNaming, sidecar_path};

    for redo in [false, true] {
        let (mut s, fs, store) = sidecar_history_fixture(redo);
        let moves = s.file_moves(&sidecar_history_entry(&s, redo).op);
        let (from, to) = &moves[0];
        let from = from.clone();
        let to = to.clone();
        let before = fs.listing();
        // The stem sidecars move first; full-name failure rolls the photo back, but the
        // first stem sidecar cannot return. The catalog has not changed this time.
        let failed_full = sidecar_path(&from, SidecarNaming::Full).with_extension("XMP");
        let stranded = sidecar_path(&to, SidecarNaming::Stem).with_extension("XMP");
        *fs.fail.borrow_mut() = vec![failed_full.to_string_lossy().into_owned(), stranded.to_string_lossy().into_owned()];
        let history = (s.undo.len(), s.redo.len());
        let error = file_history_step(&mut s, &fs, redo).unwrap_err().to_string();
        assert!(error.contains("sidecar") && error.contains("could not be moved back"), "{error}");
        let entry = sidecar_history_entry(&s, redo);
        assert_eq!(entry.retry_sidecars.len(), 1);
        assert!(entry.retry_inverse.is_empty());
        assert_eq!(s.catalog.photo(PhotoId(1)).unwrap().source, Source::File { path: from });
        assert_file_catalog_and_journal(&s, &fs, &store);
        let partial = fs.listing();
        *fs.fail.borrow_mut() = vec![stranded.to_string_lossy().into_owned()];
        assert!(file_history_step(&mut s, &fs, redo).is_err());
        assert_eq!(fs.listing(), partial);
        assert_eq!((s.undo.len(), s.redo.len()), history);
        assert_eq!(s.selection, Selection::single(PhotoId(2)));
        fs.fail.borrow_mut().clear();
        file_history_step(&mut s, &fs, redo).unwrap();
        assert_file_catalog_and_journal(&s, &fs, &store);
        file_history_step(&mut s, &fs, !redo).unwrap();
        assert_eq!(fs.listing(), before);
        assert_file_catalog_and_journal(&s, &fs, &store);
    }
}

#[test]
fn local_history_reveal_prunes_hidden_selection_but_keeps_visible_companions() {
    let mut s = session();
    for (id, path) in [(1, "/synthetic/history/a.jpg"), (2, "/synthetic/history/sub/b.jpg"), (3, "/synthetic/history/c.jpg")] {
        s.catalog
            .apply(Op::Relink { id: PhotoId(id), file_name: format!("{id}.jpg"), source: Source::File { path: path.into() }, format: None })
            .unwrap();
    }
    s.catalog.apply(Op::SetLocal { id: PhotoId(1), local: true }).unwrap();
    s.source = crate::LibrarySource::Folder;
    s.browse = Some(crate::Browse { path: "/synthetic/history".into(), subfolders: true });
    select(&mut s, &[1], 1);
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 1.0})).unwrap();
    select(&mut s, &[1, 2, 3], 2);
    s.execute("library.source", &json!({"kind": "all"})).unwrap();
    assert_eq!(s.active(), Some(PhotoId(2)));
    assert!(s.selection.contains(PhotoId(1)), "All retains the selection when its active photo matches");
    s.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(s.source, crate::LibrarySource::Folder);
    assert!(!s.browse.as_ref().unwrap().subfolders);
    assert_eq!(s.selection, Selection { ids: vec![PhotoId(1), PhotoId(3)], active: Some(PhotoId(1)) });
    assert_eq!(s.targets(&json!({})), [PhotoId(1), PhotoId(3)]);
    s.execute("edit.redo", &json!({})).unwrap();
    assert_eq!(s.targets(&json!({})), [PhotoId(1), PhotoId(3)], "already-active history also preserves only visible targets");
    s.execute("photo.rate", &json!({"rating": 4})).unwrap();
    assert_eq!(s.catalog.photo(PhotoId(1)).unwrap().rating, 4);
    assert_eq!(s.catalog.photo(PhotoId(3)).unwrap().rating, 4);
    assert_eq!(s.catalog.photo(PhotoId(2)).unwrap().rating, 0, "the hidden subfolder photo must stay untouched");
}

#[test]
fn initial_partial_rename_keeps_sidecar_repairs_for_noop_retry_and_history() {
    use crate::rename::tests::FakeFs;
    let store = lightcraft_catalog::MemStore::new();
    let mut s = memory_library(&store);
    let fs = FakeFs::new(false, &[("/p/a.jpg", "Photo"), ("/p/a.xmp", "Stem edits"), ("/p/a.jpg.xmp", "Full edits")]);
    let photo = Photo::new(PhotoId(1), Source::File { path: "/p/a.jpg".into() }, "a.jpg", "JPEG", 16, 16, "");
    s.commit("Synthetic File", Op::AddPhoto { photo: Box::new(photo) }).unwrap();
    s.persist().unwrap();
    s.undo.clear();
    select(&mut s, &[1], 1);
    let before = fs.listing();
    let plans = s.plan_rename_with(&fs, &[PhotoId(1)], "Trip", 1);
    *fs.fail.borrow_mut() = vec!["/p/a.xmp".into(), "/p/Trip.jpg".into()];
    assert!(s.execute_fn("photo.rename", |s| Ok(json!({"renamed": s.apply_rename_with(&fs, &plans)?}))).is_err());
    assert_eq!(s.catalog.photo(PhotoId(1)).unwrap().source, Source::File { path: "/p/Trip.jpg".into() });
    assert_eq!(s.undo.len(), 1);
    assert_eq!(s.undo.last().unwrap().retry_sidecars.len(), 2);
    assert_file_catalog_and_journal(&s, &fs, &store);
    let retry = s.plan_rename_with(&fs, &[PhotoId(1)], "Trip", 1);
    assert_eq!(retry[0].from_path, retry[0].to_path, "the physical photo already has the requested name");
    *fs.fail.borrow_mut() = vec!["/p/a.xmp".into()];
    let partial = fs.listing();
    assert!(s.execute_fn("photo.rename", |s| Ok(json!({"renamed": s.apply_rename_with(&fs, &retry)?}))).is_err());
    assert_eq!(fs.listing(), partial);
    assert_eq!(s.undo.last().unwrap().retry_sidecars.len(), 2, "a failed no-op retry must keep its repair queue");
    fs.fail.borrow_mut().clear();
    assert_eq!(s.execute_fn("photo.rename", |s| Ok(json!({"renamed": s.apply_rename_with(&fs, &retry)?}))).unwrap()["renamed"], 0);
    assert!(s.undo.last().unwrap().retry_sidecars.is_empty());
    assert_eq!(s.undo.len(), 1, "a repaired no-op retry adds no empty rename step");
    let after = fs.listing();
    assert_eq!(
        after,
        vec![("/p/Trip.jpg".into(), "Photo".into()), ("/p/Trip.jpg.xmp".into(), "Full edits".into()), ("/p/Trip.xmp".into(), "Stem edits".into())]
    );
    assert_file_catalog_and_journal(&s, &fs, &store);
    file_history_step(&mut s, &fs, false).unwrap();
    assert_eq!(fs.listing(), before);
    file_history_step(&mut s, &fs, true).unwrap();
    assert_eq!(fs.listing(), after);
    assert_file_catalog_and_journal(&s, &fs, &store);
}

#[test]
fn initial_sidecar_rollback_failure_keeps_a_repair_only_history_step() {
    use crate::rename::tests::FakeFs;
    let store = lightcraft_catalog::MemStore::new();
    let mut s = memory_library(&store);
    let fs = FakeFs::new(false, &[("/p/a.jpg", "Photo"), ("/p/a.XMP", "Stem edits"), ("/p/a.jpg.XMP", "Full edits")]);
    let photo = Photo::new(PhotoId(1), Source::File { path: "/p/a.jpg".into() }, "a.jpg", "JPEG", 16, 16, "");
    s.commit("Synthetic File", Op::AddPhoto { photo: Box::new(photo) }).unwrap();
    s.persist().unwrap();
    s.undo.clear();
    select(&mut s, &[1], 1);
    let before = fs.listing();
    let plans = s.plan_rename_with(&fs, &[PhotoId(1)], "Trip", 1);
    *fs.fail.borrow_mut() = vec!["/p/a.jpg.XMP".into(), "/p/Trip.XMP".into()];
    assert!(s.execute_fn("photo.rename", |s| Ok(json!({"renamed": s.apply_rename_with(&fs, &plans)?}))).is_err());
    assert_eq!(s.catalog.photo(PhotoId(1)).unwrap().source, Source::File { path: "/p/a.jpg".into() });
    assert_eq!(s.undo.len(), 1);
    assert_eq!(s.undo.last().unwrap().retry_sidecars.len(), 1);
    assert_file_catalog_and_journal(&s, &fs, &store);
    let partial = fs.listing();
    *fs.fail.borrow_mut() = vec!["/p/Trip.XMP".into()];
    assert!(file_history_step(&mut s, &fs, false).is_err());
    assert_eq!(fs.listing(), partial);
    assert_eq!(s.undo.last().unwrap().retry_sidecars.len(), 1);
    fs.fail.borrow_mut().clear();
    file_history_step(&mut s, &fs, false).unwrap();
    assert_eq!(fs.listing(), before);
    assert_file_catalog_and_journal(&s, &fs, &store);
    file_history_step(&mut s, &fs, true).unwrap();
    assert_eq!(fs.listing(), before, "redo of the repair-only step must not rename healthy files");
    assert_file_catalog_and_journal(&s, &fs, &store);
}

#[test]
fn discarded_file_history_keeps_sidecar_repairs_without_stale_catalog_inverses() {
    use crate::rename::MoveFs;
    use crate::sidecar::{SidecarNaming, sidecar_path};

    for redo in [false, true] {
        for naming in [SidecarNaming::Stem, SidecarNaming::Full] {
            let (mut s, fs, store) = sidecar_history_fixture(redo);
            let moves = s.file_moves(&sidecar_history_entry(&s, redo).op);
            let (from, to) = moves[0].clone();
            let failing = sidecar_path(&from, naming).with_extension("XMP");
            *fs.fail.borrow_mut() = vec![failing.to_string_lossy().into_owned(), to.clone()];
            assert!(file_history_step(&mut s, &fs, redo).is_err());
            let pending = sidecar_history_entry(&s, redo).retry_sidecars.clone();
            assert!(!sidecar_history_entry(&s, redo).retry_inverse.is_empty());
            // A new edit discards Redo. Enough edits also evict an unresolved Undo entry.
            for i in 0..if redo { 1 } else { 1000 } {
                s.execute("photo.rate", &json!({"ids": [2], "rating": i % 5 + 1})).unwrap();
            }
            assert!(s.redo.is_empty());
            assert_eq!(s.pending_sidecar_repairs, pending);
            assert!(s.undo.iter().all(|e| e.retry_inverse.is_empty() && e.retry_sidecars.is_empty()));
            let partial = fs.listing();
            // Metadata history remains usable without write access to the separated XMP.
            file_history_step(&mut s, &fs, false).unwrap();
            file_history_step(&mut s, &fs, true).unwrap();
            assert_eq!(s.pending_sidecar_repairs, pending);
            assert_eq!(fs.listing(), partial);
            assert_eq!(s.catalog.photo(PhotoId(1)).unwrap().source, Source::File { path: to.clone() });
            assert_file_catalog_and_journal(&s, &fs, &store);

            let plans = s.plan_rename_with(&fs, &[PhotoId(1)], "Next", 1);
            let history = (s.undo.len(), s.redo.len());
            assert!(s.apply_rename_with(&fs, &plans).is_err());
            assert_eq!(fs.listing(), partial);
            assert_eq!((s.undo.len(), s.redo.len()), history);
            assert_eq!(s.pending_sidecar_repairs, pending);
            // Retain only unfinished work after partial progress; never overwrite a collision.
            let last = pending.last().unwrap().clone();
            *fs.fail.borrow_mut() = vec![last.from.to_string_lossy().into_owned()];
            assert!(s.apply_rename_with(&fs, &plans).is_err());
            assert_eq!(s.pending_sidecar_repairs, vec![last.clone()]);
            fs.fail.borrow_mut().clear();
            fs.files.borrow_mut().push((last.to.to_string_lossy().into_owned(), b"Other edits".to_vec()));
            let occupied = fs.listing();
            assert!(s.apply_rename_with(&fs, &plans).is_err());
            assert_eq!(fs.listing(), occupied);
            assert_eq!(s.pending_sidecar_repairs, vec![last.clone()]);
            fs.remove_file(&last.to).unwrap();

            // A no-op Rename still repairs the detached sidecars, without a new undo step.
            let noop = s.plan_rename_with(&fs, &[PhotoId(1)], "{name}", 1);
            assert_eq!(s.apply_rename_with(&fs, &noop).unwrap(), 0);
            assert!(s.pending_sidecar_repairs.is_empty());
            assert_eq!((s.undo.len(), s.redo.len()), history);
            let repaired = fs.listing();
            assert_file_catalog_and_journal(&s, &fs, &store);
            s.execute_fn("photo.rename", |s| Ok(json!({"renamed": s.apply_rename_with(&fs, &plans)?}))).unwrap();
            let renamed = fs.listing();
            file_history_step(&mut s, &fs, false).unwrap();
            assert_eq!(fs.listing(), repaired, "new history restores the current name, not the discarded inverse");
            file_history_step(&mut s, &fs, true).unwrap();
            assert_eq!(fs.listing(), renamed);
            assert_file_catalog_and_journal(&s, &fs, &store);
        }
    }
}

#[test]
fn capture_time_history_reveals_one_photo_but_preserves_multi_photo_context() {
    for params in [json!({"time": "2026-04-02T12:00:00", "each": true}), json!({"hours": 1})] {
        for context in ["album", "filter"] {
            let mut s = session();
            s.catalog.apply(Op::SetCaptured { id: PhotoId(1), captured: Some("2026-04-01T12:00:00".into()) }).unwrap();
            let album = s.execute("album.create", &json!({"name": "Other Photos"})).unwrap()["id"].as_u64().unwrap();
            s.execute("album.addPhotos", &json!({"id": album, "ids": [2]})).unwrap();
            s.undo.clear();
            s.execute("photo.setCaptureTime", &params).unwrap();
            let after = s.catalog.photo(PhotoId(1)).unwrap().captured.clone();
            for (command, expected) in [("edit.undo", Some("2026-04-01T12:00:00".to_string())), ("edit.redo", after)] {
                s.execute("library.source", &json!({"kind": if context == "album" { "album" } else { "all" }, "id": album})).unwrap();
                s.execute("library.filter", &json!({"rating": if context == "filter" { 3 } else { 0 }})).unwrap();
                select(&mut s, &[2], 2);
                assert!(!s.visible().contains(&PhotoId(1)));
                s.execute(command, &json!({})).unwrap();
                assert_eq!(s.catalog.photo(PhotoId(1)).unwrap().captured, expected);
                assert_eq!(s.active(), Some(PhotoId(1)));
                assert!(s.visible().contains(&PhotoId(1)));
            }
        }
    }
    let mut s = session();
    s.execute("photo.setCaptureTime", &json!({"ids": [1, 2], "time": "2026-04-02T12:00:00", "each": true})).unwrap();
    assert_no_navigation(&mut s);
}

#[test]
fn reading_capture_time_from_a_sidecar_reveals_its_single_photo_on_history() {
    let dir = crate::tests_xmp::temp_dir("history-capture-read");
    let path = dir.join("a.png");
    let image = lightcraft_raster::Rgba8 { width: 1, height: 1, data: vec![[40, 80, 120, 255]] };
    let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&image), &lightcraft_codecs::EncodeMeta::default()).unwrap();
    std::fs::write(&path, bytes).unwrap();
    let mut s = session();
    let mut photo = Photo::new(PhotoId(1), Source::File { path: path.to_string_lossy().into_owned() }, "a.png", "PNG", 1, 1, "2026-04-01T12:00:00");
    s.catalog.apply(Op::RemovePhoto { id: PhotoId(1) }).unwrap();
    s.catalog.apply(Op::AddPhoto { photo: Box::new(photo.clone()) }).unwrap();
    photo.captured = Some("2026-04-02T12:00:00".into());
    std::fs::write(dir.join("a.xmp"), crate::sidecar::sidecar_packet(&photo, &s.catalog)).unwrap();
    let album = s.execute("album.create", &json!({"name": "Other Photos"})).unwrap()["id"].as_u64().unwrap();
    s.execute("album.addPhotos", &json!({"id": album, "ids": [2]})).unwrap();
    s.undo.clear();
    let result = s.execute("photo.readMetadataFromFile", &json!({"ids": [1]})).unwrap();
    assert_eq!(result["read"].as_array().unwrap().len(), 1);
    assert_eq!(s.catalog.photo(PhotoId(1)).unwrap().captured, photo.captured);
    for command in ["edit.undo", "edit.redo"] {
        s.execute("library.source", &json!({"kind": "album", "id": album})).unwrap();
        select(&mut s, &[2], 2);
        assert!(!s.visible().contains(&PhotoId(1)));
        s.execute(command, &json!({})).unwrap();
        assert_eq!(s.active(), Some(PhotoId(1)));
        assert!(s.visible().contains(&PhotoId(1)));
        assert_eq!(s.catalog.photo(PhotoId(1)).unwrap().captured, if command == "edit.undo" { None } else { photo.captured.clone() });
    }
    std::fs::remove_dir_all(dir).unwrap();
}
