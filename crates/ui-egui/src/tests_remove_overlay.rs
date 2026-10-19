//! Normal native scene routes for saved overlay state, editable shortcuts and hidden pins.
use std::time::Duration;

use serde_json::json;

use crate::headless::Headless;
use crate::state::{RightPanel, ToolOverlay};
use crate::{LightcraftApp, Services, UiState};

const T: Duration = Duration::from_secs(20);

fn detail() -> Headless {
    let mut h = Headless::new(LightcraftApp::new(lightcraft_engine::Session::with_demo(), Services::default()), [1200.0, 800.0], 1.0);
    h.app.run("panel.remove", json!({})).unwrap();
    assert!(h.settle(Duration::from_secs(120)), "native demo detail did not settle");
    h
}

fn key(h: &mut Headless, key: &str, shift: bool) {
    let r = h.request("ui.key", json!({"key": key, "shift": shift}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
}

fn click(h: &mut Headless, x: f64, y: f64) {
    let r = h.request("ui.pointer", json!({"events": [{"kind": "down", "x": x, "y": y}, {"kind": "up", "x": x, "y": y}]}), T);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
}

#[test]
fn remove_overlay_saved_state_commands_and_keymap() {
    let old: UiState = serde_json::from_value(json!({"removeSize": 0.03, "maskPins": false})).unwrap();
    assert_eq!(old.remove_overlay, ToolOverlay::Always);
    assert_eq!(old.remove_size, 0.03);
    assert!(!old.mask_pins);
    let mut h = detail();
    for mode in ToolOverlay::ALL {
        let r = h.request("ui.set", json!({"removeOverlay": mode}), T);
        assert_eq!(r["ok"], true, "{r}");
        let saved = serde_json::to_value(&h.app.ui).unwrap();
        assert_eq!(serde_json::from_value::<UiState>(saved).unwrap().remove_overlay, mode);
        assert_eq!(h.app.run("view.removeOverlay", json!({})).unwrap()["removeOverlay"], json!(mode.next()));
    }
    h.app.run("view.removeOverlay", json!({"mode": "auto"})).unwrap();
    for mode in [ToolOverlay::Always, ToolOverlay::Selected, ToolOverlay::Never, ToolOverlay::Auto] {
        key(&mut h, "H", true);
        assert_eq!(h.app.ui.remove_overlay, mode);
        assert_eq!(h.app.ui.right, RightPanel::Remove);
    }
    let before = h.app.ui.remove_overlay;
    for bad in [json!("unknown"), json!(null), json!(true)] {
        assert!(h.app.run("view.removeOverlay", json!({"mode": bad})).is_err());
        assert_eq!(h.app.ui.remove_overlay, before);
    }
    h.app.run("app.setShortcut", json!({"id": "view.removeOverlay", "shortcut": "Shift+K"})).unwrap();
    key(&mut h, "H", true);
    assert_eq!(h.app.ui.remove_overlay, before);
    key(&mut h, "K", true);
    assert_eq!(h.app.ui.remove_overlay, before.next());
    h.app.run("app.setShortcut", json!({"id": "view.removeOverlay", "shortcut": null})).unwrap();
    key(&mut h, "K", true);
    assert_eq!(h.app.ui.remove_overlay, before.next());
    h.app.run("panel.edit", json!({})).unwrap();
    key(&mut h, "H", false);
    assert_eq!(h.app.ui.right, RightPanel::Remove, "the saved H binding keeps opening Remove");
}

#[test]
fn remove_overlay_hidden_pins_do_not_intercept_native_scene_strokes() {
    for tool in ["remove", "heal", "clone"] {
        let mut h = detail();
        h.app.ui.tool = tool.into();
        for points in [json!([[0.25, 0.55]]), json!([[0.7, 0.25]])] {
            h.app.run("spot.add", json!({"mode": tool, "points": points, "size": 0.02, "source": [0.1, 0.0]})).unwrap();
        }
        h.app.run("spot.select", json!({"index": 1})).unwrap();
        h.app.run("view.removeOverlay", json!({"mode": "selected"})).unwrap();
        click(&mut h, 0.25, 0.55);
        let id = h.app.session.active().unwrap();
        assert_eq!(h.app.session.develop_of(id).unwrap().spots.len(), 3, "{tool}: paint over an unselected hidden pin");
        h.app.run("spot.select", json!({"index": 1})).unwrap();
        h.app.run("view.removeOverlay", json!({"mode": "never"})).unwrap();
        click(&mut h, 0.7, 0.25);
        assert_eq!(h.app.session.develop_of(id).unwrap().spots.len(), 4, "{tool}: paint over the hidden selected target");
        h.app.run("view.removeOverlay", json!({"mode": "always"})).unwrap();
        click(&mut h, 0.25, 0.55);
        assert_eq!(h.app.session.develop_of(id).unwrap().spots.len(), 4);
        assert_eq!(h.app.session.active_spot, Some(0), "{tool}: visible pins remain selectable");
    }
}

#[test]
fn remove_overlay_dropdown_dispatches_all_modes() {
    let mut h = detail();
    for mode in ToolOverlay::ALL {
        let r = h.request("ui.clickWidget", json!({"id": "dropdown:removeOverlay"}), T);
        assert_eq!(r["ok"], true, "{r}");
        h.step();
        let id = format!("removeOverlay:{}", mode.label().to_ascii_lowercase());
        let r = h.request("ui.clickWidget", json!({"id": id}), T);
        assert_eq!(r["ok"], true, "{r}");
        h.step();
        assert_eq!(h.app.ui.remove_overlay, mode);
    }
}

#[test]
fn remove_overlay_auto_hides_during_native_scene_target_drag() {
    use crate::panels::detail::Gesture;
    use egui::{Event, Modifiers, PointerButton};
    let mut h = detail();
    for points in [json!([[0.25, 0.55]]), json!([[0.7, 0.25]])] {
        h.app.run("spot.add", json!({"mode": "heal", "points": points, "size": 0.02, "source": [0.1, 0.0]})).unwrap();
    }
    assert!(h.settle(Duration::from_secs(120)), "native spots did not settle");
    let target = h.app.widgets.iter().find(|(id, _)| id == "spotPin:0").unwrap().1.center();
    h.app.run("view.removeOverlay", json!({"mode": "auto"})).unwrap();
    h.app.synthetic.push(Event::PointerGone);
    h.step();
    let pins = |h: &Headless| h.app.widgets.iter().filter(|(id, _)| id.starts_with("spotPin:") || id.starts_with("spotSource:")).count();
    assert_eq!(pins(&h), 0);
    h.app.synthetic.push(Event::PointerMoved(target));
    h.step();
    assert_eq!(pins(&h), 3);
    h.app.synthetic.push(Event::PointerButton { pos: target, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE });
    h.step();
    h.app.synthetic.push(Event::PointerMoved(target + egui::vec2(24.0, 8.0)));
    h.step();
    assert!(matches!(h.app.gesture, Some(Gesture::SpotMove { spot: 0, source: false })));
    assert_eq!(pins(&h), 0, "Auto hides both targets and the selected source during a drag");
    h.app.synthetic.push(Event::PointerButton {
        pos: target + egui::vec2(24.0, 8.0),
        button: PointerButton::Primary,
        pressed: false,
        modifiers: Modifiers::NONE,
    });
    h.step();
    assert!(h.app.gesture.is_none());
    h.step();
    assert_eq!(pins(&h), 3, "pins return over the image after the drag");
}
