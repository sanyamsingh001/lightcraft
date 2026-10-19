//! Equal-valued resets through the actual Headless control route.

use super::{A, B, T, app, assert_discarded, draft, draft_text, enter, exec, request, settings};
use crate::headless::Headless;
use serde_json::json;

fn default_exposure_draft() -> Result<Headless, String> {
    let mut h = app(false);
    h.app.run("develop.set", json!({"control": "light.exposure", "value": 0.0}))?;
    h.app.run("develop.set", json!({"control": "light.contrast", "value": 25.0}))?;
    h.step();
    draft(&mut h, "light.exposure", "3");
    assert_eq!(settings(&h, A).light.exposure, 0.0, "typing must not change the engine value");
    assert_eq!(settings(&h, A).light.contrast, 25.0);
    assert_eq!(draft_text(&h), "3");
    Ok(h)
}

#[test]
fn central_resets_retire_equal_valued_drafts_before_enter() {
    for (command, params, contrast) in [
        ("develop.reset", json!({}), 0.0),
        ("develop.resetSection", json!({"section": "light"}), 0.0),
        ("develop.resetControl", json!({"control": "light.exposure"}), 25.0),
    ] {
        let mut h = default_exposure_draft().unwrap();
        // The control protocol goes through LightcraftApp::run; no pointer blur or temporary
        // exposure change can disguise an equal-valued reset as a value-based cancellation.
        exec(&mut h, command, params);
        assert_eq!(settings(&h, A).light.exposure, 0.0);
        assert_eq!(settings(&h, A).light.contrast, contrast);
        assert_discarded(&h);
        let reset = settings(&h, A);
        let undo_after_reset = h.app.session.undo.len();
        enter(&mut h);
        assert_eq!(settings(&h, A), reset, "{command}: Enter cannot reapply the pending 3");
        assert_eq!(h.app.session.undo.len(), undo_after_reset);
        draft(&mut h, "light.exposure", "-1.23");
        enter(&mut h);
        assert_eq!(settings(&h, A).light.exposure, -1.23, "{command}: a fresh draft still commits");
    }
}

#[test]
fn a_successful_noop_full_reset_still_retires_the_draft() {
    let mut h = app(false);
    // Normalize the import defaults before typing; the second Reset changes no settings.
    exec(&mut h, "develop.reset", json!({}));
    let defaults = settings(&h, A);
    assert_eq!(defaults.light.exposure, 0.0);
    assert_eq!(defaults.light.contrast, 0.0);
    draft(&mut h, "light.exposure", "3");
    exec(&mut h, "develop.reset", json!({}));
    assert_eq!(settings(&h, A), defaults, "the entire settings snapshot remains equal");
    let undo_after_reset = h.app.session.undo.len();
    assert_discarded(&h);
    enter(&mut h);
    assert_eq!(settings(&h, A), defaults);
    assert_eq!(h.app.session.undo.len(), undo_after_reset, "retiring the draft adds no history beyond Reset itself");
}

#[test]
fn reset_button_still_discards_an_equal_valued_draft() {
    let mut h = default_exposure_draft().unwrap();
    request(&mut h, "ui.clickWidget", json!({"id": "reset"}));
    assert_discarded(&h);
    assert_eq!(settings(&h, A).light.exposure, 0.0);
    assert_eq!(settings(&h, A).light.contrast, 0.0);
    let undo_after_reset = h.app.session.undo.len();
    enter(&mut h);
    assert_eq!(settings(&h, A).light.exposure, 0.0);
    assert_eq!(h.app.session.undo.len(), undo_after_reset);
}

#[test]
fn unrelated_resets_and_edits_preserve_an_equal_valued_exposure_draft() {
    for (command, params, contrast) in [
        ("develop.reset", json!({"ids": [B.0]}), 25.0),
        ("develop.reset", json!({"ids": []}), 25.0),
        ("develop.resetSection", json!({"section": "color"}), 25.0),
        ("develop.resetControl", json!({"control": "light.contrast"}), 0.0),
        ("develop.set", json!({"control": "light.contrast", "value": 30.0}), 30.0),
    ] {
        let mut h = default_exposure_draft().unwrap();
        exec(&mut h, command, params);
        assert!(h.view.ctx.egui_wants_keyboard_input(), "{command}: the exposure draft keeps focus");
        assert_eq!(draft_text(&h), "3");
        assert_eq!(settings(&h, A).light.exposure, 0.0);
        enter(&mut h);
        assert_eq!(settings(&h, A).light.exposure, 3.0);
        assert_eq!(settings(&h, A).light.contrast, contrast);
    }
}

#[test]
fn rejected_reset_commands_preserve_the_pending_draft() {
    for (command, params) in [("develop.resetSection", json!({"section": "unknown"})), ("develop.resetControl", json!({"control": "unknown"}))] {
        let mut h = default_exposure_draft().unwrap();
        let before = settings(&h, A);
        let undo_before = h.app.session.undo.len();
        let reply = h.request("engine.execute", json!({"command": command, "params": params}), T);
        assert_eq!(reply["ok"], false, "{reply}");
        assert_eq!(settings(&h, A), before);
        assert_eq!(h.app.session.undo.len(), undo_before);
        assert!(h.view.ctx.egui_wants_keyboard_input());
        assert_eq!(draft_text(&h), "3");
        enter(&mut h);
        assert_eq!(settings(&h, A).light.exposure, 3.0, "{command}: rejection leaves the draft available");
    }
}

#[test]
fn multiple_central_resets_before_widget_sync_do_not_hide_retirement() {
    let mut h = default_exposure_draft().unwrap();
    h.app.run("develop.resetSection", json!({"section": "light"})).unwrap();
    h.app.run("develop.resetSection", json!({"section": "color"})).unwrap();
    h.app.run("develop.reset", json!({"ids": [B.0]})).unwrap();
    assert_eq!(settings(&h, A).light.exposure, 0.0);
    h.step();
    assert_discarded(&h);
    enter(&mut h);
    assert_eq!(settings(&h, A).light.exposure, 0.0);
}
