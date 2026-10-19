//! Real egui pointer and keyboard frames on the production slider readout.
use super::{SliderOut, slider, take_registry};
use crate::headless::HeadlessView;
use egui::{Event, Key, Modifiers, PointerButton, Rect, pos2, vec2};
use lightcraft_develop::{ControlSpec, controls};

struct Harness {
    view: HeadlessView,
    app: Option<crate::LightcraftApp>,
    spec: ControlSpec,
    value: f64,
    enabled: bool,
    time: f64,
    rect: Rect,
}
impl Harness {
    fn new() -> Self {
        let mut h = Self {
            view: HeadlessView::new(),
            app: None,
            spec: *controls::find("light.exposure").unwrap(),
            value: 0.71,
            enabled: true,
            time: 0.0,
            rect: Rect::NOTHING,
        };
        for _ in 0..3 {
            h.frame(vec![]);
        }
        h
    }
    fn frame(&mut self, events: Vec<Event>) -> SliderOut {
        self.time += 0.02;
        let mut out = SliderOut::default();
        self.view.run(HeadlessView::raw_input(vec2(330.0, 100.0), 1.0, self.time, events), |ui| {
            if let Some(app) = self.app.as_ref() {
                super::begin_slider_edit_frame(ui.ctx());
                super::sync_slider_edit_target(app, ui.ctx());
            }
            out = slider(ui, &self.spec, self.value, self.enabled, None);
            if self.app.is_some() {
                super::finish_slider_edit_frame(ui.ctx());
            }
        });
        self.rect = take_registry(&self.view.ctx).into_iter().find(|(id, _)| id == &format!("sliderValue:{}", self.spec.id)).unwrap().1;
        if let Some(v) = out.value {
            self.value = v;
        }
        out
    }
    fn click(&mut self, text: Option<&str>) {
        let pos = self.rect.center();
        self.frame(vec![Event::PointerMoved(pos)]);
        self.frame(vec![Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
        let mut events = vec![Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }];
        if let Some(text) = text {
            events.push(Event::Text(text.into()));
        }
        assert!(self.frame(events).value.is_none());
    }
    fn key(&mut self, key: Key) -> SliderOut {
        self.frame(vec![Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers: Modifiers::NONE }])
    }
}
#[test]
fn opening_click_retains_focus_and_first_text_replaces_the_number() {
    let mut h = Harness::new();
    let rect = h.rect;
    h.click(None);
    assert!(h.view.ctx.egui_wants_keyboard_input(), "opening release owns the keyboard immediately");
    h.frame(vec![Event::Text("1,5".into())]);
    assert_eq!(h.key(Key::Enter).value, Some(1.5));
    assert_eq!(h.rect, rect);
}
#[test]
fn opening_release_and_text_in_the_same_frame_replace_the_number() {
    let mut h = Harness::new();
    h.click(Some("1.5"));
    assert!(h.view.ctx.egui_wants_keyboard_input());
    let out = h.key(Key::Enter);
    assert_eq!(out.value, Some(1.5), "Text in the release frame replaces the selected value");
    assert!(out.drag_started && out.drag_stopped, "one command interaction");
}
#[test]
fn escape_and_disabled_controls_discard_drafts() {
    let mut h = Harness::new();
    h.click(Some("2"));
    assert!(h.key(Key::Escape).value.is_none());
    assert_eq!(h.value, 0.71);
    h.click(Some("3"));
    h.enabled = false;
    assert!(h.frame(vec![]).value.is_none());
    assert!(!h.view.ctx.egui_wants_keyboard_input());
    h.enabled = true;
    assert!(h.key(Key::Enter).value.is_none());
}
#[test]
fn an_external_value_change_cancels_the_draft_before_enter() {
    let mut h = Harness::new();
    h.click(Some("2"));
    h.value = 0.8;
    assert!(h.key(Key::Enter).value.is_none());
    assert!(!h.view.ctx.egui_wants_keyboard_input());
    assert_eq!(h.value, 0.8);
}
#[test]
fn clicking_away_still_applies_upstream_stepped_values() {
    let mut h = Harness::new();
    h.click(Some("1.234"));
    let pos = pos2(8.0, 80.0);
    h.frame(vec![Event::PointerMoved(pos)]);
    h.frame(vec![Event::PointerButton { pos, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::NONE }]);
    h.frame(vec![Event::PointerButton { pos, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE }]);
    assert_eq!(h.value, 1.23);
}
#[test]
fn accepting_untouched_text_keeps_the_original_precision() {
    let mut h = Harness::new();
    h.value = 0.7123456789;
    h.frame(vec![]);
    h.click(None);
    assert!(h.key(Key::Enter).value.is_none());
    assert_eq!(h.value, 0.7123456789);
}

#[test]
fn deletion_of_an_identical_indexed_item_still_discards_its_draft() {
    use serde_json::json;
    for eyes in [true, false] {
        let mut h = Harness::new();
        let mut app = crate::LightcraftApp::new(lightcraft_engine::Session::with_demo(), crate::Services::default());
        if eyes {
            for _ in 0..2 {
                app.session.execute("redeye.add", &json!({"center": [0.5, 0.5], "rx": 0.02, "ry": 0.015, "pupilSize": 50.0})).unwrap();
            }
            app.ui.right = crate::state::RightPanel::RedEye;
            h.spec.id = "redEye.pupilSize";
            h.value = 50.0;
        } else {
            let sample = lightcraft_develop::PointColor { hue: 45.0, ..Default::default() };
            app.session.execute("develop.merge", &json!({"settings": {"point_colors": [sample, sample]}})).unwrap();
            app.ui.right = crate::state::RightPanel::Edit;
            h.spec.id = "pointColor.hueShift";
            h.value = 0.0;
        }
        app.ui.eye = 0;
        app.ui.point_color = 0;
        h.spec.min = 0.0;
        h.spec.max = 100.0;
        h.app = Some(app);
        h.frame(vec![]);
        h.click(Some("41"));
        let app = h.app.as_mut().unwrap();
        app.session.execute(if eyes { "redeye.delete" } else { "pointColor.delete" }, &json!({"index": 0})).unwrap();
        let photo = app.session.active().unwrap();
        let before_enter = app.session.develop_of(photo).unwrap();
        let undo = app.session.undo.len();
        assert!(h.key(Key::Enter).value.is_none(), "an identical item at the reused index cannot inherit the draft");
        assert!(!h.view.ctx.egui_wants_keyboard_input());
        let app = h.app.as_ref().unwrap();
        assert_eq!(app.session.develop_of(photo).unwrap(), before_enter);
        assert_eq!(app.session.undo.len(), undo);
    }
}
