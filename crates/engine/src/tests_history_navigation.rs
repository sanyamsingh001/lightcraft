//! Single-photo reveal, source invalidation and Redo interaction history.

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
    assert_eq!(s.selection.ids, [PhotoId(2), PhotoId(3)], "All prunes the Local-only companion");
    // Source navigation prunes Local-only photos. Seed the hidden selection for history reveal.
    select(&mut s, &[1, 2, 3], 2);
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
