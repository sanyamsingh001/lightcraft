//! Actual input interrupted by commands/shortcuts: pending mask input cannot move to a new target.

use lightcraft_catalog::{Op, Photo, PhotoId, Source};
use lightcraft_develop::{DevelopSettings, MaskShape};
use serde_json::json;

use super::{T, click_widget, exec, library_view, pointer};
use crate::headless::Headless;
use crate::panels::detail::Gesture;
use crate::{LightcraftApp, Services};

const A: PhotoId = PhotoId(1);
const B: PhotoId = PhotoId(2);

fn two_photos() -> Headless {
    let mut session = lightcraft_engine::Session::new();
    let ops = [A, B]
        .into_iter()
        .map(|id| Op::AddPhoto {
            photo: Box::new(Photo::new(id, Source::Demo { scene: 0 }, &format!("{}.jpg", id.0), "JPEG", 640, 480, "2026-01-01T00:00:00")),
        })
        .collect();
    session.commit("Synthetic photos", Op::Batch { ops }).unwrap();
    session.execute("library.select", &json!({"ids": [A.0], "active": A.0})).unwrap();
    let album = session.execute("album.create", &json!({"name": "Two mask targets"})).unwrap();
    session.execute("album.addPhotos", &json!({"id": album["id"], "ids": [A.0, B.0]})).unwrap();
    session.execute("library.source", &json!({"kind": "album", "id": album["id"]})).unwrap();
    session.execute("library.filter", &json!({"kind": "image"})).unwrap();
    let mut app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    app.ui.view = crate::state::ViewMode::Detail;
    app.ui.right = crate::state::RightPanel::Masking;
    app.ui.left_panel = false;
    let mut h = Headless::new(app, [1200.0, 1400.0], 1.0);
    for _ in 0..3 {
        h.step();
    }
    assert_eq!(h.app.session.visible_cloned().len(), 2);
    h
}

fn library_context(h: &Headless) -> serde_json::Value {
    json!({"source": h.app.session.source, "filter": h.app.session.filter})
}

fn settings(h: &Headless, photo: PhotoId) -> DevelopSettings {
    (*h.app.session.develop_of(photo).unwrap()).clone()
}

fn select(h: &mut Headless, photo: PhotoId) {
    exec(h, "library.select", json!({"ids": [photo.0], "active": photo.0}));
    assert_eq!(h.app.session.active(), Some(photo));
}

fn brush_strokes(d: &DevelopSettings, mask: usize) -> usize {
    let MaskShape::Brush { strokes } = &d.masks[mask].components[0].shape else { panic!("brush") };
    strokes.len()
}

fn samples(d: &DevelopSettings, component: usize) -> Vec<[f64; 3]> {
    let MaskShape::ColorRange { samples, .. } = &d.masks[0].components[component].shape else { panic!("colour range") };
    samples.clone()
}

fn add_radial(h: &mut Headless) -> u32 {
    exec(h, "mask.add", json!({"kind": "radial", "center": [0.5, 0.5], "rx": 0.13, "ry": 0.09, "angle": 0.0, "feather": 37.0}));
    h.app.session.active_mask.unwrap()
}

fn radius(d: &DevelopSettings, mask: usize) -> f64 {
    let MaskShape::Radial { rx, .. } = &d.masks[mask].components[0].shape else { panic!("radial") };
    *rx
}

fn begin_handle_drag(h: &mut Headless, mask: u32) {
    let widget = format!("maskHandle:{mask}:0:1");
    let start = h.app.widgets.iter().find(|(id, _)| id == &widget).unwrap().1.center();
    let img = h.app.image_rect.unwrap();
    let x = (start.x - img.left()) / img.width();
    let y = (start.y - img.top()) / img.height();
    let to_x = x + 36.0 / img.width();
    pointer(h, json!([{"kind": "down", "x": x, "y": y}, {"kind": "drag", "x": to_x, "y": y}]));
    assert!(matches!(h.app.gesture, Some(Gesture::MaskHandle { .. })));
    assert!(h.app.session.interaction.is_some());
}

fn finish_pointer(h: &mut Headless) {
    pointer(h, json!([{"kind": "drag", "x": 0.8, "y": 0.5}, {"kind": "up", "x": 0.8, "y": 0.5}]));
    assert!(h.app.gesture.is_none());
    assert!(h.app.session.interaction.is_none());
}

fn drag_handle(h: &mut Headless, mask: u32) {
    let r = h.request("ui.dragWidget", json!({"id": format!("maskHandle:{mask}:0:1"), "dx": 24.0, "steps": 4}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(h.app.gesture.is_none());
    assert!(h.app.session.interaction.is_none());
}

#[test]
fn brush_input_cannot_follow_photo_selection_before_release() {
    for dragged in [false, true] {
        let mut h = two_photos();
        click_widget(&mut h, "maskNew:brush");
        let a = settings(&h, A);
        let b = settings(&h, B);
        let source = h.app.session.source;
        let filter = h.app.session.filter.clone();
        let undo_before = h.app.session.undo.len();
        let mut events = vec![json!({"kind": "down", "x": 0.25, "y": 0.35})];
        if dragged {
            events.push(json!({"kind": "drag", "x": 0.4, "y": 0.35}));
        }
        pointer(&mut h, json!(events));
        if dragged {
            assert!(matches!(h.app.gesture, Some(Gesture::Brush { photo: A, mask: Some(1), .. })));
        } else {
            assert!(h.app.gesture.is_none(), "a stationary press has not committed or begun dragging");
        }
        assert_eq!(settings(&h, A), a, "unfinished strokes are drafts");
        select(&mut h, B);
        assert!(h.app.gesture.is_none());
        finish_pointer(&mut h);
        assert_eq!(settings(&h, A), a);
        assert_eq!(settings(&h, B), b, "the new photo cannot inherit the old stroke or its release");
        assert_eq!(h.app.session.undo.len(), undo_before);
        assert_eq!((&h.app.session.source, &h.app.session.filter), (&source, &filter));
        assert_eq!(h.app.session.selection.ids, [B]);
        // A fresh pointer press still paints on B after the interrupted pointer is released.
        pointer(&mut h, json!([{"kind": "down", "x": 0.65, "y": 0.55}, {"kind": "up", "x": 0.65, "y": 0.55}]));
        assert_eq!(brush_strokes(&settings(&h, B), 0), 1);
        assert_eq!(settings(&h, A), a);
        assert_eq!(h.app.session.undo.len(), undo_before + 1);
    }
}

#[test]
fn brush_input_cannot_follow_mask_selection_before_release() {
    for dragged in [false, true] {
        let mut h = two_photos();
        click_widget(&mut h, "maskNew:brush");
        click_widget(&mut h, "maskNew:brush");
        exec(&mut h, "mask.select", json!({"id": 1}));
        let before = settings(&h, A);
        let view = library_view(&h);
        let undo_before = h.app.session.undo.len();
        let mut events = vec![json!({"kind": "down", "x": 0.25, "y": 0.35})];
        if dragged {
            events.push(json!({"kind": "drag", "x": 0.4, "y": 0.35}));
        }
        pointer(&mut h, json!(events));
        if dragged {
            assert!(matches!(h.app.gesture, Some(Gesture::Brush { mask: Some(1), .. })));
        } else {
            assert!(h.app.gesture.is_none(), "a stationary press has not committed or begun dragging");
        }
        exec(&mut h, "mask.select", json!({"id": 2}));
        assert!(h.app.gesture.is_none());
        finish_pointer(&mut h);
        assert_eq!(settings(&h, A), before);
        assert_eq!(h.app.session.undo.len(), undo_before);
        assert_eq!(h.app.session.active_mask, Some(2));
        assert_eq!(library_view(&h), view);
        pointer(&mut h, json!([{"kind": "down", "x": 0.65, "y": 0.55}, {"kind": "up", "x": 0.65, "y": 0.55}]));
        let after = settings(&h, A);
        assert_eq!(after.masks[0], before.masks[0]);
        assert_eq!(brush_strokes(&after, 1), 1);
        assert_eq!(h.app.session.undo.len(), undo_before + 1);
        assert_eq!(library_view(&h), view);
    }
}

#[test]
fn handle_input_cannot_follow_photo_selection_even_with_equal_mask_ids() {
    let mut h = two_photos();
    let context = library_context(&h);
    let mask_a = add_radial(&mut h);
    select(&mut h, B);
    let mask_b = add_radial(&mut h);
    assert_eq!(mask_a, mask_b, "mask ids are local to each photo");
    let b = settings(&h, B);
    select(&mut h, A);
    exec(&mut h, "mask.select", json!({"id": mask_a}));
    let original = settings(&h, A);
    let undo_before = h.app.session.undo.len();
    begin_handle_drag(&mut h, mask_a);
    let a = settings(&h, A);
    assert!(radius(&a, 0) > radius(&original, 0));
    assert_eq!(h.app.session.undo.len(), undo_before, "live handle updates wait for interaction completion");
    select(&mut h, B);
    assert!(h.app.gesture.is_none());
    // Even explicitly selecting B's same-numbered mask cannot make the old handle valid again.
    exec(&mut h, "mask.select", json!({"id": mask_b}));
    finish_pointer(&mut h);
    assert_eq!(settings(&h, B), b);
    assert_eq!(settings(&h, A), a, "A retains only the handle movement before switching");
    assert_eq!(h.app.session.undo.len(), undo_before + 1);
    drag_handle(&mut h, mask_b);
    assert!(radius(&settings(&h, B), 0) > radius(&b, 0));
    assert_eq!(settings(&h, A), a);
    assert_eq!(h.app.session.undo.len(), undo_before + 2);
    assert_eq!(h.app.session.selection.ids, [B]);
    assert_eq!(library_context(&h), context);
}

#[test]
fn handle_input_cannot_follow_another_mask_on_the_same_photo() {
    let mut h = two_photos();
    let first = add_radial(&mut h);
    let second = add_radial(&mut h);
    exec(&mut h, "mask.select", json!({"id": first}));
    let view = library_view(&h);
    let original = settings(&h, A);
    let undo_before = h.app.session.undo.len();
    begin_handle_drag(&mut h, first);
    let changed = settings(&h, A);
    assert!(radius(&changed, 0) > radius(&original, 0));
    exec(&mut h, "mask.select", json!({"id": second}));
    finish_pointer(&mut h);
    assert_eq!(settings(&h, A), changed);
    assert_eq!(changed.masks[1], original.masks[1], "the new mask stays untouched");
    assert_eq!(h.app.session.undo.len(), undo_before + 1);
    assert_eq!(library_view(&h), view);
    drag_handle(&mut h, second);
    assert_eq!(settings(&h, A).masks[0], changed.masks[0]);
    assert!(radius(&settings(&h, A), 1) > radius(&original, 1));
    assert_eq!(h.app.session.undo.len(), undo_before + 2);
}

#[test]
fn brush_shortcut_finishes_radial_interaction_before_a_fresh_stroke() {
    let mut h = two_photos();
    let mask = add_radial(&mut h);
    let original = settings(&h, A);
    let view = library_view(&h);
    let undo_before = h.app.session.undo.len();
    begin_handle_drag(&mut h, mask);
    let resized = settings(&h, A);
    assert!(radius(&resized, 0) > radius(&original, 0));
    assert_eq!(h.app.session.undo.len(), undo_before);
    let r = h.request("ui.key", json!({"key": "B"}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(h.app.ui.tool, "brush");
    assert!(h.app.gesture.is_none());
    assert!(h.app.session.interaction.is_none(), "the old radial interaction finishes when Brush takes over");
    assert_eq!(h.app.session.undo.len(), undo_before + 1);
    finish_pointer(&mut h);
    assert_eq!(settings(&h, A), resized, "the held pointer cannot turn into a brush stroke");
    assert_eq!(library_view(&h), view);
    pointer(&mut h, json!([{"kind": "down", "x": 0.65, "y": 0.55}, {"kind": "up", "x": 0.65, "y": 0.55}]));
    let painted = settings(&h, A);
    assert_eq!(painted.masks[0].components[0], resized.masks[0].components[0]);
    let MaskShape::Brush { strokes } = &painted.masks[0].components[1].shape else { panic!("brush") };
    assert_eq!(strokes.len(), 1);
    assert_eq!(h.app.session.undo.len(), undo_before + 2, "resizing and fresh painting remain separate undo steps");
    assert!(h.app.gesture.is_none() && h.app.session.interaction.is_none());
    exec(&mut h, "edit.undo", json!({}));
    assert_eq!(settings(&h, A), resized);
    exec(&mut h, "edit.undo", json!({}));
    assert_eq!(settings(&h, A), original);
    assert_eq!(library_view(&h), view);
}

#[test]
fn handle_press_before_drag_threshold_cannot_follow_a_photo_or_mask_switch() {
    for another_photo in [false, true] {
        let mut h = two_photos();
        let first = add_radial(&mut h);
        if another_photo {
            select(&mut h, B);
        }
        let second = add_radial(&mut h);
        if another_photo {
            assert_eq!(first, second);
            select(&mut h, A);
        }
        exec(&mut h, "mask.select", json!({"id": first}));
        let a = settings(&h, A);
        let b = settings(&h, B);
        let undo_before = h.app.session.undo.len();
        let widget = format!("maskHandle:{first}:0:1");
        let at = h.app.widgets.iter().find(|(id, _)| id == &widget).unwrap().1.center();
        let img = h.app.image_rect.unwrap();
        let x = (at.x - img.left()) / img.width();
        let y = (at.y - img.top()) / img.height();
        pointer(&mut h, json!([{"kind": "down", "x": x, "y": y}]));
        assert!(h.app.gesture.is_none());
        assert!(h.app.session.interaction.is_none());
        if another_photo {
            select(&mut h, B);
        }
        exec(&mut h, "mask.select", json!({"id": second}));
        finish_pointer(&mut h);
        assert_eq!(settings(&h, A), a);
        assert_eq!(settings(&h, B), b);
        assert_eq!(h.app.session.undo.len(), undo_before, "an old press cannot begin editing the newly selected equal-looking handle");
    }
}

fn key(h: &mut Headless, name: &str) {
    let r = h.request("ui.key", json!({"key": name}), T);
    assert_eq!(r["ok"], true, "{name}: {r}");
}

#[test]
fn unfinished_brush_strokes_are_aborted_on_keyboard_tool_and_panel_exits() {
    for exit in ["Enter", "Escape", "tool.none", "panel.info"] {
        let mut h = two_photos();
        click_widget(&mut h, "maskNew:brush");
        let a = settings(&h, A);
        let b = settings(&h, B);
        let view = library_view(&h);
        let undo_before = h.app.session.undo.len();
        pointer(&mut h, json!([{"kind": "down", "x": 0.25, "y": 0.35}, {"kind": "drag", "x": 0.4, "y": 0.35}]));
        assert!(matches!(h.app.gesture, Some(Gesture::Brush { .. })));
        assert_eq!(settings(&h, A), a, "the unfinished stroke is only a draft");
        if matches!(exit, "Enter" | "Escape") {
            key(&mut h, exit);
        } else {
            exec(&mut h, exit, json!({}));
        }
        assert!(h.app.gesture.is_none(), "{exit}: leaving Brush retires its points");
        finish_pointer(&mut h);
        assert_eq!(settings(&h, A), a);
        assert_eq!(settings(&h, B), b);
        assert_eq!(h.app.session.undo.len(), undo_before);
        key(&mut h, "B");
        pointer(&mut h, json!([{"kind": "down", "x": 0.65, "y": 0.55}, {"kind": "up", "x": 0.65, "y": 0.55}]));
        let painted = settings(&h, A);
        let MaskShape::Brush { strokes } = &painted.masks[0].components[0].shape else { panic!("brush") };
        assert_eq!(strokes.len(), 1);
        assert_eq!(strokes[0].points.len(), 1, "{exit}: a fresh click cannot bridge to abandoned points");
        assert!((strokes[0].points[0].x - 0.65).abs() < 1e-5);
        assert!((strokes[0].points[0].y - 0.55).abs() < 1e-5);
        assert_eq!(h.app.session.undo.len(), undo_before + 1);
        assert_eq!(settings(&h, B), b);
        assert_eq!(library_view(&h), view);
        exec(&mut h, "edit.undo", json!({}));
        assert_eq!(settings(&h, A), a, "the only undo step is the fresh stroke");
    }
}

#[test]
fn brush_shortcut_before_drag_threshold_requires_a_fresh_press() {
    for move_after_shortcut in [false, true] {
        let mut h = two_photos();
        let mask = add_radial(&mut h);
        let a = settings(&h, A);
        let b = settings(&h, B);
        let view = library_view(&h);
        let undo_before = h.app.session.undo.len();
        let widget = format!("maskHandle:{mask}:0:1");
        let at = h.app.widgets.iter().find(|(id, _)| id == &widget).unwrap().1.center();
        let img = h.app.image_rect.unwrap();
        let x = (at.x - img.left()) / img.width();
        let y = (at.y - img.top()) / img.height();
        pointer(&mut h, json!([{"kind": "down", "x": x, "y": y}]));
        assert!(h.app.gesture.is_none() && h.app.session.interaction.is_none());
        key(&mut h, "B");
        assert_eq!(h.app.ui.tool, "brush");
        if move_after_shortcut {
            finish_pointer(&mut h);
        } else {
            pointer(&mut h, json!([{"kind": "up", "x": x, "y": y}]));
        }
        assert!(h.app.gesture.is_none() && h.app.session.interaction.is_none());
        assert_eq!(settings(&h, A), a, "the held handle press must not add a brush component");
        assert_eq!(settings(&h, B), b);
        assert_eq!(h.app.session.undo.len(), undo_before);
        pointer(&mut h, json!([{"kind": "down", "x": 0.65, "y": 0.55}, {"kind": "up", "x": 0.65, "y": 0.55}]));
        let painted = settings(&h, A);
        assert_eq!(painted.masks[0].components[0], a.masks[0].components[0], "painting preserves the radial geometry");
        let MaskShape::Brush { strokes } = &painted.masks[0].components[1].shape else { panic!("brush") };
        assert_eq!(strokes.len(), 1);
        assert_eq!(strokes[0].points.len(), 1);
        assert_eq!(h.app.session.undo.len(), undo_before + 1);
        assert_eq!(library_view(&h), view);
        exec(&mut h, "edit.undo", json!({}));
        assert_eq!(settings(&h, A), a);
    }
}

#[test]
fn held_handle_cannot_overwrite_a_deleted_replaced_or_undone_component() {
    for change in ["delete", "replace", "undo"] {
        let mut h = two_photos();
        let mask = add_radial(&mut h);
        exec(
            &mut h,
            "mask.addComponent",
            json!({
                "kind": "radial", "center": [0.35, 0.4], "rx": 0.1, "ry": 0.06, "angle": 15.0, "feather": 83.0
            }),
        );
        let original = settings(&h, A);
        let b = settings(&h, B);
        let view = library_view(&h);
        begin_handle_drag(&mut h, mask);
        match change {
            "delete" => {
                exec(&mut h, "mask.component", json!({"component": 0, "action": "delete"}));
            }
            "replace" => {
                exec(
                    &mut h,
                    "mask.update",
                    json!({"component": 0, "shape": {
                        "kind": "colorRange", "samples": [[20.0, 3.0, 4.0], [70.0, 5.0, 6.0]], "refine": 73.5
                    }}),
                );
            }
            _ => {
                exec(&mut h, "edit.undo", json!({}));
            }
        }
        let edited = settings(&h, A);
        assert_eq!(h.app.session.active(), Some(A));
        assert_eq!(h.app.session.active_mask, Some(mask), "{change}: target ids alone still match");
        match change {
            "delete" => assert_eq!(edited.masks[0].components[0], original.masks[0].components[1]),
            "replace" => assert_eq!(samples(&edited, 0), [[20.0, 3.0, 4.0], [70.0, 5.0, 6.0]]),
            _ => assert_eq!(edited, original),
        }
        assert!(h.app.gesture.is_none() && h.app.session.interaction.is_none(), "{change}: retire the stale handle");
        let undo_after_edit = h.app.session.undo.len();
        finish_pointer(&mut h);
        assert_eq!(settings(&h, A), edited, "{change}: the stale drag cannot overwrite edited settings");
        assert_eq!(settings(&h, B), b);
        assert_eq!(h.app.session.undo.len(), undo_after_edit);
        assert_eq!(library_view(&h), view);
        if change != "replace" {
            drag_handle(&mut h, mask);
            assert!(radius(&settings(&h, A), 0) > radius(&edited, 0), "a fresh press can resize the surviving target");
            exec(&mut h, "edit.undo", json!({}));
            assert_eq!(settings(&h, A), edited);
        }
    }
}

#[test]
fn component_index_reuse_before_drag_threshold_requires_a_fresh_press() {
    let mut h = two_photos();
    let mask = add_radial(&mut h);
    // An overlapping component reuses the exact handle coordinates and index after deletion.
    exec(
        &mut h,
        "mask.addComponent",
        json!({
            "kind": "radial", "center": [0.5, 0.5], "rx": 0.13, "ry": 0.09, "angle": 0.0, "feather": 83.0
        }),
    );
    let original = settings(&h, A);
    let b = settings(&h, B);
    let view = library_view(&h);
    let widget = format!("maskHandle:{mask}:0:1");
    let at = h.app.widgets.iter().find(|(id, _)| id == &widget).unwrap().1.center();
    let img = h.app.image_rect.unwrap();
    let x = (at.x - img.left()) / img.width();
    let y = (at.y - img.top()) / img.height();
    pointer(&mut h, json!([{"kind": "down", "x": x, "y": y}]));
    assert!(h.app.gesture.is_none() && h.app.session.interaction.is_none());
    exec(&mut h, "mask.component", json!({"component": 0, "action": "delete"}));
    let edited = settings(&h, A);
    assert_eq!(edited.masks[0].components[0], original.masks[0].components[1]);
    assert_eq!(h.app.session.active_mask, Some(mask));
    let undo_after_edit = h.app.session.undo.len();
    finish_pointer(&mut h);
    assert_eq!(settings(&h, A), edited, "the held press cannot resize the reused component slot");
    assert_eq!(settings(&h, B), b);
    assert_eq!(h.app.session.undo.len(), undo_after_edit);
    assert_eq!(library_view(&h), view);
    drag_handle(&mut h, mask);
    assert!(radius(&settings(&h, A), 0) > radius(&edited, 0));
    exec(&mut h, "edit.undo", json!({}));
    assert_eq!(settings(&h, A), edited);
}

#[test]
fn undo_removing_a_brush_mask_cannot_resurrect_it_from_held_input() {
    for dragged in [false, true] {
        let mut h = two_photos();
        click_widget(&mut h, "maskNew:brush");
        let mask = h.app.session.active_mask;
        let b = settings(&h, B);
        let view = library_view(&h);
        let mut events = vec![json!({"kind": "down", "x": 0.25, "y": 0.35})];
        if dragged {
            events.push(json!({"kind": "drag", "x": 0.4, "y": 0.35}));
        }
        pointer(&mut h, json!(events));
        exec(&mut h, "edit.undo", json!({}));
        let undone = settings(&h, A);
        assert!(undone.masks.is_empty());
        assert_eq!(h.app.session.active_mask, mask, "undo leaves the same selected id after removing its mask");
        assert!(h.app.gesture.is_none());
        let undo_after_edit = h.app.session.undo.len();
        finish_pointer(&mut h);
        assert_eq!(settings(&h, A), undone, "releasing an interrupted stroke cannot create a replacement mask");
        assert_eq!(settings(&h, B), b);
        assert_eq!(h.app.session.undo.len(), undo_after_edit);
        pointer(&mut h, json!([{"kind": "down", "x": 0.65, "y": 0.55}, {"kind": "up", "x": 0.65, "y": 0.55}]));
        let painted = settings(&h, A);
        assert_eq!(painted.masks.len(), 1);
        let MaskShape::Brush { strokes } = &painted.masks[0].components[0].shape else { panic!("brush") };
        assert_eq!(strokes.len(), 1);
        assert_eq!(strokes[0].points.len(), 1, "the fresh stroke never inherits abandoned points");
        assert_eq!(h.app.session.undo.len(), undo_after_edit + 1);
        assert_eq!(library_view(&h), view);
        exec(&mut h, "edit.undo", json!({}));
        assert_eq!(settings(&h, A), undone);
    }
}

#[test]
fn unrelated_adjustments_and_other_mask_edits_keep_the_owned_drag_live() {
    let mut h = two_photos();
    let first = add_radial(&mut h);
    let second = add_radial(&mut h);
    exec(&mut h, "mask.select", json!({"id": first}));
    let original = settings(&h, A);
    let b = settings(&h, B);
    let view = library_view(&h);
    let undo_before = h.app.session.undo.len();
    begin_handle_drag(&mut h, first);
    exec(&mut h, "mask.adjust", json!({"id": first, "values": {"exposure": 0.5}}));
    exec(
        &mut h,
        "mask.update",
        json!({"id": second, "component": 0, "shape": {
            "kind": "radial", "center": [0.35, 0.4], "rx": 0.1, "ry": 0.06, "angle": 15.0, "feather": 83.0
        }}),
    );
    let edited = settings(&h, A);
    assert!(matches!(h.app.gesture, Some(Gesture::MaskHandle { mask, .. }) if mask == first));
    assert!(h.app.session.interaction.is_some());
    finish_pointer(&mut h);
    let moved = settings(&h, A);
    assert!(radius(&moved, 0) > radius(&edited, 0), "continued dragging still updates its own component");
    assert_eq!(moved.masks[0].adjust.exposure, 0.5);
    assert_eq!(moved.masks[1], edited.masks[1]);
    assert_eq!(settings(&h, B), b);
    assert_eq!(h.app.session.undo.len(), undo_before + 1, "live updates keep one interaction undo step");
    assert_eq!(library_view(&h), view);
    exec(&mut h, "edit.undo", json!({}));
    assert_eq!(settings(&h, A), original);
}

#[test]
fn unselected_overlapping_pin_deletion_before_drag_cannot_retarget_held_input() {
    for continue_dragging in [false, true] {
        let mut h = two_photos();
        let selected = add_radial(&mut h);
        exec(
            &mut h,
            "mask.add",
            json!({
                "kind": "radial", "center": [0.7, 0.65], "rx": 0.1, "ry": 0.07, "angle": 0.0, "feather": 31.0
            }),
        );
        let unselected = h.app.session.active_mask.unwrap();
        exec(
            &mut h,
            "mask.addComponent",
            json!({
                "kind": "radial", "center": [0.7, 0.65], "rx": 0.18, "ry": 0.11, "angle": 27.0, "feather": 83.0
            }),
        );
        exec(&mut h, "mask.select", json!({"id": selected}));
        let before = settings(&h, A);
        let other_photo = settings(&h, B);
        let view = library_view(&h);
        let first_pin = format!("maskPin:{unselected}:0");
        let replacement_pin = format!("maskPin:{unselected}:1");
        let at = h.app.widgets.iter().find(|(id, _)| id == &first_pin).unwrap().1.center();
        let overlapping = h.app.widgets.iter().find(|(id, _)| id == &replacement_pin).unwrap().1.center();
        assert_eq!(at, overlapping, "both unselected components have exactly overlapping pins");
        let img = h.app.image_rect.unwrap();
        let x = (at.x - img.left()) / img.width();
        let y = (at.y - img.top()) / img.height();
        let to_x = x + 0.06;
        let to_y = y + 0.04;
        pointer(&mut h, json!([{"kind": "down", "x": x, "y": y}]));
        assert!(h.app.gesture.is_none() && h.app.session.interaction.is_none());
        assert_eq!(h.app.session.active_mask, Some(selected), "pressing the pin has not selected its mask yet");
        exec(&mut h, "mask.component", json!({"id": unselected, "component": 0, "action": "delete"}));
        let edited = settings(&h, A);
        assert_eq!(edited.masks[0], before.masks[0]);
        assert_eq!(edited.masks[1].components, before.masks[1].components[1..]);
        assert_eq!(h.app.session.active_mask, Some(selected), "deleting an unselected component leaves the selected context unchanged");
        let undo_after_delete = h.app.session.undo.len();
        if continue_dragging {
            pointer(&mut h, json!([{"kind": "drag", "x": to_x, "y": to_y}, {"kind": "up", "x": to_x, "y": to_y}]));
        } else {
            pointer(&mut h, json!([{"kind": "up", "x": x, "y": y}]));
        }
        assert!(h.app.gesture.is_none() && h.app.session.interaction.is_none());
        assert_eq!(settings(&h, A), edited, "the replacement component survives the stale press through release");
        assert_eq!(settings(&h, B), other_photo);
        assert_eq!(h.app.session.active_mask, Some(selected), "the old press cannot select the replacement's mask");
        assert_eq!(h.app.session.undo.len(), undo_after_delete);
        assert_eq!(library_view(&h), view);
        // A new press on the surviving pin deliberately selects and moves its component.
        pointer(
            &mut h,
            json!([
                {"kind": "down", "x": x, "y": y},
                {"kind": "drag", "x": to_x, "y": to_y},
                {"kind": "up", "x": to_x, "y": to_y}
            ]),
        );
        let moved = settings(&h, A);
        assert_eq!(h.app.session.active_mask, Some(unselected));
        assert_eq!(moved.masks[0], edited.masks[0]);
        assert_eq!(moved.masks[1].components.len(), 1);
        let MaskShape::Radial { center, rx, ry, angle, feather, .. } = &moved.masks[1].components[0].shape else { panic!("radial") };
        assert!((center.x - 0.76).abs() < 1e-5 && (center.y - 0.69).abs() < 1e-5, "only the fresh press moves the surviving pin");
        assert_eq!((*rx, *ry, *angle, *feather), (0.18, 0.11, 27.0, 83.0));
        assert_eq!(settings(&h, B), other_photo);
        assert_eq!(h.app.session.undo.len(), undo_after_delete + 1);
        assert_eq!(library_view(&h), view);
        exec(&mut h, "edit.undo", json!({}));
        assert_eq!(settings(&h, A), edited, "fresh pin movement remains one undo step after deletion");
    }
}

#[test]
fn undo_restoring_an_overlapping_pin_keeps_the_pressed_click_owner() {
    let mut h = two_photos();
    let selected = add_radial(&mut h);
    exec(
        &mut h,
        "mask.add",
        json!({
            "kind": "radial", "center": [0.7, 0.65], "rx": 0.1, "ry": 0.07, "angle": 0.0, "feather": 31.0
        }),
    );
    let earlier = h.app.session.active_mask.unwrap();
    exec(
        &mut h,
        "mask.add",
        json!({
            "kind": "radial", "center": [0.7, 0.65], "rx": 0.18, "ry": 0.11, "angle": 27.0, "feather": 83.0
        }),
    );
    let pressed = h.app.session.active_mask.unwrap();
    let overlapping = settings(&h, A);
    let other_photo = settings(&h, B);
    let undo_before_move = h.app.session.undo.len();
    exec(
        &mut h,
        "mask.update",
        json!({"id": earlier, "shape": {
            "kind": "radial", "center": [0.3, 0.3], "rx": 0.1, "ry": 0.07, "angle": 0.0, "feather": 31.0
        }}),
    );
    exec(&mut h, "mask.select", json!({"id": selected}));
    let view = library_view(&h);
    let pin = format!("maskPin:{pressed}:0");
    let at = h.app.widgets.iter().find(|(id, _)| id == &pin).unwrap().1.center();
    let img = h.app.image_rect.unwrap();
    let x = (at.x - img.left()) / img.width();
    let y = (at.y - img.top()) / img.height();
    pointer(&mut h, json!([{"kind": "down", "x": x, "y": y}]));
    assert!(h.app.gesture.is_none() && h.app.session.interaction.is_none());
    assert_eq!(h.app.session.active_mask, Some(selected));
    let r = h.request("ui.key", json!({"key": "Z", "cmd": true}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(settings(&h, A), overlapping);
    assert_eq!(h.app.session.active_mask, Some(selected), "unrelated undo keeps the selected mask and pressed component intact");
    assert_eq!(h.app.session.undo.len(), undo_before_move);
    let restored = h.app.widgets.iter().find(|(id, _)| id == &format!("maskPin:{earlier}:0")).unwrap().1.center();
    assert_eq!(restored, at, "undo restores the earlier mask's pin exactly over the pressed pin");
    pointer(&mut h, json!([{"kind": "up", "x": x, "y": y}]));
    assert_eq!(h.app.session.active_mask, Some(pressed), "release selects the pin captured on press, not the newly overlapping pin");
    assert_eq!(settings(&h, A), overlapping);
    assert_eq!(settings(&h, B), other_photo);
    assert_eq!(h.app.session.undo.len(), undo_before_move, "pin selection adds no undo step");
    assert_eq!(library_view(&h), view);
    assert!(h.app.gesture.is_none() && h.app.session.interaction.is_none());
    // A fresh press still prefers the selected mask among overlapping pins.
    pointer(&mut h, json!([{"kind": "down", "x": x, "y": y}, {"kind": "up", "x": x, "y": y}]));
    assert_eq!(h.app.session.active_mask, Some(pressed));
    // With another mask selected, a fresh press keeps the existing tie order for unselected pins.
    exec(&mut h, "mask.select", json!({"id": selected}));
    pointer(&mut h, json!([{"kind": "down", "x": x, "y": y}, {"kind": "up", "x": x, "y": y}]));
    assert_eq!(h.app.session.active_mask, Some(earlier));
    assert_eq!(settings(&h, A), overlapping);
    assert_eq!(h.app.session.undo.len(), undo_before_move);
    // Fresh dragging moves only its newly chosen component and remains one undo step.
    pointer(
        &mut h,
        json!([
            {"kind": "down", "x": x, "y": y},
            {"kind": "drag", "x": x + 0.06, "y": y + 0.04},
            {"kind": "up", "x": x + 0.06, "y": y + 0.04}
        ]),
    );
    let moved = settings(&h, A);
    assert_eq!(h.app.session.active_mask, Some(earlier));
    assert_eq!(moved.masks[0], overlapping.masks[0]);
    assert_eq!(moved.masks[2], overlapping.masks[2]);
    let MaskShape::Radial { center, rx, ry, angle, feather, .. } = &moved.masks[1].components[0].shape else { panic!("radial") };
    assert!((center.x - 0.76).abs() < 1e-5 && (center.y - 0.69).abs() < 1e-5);
    assert_eq!((*rx, *ry, *angle, *feather), (0.1, 0.07, 0.0, 31.0));
    assert_eq!(settings(&h, B), other_photo);
    assert_eq!(library_view(&h), view);
    assert!(h.app.gesture.is_none() && h.app.session.interaction.is_none());
    assert_eq!(h.app.session.undo.len(), undo_before_move + 1);
    exec(&mut h, "edit.undo", json!({}));
    assert_eq!(settings(&h, A), overlapping);
}

fn color_components(count: usize) -> Headless {
    let mut h = two_photos();
    for component in 0..count {
        let command = if component == 0 { "mask.add" } else { "mask.addComponent" };
        exec(
            &mut h,
            command,
            json!({
                "kind": "colorRange", "refine": 30.0 + component as f64,
                "samples": [[40.0 + component as f64, 2.0, 3.0], [60.0 + component as f64, 4.0, 5.0]]
            }),
        );
    }
    h
}

#[test]
fn selecting_a_radial_mask_disarms_color_picking_and_restores_handles() {
    let mut h = two_photos();
    let radial = add_radial(&mut h);
    click_widget(&mut h, "maskNew:colorRange");
    pointer(&mut h, json!([{"kind": "down", "x": 0.25, "y": 0.3}, {"kind": "up", "x": 0.25, "y": 0.3}]));
    let before = settings(&h, A);
    let view = library_view(&h);
    let undo_before = h.app.session.undo.len();
    assert!(h.app.mask_color_picker.is_some());
    click_widget(&mut h, &format!("mask:{radial}"));
    assert!(h.app.ui.tool.is_empty());
    assert!(h.app.mask_color_picker.is_none());
    drag_handle(&mut h, radial);
    let after = settings(&h, A);
    assert!(radius(&after, 0) > radius(&before, 0));
    assert_eq!(after.masks[1], before.masks[1], "handle input never samples the unselected colour range");
    assert_eq!(h.app.session.undo.len(), undo_before + 1);
    assert_eq!(library_view(&h), view);
}

#[test]
fn pick_buttons_replace_and_shift_add_to_their_own_color_component() {
    let mut h = two_photos();
    click_widget(&mut h, "maskNew:colorRange");
    pointer(&mut h, json!([{"kind": "down", "x": 0.25, "y": 0.3}, {"kind": "up", "x": 0.25, "y": 0.3}]));
    let first = samples(&settings(&h, A), 0);
    assert_eq!(first.len(), 1);
    exec(&mut h, "mask.addComponent", json!({"kind": "colorRange", "op": "add"}));
    let view = library_view(&h);
    let undo_before = h.app.session.undo.len();
    // Switching directly from Pick 0 to Pick 1 must arm component 1, not disarm all sampling.
    click_widget(&mut h, "button:colorPick1");
    assert_eq!(h.app.mask_color_picker.as_ref().unwrap().component, 1);
    pointer(&mut h, json!([{"kind": "down", "x": 0.7, "y": 0.6}, {"kind": "up", "x": 0.7, "y": 0.6}]));
    let second = samples(&settings(&h, A), 1);
    assert_eq!(second.len(), 1);
    assert_eq!(samples(&settings(&h, A), 0), first);
    let r = h.request("ui.pointer", json!({"events": [{"kind": "down", "x": 0.3, "y": 0.7}, {"kind": "up", "x": 0.3, "y": 0.7}], "shift": true}), T);
    assert_eq!(r["ok"], true, "{r}");
    let added = samples(&settings(&h, A), 1);
    assert_eq!(added.len(), 2);
    assert_eq!(added[0], second[0]);
    assert_eq!(samples(&settings(&h, A), 0), first);
    pointer(&mut h, json!([{"kind": "down", "x": 0.1, "y": 0.2}, {"kind": "up", "x": 0.1, "y": 0.2}]));
    let replaced = samples(&settings(&h, A), 1);
    assert_eq!(replaced.len(), 1, "plain clicks replace only the armed component");
    assert_eq!(samples(&settings(&h, A), 0), first);
    click_widget(&mut h, "button:colorPick0");
    assert_eq!(h.app.mask_color_picker.as_ref().unwrap().component, 0);
    let r =
        h.request("ui.pointer", json!({"events": [{"kind": "down", "x": 0.35, "y": 0.65}, {"kind": "up", "x": 0.35, "y": 0.65}], "shift": true}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(samples(&settings(&h, A), 0).len(), 2);
    assert_eq!(samples(&settings(&h, A), 1), replaced);
    assert_eq!(h.app.session.undo.len(), undo_before + 4);
    assert_eq!(library_view(&h), view);
    assert!(h.app.gesture.is_none() && h.app.session.interaction.is_none());
}

#[test]
fn pending_color_pick_cannot_follow_photo_selection_on_release() {
    let mut h = two_photos();
    let context = library_context(&h);
    let first = exec(&mut h, "mask.add", json!({"kind": "colorRange"}))["activeMask"].clone();
    select(&mut h, B);
    exec(&mut h, "mask.add", json!({"kind": "colorRange"}));
    let b = settings(&h, B);
    select(&mut h, A);
    exec(&mut h, "mask.select", json!({"id": first}));
    click_widget(&mut h, "button:colorPick0");
    let a = settings(&h, A);
    let undo_before = h.app.session.undo.len();
    pointer(&mut h, json!([{"kind": "down", "x": 0.25, "y": 0.3}]));
    select(&mut h, B);
    exec(&mut h, "mask.select", json!({"id": 1}));
    assert!(h.app.mask_color_picker.is_none());
    assert!(h.app.ui.tool.is_empty());
    pointer(&mut h, json!([{"kind": "up", "x": 0.25, "y": 0.3}]));
    assert_eq!(settings(&h, A), a);
    assert_eq!(settings(&h, B), b);
    assert_eq!(h.app.session.undo.len(), undo_before);
    click_widget(&mut h, "button:colorPick0");
    pointer(&mut h, json!([{"kind": "down", "x": 0.7, "y": 0.6}, {"kind": "up", "x": 0.7, "y": 0.6}]));
    assert_eq!(samples(&settings(&h, B), 0).len(), 1);
    assert_eq!(settings(&h, A), a);
    assert_eq!(h.app.session.selection.ids, [B]);
    assert_eq!(library_context(&h), context);
}

#[test]
fn picker_component_deletion_and_index_reuse_disarm_before_sampling() {
    for (armed, action) in [(0, "delete"), (1, "delete"), (1, "duplicate")] {
        let mut h = color_components(3);
        click_widget(&mut h, &format!("button:colorPick{armed}"));
        let view = library_view(&h);
        let b = settings(&h, B);
        let photo = h.app.session.active();
        let mask = h.app.session.active_mask;
        exec(&mut h, "mask.component", json!({"component": 0, "action": action}));
        let edited = settings(&h, A);
        // The old slot remains a Color Range on the same photo and mask. Id/kind checks alone
        // therefore cannot distinguish it from the component that was actually armed.
        assert_eq!(h.app.session.active(), photo);
        assert_eq!(h.app.session.active_mask, mask);
        assert!(matches!(edited.masks[0].components[armed].shape, MaskShape::ColorRange { .. }));
        assert!(h.app.mask_color_picker.is_none(), "{armed}/{action}: structural edits retire the armed target");
        assert!(h.app.ui.tool.is_empty());
        let undo_after_edit = h.app.session.undo.len();
        pointer(&mut h, json!([{"kind": "down", "x": 0.25, "y": 0.3}, {"kind": "up", "x": 0.25, "y": 0.3}]));
        assert_eq!(settings(&h, A), edited, "an old Pick cannot replace samples in a reused index");
        assert_eq!(settings(&h, B), b);
        assert_eq!(h.app.session.undo.len(), undo_after_edit);
        assert_eq!(library_view(&h), view);
        // A newly armed component remains editable after cancellation.
        exec(&mut h, "view.zoomFit", json!({}));
        click_widget(&mut h, "button:colorPick0");
        pointer(&mut h, json!([{"kind": "down", "x": 0.65, "y": 0.55}, {"kind": "up", "x": 0.65, "y": 0.55}]));
        let sampled = settings(&h, A);
        assert_eq!(samples(&sampled, 0).len(), 1);
        assert_eq!(sampled.masks[0].components[1..], edited.masks[0].components[1..], "fresh sampling only edits the chosen component");
        assert_eq!(h.app.session.undo.len(), undo_after_edit + 1);
        exec(&mut h, "edit.undo", json!({}));
        assert_eq!(settings(&h, A), edited);
        assert_eq!(library_view(&h), view);
    }
}

#[test]
fn editing_the_armed_component_cancels_its_snapshot_without_changing_ids() {
    let mut h = color_components(2);
    click_widget(&mut h, "button:colorPick0");
    let view = library_view(&h);
    let b = settings(&h, B);
    let mask = h.app.session.active_mask;
    exec(
        &mut h,
        "mask.update",
        json!({"component": 0, "shape": {
            "kind": "colorRange", "samples": [[20.0, 3.0, 4.0], [70.0, 5.0, 6.0]], "refine": 73.5
        }}),
    );
    let edited = settings(&h, A);
    assert_eq!(h.app.session.active_mask, mask);
    assert_eq!(edited.masks[0].components.len(), 2);
    assert_eq!(samples(&edited, 0), [[20.0, 3.0, 4.0], [70.0, 5.0, 6.0]]);
    assert!(h.app.mask_color_picker.is_none(), "an in-place edit invalidates the armed component snapshot");
    assert!(h.app.ui.tool.is_empty());
    let undo_after_edit = h.app.session.undo.len();
    pointer(&mut h, json!([{"kind": "down", "x": 0.25, "y": 0.3}, {"kind": "up", "x": 0.25, "y": 0.3}]));
    assert_eq!(settings(&h, A), edited, "the edited samples and Refine value must survive the stale pick");
    assert_eq!(settings(&h, B), b);
    assert_eq!(h.app.session.undo.len(), undo_after_edit);
    assert_eq!(library_view(&h), view);
}

#[test]
fn successful_sampling_refreshes_the_picker_snapshot_and_unrelated_edits_keep_it_armed() {
    let mut h = color_components(2);
    click_widget(&mut h, "button:colorPick0");
    let view = library_view(&h);
    let before = settings(&h, A);
    let undo_before = h.app.session.undo.len();
    pointer(&mut h, json!([{"kind": "down", "x": 0.25, "y": 0.3}, {"kind": "up", "x": 0.25, "y": 0.3}]));
    let first = samples(&settings(&h, A), 0);
    assert_eq!(first.len(), 1);
    assert!(h.app.mask_color_picker.is_some(), "the picker's own successful edit refreshes its snapshot");
    exec(&mut h, "mask.adjust", json!({"values": {"exposure": 0.5}}));
    assert!(h.app.mask_color_picker.is_some(), "an unrelated adjustment does not replace the component");
    let r = h.request(
        "ui.pointer",
        json!({"events": [
        {"kind": "down", "x": 0.65, "y": 0.55}, {"kind": "up", "x": 0.65, "y": 0.55}
    ], "shift": true}),
        T,
    );
    assert_eq!(r["ok"], true, "{r}");
    let added = settings(&h, A);
    assert_eq!(samples(&added, 0).len(), 2);
    assert_eq!(samples(&added, 0)[0], first[0]);
    assert_eq!(added.masks[0].components[1], before.masks[0].components[1]);
    assert_eq!(added.masks[0].adjust.exposure, 0.5);
    assert!(h.app.mask_color_picker.is_some());
    assert_eq!(h.app.session.undo.len(), undo_before + 3);
    exec(&mut h, "edit.undo", json!({}));
    assert_eq!(samples(&settings(&h, A), 0), first);
    assert!(h.app.mask_color_picker.is_none(), "undo changes the snapshot without changing the target ids");
    assert_eq!(library_view(&h), view);
}
