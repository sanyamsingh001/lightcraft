//! Drive the real panels and selection commands: equal values do not identify an editing target.

use std::time::Duration;

use lightcraft_catalog::{MediaKind, Op, Photo, PhotoId, Source};
use lightcraft_develop::{DevelopSettings, MaskShape};
use serde_json::{Value, json};

use crate::headless::Headless;
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const A: PhotoId = PhotoId(1);
const B: PhotoId = PhotoId(2);

fn app(raw_a: bool) -> Headless {
    let mut session = lightcraft_engine::Session::new();
    let ops = [A, B]
        .into_iter()
        .map(|id| {
            let raw = raw_a && id == A;
            let mut photo = Photo::new(
                id,
                Source::Demo { scene: 0 },
                if raw { "A.dng" } else { "image.jpg" },
                if raw { "DNG" } else { "JPEG" },
                320,
                240,
                "2026-01-01T00:00:00",
            );
            if raw {
                photo.kind = MediaKind::Raw;
            }
            Op::AddPhoto { photo: Box::new(photo) }
        })
        .collect();
    session.commit("Synthetic photos", Op::Batch { ops }).unwrap();
    // Both photos deliberately start with identical settings, so value-based guards fail.
    for id in [A, B] {
        session.execute("library.select", &json!({"ids": [id.0], "active": id.0})).unwrap();
        session.execute("develop.set", &json!({"control": "light.exposure", "value": 0.71})).unwrap();
        session.execute("develop.set", &json!({"control": "wb.temp", "value": 6500.0})).unwrap();
    }
    session.execute("library.select", &json!({"ids": [A.0], "active": A.0})).unwrap();
    let mut app = LightcraftApp::new(session, Services { png: None, ..Default::default() });
    app.ui.view = crate::state::ViewMode::Detail;
    app.ui.right = crate::state::RightPanel::Edit;
    app.ui.left_panel = false;
    app.ui.histogram = false;
    app.ui.open_sections = vec!["light".into(), "color".into()];
    let mut h = Headless::new(app, [1200.0, 1800.0], 1.0);
    for _ in 0..3 {
        h.step();
    }
    h
}

fn request(h: &mut Headless, method: &str, params: Value) -> Value {
    let r = h.request(method, params, T);
    assert_eq!(r["ok"], true, "{method}: {r}");
    r["result"].clone()
}

fn exec(h: &mut Headless, command: &str, params: Value) {
    request(h, "engine.execute", json!({"command": command, "params": params}));
}

fn select(h: &mut Headless, id: PhotoId) {
    exec(h, "library.select", json!({"ids": [id.0], "active": id.0}));
    assert_eq!(h.app.session.active(), Some(id));
}

fn settings(h: &Headless, id: PhotoId) -> DevelopSettings {
    (*h.app.session.develop_of(id).unwrap()).clone()
}

fn has_widget(h: &Headless, id: &str) -> bool {
    h.app.widgets.iter().any(|(name, _)| name == id)
}

fn draft(h: &mut Headless, control: &str, text: &str) {
    request(h, "ui.clickWidget", json!({"id": format!("sliderValue:{control}")}));
    assert!(h.view.ctx.egui_wants_keyboard_input());
    assert!(has_widget(h, &format!("sliderValueEdit:{control}")));
    request(h, "ui.text", json!({"text": text}));
}

fn enter(h: &mut Headless) {
    request(h, "ui.key", json!({"key": "Enter"}));
}

fn assert_discarded(h: &Headless) {
    assert!(!h.view.ctx.egui_wants_keyboard_input(), "switching targets releases numeric text focus");
    assert!(!h.app.widgets.iter().any(|(id, _)| id.starts_with("sliderValueEdit:")), "a target cannot inherit an inline draft");
}

#[test]
fn selecting_an_equal_valued_photo_discards_the_old_draft_before_enter() {
    let mut h = app(false);
    let a = settings(&h, A);
    let b = settings(&h, B);
    assert_eq!(a.light.exposure, b.light.exposure);
    let undo_before = h.app.session.undo.len();
    let row = h.app.widgets.iter().find(|(id, _)| id == "sliderValue:light.exposure").unwrap().1;
    draft(&mut h, "light.exposure", "1.23");
    assert_eq!(settings(&h, A), a, "typing is still a draft on A");
    // There is no pointer blur here: library selection changes the target under the same UI id.
    select(&mut h, B);
    assert_discarded(&h);
    enter(&mut h);
    assert_eq!(settings(&h, A), a);
    assert_eq!(settings(&h, B), b, "B must not receive A's draft on Enter or focus loss");
    assert_eq!(h.app.session.undo.len(), undo_before);
    assert_eq!(h.app.widgets.iter().find(|(id, _)| id == "sliderValue:light.exposure").unwrap().1, row, "switching targets preserves resting layout");
    select(&mut h, A);
    assert_discarded(&h);
    select(&mut h, B);
    draft(&mut h, "light.exposure", "-1.23");
    enter(&mut h);
    assert_eq!(settings(&h, B).light.exposure, -1.23, "a fresh draft commits to the new target");
    assert_eq!(settings(&h, A), a);
    assert_eq!(h.app.session.undo.len(), undo_before + 1);
    exec(&mut h, "edit.undo", json!({}));
    assert_eq!(settings(&h, B), b);
}

#[test]
fn selecting_a_mask_row_discards_the_other_masks_draft_in_the_same_frame() {
    let mut h = app(false);
    request(&mut h, "ui.set", json!({"right": "masking"}));
    for _ in 0..2 {
        exec(&mut h, "mask.add", json!({"kind": "linear"}));
        exec(&mut h, "mask.adjust", json!({"values": {"exposure": 0.5}}));
    }
    let d = settings(&h, A);
    let (first, second) = (d.masks[0].id, d.masks[1].id);
    exec(&mut h, "mask.select", json!({"id": first}));
    let undo_before = h.app.session.undo.len();
    draft(&mut h, "exposure", "2.12");
    assert_eq!(settings(&h, A), d);
    // A real row click changes active_mask while the Masking panel is being drawn.
    request(&mut h, "ui.clickWidget", json!({"id": format!("mask:{second}")}));
    assert_eq!(h.app.session.active_mask, Some(second));
    assert_discarded(&h);
    enter(&mut h);
    assert_eq!(settings(&h, A), d, "neither mask receives the discarded draft");
    assert_eq!(h.app.session.undo.len(), undo_before);
    // With text focus released, Enter follows the normal Done shortcut back to Edit.
    assert_eq!(h.app.ui.right, crate::state::RightPanel::Edit);
    request(&mut h, "ui.set", json!({"right": "masking"}));
    draft(&mut h, "exposure", "-1.12");
    enter(&mut h);
    assert_eq!(h.app.ui.right, crate::state::RightPanel::Masking, "focused Enter commits the number without firing Done");
    let changed = settings(&h, A);
    assert_eq!(changed.masks[0].adjust.exposure, 0.5);
    assert_eq!(changed.masks[1].adjust.exposure, -1.12);
    assert_eq!(h.app.session.undo.len(), undo_before + 1);
    exec(&mut h, "edit.undo", json!({}));
    assert_eq!(settings(&h, A), d);
    exec(&mut h, "mask.select", json!({"id": first}));
    assert_discarded(&h);
}

#[test]
fn raw_to_jpeg_discards_hidden_kelvin_draft_and_uses_jpeg_entry_bounds() {
    let mut h = app(true);
    assert!(h.app.session.catalog.photo(A).unwrap().develops_raw());
    assert!(has_widget(&h, "sliderValue:wb.temp"));
    let a = settings(&h, A);
    let b = settings(&h, B);
    let undo_before = h.app.session.undo.len();
    draft(&mut h, "wb.temp", "8500.125");
    select(&mut h, B);
    assert_discarded(&h);
    assert!(has_widget(&h, "sliderValue:wb.tempRel"));
    assert!(!has_widget(&h, "sliderValue:wb.temp"));
    enter(&mut h);
    assert_eq!(settings(&h, A), a);
    assert_eq!(settings(&h, B), b, "a Kelvin draft cannot become a JPEG adjustment");
    assert_eq!(h.app.session.undo.len(), undo_before);
    select(&mut h, A);
    assert_discarded(&h);
    enter(&mut h);
    assert_eq!(settings(&h, A), a, "returning to raw cannot resurrect the hidden draft");
    select(&mut h, B);
    for (i, (text, relative)) in [("25.0", 25.0), ("999.125", 100.0), ("-999.125", -100.0)].into_iter().enumerate() {
        draft(&mut h, "wb.tempRel", text);
        enter(&mut h);
        let expected_kelvin = 1e6 / (1e6 / 6500.0 - relative * 0.8);
        assert!((settings(&h, B).wb.temp - expected_kelvin).abs() < 1e-9, "{text}: JPEG entry uses its relative scale and clamps before conversion");
        assert_eq!(settings(&h, A), a);
        assert_eq!(h.app.session.undo.len(), undo_before + i + 1);
    }
}

#[test]
fn switching_away_from_the_adjustment_panel_discards_hidden_drafts() {
    let mut h = app(false);
    let before = settings(&h, A);
    let undo_before = h.app.session.undo.len();
    draft(&mut h, "light.exposure", "3.1234");
    request(&mut h, "ui.set", json!({"right": "info"}));
    assert_discarded(&h);
    request(&mut h, "ui.set", json!({"right": "edit"}));
    assert_discarded(&h);
    enter(&mut h);
    assert_eq!(settings(&h, A), before);
    assert_eq!(h.app.session.undo.len(), undo_before);
}

fn two_ranges() -> Headless {
    let mut h = app(false);
    request(&mut h, "ui.set", json!({"right": "masking"}));
    exec(&mut h, "mask.add", json!({"kind": "luminanceRange", "lo": 0.1, "hi": 0.4}));
    exec(&mut h, "mask.addComponent", json!({"kind": "luminanceRange", "lo": 0.6, "hi": 0.9}));
    h
}

fn readout_rects(h: &Headless, control: &str) -> Vec<egui::Rect> {
    let name = format!("sliderValue:{control}");
    h.app.widgets.iter().filter(|(id, _)| id == &name).map(|(_, r)| *r).collect()
}

#[test]
fn two_range_components_commit_only_to_the_clicked_component() {
    let mut h = two_ranges();
    let before = settings(&h, A);
    let rows = readout_rects(&h, "smoothness");
    assert_eq!(rows.len(), 2, "keep both existing automation ids");
    let undo_before = h.app.session.undo.len();
    // The existing clickWidget lookup chooses the last matching automation id.
    draft(&mut h, "smoothness", "73.0");
    assert_eq!(h.app.widgets.iter().filter(|(id, _)| id == "sliderValueEdit:smoothness").count(), 1, "one component owns the draft");
    enter(&mut h);
    let mut expected = before.clone();
    let MaskShape::LuminanceRange { lo_feather, hi_feather, .. } = &mut expected.masks[0].components[1].shape else { panic!("expected range") };
    *lo_feather = 73.0 / 200.0;
    *hi_feather = 73.0 / 200.0;
    assert_eq!(settings(&h, A), expected, "the first component and all other settings stay intact");
    assert_eq!(h.app.ui.right, crate::state::RightPanel::Masking, "focused Enter must not fire Done");
    assert_eq!(h.app.session.undo.len(), undo_before + 1);
    assert_eq!(readout_rects(&h, "smoothness"), rows, "entry preserves resting layout");
    exec(&mut h, "edit.undo", json!({}));
    assert_eq!(settings(&h, A), before);
}

#[test]
fn deleting_an_earlier_equal_valued_component_cannot_reuse_its_draft() {
    let mut h = two_ranges();
    let row = readout_rects(&h, "smoothness")[0].center();
    request(&mut h, "ui.click", json!({"x": row.x, "y": row.y}));
    assert!(h.view.ctx.egui_wants_keyboard_input());
    request(&mut h, "ui.text", json!({"text": "73.0"}));
    // Both ranges have the same smoothness. Removing component 0 moves component 1 into
    // its UI index without changing the mask id or the underlying slider value.
    exec(&mut h, "mask.component", json!({"component": 0, "action": "delete"}));
    let remaining = settings(&h, A);
    assert_eq!(remaining.masks[0].components.len(), 1);
    assert_discarded(&h);
    let undo_after_delete = h.app.session.undo.len();
    enter(&mut h);
    assert_eq!(settings(&h, A), remaining);
    assert_eq!(h.app.session.undo.len(), undo_after_delete);
}

fn two_spots() -> Headless {
    let mut h = app(false);
    request(&mut h, "ui.set", json!({"right": "remove"}));
    for x in [0.25, 0.75] {
        exec(&mut h, "spot.add", json!({"mode": "clone", "points": [[x, 0.5]], "source": [0.05, 0.0], "size": 0.02, "opacity": 100.0}));
    }
    h
}

#[test]
fn two_spots_discard_the_old_draft_and_accept_fresh_entry_on_the_selected_spot() {
    let mut h = two_spots();
    exec(&mut h, "spot.select", json!({"index": 0}));
    let before = settings(&h, A);
    assert_eq!(before.spots[0].opacity, before.spots[1].opacity);
    let rows = readout_rects(&h, "ui.removeOpacity");
    let undo_before = h.app.session.undo.len();
    draft(&mut h, "ui.removeOpacity", "38.0");
    // Selection through the shared command path does not blur the text field.
    exec(&mut h, "spot.select", json!({"index": 1}));
    assert_discarded(&h);
    enter(&mut h);
    assert_eq!(settings(&h, A), before);
    assert_eq!(h.app.session.undo.len(), undo_before);
    assert_eq!(h.app.ui.right, crate::state::RightPanel::Edit);
    request(&mut h, "ui.set", json!({"right": "remove"}));
    draft(&mut h, "ui.removeOpacity", "41.0");
    enter(&mut h);
    let mut expected = before.clone();
    expected.spots[1].opacity = 41.0;
    assert_eq!(settings(&h, A), expected, "only the selected spot receives a fresh number");
    assert_eq!(h.app.ui.right, crate::state::RightPanel::Remove);
    assert_eq!(h.app.session.undo.len(), undo_before + 1);
    assert_eq!(readout_rects(&h, "ui.removeOpacity"), rows);
    exec(&mut h, "edit.undo", json!({}));
    assert_eq!(settings(&h, A), before);
}

#[test]
fn deleting_and_undoing_a_component_does_not_resurrect_its_hidden_draft() {
    let mut h = app(false);
    request(&mut h, "ui.set", json!({"right": "masking"}));
    exec(&mut h, "mask.add", json!({"kind": "linear"}));
    exec(&mut h, "mask.addComponent", json!({"kind": "luminanceRange"}));
    let before = settings(&h, A);
    let active = h.app.session.active_mask;
    let undo_before = h.app.session.undo.len();
    draft(&mut h, "smoothness", "73.0");
    exec(&mut h, "mask.component", json!({"component": 1, "action": "delete"}));
    assert_eq!(h.app.session.active_mask, active, "the same mask remains selected");
    assert!(!has_widget(&h, "sliderValue:smoothness"));
    assert_discarded(&h);
    for _ in 0..3 {
        h.step();
    }
    exec(&mut h, "edit.undo", json!({}));
    assert!(has_widget(&h, "sliderValue:smoothness"));
    assert_discarded(&h);
    enter(&mut h);
    assert_eq!(settings(&h, A), before, "undo restores the component without its old draft");
    assert_eq!(h.app.session.undo.len(), undo_before);
}

#[test]
fn hiding_and_reopening_a_section_discards_drafts_without_pointer_blur() {
    let mut h = app(false);
    let before = settings(&h, A);
    let rows = readout_rects(&h, "light.exposure");
    let undo_before = h.app.session.undo.len();
    draft(&mut h, "light.exposure", "3.1234");
    exec(&mut h, "section.light", json!({}));
    assert!(!has_widget(&h, "sliderValue:light.exposure"));
    assert_discarded(&h);
    for _ in 0..3 {
        h.step();
    }
    exec(&mut h, "section.light", json!({}));
    assert_discarded(&h);
    enter(&mut h);
    assert_eq!(settings(&h, A), before);
    assert_eq!(h.app.session.undo.len(), undo_before);
    assert_eq!(readout_rects(&h, "light.exposure"), rows);
}

#[test]
fn reset_undo_and_redo_cancel_drafts_when_the_underlying_control_changes() {
    for (command, params, restored_exposure) in [
        ("develop.resetControl", json!({"control": "light.exposure"}), 0.0),
        ("develop.resetSection", json!({"section": "light"}), 0.0),
        ("develop.reset", json!({}), 0.0),
        ("edit.undo", json!({}), 0.71),
        ("edit.redo", json!({}), 1.5),
    ] {
        let mut h = app(false);
        exec(&mut h, "develop.set", json!({"control": "light.exposure", "value": 1.5}));
        if command == "edit.redo" {
            exec(&mut h, "edit.undo", json!({}));
        }
        draft(&mut h, "light.exposure", "2.3456");
        exec(&mut h, command, params);
        assert_discarded(&h);
        let restored = settings(&h, A);
        assert_eq!(restored.light.exposure, restored_exposure, "{command}");
        let undo_after_command = h.app.session.undo.len();
        enter(&mut h);
        assert_eq!(settings(&h, A), restored, "{command}: Enter cannot overwrite the restored settings");
        assert_eq!(h.app.session.undo.len(), undo_after_command, "{command}: cancellation adds no undo step");
        draft(&mut h, "light.exposure", "-1.23");
        enter(&mut h);
        assert_eq!(settings(&h, A).light.exposure, -1.23, "{command}: a fresh entry still commits");
    }
}

#[test]
fn an_unrelated_control_change_preserves_the_focused_draft() {
    let mut h = app(false);
    draft(&mut h, "light.exposure", "1.23");
    exec(&mut h, "develop.set", json!({"control": "light.contrast", "value": 25.0}));
    assert!(h.view.ctx.egui_wants_keyboard_input(), "invalidation belongs to the edited control, not the catalog revision");
    assert!(has_widget(&h, "sliderValueEdit:light.exposure"));
    let before_entry = settings(&h, A);
    let undo_before = h.app.session.undo.len();
    enter(&mut h);
    assert_eq!(settings(&h, A).light.exposure, 1.23);
    assert_eq!(settings(&h, A).light.contrast, 25.0);
    assert_eq!(h.app.session.undo.len(), undo_before + 1);
    exec(&mut h, "edit.undo", json!({}));
    assert_eq!(settings(&h, A), before_entry, "the numeric entry is a separate undo step");
}

#[test]
fn undoing_spot_deletion_cannot_transfer_a_draft_to_a_reused_index() {
    let mut h = two_spots();
    let original = settings(&h, A);
    exec(&mut h, "spot.delete", json!({"index": 0}));
    assert_eq!(h.app.session.active_spot, Some(0));
    assert_eq!(settings(&h, A).spots[0], original.spots[1]);
    draft(&mut h, "ui.removeOpacity", "38.0");
    exec(&mut h, "edit.undo", json!({}));
    // Undo restores the first spot in slot 0 without changing active_spot or opacity.
    assert_eq!(h.app.session.active_spot, Some(0));
    assert_eq!(settings(&h, A), original);
    assert_discarded(&h);
    let undo_after_restore = h.app.session.undo.len();
    enter(&mut h);
    assert_eq!(settings(&h, A), original, "the restored slot must not inherit the other spot's draft");
    assert_eq!(h.app.session.undo.len(), undo_after_restore);
}

#[test]
fn otherwise_equal_spots_discard_drafts_when_undo_restores_radius_or_feather() {
    for (key, first, second) in [("size", 0.01, 0.04), ("feather", 25.0, 75.0)] {
        let mut h = app(false);
        request(&mut h, "ui.set", json!({"right": "remove"}));
        for value in [first, second] {
            let mut params = json!({"mode": "clone", "points": [[0.5, 0.5]], "source": [0.05, 0.0], "size": 0.02, "feather": 50.0, "opacity": 100.0});
            params[key] = json!(value);
            exec(&mut h, "spot.add", params);
        }
        let original = settings(&h, A);
        let other_photo = settings(&h, B);
        let mut expected_second = original.spots[0].clone();
        if key == "size" {
            expected_second.size = second;
        } else {
            expected_second.feather = second;
        }
        assert_eq!(original.spots[1], expected_second, "{key}: only this property distinguishes the two targets");
        exec(&mut h, "spot.delete", json!({"index": 0}));
        assert_eq!(h.app.session.active_spot, Some(0));
        assert_eq!(settings(&h, A).spots[0], original.spots[1]);
        draft(&mut h, "ui.removeOpacity", "38.0");
        exec(&mut h, "edit.undo", json!({}));
        assert_eq!(h.app.session.active_spot, Some(0));
        assert_eq!(settings(&h, A), original);
        assert_discarded(&h);
        let undo_after_restore = h.app.session.undo.len();
        enter(&mut h);
        assert_eq!(settings(&h, A), original, "{key}: a restored spot cannot inherit the other spot's opacity draft");
        assert_eq!(settings(&h, B), other_photo);
        assert_eq!(h.app.session.undo.len(), undo_after_restore);
        request(&mut h, "ui.set", json!({"right": "remove"}));
        draft(&mut h, "ui.removeOpacity", "41.0");
        enter(&mut h);
        let mut expected = original.clone();
        expected.spots[0].opacity = 41.0;
        assert_eq!(settings(&h, A), expected, "{key}: a fresh draft still reaches the selected spot exactly");
        assert_eq!(h.app.session.undo.len(), undo_after_restore + 1);
        exec(&mut h, "edit.undo", json!({}));
        assert_eq!(settings(&h, A), original);
    }
}

#[test]
fn undoing_same_kind_component_replacement_discards_the_old_numeric_draft() {
    let mut h = two_ranges();
    let original = settings(&h, A);
    select(&mut h, B);
    exec(&mut h, "mask.add", json!({"kind": "luminanceRange", "lo": 0.2, "hi": 0.3}));
    exec(&mut h, "mask.addComponent", json!({"kind": "luminanceRange", "lo": 0.7, "hi": 0.8}));
    let other_photo = settings(&h, B);
    assert_eq!(original.masks[0].id, other_photo.masks[0].id);
    assert_eq!(original.masks[0].components.len(), other_photo.masks[0].components.len());
    for (before, after) in original.masks[0].components.iter().zip(&other_photo.masks[0].components) {
        let MaskShape::LuminanceRange { lo_feather: before_lo, hi_feather: before_hi, .. } = &before.shape else { panic!("expected range") };
        let MaskShape::LuminanceRange { lo_feather: after_lo, hi_feather: after_hi, .. } = &after.shape else { panic!("expected range") };
        assert_eq!((before_lo, before_hi), (after_lo, after_hi), "same-kind replacement preserves the numeric baseline");
        assert_ne!(before, after, "the component content differs");
    }
    exec(&mut h, "develop.copy", json!({"groups": ["masks"]}));
    select(&mut h, A);
    exec(&mut h, "develop.paste", json!({"groups": ["masks"]}));
    exec(&mut h, "mask.select", json!({"id": original.masks[0].id}));
    assert_eq!(settings(&h, A).masks, other_photo.masks);
    let rows = readout_rects(&h, "smoothness");
    draft(&mut h, "smoothness", "73.0");
    exec(&mut h, "edit.undo", json!({}));
    assert_eq!(h.app.session.active_mask, Some(original.masks[0].id));
    assert_eq!(settings(&h, A), original);
    assert_discarded(&h);
    let undo_after_restore = h.app.session.undo.len();
    enter(&mut h);
    assert_eq!(settings(&h, A), original, "Enter cannot apply the pasted component's draft to the restored component");
    assert_eq!(settings(&h, B), other_photo);
    assert_eq!(h.app.session.undo.len(), undo_after_restore);
    request(&mut h, "ui.set", json!({"right": "masking"}));
    draft(&mut h, "smoothness", "73.0");
    enter(&mut h);
    let mut expected = original.clone();
    let MaskShape::LuminanceRange { lo_feather, hi_feather, .. } = &mut expected.masks[0].components[1].shape else { panic!("expected range") };
    *lo_feather = 73.0 / 200.0;
    *hi_feather = 73.0 / 200.0;
    assert_eq!(settings(&h, A), expected);
    assert_eq!(h.app.session.undo.len(), undo_after_restore + 1);
    assert_eq!(readout_rects(&h, "smoothness"), rows, "target snapshots preserve resting layout");
    exec(&mut h, "edit.undo", json!({}));
    assert_eq!(settings(&h, A), original);
}

#[test]
fn unrelated_global_edits_preserve_mask_and_spot_numeric_drafts() {
    for mask in [true, false] {
        let mut h = if mask { two_ranges() } else { two_spots() };
        let control = if mask { "smoothness" } else { "ui.removeOpacity" };
        draft(&mut h, control, "41.0");
        exec(&mut h, "develop.set", json!({"control": "light.contrast", "value": 25.0}));
        assert!(h.view.ctx.egui_wants_keyboard_input(), "unrelated settings changes leave the target's text focused");
        assert!(has_widget(&h, &format!("sliderValueEdit:{control}")));
        let before_entry = settings(&h, A);
        let undo_before = h.app.session.undo.len();
        enter(&mut h);
        let mut expected = before_entry.clone();
        if mask {
            let MaskShape::LuminanceRange { lo_feather, hi_feather, .. } = &mut expected.masks[0].components[1].shape else {
                panic!("expected range")
            };
            *lo_feather = 41.0 / 200.0;
            *hi_feather = 41.0 / 200.0;
        } else {
            expected.spots[1].opacity = 41.0;
        }
        assert_eq!(settings(&h, A), expected, "only the numeric target changes; contrast stays at 25");
        assert_eq!(h.app.session.undo.len(), undo_before + 1);
        exec(&mut h, "edit.undo", json!({}));
        assert_eq!(settings(&h, A), before_entry, "undoing the numeric entry preserves the unrelated global edit");
    }
}

fn show_indexed_targets(h: &mut Headless, eyes: bool, index: usize) {
    let params = if eyes {
        json!({"right": "redEye", "eye": index})
    } else {
        json!({"right": "edit", "pointColor": index, "openSections": ["light", "color"], "openFlyouts": ["pointColor"]})
    };
    request(h, "ui.set", params);
}

fn indexed_targets(eyes: bool) -> Headless {
    let mut h = app(false);
    if eyes {
        for x in [0.25, 0.75] {
            exec(&mut h, "redeye.add", json!({"center": [x, 0.5], "rx": 0.02, "ry": 0.015, "pupilSize": 50.0, "darken": 50.0}));
        }
    } else {
        let samples = [45.0, 225.0].map(|hue| lightcraft_develop::PointColor { hue, ..Default::default() });
        exec(&mut h, "develop.merge", json!({"settings": {"point_colors": samples}}));
    }
    show_indexed_targets(&mut h, eyes, 0);
    h
}

fn indexed_control(eyes: bool) -> &'static str {
    if eyes { "redEye.pupilSize" } else { "pointColor.hueShift" }
}

fn set_indexed_expected(settings: &mut DevelopSettings, eyes: bool, index: usize, value: f64) {
    if eyes {
        settings.red_eye[index].pupil_size = value;
    } else {
        settings.point_colors[index].hue_shift = value;
    }
}

#[test]
fn deleting_undoing_and_redoing_indexed_targets_cannot_transfer_numeric_drafts() {
    for eyes in [true, false] {
        let mut h = indexed_targets(eyes);
        let original = settings(&h, A);
        let other_photo = settings(&h, B);
        let control = indexed_control(eyes);
        let command = if eyes { "redeye.delete" } else { "pointColor.delete" };
        if eyes {
            assert_ne!(original.red_eye[0], original.red_eye[1]);
            assert_eq!(original.red_eye[0].pupil_size, original.red_eye[1].pupil_size);
        } else {
            assert_ne!(original.point_colors[0], original.point_colors[1]);
            assert_eq!(original.point_colors[0].hue_shift, original.point_colors[1].hue_shift);
        }
        // Deletion reuses slot 0 without changing the numeric baseline or the widget id.
        draft(&mut h, control, "37.1234");
        exec(&mut h, command, json!({"index": 0}));
        let remaining = settings(&h, A);
        if eyes {
            assert_eq!(remaining.red_eye, vec![original.red_eye[1]]);
        } else {
            assert_eq!(remaining.point_colors, vec![original.point_colors[1]]);
        }
        assert_discarded(&h);
        let undo_after_delete = h.app.session.undo.len();
        enter(&mut h);
        assert_eq!(settings(&h, A), remaining);
        assert_eq!(h.app.session.undo.len(), undo_after_delete);

        show_indexed_targets(&mut h, eyes, 0);
        draft(&mut h, control, "38.2345");
        exec(&mut h, "edit.undo", json!({}));
        assert_eq!((h.app.ui.eye, h.app.ui.point_color), (0, 0), "history preserves the selected index");
        assert_eq!(settings(&h, A), original);
        assert_discarded(&h);
        let undo_after_restore = h.app.session.undo.len();
        enter(&mut h);
        assert_eq!(settings(&h, A), original, "the restored first item cannot inherit the second item's draft");
        assert_eq!(h.app.session.undo.len(), undo_after_restore);

        show_indexed_targets(&mut h, eyes, 0);
        draft(&mut h, control, "39.3456");
        exec(&mut h, "edit.redo", json!({}));
        assert_eq!(settings(&h, A), remaining);
        assert_discarded(&h);
        let undo_after_redo = h.app.session.undo.len();
        enter(&mut h);
        assert_eq!(settings(&h, A), remaining, "redo cannot transfer the restored item's draft to the remaining item");
        assert_eq!(h.app.session.undo.len(), undo_after_redo);

        show_indexed_targets(&mut h, eyes, 0);
        draft(&mut h, control, "41.0");
        enter(&mut h);
        let mut expected = remaining.clone();
        set_indexed_expected(&mut expected, eyes, 0, 41.0);
        assert_eq!(settings(&h, A), expected, "a fresh entry still reaches the surviving target");
        assert_eq!(settings(&h, B), other_photo);
        assert_eq!(h.app.session.undo.len(), undo_after_redo + 1);
        exec(&mut h, "edit.undo", json!({}));
        assert_eq!(settings(&h, A), remaining);
    }
}

#[test]
fn changing_eye_or_point_color_selection_discards_equal_valued_drafts() {
    for eyes in [true, false] {
        let mut h = indexed_targets(eyes);
        let before = settings(&h, A);
        let other_photo = settings(&h, B);
        let undo_before = h.app.session.undo.len();
        let control = indexed_control(eyes);
        draft(&mut h, control, "37.1234");
        // ui.set changes the index without pointer blur or a different slider id.
        show_indexed_targets(&mut h, eyes, 1);
        assert_discarded(&h);
        enter(&mut h);
        assert_eq!(settings(&h, A), before);
        assert_eq!(h.app.session.undo.len(), undo_before);
        show_indexed_targets(&mut h, eyes, 1);
        draft(&mut h, control, "41.0");
        enter(&mut h);
        let mut expected = before.clone();
        set_indexed_expected(&mut expected, eyes, 1, 41.0);
        assert_eq!(settings(&h, A), expected);
        assert_eq!(settings(&h, B), other_photo);
        assert_eq!(h.app.session.undo.len(), undo_before + 1);
        exec(&mut h, "edit.undo", json!({}));
        assert_eq!(settings(&h, A), before);
        if !eyes {
            // The sample row changes selection during panel drawing. Its fields must use
            // the new sample, and its former draft must not survive that same panel draw.
            draft(&mut h, control, "43.5678");
            request(&mut h, "ui.clickWidget", json!({"id": "pointColor:0"}));
            assert_eq!(h.app.ui.point_color, 0);
            assert_discarded(&h);
            draft(&mut h, control, "46.0");
            enter(&mut h);
            let mut expected = before.clone();
            set_indexed_expected(&mut expected, false, 0, 46.0);
            assert_eq!(settings(&h, A), expected, "the clicked sample owns its next entry");
        }
    }
}

#[test]
fn unrelated_global_and_other_item_edits_preserve_indexed_numeric_drafts() {
    for eyes in [true, false] {
        let mut h = indexed_targets(eyes);
        let control = indexed_control(eyes);
        draft(&mut h, control, "41.0");
        exec(&mut h, "develop.set", json!({"control": "light.contrast", "value": 25.0}));
        let other_control = if eyes { "redEye.1.darken" } else { "pointColor.1.satShift" };
        exec(&mut h, "develop.set", json!({"control": other_control, "value": 62.3456}));
        assert!(h.view.ctx.egui_wants_keyboard_input());
        assert!(has_widget(&h, &format!("sliderValueEdit:{control}")), "only the selected item's content can invalidate this entry");
        let before_entry = settings(&h, A);
        let undo_before = h.app.session.undo.len();
        enter(&mut h);
        let mut expected = before_entry.clone();
        set_indexed_expected(&mut expected, eyes, 0, 41.0);
        assert_eq!(settings(&h, A), expected, "the global and other item edits stay intact");
        assert_eq!(h.app.session.undo.len(), undo_before + 1);
        exec(&mut h, "edit.undo", json!({}));
        assert_eq!(settings(&h, A), before_entry);
    }
}

#[test]
fn point_color_content_and_selection_changes_preserve_a_global_numeric_draft() {
    let mut h = indexed_targets(false);
    draft(&mut h, "light.exposure", "1.23");
    exec(&mut h, "develop.set", json!({"control": "pointColor.0.hueShift", "value": 25.0}));
    show_indexed_targets(&mut h, false, 1);
    assert!(h.view.ctx.egui_wants_keyboard_input());
    assert!(has_widget(&h, "sliderValueEdit:light.exposure"));
    let before_entry = settings(&h, A);
    let undo_before = h.app.session.undo.len();
    enter(&mut h);
    let mut expected = before_entry.clone();
    expected.light.exposure = 1.23;
    assert_eq!(settings(&h, A), expected);
    assert_eq!(h.app.session.undo.len(), undo_before + 1);
    exec(&mut h, "edit.undo", json!({}));
    assert_eq!(settings(&h, A), before_entry);
}

fn focused_draft_text(h: &Headless) -> String {
    let id = h.view.ctx.memory(|m| m.focused()).expect("numeric entry owns focus");
    h.view.ctx.data(|d| d.get_temp::<super::NumericDraft>(id)).expect("focused numeric draft").text
}

#[test]
fn selected_spot_fields_reopen_from_stored_f64_precision() {
    let mut h = two_spots();
    exec(&mut h, "spot.select", json!({"index": 0}));
    exec(&mut h, "spot.update", json!({"size": 0.037123456789, "feather": 41.2367891, "opacity": 37.123456789}));
    let original = settings(&h, A);
    let other_photo = settings(&h, B);
    assert_ne!(original.spots[0].size, h.app.ui.remove_size as f64);
    assert_ne!(original.spots[0].feather, h.app.ui.remove_feather as f64);
    assert_ne!(original.spots[0].opacity, h.app.ui.remove_opacity as f64);
    for (control, key, typed) in [("ui.removeSize", "size", 42.0), ("ui.removeFeather", "feather", 43.0), ("ui.removeOpacity", "opacity", 39.0)] {
        let (stored, scale) = match key {
            "size" => (original.spots[0].size, 1000.0),
            "feather" => (original.spots[0].feather, 1.0),
            _ => (original.spots[0].opacity, 1.0),
        };
        let undo_before = h.app.session.undo.len();
        request(&mut h, "ui.clickWidget", json!({"id": format!("sliderValue:{control}")}));
        assert_eq!(focused_draft_text(&h), (stored * scale).to_string(), "{key}: opening entry preserves the stored value");
        enter(&mut h);
        assert_eq!(settings(&h, A), original, "accepting the original text preserves all settings");
        assert_eq!(h.app.session.undo.len(), undo_before, "unchanged entry adds no undo step");

        draft(&mut h, control, &typed.to_string());
        enter(&mut h);
        let mut expected = original.clone();
        match key {
            "size" => expected.spots[0].size = typed / scale,
            "feather" => expected.spots[0].feather = typed,
            _ => expected.spots[0].opacity = typed,
        }
        assert_eq!(settings(&h, A), expected, "only the selected spot field changes, using upstream control steps");
        assert_eq!(settings(&h, B), other_photo);
        assert_eq!(h.app.session.undo.len(), undo_before + 1);
        let reopened = match key {
            "size" => expected.spots[0].size * scale,
            "feather" => expected.spots[0].feather,
            _ => expected.spots[0].opacity,
        };
        request(&mut h, "ui.clickWidget", json!({"id": format!("sliderValue:{control}")}));
        assert_eq!(focused_draft_text(&h), reopened.to_string(), "{key}: re-entry uses the committed f64 value");
        enter(&mut h);
        assert_eq!(settings(&h, A), expected);
        assert_eq!(h.app.session.undo.len(), undo_before + 1);
        exec(&mut h, "edit.undo", json!({}));
        assert_eq!(settings(&h, A), original);
        exec(&mut h, "edit.redo", json!({}));
        assert_eq!(settings(&h, A), expected);
        exec(&mut h, "edit.undo", json!({}));
        assert_eq!(settings(&h, A), original);
    }
}
