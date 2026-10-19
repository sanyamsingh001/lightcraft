//! Equal-valued resets through the actual Headless control route.

use super::{A, B, T, app, assert_discarded, draft, enter, exec, request, settings};
use crate::headless::Headless;
use serde_json::json;

fn draft_text(h: &Headless) -> Option<String> {
    let field = h.view.ctx.memory(|memory| memory.focused())?;
    h.view.ctx.data(|data| data.get_temp::<super::super::NumericDraft>(field)).map(|draft| draft.text)
}

fn default_exposure_draft() -> Result<Headless, String> {
    let mut h = app(false);
    h.app.run("develop.set", json!({"control": "light.exposure", "value": 0.0}))?;
    h.app.run("develop.set", json!({"control": "light.contrast", "value": 25.0}))?;
    h.step();
    draft(&mut h, "light.exposure", "3");
    assert_eq!(settings(&h, A).light.exposure, 0.0, "typing must not change the engine value");
    assert_eq!(settings(&h, A).light.contrast, 25.0);
    assert_eq!(draft_text(&h).as_deref(), Some("3"));
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
    request(&mut h, "ui.clickWidget", json!({"id": "button:reset"}));
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
        assert_eq!(draft_text(&h).as_deref(), Some("3"));
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
        assert_eq!(draft_text(&h).as_deref(), Some("3"));
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

fn relative_wb_draft(control: &str) -> Headless {
    let mut h = app(false);
    exec(&mut h, "develop.resetSection", json!({"section": "color"}));
    let wb = settings(&h, A).wb;
    assert_eq!(wb.temp, 6500.0);
    assert_eq!(wb.tint, 0.0);
    draft(&mut h, control, "20");
    assert_eq!(draft_text(&h).as_deref(), Some("20"));
    assert_eq!(settings(&h, A).wb, wb, "typing is still a relative WB draft");
    h
}

#[test]
fn successful_noop_relative_wb_control_and_section_resets_retire_drafts() {
    for (control, stored) in [("wb.tempRel", "wb.temp"), ("wb.tintRel", "wb.tint")] {
        for (command, params) in [("develop.resetControl", json!({"control": stored})), ("develop.resetSection", json!({"section": "color"}))] {
            let mut h = relative_wb_draft(control);
            let before = settings(&h, A);
            exec(&mut h, command, params);
            assert_eq!(settings(&h, A), before, "{control}: the successful reset changes no settings");
            assert_discarded(&h);
            let undo_after_reset = h.app.session.undo.len();
            enter(&mut h);
            assert_eq!(settings(&h, A), before, "{control}: Enter cannot reapply the pending 20");
            assert_eq!(h.app.session.undo.len(), undo_after_reset);
            draft(&mut h, control, "10");
            enter(&mut h);
            let wb = settings(&h, A).wb;
            if stored == "wb.temp" {
                assert!((wb.temp - 1e6 / (1e6 / 6500.0 - 10.0 * 0.8)).abs() < 1e-9);
            } else {
                assert_eq!(wb.tint, 10.0);
            }
        }
    }
}

#[test]
fn unrelated_and_rejected_resets_preserve_relative_wb_drafts() {
    for (control, other) in [("wb.tempRel", "wb.tint"), ("wb.tintRel", "wb.temp")] {
        for (command, params) in [
            ("develop.reset", json!({"ids": [B.0]})),
            ("develop.reset", json!({"ids": []})),
            ("develop.resetSection", json!({"section": "light"})),
            ("develop.resetControl", json!({"control": other})),
            ("develop.set", json!({"control": "light.contrast", "value": 25.0})),
        ] {
            let mut h = relative_wb_draft(control);
            exec(&mut h, command, params);
            assert!(h.view.ctx.egui_wants_keyboard_input());
            assert_eq!(draft_text(&h).as_deref(), Some("20"));
            let before_commit = settings(&h, A);
            enter(&mut h);
            let mut expected = before_commit;
            expected.wb.mode = lightcraft_develop::WbMode::Custom;
            if control == "wb.tempRel" {
                expected.wb.temp = 1e6 / (1e6 / 6500.0 - 20.0 * 0.8);
            } else {
                expected.wb.tint = 20.0;
            }
            assert_eq!(settings(&h, A), expected, "{control}: unrelated edits remain intact");
        }
        for (command, params) in [("develop.resetSection", json!({"section": "unknown"})), ("develop.resetControl", json!({"control": "unknown"}))] {
            let mut h = relative_wb_draft(control);
            let before = settings(&h, A);
            let undo_before = h.app.session.undo.len();
            let reply = h.request("engine.execute", json!({"command": command, "params": params}), T);
            assert_eq!(reply["ok"], false, "{reply}");
            assert_eq!(settings(&h, A), before);
            assert_eq!(h.app.session.undo.len(), undo_before);
            assert!(h.view.ctx.egui_wants_keyboard_input());
            assert_eq!(draft_text(&h).as_deref(), Some("20"));
            enter(&mut h);
            assert_ne!(settings(&h, A).wb, before.wb, "{control}: rejected reset leaves the draft usable");
        }
    }
}

#[test]
fn relative_wb_drafts_cannot_follow_equal_photo_library_or_panel_targets() {
    use lightcraft_catalog::MemStore;
    use lightcraft_engine::library::LibraryStores;

    for control in ["wb.tempRel", "wb.tintRel"] {
        for exit in 0..4 {
            let mut h = relative_wb_draft(control);
            match exit {
                0 => exec(&mut h, "library.select", json!({"ids": [B.0], "active": B.0})),
                1 => {
                    request(&mut h, "ui.set", json!({"right": "remove"}));
                }
                2 => {
                    h.app
                        .session
                        .open_library_in(
                            LibraryStores {
                                dir: "relative-wb-new-library".into(),
                                catalog: Box::new(MemStore::new()),
                                files: Box::new(MemStore::new()),
                                on_disk: false,
                            },
                            true,
                        )
                        .unwrap();
                    // Demo seeding chooses capture-date order; pin the reused ID before a frame.
                    h.app.session.execute("library.select", &json!({"ids": [A.0], "active": A.0})).unwrap();
                    assert_eq!(h.app.session.active(), Some(A), "new library reuses the photo id");
                    h.step();
                }
                _ => {
                    h.app.session = lightcraft_engine::Session::with_demo();
                    h.app.session.execute("library.select", &json!({"ids": [A.0], "active": A.0})).unwrap();
                    assert_eq!(h.app.session.active(), Some(A), "new session reuses the photo id");
                    h.step();
                }
            }
            assert_discarded(&h);
            let photo = h.app.session.active().unwrap();
            let before = settings(&h, photo);
            let undo_before = h.app.session.undo.len();
            enter(&mut h);
            assert_eq!(settings(&h, photo), before, "{control}: the new target cannot inherit 20");
            assert_eq!(h.app.session.undo.len(), undo_before);
        }
    }
}

// A real in-memory catalog store with the same append failure contract as a disk journal.
struct FailingNumericAppend {
    store: lightcraft_catalog::MemStore,
    fail: std::sync::Arc<std::sync::atomic::AtomicBool>,
}
impl lightcraft_catalog::Store for FailingNumericAppend {
    fn read(&mut self, name: &str) -> std::io::Result<Option<Vec<u8>>> {
        lightcraft_catalog::Store::read(&mut self.store, name)
    }
    fn write_atomic(&mut self, name: &str, data: &[u8]) -> std::io::Result<()> {
        lightcraft_catalog::Store::write_atomic(&mut self.store, name, data)
    }
    fn append(&mut self, name: &str, data: &[u8]) -> std::io::Result<()> {
        if self.fail.load(std::sync::atomic::Ordering::Relaxed) {
            Err(std::io::Error::other("injected numeric journal append failure"))
        } else {
            lightcraft_catalog::Store::append(&mut self.store, name, data)
        }
    }
    fn truncate(&mut self, name: &str, len: u64) -> std::io::Result<()> {
        lightcraft_catalog::Store::truncate(&mut self.store, name, len)
    }
    fn describe(&self) -> String {
        lightcraft_catalog::Store::describe(&self.store)
    }
}

#[test]
fn reset_applied_before_journal_failure_retires_an_equal_valued_draft() {
    use lightcraft_catalog::{MemStore, Op};
    use lightcraft_engine::library::LibraryStores;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    for (command, params) in [("develop.reset", json!({})), ("develop.resetSection", json!({"section": "light"}))] {
        let mut h = app(false);
        let photo = h.app.session.catalog.photo(A).unwrap().as_ref().clone();
        let fail = Arc::new(AtomicBool::new(false));
        h.app
            .session
            .open_library_in(
                LibraryStores {
                    dir: "numeric-unsaved-reset".into(),
                    catalog: Box::new(FailingNumericAppend { store: MemStore::new(), fail: fail.clone() }),
                    files: Box::new(MemStore::new()),
                    on_disk: false,
                },
                false,
            )
            .unwrap();
        h.app.session.commit("Synthetic photo", Op::AddPhoto { photo: Box::new(photo) }).unwrap();
        h.app.run("library.select", json!({"ids": [A.0], "active": A.0})).unwrap();
        h.app.run("develop.set", json!({"control": "light.exposure", "value": 0.0})).unwrap();
        h.app.run("develop.set", json!({"control": "light.contrast", "value": 25.0})).unwrap();
        h.step();
        draft(&mut h, "light.exposure", "3");
        assert_eq!(draft_text(&h).as_deref(), Some("3"));
        fail.store(true, Ordering::Relaxed);
        let error = h.app.run(command, params).unwrap_err();
        assert!(error.starts_with("saved in memory but not written to disk:"), "{error}");
        assert_eq!(settings(&h, A).light.exposure, 0.0, "the draft's stored value remains equal");
        assert_eq!(settings(&h, A).light.contrast, 0.0, "Reset applied despite the failed append");
        let reset = settings(&h, A);
        let undo_after_reset = h.app.session.undo.len();
        fail.store(false, Ordering::Relaxed);
        h.step();
        assert_discarded(&h);
        enter(&mut h);
        assert_eq!(settings(&h, A), reset, "{command}: Enter cannot overwrite the applied Reset with 3");
        assert_eq!(h.app.session.undo.len(), undo_after_reset);
        draft(&mut h, "light.exposure", "-1.23");
        enter(&mut h);
        assert_eq!(settings(&h, A).light.exposure, -1.23, "a fresh draft still commits after saving recovers");
    }
}

#[test]
fn delete_reuse_after_journal_failure_retires_drafts_and_invalid_deletes_preserve_them() {
    use lightcraft_catalog::{MemStore, Op, Source};
    use lightcraft_engine::library::LibraryStores;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    for (kind, control) in [(0, "redEye.pupilSize"), (1, "smoothness"), (2, "pointColor.hueShift")] {
        let mut h = app(false);
        let photo = h.app.session.catalog.photo(A).unwrap().as_ref().clone();
        let fail = Arc::new(AtomicBool::new(false));
        h.app
            .session
            .open_library_in(
                LibraryStores {
                    dir: "numeric-unsaved-delete".into(),
                    catalog: Box::new(FailingNumericAppend { store: MemStore::new(), fail: fail.clone() }),
                    files: Box::new(MemStore::new()),
                    on_disk: false,
                },
                false,
            )
            .unwrap();
        h.app.session.commit("Synthetic photo", Op::AddPhoto { photo: Box::new(photo) }).unwrap();
        h.app.run("library.select", json!({"ids": [A.0], "active": A.0})).unwrap();
        let (delete, params, restore, replacement) = match kind {
            0 => {
                let eye = json!({"center": [0.25, 0.5], "rx": 0.02, "ry": 0.015, "pupilSize": 50.0, "darken": 50.0});
                exec(&mut h, "redeye.add", eye.clone());
                super::show_indexed_targets(&mut h, true, 0);
                ("redeye.delete", json!({"index": 0}), "redeye.add", eye)
            }
            1 => {
                request(&mut h, "ui.set", json!({"right": "masking"}));
                exec(&mut h, "mask.add", json!({"kind": "luminanceRange", "lo": 0.1, "hi": 0.9}));
                let duplicate = json!({"component": 0, "action": "duplicate"});
                exec(&mut h, "mask.component", duplicate.clone());
                ("mask.component", json!({"component": 1, "action": "delete"}), "mask.component", duplicate)
            }
            _ => {
                h.app
                    .session
                    .catalog
                    .apply(Op::Relink { id: A, file_name: "image.jpg".into(), source: Source::Demo { scene: 1 }, format: None })
                    .unwrap();
                let point = json!({"x": 0.25, "y": 0.5});
                exec(&mut h, "pointColor.pick", point.clone());
                super::show_indexed_targets(&mut h, false, 0);
                ("pointColor.delete", json!({"index": 0}), "pointColor.pick", point)
            }
        };
        draft(&mut h, control, "41");
        let before = settings(&h, A);
        fail.store(true, Ordering::Relaxed);
        let error = h.app.run("photo.rate", json!({"rating": 4})).unwrap_err();
        assert!(error.starts_with("saved in memory but not written to disk:"), "{error}");
        assert_eq!(h.app.session.catalog.photo(A).unwrap().rating, 4);
        let undo = h.app.session.undo.len();
        let mut invalid = params.clone();
        invalid[if kind == 1 { "component" } else { "index" }] = json!(99);
        let error = h.app.run(delete, invalid).unwrap_err();
        assert!(!error.starts_with("saved in memory but not written to disk:"), "invalid delete remains its own error: {error}");
        h.step();
        assert_eq!(settings(&h, A), before);
        assert_eq!(h.app.session.undo.len(), undo);
        assert_eq!(h.app.session.unsaved().map(|u| u.0), Some(1));
        assert!(h.view.ctx.egui_wants_keyboard_input(), "{delete}: rejected delete preserves focus with an older queued failure");
        assert_eq!(draft_text(&h).as_deref(), Some("41"));
        let mut deleted = before.clone();
        match kind {
            0 => {
                deleted.red_eye.remove(0);
            }
            1 => {
                deleted.masks[0].components.remove(1);
            }
            _ => {
                deleted.point_colors.remove(0);
            }
        }
        let error = h.app.run(delete, params).unwrap_err();
        assert!(error.starts_with("saved in memory but not written to disk:"), "{delete}: {error}");
        assert_eq!(settings(&h, A), deleted, "{delete}: removal applied before the append failed");
        let error = h.app.run(restore, replacement).unwrap_err();
        assert!(error.starts_with("saved in memory but not written to disk:"), "{restore}: {error}");
        assert_eq!(settings(&h, A), before, "replacement reuses identical values before widget sync");
        assert_eq!(h.app.session.undo.len(), undo + 2);
        h.step();
        assert_discarded(&h);
        enter(&mut h);
        assert_eq!(settings(&h, A), before, "{delete}: Enter cannot apply the deleted target's draft");
        assert_eq!(h.app.session.undo.len(), undo + 2);
        assert_eq!(h.app.session.unsaved().map(|u| u.0), Some(3));
        fail.store(false, Ordering::Relaxed);
        h.app.run("library.info", json!({})).unwrap();
        assert!(h.app.session.unsaved().is_none(), "a successful retry writes all queued operations");
        assert_eq!(settings(&h, A), before);
        assert_eq!(h.app.session.catalog.photo(A).unwrap().rating, 4);
        if kind == 1 {
            request(&mut h, "ui.set", json!({"right": "masking"}));
        } else {
            super::show_indexed_targets(&mut h, kind == 0, 0);
        }
        draft(&mut h, control, "42");
        enter(&mut h);
        let mut expected = before;
        match kind {
            0 => expected.red_eye[0].pupil_size = 42.0,
            1 => {
                let lightcraft_develop::MaskShape::LuminanceRange { lo_feather, hi_feather, .. } = &mut expected.masks[0].components[1].shape else {
                    panic!("expected range")
                };
                *lo_feather = 42.0 / 200.0;
                *hi_feather = 42.0 / 200.0;
            }
            _ => expected.point_colors[0].hue_shift = 42.0,
        }
        assert_eq!(settings(&h, A), expected, "{delete}: a fresh draft commits after the successful retry");
        assert_eq!(h.app.session.undo.len(), undo + 3);
        assert!(h.app.session.unsaved().is_none());
    }
}
