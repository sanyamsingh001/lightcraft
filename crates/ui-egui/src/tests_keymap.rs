//! Headless tests of the editable keymap (Help ▸ Keyboard Shortcuts): a changed shortcut fires
//! its command and not the old one's, the editor records a key press without running it, Esc
//! cancels recording without closing the dialog, and the keymap survives a save/load of `ui.json`.

use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::state::{Dialog, ViewMode};
use crate::{LightcraftApp, Services};

const T: Duration = Duration::from_secs(20);
const SETTLE: Duration = Duration::from_secs(120);

fn demo() -> Headless {
    let services = Services { png: None, ..Default::default() };
    let app = LightcraftApp::new(lightcraft_engine::Session::with_demo(), services);
    let mut h = Headless::new(app, [1400.0, 900.0], 1.0);
    let r = h.request("ui.set", json!({"view": "detail"}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.settle(SETTLE);
    h
}

fn key(h: &mut Headless, k: &str, shift: bool) {
    let r = h.request("ui.key", json!({"key": k, "shift": shift}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
}

fn click(h: &mut Headless, id: &str) {
    let r = h.request("ui.clickWidget", json!({"id": id}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
}

#[test]
fn rebound_shortcut_fires_and_the_old_key_does_not() {
    let mut h = demo();
    let r = h.app.run("app.setShortcut", json!({"id": "view.survey", "shortcut": "Shift+K"})).unwrap();
    assert_eq!(r["shortcut"], "Shift+K", "{r}");
    // the old key (N) no longer opens Survey…
    key(&mut h, "N", false);
    assert_eq!(h.app.ui.view, ViewMode::Detail);
    // …the new one does, and menus show it
    key(&mut h, "K", true);
    assert_eq!(h.app.ui.view, ViewMode::Survey);
    let entry = crate::menus::menu_entries(&h.app).into_iter().find(|e| e.id == "view.survey").unwrap();
    assert_eq!(entry.shortcut.as_deref(), Some("Shift+K"));
    // the keymap is saved with the UI state
    let saved = serde_json::to_string(&h.app.ui).unwrap();
    let back = serde_json::from_str::<crate::UiState>(&saved).unwrap();
    assert_eq!(back.settings.keymap.get("view.survey").map(String::as_str), Some("Shift+K"));
}

#[test]
fn the_editor_records_a_key_and_esc_cancels_without_closing() {
    let mut h = demo();
    h.app.run("app.shortcuts", json!({})).unwrap();
    h.step();
    h.step();
    // the list is longer than the dialog: narrow it so the row is on screen
    click(&mut h, "field:shortcutsSearch");
    assert_eq!(h.request("ui.text", json!({"text": "compare"}), T)["ok"], true);
    h.step();
    h.step();
    // record Shift+J for Compare: the key press is taken, not run
    click(&mut h, "button:shortcut-view.compare");
    assert_eq!(h.app.recording_shortcut.as_deref(), Some("view.compare"));
    key(&mut h, "J", true);
    assert_eq!(h.app.recording_shortcut, None);
    assert_eq!(crate::shortcuts::shortcut_of(&h.app.ui.settings.keymap, "view.compare"), Some("Shift+J"));
    assert_eq!(h.app.ui.view, ViewMode::Detail);
    assert!(matches!(h.app.ui.dialog, Some(Dialog::Shortcuts)));
    // Esc while recording cancels; the dialog stays open and nothing changes
    click(&mut h, "button:shortcut-view.compare");
    key(&mut h, "Escape", false);
    assert_eq!(h.app.recording_shortcut, None);
    assert!(matches!(h.app.ui.dialog, Some(Dialog::Shortcuts)));
    assert_eq!(crate::shortcuts::shortcut_of(&h.app.ui.settings.keymap, "view.compare"), Some("Shift+J"));
    // the next Esc closes it; the new key works
    key(&mut h, "Escape", false);
    assert!(h.app.ui.dialog.is_none());
    key(&mut h, "J", true);
    assert_eq!(h.app.ui.view, ViewMode::Compare);
    // Reset All restores the declared keys
    h.app.run("app.resetShortcuts", json!({})).unwrap();
    assert_eq!(crate::shortcuts::shortcut_of(&h.app.ui.settings.keymap, "view.compare"), Some("Shift+C"));
}

#[test]
fn a_key_given_to_a_command_overrides_a_fixed_key() {
    let mut h = demo();
    let before = h.app.session.active().and_then(|id| h.app.session.catalog.photo(id)).map(|p| p.rating);
    // 3 (rate ★★★) now opens Survey instead
    h.app.run("app.setShortcut", json!({"id": "view.survey", "shortcut": "3"})).unwrap();
    key(&mut h, "3", false);
    assert_eq!(h.app.ui.view, ViewMode::Survey);
    let after = h.app.session.active().and_then(|id| h.app.session.catalog.photo(id)).map(|p| p.rating);
    assert_eq!(before, after, "the rating key must not fire too");
}

/// Open the editor, filtered to `search`, and start recording for `id`.
fn record(h: &mut Headless, search: &str, id: &str) {
    if h.app.ui.dialog.is_none() {
        h.app.run("app.shortcuts", json!({})).unwrap();
        h.step();
        h.step();
        click(h, "field:shortcutsSearch");
        assert_eq!(h.request("ui.text", json!({"text": search}), T)["ok"], true);
        h.step();
        h.step();
    }
    click(h, &format!("button:shortcut-{id}"));
    assert_eq!(h.app.recording_shortcut.as_deref(), Some(id));
}

/// Pressing ⌘ (a key event of its own) before S used to record `Cmd+SuperLeft`, so no ⌘ shortcut
/// could be set; ⌘C/⌘X/⌘V arrive as copy/cut/paste events, not keys, and were ignored.
#[test]
fn modifiers_wait_for_their_key_and_clipboard_keys_record() {
    let mut h = demo();
    record(&mut h, "survey", "view.survey");
    let r = h.request("ui.key", json!({"key": "SuperLeft", "cmd": true}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert_eq!(h.app.recording_shortcut.as_deref(), Some("view.survey"), "still waiting after ⌘ alone");
    let r = h.request("ui.key", json!({"key": "K", "cmd": true, "shift": true}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    h.step();
    assert_eq!(h.app.recording_shortcut, None);
    assert_eq!(crate::shortcuts::shortcut_of(&h.app.ui.settings.keymap, "view.survey"), Some("Cmd+Shift+K"));
    // ⌘⌥C: the windowing layer sends a Copy event with the modifiers held
    record(&mut h, "survey", "view.survey");
    let m = egui::Modifiers { alt: true, ..egui::Modifiers::COMMAND };
    h.app.synthetic.push(egui::Event::Key { key: egui::Key::AltLeft, physical_key: None, pressed: true, repeat: false, modifiers: m });
    h.app.synthetic.push(egui::Event::Copy);
    h.step();
    h.step();
    assert_eq!(crate::shortcuts::shortcut_of(&h.app.ui.settings.keymap, "view.survey"), Some("Cmd+Alt+C"));
    // a saved modifier-only shortcut (from the bug) means no shortcut
    assert_eq!(crate::shortcuts::parse("Cmd+SuperLeft"), None);
}

/// On Windows and Linux the windowing layer turns Ctrl+C / Ctrl+V into clipboard events, not key
/// presses: outside a text field they still copy the edit settings, and with ⇧ open Paste Selected
/// Settings.
#[test]
fn clipboard_events_run_the_copy_and_paste_shortcuts() {
    let mut h = demo();
    assert!(h.app.session.clipboard.is_none());
    h.events.push(egui::Event::Copy);
    h.settle(SETTLE);
    assert!(h.app.session.clipboard.is_some(), "Ctrl+C copied the edit settings");
    let shift_cmd = egui::Modifiers::SHIFT | egui::Modifiers::COMMAND;
    h.events.extend([egui::Event::ModifiersChanged(shift_cmd), egui::Event::Paste("text".into())]);
    h.settle(SETTLE);
    assert!(matches!(h.app.ui.dialog, Some(Dialog::PasteSettings { .. })), "⇧Ctrl+V: {:?}", h.app.ui.dialog);
}

/// Windows also pastes with ⇧Insert and cuts with ⇧Delete: the clipboard events they become are
/// the plain Paste / Cut, not ⇧Ctrl+V (Paste Selected Settings).
#[test]
fn shift_insert_pastes_plainly() {
    let mut h = demo();
    h.events.push(egui::Event::Copy);
    h.settle(SETTLE);
    let undo = h.app.session.undo.len();
    h.events.extend([egui::Event::ModifiersChanged(egui::Modifiers::SHIFT), egui::Event::Paste("text".into())]);
    h.settle(SETTLE);
    assert_eq!(h.app.ui.dialog, None, "no Paste Selected Settings");
    assert_eq!(h.app.session.undo.len(), undo + 1, "the settings were pasted");
}
/// The opening release can carry both physical shortcut keys and the replacement text.
#[test]
fn opening_numeric_release_keeps_keys_in_the_editor() {
    use egui::{Event, Key, Modifiers, PointerButton};

    for terminal in [Key::Enter, Key::Escape] {
        let mut h = demo();
        h.app.run("photo.rate", json!({"rating": 0})).unwrap();
        h.app.run("develop.set", json!({"values": {"light.exposure": 0.7}})).unwrap();
        h.app.run("app.setShortcut", json!({"id": "photo.increaseRating", "shortcut": "Enter"})).unwrap();
        let r = h.request("ui.set", json!({"view": "detail", "right": "edit", "openSections": ["light"], "histogram": false}), T);
        assert_eq!(r["ok"], true, "{r}");
        assert!(h.settle(SETTLE));
        assert!(h.app.fonts_ready);
        assert!(h.app.recording_shortcut.is_none() && h.app.ui.name_edit.is_none());
        assert!(h.app.native_shortcuts.is_empty() && h.app.synthetic.is_empty() && h.events.is_empty());
        assert!(!h.view.ctx.egui_wants_keyboard_input());
        assert_eq!(crate::shortcuts::shortcut_of(&h.app.ui.settings.keymap, "photo.increaseRating"), Some("Enter"));
        let active = h.app.session.active().unwrap();
        let selection = h.app.session.selection.ids.clone();
        let (view, right, tool) = (h.app.ui.view, h.app.ui.right, h.app.ui.tool.clone());
        assert!(h.app.ui.dialog.is_none());
        assert_eq!(h.app.session.catalog.photo(active).unwrap().rating, 0);
        assert!((h.app.session.develop_of(active).unwrap().light.exposure - 0.7).abs() < 1e-9);
        let (undo, redo) = (h.app.session.undo.len(), h.app.session.redo.len());
        let pos = h.app.widgets.iter().rev().find(|(id, _)| id == "sliderValue:light.exposure").unwrap().1.center();
        h.events.push(Event::PointerMoved(pos));
        h.step();
        h.events.push(Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
        h.step();
        assert!(h.view.ctx.egui_wants_keyboard_input(), "the press focuses the editor before app shortcuts run");
        h.events.extend([
            Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE },
            Event::Key { key: Key::Num2, physical_key: Some(Key::Num2), pressed: true, repeat: false, modifiers: Modifiers::NONE },
            Event::Text("2".into()),
            Event::Key { key: terminal, physical_key: Some(terminal), pressed: true, repeat: false, modifiers: Modifiers::NONE },
        ]);
        h.step();
        // Observe the consuming frame directly, then exactly one empty frame: no request drains.
        for next_frame in [false, true] {
            if next_frame {
                assert!(h.events.is_empty() && h.app.synthetic.is_empty());
                h.step();
            }
            let expected = if terminal == Key::Enter { 2.0 } else { 0.7 };
            assert!((h.app.session.develop_of(active).unwrap().light.exposure - expected).abs() < 1e-9, "{terminal:?}, next={next_frame}");
            assert_eq!(h.app.session.undo.len(), undo + usize::from(terminal == Key::Enter), "{terminal:?}, next={next_frame}");
            assert_eq!(h.app.session.redo.len(), redo);
            assert_eq!(h.app.session.catalog.photo(active).unwrap().rating, 0, "neither Num2 nor the saved Enter shortcut leaked");
            assert_eq!(h.app.session.active(), Some(active));
            assert_eq!(h.app.session.selection.ids, selection);
            assert_eq!((h.app.ui.view, h.app.ui.right, &h.app.ui.tool), (view, right, &tool));
            assert!(h.app.ui.dialog.is_none());
            assert!(!h.view.ctx.egui_wants_keyboard_input());
            if next_frame {
                assert!(h.app.widgets.iter().all(|(id, _)| id != "sliderField:light.exposure"), "the editor is gone");
                assert!(h.app.widgets.iter().any(|(id, _)| id == "sliderValue:light.exposure"));
            }
        }
    }
}

fn select_none_ui_selection(h: &mut Headless, view: &str) -> Vec<lightcraft_catalog::PhotoId> {
    let ids: Vec<_> = h.app.session.visible_cloned().into_iter().take(2).collect();
    assert_eq!(ids.len(), 2);
    h.app.run("library.select", json!({"ids": ids, "active": ids[1]})).unwrap();
    let r = h.request("ui.set", json!({"view": view, "right": "edit", "filmstrip": true, "language": "en"}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert!(h.settle(SETTLE));
    assert_eq!(h.app.session.selection.ids, ids);
    assert!(h.app.ui.dialog.is_none() && !h.view.ctx.egui_wants_keyboard_input());
    ids
}

fn assert_select_none_ui(h: &mut Headless, view: &str, ids: &[lightcraft_catalog::PhotoId]) {
    assert!(h.settle(SETTLE));
    for _ in 0..3 {
        h.step();
        assert_eq!(h.app.session.selection, lightcraft_engine::Selection::default(), "{view}");
        assert!(h.app.session.targets(&serde_json::Value::Null).is_empty());
        for id in ids {
            assert_eq!(h.app.session.selection.state_of(*id), lightcraft_engine::SelectionState::NotSelected);
        }
    }
    let r = h.request("ui.inspect", json!({}), T);
    assert_eq!(r["ok"], true, "{r}");
    assert_eq!(r["result"]["selection"], json!([]), "{r}");
    assert!(r["result"]["active"].is_null(), "{r}");
    if view == "detail" {
        assert!(
            h.view.painted_shapes().iter().any(|shape| { matches!(shape, egui::Shape::Text(text) if text.galley.job.text == "No photo selected") })
        );
        assert!(
            h.view
                .painted_shapes()
                .iter()
                .any(|shape| { matches!(shape, egui::Shape::Text(text) if text.galley.job.text == "Choose a photo in the grid or filmstrip") })
        );
    }
    // Grid and filmstrip still let the user choose a photo after clearing every selection.
    let prefix = if view == "detail" { "film" } else { "thumb" };
    click(h, &format!("{prefix}:{}", ids[0].0));
    assert!(h.settle(SETTLE));
    assert_eq!(h.app.session.selection, lightcraft_engine::Selection::single(ids[0]));
}

/// Click the rendered menu item: ui.menu.invoke is an alias for app.run, not a menu click.
#[test]
fn select_none_menu_clears_grid_and_detail_selection() {
    let mut h = demo();
    for view in ["photoGrid", "detail"] {
        let ids = select_none_ui_selection(&mut h, view);
        click(&mut h, "menu:Edit");
        assert!(h.settle(SETTLE));
        // Menu items are not registered widgets; use the label actually painted by egui.
        let at = h
            .view
            .painted_shapes()
            .iter()
            .find_map(|shape| match shape {
                egui::Shape::Text(text) if text.galley.job.text.trim() == "Deselect All" => Some(text.visual_bounding_rect().center()),
                _ => None,
            })
            .expect("Edit menu contains Deselect All");
        let r = h.request("ui.click", json!({"x": at.x, "y": at.y}), T);
        assert_eq!(r["ok"], true, "{r}");
        assert_select_none_ui(&mut h, view, &ids);
    }
}

/// Primary Cmd+Shift+A and secondary Cmd+D take the normal headless key dispatch path.
#[test]
fn select_none_primary_and_alias_clear_grid_and_detail_selection() {
    let mut h = demo();
    for view in ["photoGrid", "detail"] {
        for (key, shift) in [("A", true), ("D", false)] {
            let ids = select_none_ui_selection(&mut h, view);
            let r = h.request("ui.key", json!({"key": key, "cmd": true, "shift": shift}), T);
            assert_eq!(r["ok"], true, "{r}");
            assert_select_none_ui(&mut h, view, &ids);
        }
    }
}

// Inspect painted canvas geometry, not the UI flag: an interior line spanning the image
// in each direction identifies the transform grid without depending on its color or spacing.
fn geometry_drag_grid_counts(h: &Headless) -> (usize, usize) {
    let image = h.app.image_rect.expect("setup: Detail has an image rectangle");
    assert!(image.is_positive());
    assert!(h.app.canvas_rect.expect("setup: Detail canvas exists").contains_rect(image));
    let spans = |a: f32, b: f32, lo: f32, hi: f32| (a.min(b) - lo).abs() < 1.0 && (a.max(b) - hi).abs() < 1.0;
    let mut vertical = 0;
    let mut horizontal = 0;
    for shape in h.view.painted_shapes() {
        let egui::Shape::LineSegment { points: [a, b], .. } = shape else { continue };
        if (a.x - b.x).abs() < 1.0 && a.x > image.left() + 1.0 && a.x < image.right() - 1.0 && spans(a.y, b.y, image.top(), image.bottom()) {
            vertical += 1;
        }
        if (a.y - b.y).abs() < 1.0 && a.y > image.top() + 1.0 && a.y < image.bottom() - 1.0 && spans(a.x, b.x, image.left(), image.right()) {
            horizontal += 1;
        }
    }
    (vertical, horizontal)
}

/// A real slider drag can lose its widget when Deselect removes the active photo.
#[test]
fn geometry_drag_cleanup_after_deselect() {
    // Three fixed cases, not a Cartesian matrix. Menu means the real menu dispatch function;
    // the original select_none_menu fixture separately covers clicking the painted menu item.
    for (route, hide_after_release) in [("primary", true), ("alias", false), ("menu", true)] {
        let mut h = demo();
        h.size = egui::vec2(1400.0, 1200.0);
        let r = h.request("ui.set", json!({"view": "detail", "right": "crop", "filmstrip": true, "cropOverlay": "none", "language": "en"}), T);
        assert_eq!(r["ok"], true, "setup: {r}");
        let photo = h.app.session.active().expect("setup: demo photo selected");
        h.app.run("library.select", json!({"ids": [photo.0], "active": photo.0})).unwrap();
        assert!(h.settle(SETTLE));
        let panel = h.app.widgets.iter().find(|(id, _)| id == "panel:right_panel").expect("setup: right panel").1;
        let track = h.app.widgets.iter().find(|(id, _)| id == "slider:geometry.vertical").expect("setup: Geometry slider").1;
        assert!(track.is_positive() && panel.contains_rect(track), "setup: actual slider must be visible: {track:?} in {panel:?}");
        assert!(track.top() > panel.top() + 40.0, "setup: slider must be below dock chrome");
        assert_eq!(geometry_drag_grid_counts(&h), (0, 0), "setup: no transform grid before dragging");
        let original = h.app.session.develop_of(photo).unwrap();
        let undo = h.app.session.undo.len();
        let start = track.center();
        let held_at = egui::pos2(track.left() + track.width() * 0.65, track.center().y);
        h.app.synthetic.extend([
            egui::Event::PointerMoved(start),
            egui::Event::PointerButton { pos: start, button: egui::PointerButton::Primary, pressed: true, modifiers: egui::Modifiers::NONE },
            egui::Event::PointerMoved(held_at),
        ]);
        // The existing raw_input_hook consumes each pointer event on its own frame.
        for _ in 0..4 {
            h.step();
        }
        assert!(h.settle(SETTLE));
        assert!(h.view.ctx.input(|i| i.pointer.primary_down()), "setup: real pointer still held");
        assert!(h.app.session.interaction.is_some(), "setup: widget began an engine interaction");
        assert_eq!(h.app.ui.dragging_control.as_deref(), Some("geometry.vertical"));
        let held = h.app.session.develop_of(photo).unwrap();
        assert_ne!(lightcraft_develop::controls::get(&held, "geometry.vertical"), lightcraft_develop::controls::get(&original, "geometry.vertical"));
        assert_eq!(h.app.session.undo.len(), undo, "setup: held drag has no commit yet");
        let held_grid = geometry_drag_grid_counts(&h);
        assert!(held_grid.0 >= 2 && held_grid.1 >= 2, "setup: real drag paints both grid directions: {held_grid:?}");
        assert_eq!(h.app.ui.right, crate::state::RightPanel::Crop);
        match route {
            "primary" | "alias" => {
                let (key, shift) = if route == "primary" { ("A", true) } else { ("D", false) };
                let r = h.request("ui.key", json!({"key": key, "cmd": true, "shift": shift}), T);
                assert_eq!(r["ok"], true, "{route}: {r}");
            }
            _ => {
                crate::menubar::run_item(&mut h.app, "library.selectNone", json!({})).unwrap();
            }
        }
        h.step();
        let empty_marker = h.app.ui.dragging_control.clone();
        assert_eq!(h.app.ui.right, crate::state::RightPanel::Crop);
        assert!(h.app.widgets.iter().any(|(id, _)| id == "panel:right_panel"));
        assert!(!h.app.widgets.iter().any(|(id, _)| id == "slider:geometry.vertical"));
        assert_eq!(h.app.session.selection, lightcraft_engine::Selection::default());
        assert!(h.app.session.interaction.is_none());
        assert!(h.view.ctx.input(|i| i.pointer.primary_down()));
        assert!(h.painted_text().iter().any(|text| text == "Choose a photo in the grid or filmstrip"));
        assert_eq!(h.app.session.develop_of(photo).unwrap(), held, "Deselect commits, not cancels, the live edit");
        assert_eq!(h.app.session.undo.len(), undo + 1);
        h.app.synthetic.push(egui::Event::PointerButton {
            pos: held_at,
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        for _ in 0..3 {
            h.step();
        }
        let released_marker = h.app.ui.dragging_control.clone();
        assert!(!h.view.ctx.input(|i| i.pointer.primary_down()));
        assert_eq!(h.app.session.selection, lightcraft_engine::Selection::default());
        assert_eq!(h.app.ui.right, crate::state::RightPanel::Crop);
        assert!(h.app.widgets.iter().any(|(id, _)| id == "panel:right_panel"));
        assert!(!h.app.widgets.iter().any(|(id, _)| id == "slider:geometry.vertical"));
        if hide_after_release {
            // Close only after the empty-selection release, so panel hiding cannot cause it.
            // Keep the slider absent on reselection to exclude delayed slider-stop cleanup.
            // Do not use ui.set: it reconstructs UiState and resets skipped transient fields.
            h.app.run("panel.close", json!({})).unwrap();
            h.step();
            assert_eq!(h.app.ui.right, crate::state::RightPanel::None);
            assert!(!h.app.widgets.iter().any(|(id, _)| id == "slider:geometry.vertical"));
            assert_eq!(h.app.session.selection, lightcraft_engine::Selection::default());
            assert!(h.app.session.interaction.is_none());
            assert_eq!(h.app.ui.dragging_control, released_marker);
        }
        click(&mut h, &format!("film:{}", photo.0));
        assert!(h.settle(SETTLE));
        assert_eq!(h.app.session.selection, lightcraft_engine::Selection::single(photo));
        assert!(h.app.session.interaction.is_none());
        assert!(!h.view.ctx.input(|i| i.pointer.primary_down()));
        assert_eq!(h.app.session.develop_of(photo).unwrap(), held);
        assert_eq!(h.app.session.undo.len(), undo + 1, "release and reselection do not recommit");
        assert_eq!(h.app.ui.right, if hide_after_release { crate::state::RightPanel::None } else { crate::state::RightPanel::Crop });
        let restored_grid = geometry_drag_grid_counts(&h);
        eprintln!(
            "geometry-deselect {route}, hide_after_release={hide_after_release}: empty={empty_marker:?}, released={released_marker:?}, restored={:?}, held_grid={held_grid:?}, restored_grid={restored_grid:?}",
            h.app.ui.dragging_control
        );
        // Take every observation before the semantic assertion, so a BEFORE failure reports
        // empty-selection, released and reselected states plus the actual painted geometry.
        assert!(
            empty_marker.is_none() && released_marker.is_none() && h.app.ui.dragging_control.is_none() && restored_grid == (0, 0),
            "Deselect must retire Geometry drag UI: {route}, hide_after_release={hide_after_release}, empty={empty_marker:?}, released={released_marker:?}, restored={:?}, painted={restored_grid:?}",
            h.app.ui.dragging_control
        );
    }
}
