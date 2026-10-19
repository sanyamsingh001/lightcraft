//! Existing regressions for atomic source/filter selection navigation.

use crate::Session;
use serde_json::json;

#[test]
fn source_navigation_keeps_final_selection_and_records_only_the_origin() {
    use lightcraft_catalog::{Op, Photo, PhotoId, Source};
    use lightcraft_develop::DevelopSettings;
    use std::sync::Arc;

    let mut s = Session::new();
    for id in [2, 3, 5, 7] {
        let mut p = Photo::new(
            PhotoId(id),
            Source::File { path: format!("/synthetic-navigation/{id}.jpg") },
            &format!("{id}.jpg"),
            "JPEG",
            4,
            3,
            "2026-01-01",
        );
        p.captured = Some(if id == 3 { "2026-04-20T12:00:00" } else { "2026-04-12T12:00:00" }.into());
        p.rating = 5;
        p.local = id == 5;
        let mut settings = DevelopSettings::default();
        settings.light.exposure = match id {
            3 => 1.25,
            7 => 2.5,
            _ => 0.0,
        };
        p.develop = Arc::new(settings);
        s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    s.source = crate::LibrarySource::Folder;
    s.browse = Some(crate::Browse { path: "/synthetic-navigation".into(), subfolders: false });
    s.filter.rating = 5;
    s.execute("library.select", &json!({"ids": [3, 2, 5], "active": 3})).unwrap();
    s.execute("develop.beginInteraction", &json!({"label": "Exposure"})).unwrap();
    s.execute("develop.set", &json!({"control": "light.exposure", "value": 1.5})).unwrap();
    let undo_before = s.undo.len();
    let journal_before = s.journal.len();
    let params = json!({"kind": "all", "filter": {"date": "2026-04-12"}, "keepSelection": true});
    let result = s.execute("library.source", &params).unwrap();
    assert_eq!(result["count"], 2);
    assert_eq!(s.visible(), [PhotoId(7), PhotoId(2)], "the first result differs from the retained selected photo");
    assert_eq!(s.selection.ids, [PhotoId(2)], "hidden Local and filtered photos are pruned");
    assert_eq!(s.active(), Some(PhotoId(2)));
    assert_eq!(s.previous_active, Some(PhotoId(3)), "navigation records the actual origin");
    assert!(s.interaction.is_none(), "navigation finishes edits on the old photo");
    assert_eq!(s.undo.len(), undo_before + 1, "the old interaction is committed once");
    let origin = s.catalog.photo(PhotoId(3)).unwrap();
    assert_eq!(origin.versions.len(), 1);
    assert!(origin.versions[0].auto);
    assert_eq!(origin.versions[0].settings.light.exposure, 1.5, "version captures the finished edit");
    let unvisited = s.catalog.photo(PhotoId(7)).unwrap();
    assert!(unvisited.is_edited(), "the intermediate candidate has edits that would otherwise trigger a version");
    assert!(unvisited.versions.is_empty(), "an unvisited first result must not receive an automatic version");
    assert_eq!(s.journal.len(), journal_before + 1, "one registered navigation command covers the whole transition");
    assert_eq!(s.journal.last().unwrap(), &("library.source".to_string(), params));
    let pasted = s.execute("develop.pastePrevious", &json!({"groups": ["light"]})).unwrap();
    assert_eq!(pasted["from"], 3);
    assert_eq!(pasted["changed"], 1);
    assert_eq!(s.develop_of(PhotoId(2)).unwrap().light.exposure, 1.5);
    assert_eq!(s.develop_of(PhotoId(7)).unwrap().light.exposure, 2.5);
    assert!(s.catalog.photo(PhotoId(7)).unwrap().versions.is_empty());
}

#[test]
fn failed_source_filter_preserves_pending_interaction_and_history() {
    use lightcraft_catalog::{Op, Photo, PhotoId, Source};

    let history_snapshot =
        |entries: &[crate::UndoEntry]| entries.iter().map(|entry| (entry.label.clone(), entry.op.clone(), entry.folder.clone())).collect::<Vec<_>>();
    for source_params in [json!({"kind": "all"}), json!({"kind": "libraryFolder", "path": "/synthetic-navigation"})] {
        for keep_selection in [false, true] {
            for patch in [json!({"date": "2026-05-13", "rating": "five"}), json!({"ratingOp": "invalid"})] {
                let mut s = Session::new();
                for id in [1, 2] {
                    let mut photo = Photo::new(
                        PhotoId(id),
                        Source::File { path: format!("/synthetic-navigation/{id}.jpg") },
                        &format!("{id}.jpg"),
                        "JPEG",
                        4,
                        3,
                        "2026-01-01",
                    );
                    photo.rating = 5;
                    photo.captured = Some("2026-04-12T12:00:00".into());
                    s.catalog.apply(Op::AddPhoto { photo: Box::new(photo) }).unwrap();
                }
                s.source = crate::LibrarySource::Folder;
                s.browse = Some(crate::Browse { path: "/synthetic-navigation".into(), subfolders: false });
                s.library_folder = Some("/synthetic-original-folder".into());
                s.filter.library_folder = Some("/synthetic-navigation".into());
                s.filter.rating = 5;
                s.filter.date = Some("2026-04-12".into());
                s.execute("library.select", &json!({"ids": [1, 2], "active": 1})).unwrap();
                s.previous_active = Some(PhotoId(2));
                s.execute("develop.set", &json!({"control": "light.exposure", "value": 0.5})).unwrap();
                s.execute("develop.set", &json!({"control": "light.contrast", "value": 25.0})).unwrap();
                s.execute("edit.undo", &json!({})).unwrap();
                assert!(!s.undo.is_empty() && !s.redo.is_empty(), "both history branches must exist before the pending edit");
                let original = s.develop_of(PhotoId(1)).unwrap();
                s.execute("develop.beginInteraction", &json!({"label": "Exposure"})).unwrap();
                s.execute("develop.set", &json!({"control": "light.exposure", "value": 1.25})).unwrap();

                // follow_folder leaves this Local constraint; validation must not clear it.
                s.filter.folder = Some("/synthetic-navigation".into());
                s.filter.subfolders = true;

                let source = s.source;
                let filter = s.filter.clone();
                let browse = s.browse.clone();
                let library_folder = s.library_folder.clone();
                let selection = s.selection.clone();
                let previous_active = s.previous_active;
                let visible = s.visible_cloned();
                let photos = s.catalog.photos().cloned().collect::<Vec<_>>();
                let revision = s.catalog.revision;
                let undo = history_snapshot(&s.undo);
                let redo = history_snapshot(&s.redo);
                let journal = s.journal.clone();
                let pending_log = s.pending_log.clone();
                let interaction = s.interaction.clone().unwrap();
                assert_eq!(s.develop_of(PhotoId(1)).unwrap().light.exposure, 1.25);

                let mut params = source_params.clone();
                params["filter"] = patch;
                params["keepSelection"] = json!(keep_selection);
                let error = s.execute("library.source", &params).unwrap_err();
                assert!(matches!(error, crate::EngineError::BadParams { ref cmd, .. } if cmd == "library.source"));
                assert_eq!(s.source, source);
                assert_eq!(s.filter, filter, "a valid field in a malformed patch must not be partially applied");
                assert_eq!(s.browse, browse);
                assert_eq!(s.library_folder, library_folder, "failed navigation must not replace the library folder");
                assert_eq!(s.selection, selection);
                assert_eq!(s.previous_active, previous_active);
                assert_eq!(s.visible(), visible);
                assert_eq!(s.catalog.photos().cloned().collect::<Vec<_>>(), photos, "settings, history and versions remain unchanged");
                assert_eq!(s.catalog.revision, revision, "failed validation must not mutate the catalog");
                let pending = s.interaction.as_ref().expect("failed navigation keeps the drag pending");
                assert_eq!(pending.label, interaction.label);
                assert_eq!(pending.photo, interaction.photo);
                assert!(std::sync::Arc::ptr_eq(&pending.original, &interaction.original));
                assert_eq!(history_snapshot(&s.undo), undo);
                assert_eq!(history_snapshot(&s.redo), redo);
                assert_eq!(s.journal, journal);
                assert_eq!(s.pending_log, pending_log);

                s.execute("develop.cancelInteraction", &json!({})).unwrap();
                assert!(s.interaction.is_none());
                assert_eq!(s.develop_of(PhotoId(1)).unwrap(), original, "cancel still restores the complete pre-drag settings");
                assert_eq!(s.source, source);
                assert_eq!(s.filter, filter);
                assert_eq!(s.browse, browse);
                assert_eq!(s.library_folder, library_folder, "failed navigation must not replace the library folder");
                assert_eq!(s.selection, selection);
                assert_eq!(history_snapshot(&s.undo), undo);
                assert_eq!(history_snapshot(&s.redo), redo);
                s.execute("edit.redo", &json!({})).unwrap();
                let restored = s.develop_of(PhotoId(1)).unwrap();
                assert_eq!(restored.light.exposure, 0.5);
                assert_eq!(restored.light.contrast, 25.0, "the existing redo operation remains usable after cancellation");
            }
        }
    }
}

#[test]
fn global_source_clears_local_scope_after_folder_follow() {
    use lightcraft_catalog::{Op, Photo, PhotoId, Source};
    let mut s = Session::new();
    for (id, path, local) in [(1, "/synthetic-navigation/old/1.jpg", true), (2, "/synthetic-navigation/elsewhere/2.jpg", false)] {
        let mut p = Photo::new(PhotoId(id), Source::File { path: path.into() }, &format!("{id}.jpg"), "JPEG", 4, 3, "2026-04-12T00:00:00");
        p.local = local;
        p.rating = 5;
        p.captured = Some("2026-04-12T00:00:00".into());
        s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    s.source = crate::LibrarySource::Folder;
    s.browse = Some(crate::Browse { path: "/synthetic-navigation/old".into(), subfolders: true });
    s.filter.rating = 5;
    s.filter.subfolders = true;
    crate::cmd::browse::follow_folder(&mut s, "/synthetic-navigation/old", "/synthetic-navigation/renamed");
    assert_eq!(s.filter.folder.as_deref().map(std::path::Path::new), Some(std::path::Path::new("/synthetic-navigation/renamed")));
    s.execute("library.source", &json!({"kind": "all", "filter": {"date": "2026-04-12"}, "keepSelection": true})).unwrap();
    assert!(s.filter.folder.is_none());
    assert!(!s.filter.subfolders);
    assert_eq!(s.filter.rating, 5);
    assert_eq!(s.visible(), [PhotoId(2)], "global matches include the other folder and exclude Local-only records");
    assert_eq!(s.selection, crate::Selection::single(PhotoId(2)));
}

#[test]
fn ordinary_album_switch_prunes_hidden_bulk_rating_targets() {
    use lightcraft_catalog::{Op, Photo, PhotoId, Source};
    let mut s = Session::new();
    for id in [1, 2, 3] {
        let mut photo = Photo::new(PhotoId(id), Source::Demo { scene: 1 }, &format!("{id}.jpg"), "JPEG", 4, 3, "2026-04-12T00:00:00");
        photo.rating = 3;
        s.catalog.apply(Op::AddPhoto { photo: Box::new(photo) }).unwrap();
    }
    s.execute("library.select", &json!({"ids": [1, 3], "active": 1})).unwrap();
    let album = s.execute("album.create", &json!({"name": "Visible", "addSelected": true})).unwrap()["id"].as_u64().unwrap();
    s.execute("library.select", &json!({"ids": [1, 2], "active": 1})).unwrap();
    s.execute("library.filter", &json!({"rating": 3, "ratingOp": "exactly"})).unwrap();
    let filter = s.filter.clone();
    // Ordinary sidebar album clicks omit keepSelection.
    s.execute("library.source", &json!({"kind": "album", "id": album})).unwrap();
    assert_eq!(s.selection, crate::Selection::single(PhotoId(1)));
    assert_eq!(s.filter, filter, "source navigation preserves independent filters");
    assert_eq!(s.visible().len(), 2);
    assert_eq!(s.execute("photo.rate", &json!({"rating": 4})).unwrap()["changed"], 1);
    assert_eq!(s.catalog.photo(PhotoId(1)).unwrap().rating, 4);
    assert_eq!(s.catalog.photo(PhotoId(2)).unwrap().rating, 3, "the hidden former selection is not rated");
    assert_eq!(s.catalog.photo(PhotoId(3)).unwrap().rating, 3, "visible unselected photos are not rated");
}

#[test]
fn every_explicit_source_prunes_hidden_ids_with_a_visible_active_photo() {
    use lightcraft_catalog::{Flag, Op, Photo, PhotoId, Source};
    for kind in ["all", "recentlyAdded", "recentlyDeleted", "picks", "libraryFolder"] {
        for keep in [None, Some(false), Some(true)] {
            let mut s = Session::new();
            for id in [1, 2] {
                let mut photo = Photo::new(
                    PhotoId(id),
                    Source::File { path: format!("/synthetic-source/{id}.jpg") },
                    &format!("{id}.jpg"),
                    "JPEG",
                    4,
                    3,
                    "2026-04-12T00:00:00",
                );
                photo.rating = if id == 1 { 3 } else { 1 };
                photo.flag = Flag::Pick;
                photo.deleted = kind == "recentlyDeleted";
                s.catalog.apply(Op::AddPhoto { photo: Box::new(photo) }).unwrap();
            }
            s.selection = crate::Selection { ids: vec![PhotoId(1), PhotoId(2)], active: Some(PhotoId(1)) };
            s.execute("library.filter", &json!({"rating": 3})).unwrap();
            assert_eq!(s.selection.ids, [PhotoId(1), PhotoId(2)], "standalone filters retain selection");
            let filter = s.filter.clone();
            let mut params = json!({"kind": kind, "path": "/synthetic-source"});
            if let Some(keep) = keep {
                params["keepSelection"] = json!(keep);
            }
            s.execute("library.source", &params).unwrap();
            assert_eq!(s.selection, crate::Selection::single(PhotoId(1)), "{params}");
            assert_eq!(s.filter, filter, "{params}");
            assert_eq!(s.targets(&json!({})), [PhotoId(1)], "{params}");
        }
    }
}

#[test]
fn source_selection_fallbacks_and_empty_results_are_preserved() {
    use lightcraft_catalog::{Op, Photo, PhotoId, Source};
    for keep in [false, true] {
        let mut s = Session::new();
        for id in [1, 2, 3] {
            let mut photo = Photo::new(PhotoId(id), Source::Demo { scene: 1 }, &format!("{id}.jpg"), "JPEG", 4, 3, "2026-04-12T00:00:00");
            photo.rating = if id == 1 { 1 } else { 3 };
            s.catalog.apply(Op::AddPhoto { photo: Box::new(photo) }).unwrap();
        }
        s.selection = crate::Selection { ids: vec![PhotoId(1), PhotoId(2)], active: Some(PhotoId(1)) };
        s.execute("library.filter", &json!({"rating": 3})).unwrap();
        let first = s.visible()[0];
        s.execute("library.source", &json!({"kind": "all", "keepSelection": keep})).unwrap();
        assert_eq!(s.selection, crate::Selection::single(if keep { PhotoId(2) } else { first }));
        s.execute("library.source", &json!({"kind": "all", "filter": {"rating": 5}, "keepSelection": keep})).unwrap();
        assert!(s.selection.ids.is_empty());
        assert!(s.active().is_none());
        assert!(s.targets(&json!({})).is_empty());
    }
}
