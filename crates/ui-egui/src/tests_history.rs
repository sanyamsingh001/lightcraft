//! History navigation uses the shared dispatch path and preserves the current UI layout.

use lightcraft_catalog::{Op, Photo, PhotoId, Source};
use lightcraft_engine::{Selection, Session};
use serde_json::json;

use crate::panels::detail::Gesture;
use crate::state::{RightPanel, ViewMode, Zoom};
use crate::{HoverPreview, LightcraftApp, Services};

fn app() -> LightcraftApp {
    let mut s = Session::new();
    for id in 1..=3 {
        let name = format!("synthetic-{id}.jpg");
        let photo = Photo::new(PhotoId(id), Source::File { path: name.clone() }, &name, "JPEG", 16, 16, "");
        s.catalog.apply(Op::AddPhoto { photo: Box::new(photo) }).unwrap();
    }
    s.selection = Selection::single(PhotoId(1));
    LightcraftApp::new(s, Services::default())
}

fn transient_tools(app: &mut LightcraftApp) {
    app.gesture = Some(Gesture::SpotMove { spot: 0, source: false });
    app.hover_preview = Some(HoverPreview { label: "Synthetic Preview".into(), settings: Default::default() });
    app.ui.detail_due = Some((100.0, 7));
    app.ui.renaming_mask = Some((7, "Synthetic Mask".into()));
}

#[test]
fn undo_redo_dispatch_follows_the_photo_and_clears_tools_without_changing_layout() {
    let mut app = app();
    app.ui.view = ViewMode::Detail;
    app.ui.right = RightPanel::Masking;
    app.ui.zoom = Zoom::Percent(100);
    app.ui.pan = (0.3, 0.7);
    app.run("develop.set", json!({"control": "light.exposure", "value": 1.0})).unwrap();
    for (command, exposure) in [("edit.undo", 0.0), ("edit.redo", 1.0)] {
        app.run("library.select", json!({"ids": [2]})).unwrap();
        transient_tools(&mut app);
        app.run(command, json!({})).unwrap();
        assert_eq!(app.session.active(), Some(PhotoId(1)));
        assert_eq!(app.session.develop_of(PhotoId(1)).unwrap().light.exposure, exposure);
        assert!(app.gesture.is_none());
        assert!(app.hover_preview.is_none());
        assert!(app.ui.detail_due.is_none());
        assert!(app.ui.renaming_mask.is_none());
        assert_eq!((app.ui.view, app.ui.right, app.ui.zoom, app.ui.pan), (ViewMode::Detail, RightPanel::Masking, Zoom::Percent(100), (0.3, 0.7)));
    }
}

#[test]
fn global_history_dispatch_preserves_the_photo_and_transient_tools() {
    let mut app = app();
    app.run("album.create", json!({"name": "Synthetic Album", "addSelected": true})).unwrap();
    app.run("library.select", json!({"ids": [2]})).unwrap();
    transient_tools(&mut app);
    for command in ["edit.undo", "edit.redo"] {
        app.run(command, json!({})).unwrap();
        assert_eq!(app.session.active(), Some(PhotoId(2)));
        assert!(app.gesture.is_some());
        assert!(app.hover_preview.is_some());
        assert_eq!(app.ui.detail_due, Some((100.0, 7)));
        assert_eq!(app.ui.renaming_mask.as_ref().map(|r| r.0), Some(7));
    }
}

#[test]
fn failed_history_dispatch_keeps_the_viewed_photo_and_tools() {
    let mut app = app();
    app.run("photo.rate", json!({"rating": 5})).unwrap();
    app.run("library.select", json!({"ids": [2]})).unwrap();
    app.session.catalog.apply(Op::RemovePhoto { id: PhotoId(1) }).unwrap();
    transient_tools(&mut app);
    assert!(app.run("edit.undo", json!({})).is_err());
    assert_eq!(app.session.active(), Some(PhotoId(2)));
    assert!(app.gesture.is_some());
    assert!(app.hover_preview.is_some());
    assert_eq!(app.ui.detail_due, Some((100.0, 7)));
    assert_eq!(app.ui.renaming_mask.as_ref().map(|r| r.0), Some(7));
}

#[test]
fn history_navigation_reveals_cells_in_both_grids_and_the_detail_filmstrip() {
    for view in [ViewMode::PhotoGrid, ViewMode::SquareGrid, ViewMode::Detail] {
        for context in ["album", "folder", "filter"] {
            let mut app = app();
            app.ui.view = view;
            app.ui.left_panel = false;
            app.ui.filmstrip = true;
            let album = app.run("album.create", json!({"name": "Other Photos"})).unwrap()["id"].as_u64().unwrap();
            app.run("album.addPhotos", json!({"id": album, "ids": [2]})).unwrap();
            app.session.undo.clear();
            app.run("develop.set", json!({"control": "light.exposure", "value": 1.0})).unwrap();
            let mut h = crate::headless::Headless::new(app, [1200.0, 800.0], 1.0);
            for (command, exposure) in [("edit.undo", 0.0), ("edit.redo", 1.0)] {
                h.app.run("library.select", json!({"ids": [2]})).unwrap();
                match context {
                    "album" => {
                        h.app.run("library.source", json!({"kind": "album", "id": album})).unwrap();
                    }
                    "folder" => {
                        h.app.session.source = lightcraft_engine::LibrarySource::Folder;
                        h.app.session.browse = Some(lightcraft_engine::Browse { path: "unrelated-folder".into(), subfolders: false });
                    }
                    _ => {}
                }
                h.app.run("library.filter", json!({"only": [2], "text": "synthetic-2"})).unwrap();
                for _ in 0..4 {
                    h.step();
                }
                let cell = if view == ViewMode::Detail { "film:1" } else { "thumb:1" };
                assert!(!h.app.widgets.iter().any(|(id, _)| id == cell), "target should start hidden: {view:?}, {context}");
                h.app.run(command, json!({})).unwrap();
                for _ in 0..4 {
                    h.step();
                }
                assert_eq!(h.app.session.active(), Some(PhotoId(1)));
                assert!(h.app.session.visible().contains(&PhotoId(1)));
                assert_eq!(h.app.session.source, lightcraft_engine::LibrarySource::All);
                assert_eq!(h.app.session.filter, lightcraft_catalog::Filter::default());
                assert_eq!(h.app.ui.view, view);
                assert_eq!(h.app.session.develop_of(PhotoId(1)).unwrap().light.exposure, exposure);
                let rect = h.app.widgets.iter().find(|(id, _)| id == cell).map(|(_, rect)| *rect).expect("affected photo cell drawn");
                assert!(rect.intersects(h.view.ctx.content_rect()), "{command}: {view:?}, {context}, {rect:?}");
            }
        }
    }
}

#[test]
fn history_navigation_keeps_matching_album_and_filter_in_grid_and_detail() {
    for view in [ViewMode::PhotoGrid, ViewMode::Detail] {
        let mut app = app();
        app.ui.view = view;
        let album = app.run("album.create", json!({"name": "Edited Photos", "addSelected": true})).unwrap()["id"].as_u64().unwrap();
        app.session.undo.clear();
        app.run("develop.set", json!({"control": "light.exposure", "value": 1.0})).unwrap();
        app.run("library.source", json!({"kind": "album", "id": album})).unwrap();
        app.run("library.filter", json!({"text": "synthetic-1"})).unwrap();
        let filter = app.session.filter.clone();
        for command in ["edit.undo", "edit.redo"] {
            app.run("library.select", json!({"ids": [2]})).unwrap();
            app.run(command, json!({})).unwrap();
            assert_eq!(app.session.active(), Some(PhotoId(1)));
            assert!(app.session.visible().contains(&PhotoId(1)));
            assert_eq!(app.session.source, lightcraft_engine::LibrarySource::Album(lightcraft_catalog::AlbumId(album)));
            assert_eq!(app.session.filter, filter);
            assert_eq!(app.ui.view, view);
        }
    }
}

#[test]
fn redo_ends_the_drag_gesture_even_when_committing_it_invalidates_the_redo_branch() {
    for changed in [false, true] {
        let mut app = app();
        app.run("photo.rate", json!({"rating": 5})).unwrap();
        app.run("edit.undo", json!({})).unwrap();
        app.run("library.select", json!({"ids": [2]})).unwrap();
        app.run("develop.beginInteraction", json!({"label": "Targeted Adjustment"})).unwrap();
        if changed {
            app.run("develop.set", json!({"control": "light.exposure", "value": 0.75})).unwrap();
        }
        app.gesture = Some(Gesture::Targeted { at: lightcraft_geom::Point { x: 0.5, y: 0.5 }, acc: 0.0 });
        let result = app.run("edit.redo", json!({}));
        assert!(app.session.interaction.is_none());
        assert!(app.gesture.is_none(), "the pointer release must not leave an orphaned gesture");
        if changed {
            assert!(result.unwrap()["redone"].is_null());
            assert_eq!(app.session.active(), Some(PhotoId(2)));
            assert_eq!(app.session.develop_of(PhotoId(2)).unwrap().light.exposure, 0.75);
            assert_eq!(app.session.catalog.photo(PhotoId(1)).unwrap().rating, 0);
        } else {
            result.unwrap();
            assert_eq!(app.session.active(), Some(PhotoId(1)));
            assert_eq!(app.session.catalog.photo(PhotoId(1)).unwrap().rating, 5);
        }
        let history = (app.session.undo.len(), app.session.redo.len());
        app.run("develop.endInteraction", json!({})).unwrap();
        assert_eq!((app.session.undo.len(), app.session.redo.len()), history, "a later pointer release must not commit again");
    }
}
