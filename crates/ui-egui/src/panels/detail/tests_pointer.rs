//! Real egui event frames through the canvas press helper used by mask_overlay.
use super::{brush_pointer_pos, completed_brush_drag, mask_pointer_press_origin};
use crate::headless::HeadlessView;
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, Sense, pos2, vec2};

#[derive(Default)]
struct Frame {
    origin: Option<Pos2>,
    sample: Option<Pos2>,
    interact: Option<Pos2>,
    completed: Option<(Pos2, Pos2)>,
    stopped: bool,
    clicked: bool,
    pressed: bool,
    released: bool,
    down_on: bool,
}

struct Harness {
    view: HeadlessView,
    time: f64,
}

impl Harness {
    fn new() -> Self {
        let mut h = Self { view: HeadlessView::new(), time: 0.0 };
        for _ in 0..3 {
            h.frame(vec![]);
        }
        h
    }

    fn frame(&mut self, events: Vec<Event>) -> Frame {
        self.time += 0.02;
        let mut frame = Frame::default();
        self.view.run(HeadlessView::raw_input(vec2(330.0, 200.0), 1.0, self.time, events), |ui| {
            let canvas = Rect::from_min_size(pos2(20.0, 20.0), vec2(240.0, 140.0));
            let resp = ui.interact(canvas, egui::Id::new("loupe"), Sense::click_and_drag());
            frame.origin = mask_pointer_press_origin(&resp);
            frame.completed = completed_brush_drag(&resp);
            frame.sample = brush_pointer_pos(&resp, frame.completed);
            frame.interact = resp.interact_pointer_pos();
            frame.stopped = resp.drag_stopped();
            frame.clicked = resp.clicked();
            frame.down_on = resp.is_pointer_button_down_on();
            (frame.pressed, frame.released) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_released()));
        });
        frame
    }
}

fn button(pos: Pos2, button: PointerButton, pressed: bool) -> Event {
    Event::PointerButton { pos, button, pressed, modifiers: Modifiers::NONE }
}

#[test]
fn rapid_primary_pin_click_captures_its_owner_in_the_release_frame() {
    let mut h = Harness::new();
    let pin = pos2(100.0, 80.0);
    h.frame(vec![Event::PointerMoved(pin)]);
    let frame = h.frame(vec![button(pin, PointerButton::Primary, true), button(pin, PointerButton::Primary, false)]);
    assert!(frame.clicked && frame.pressed && frame.released);
    assert!(!frame.down_on, "a released quick click does not satisfy the old capture gate");
    assert_eq!(frame.origin, Some(pin), "release-time pin selection needs the owner of this batched press");
    assert_eq!(h.frame(vec![]).origin, None);
}

#[test]
fn split_frame_click_keeps_the_pressed_pin_until_release() {
    let mut h = Harness::new();
    let pin = pos2(100.0, 80.0);
    h.frame(vec![Event::PointerMoved(pin)]);
    let press = h.frame(vec![button(pin, PointerButton::Primary, true)]);
    assert_eq!(press.origin, Some(pin));
    assert!(press.down_on && !press.clicked);
    assert_eq!(h.frame(vec![]).origin, None, "a held pointer must not recapture a pin");
    let release_at = pin + vec2(2.0, 0.0);
    let release = h.frame(vec![Event::PointerMoved(release_at), button(release_at, PointerButton::Primary, false)]);
    assert!(release.clicked && release.released);
    assert_eq!(release.origin, None, "release uses the captured owner even if another pin now overlaps it");
}

#[test]
fn dragging_and_release_cannot_recapture_a_mask_target() {
    let mut h = Harness::new();
    let pin = pos2(100.0, 80.0);
    h.frame(vec![Event::PointerMoved(pin)]);
    assert_eq!(h.frame(vec![button(pin, PointerButton::Primary, true)]).origin, Some(pin));
    let moved = pin + vec2(30.0, 0.0);
    assert_eq!(h.frame(vec![Event::PointerMoved(moved)]).origin, None);
    assert_eq!(h.frame(vec![button(moved, PointerButton::Primary, false)]).origin, None);
}

#[test]
fn secondary_click_and_primary_press_outside_the_canvas_have_no_mask_owner() {
    let mut h = Harness::new();
    let pin = pos2(100.0, 80.0);
    h.frame(vec![Event::PointerMoved(pin)]);
    assert_eq!(h.frame(vec![button(pin, PointerButton::Secondary, true), button(pin, PointerButton::Secondary, false)]).origin, None);
    let outside = pos2(300.0, 180.0);
    h.frame(vec![Event::PointerMoved(outside)]);
    let frame = h.frame(vec![button(outside, PointerButton::Primary, true), button(outside, PointerButton::Primary, false)]);
    assert!(frame.pressed && frame.released);
    assert!(!frame.clicked);
    assert_eq!(frame.origin, None, "only a press owned by the canvas may choose a mask");
}

#[test]
fn held_brush_release_then_hover_keeps_the_recorded_release_sample() {
    let mut h = Harness::new();
    let (a, b, c, hover) = (pos2(60.0, 80.0), pos2(100.0, 80.0), pos2(140.0, 80.0), pos2(200.0, 80.0));
    h.frame(vec![Event::PointerMoved(a)]);
    assert_eq!(h.frame(vec![button(a, PointerButton::Primary, true)]).origin, Some(a));
    assert_eq!(h.frame(vec![Event::PointerMoved(b)]).sample, Some(b));
    let release = h.frame(vec![Event::PointerMoved(c), button(c, PointerButton::Primary, false), Event::PointerMoved(hover)]);
    assert!(release.stopped && release.released);
    assert_eq!(release.interact, Some(hover), "egui's interaction position includes the later hover");
    assert_eq!(release.origin, None, "a held release retains the earlier press owner");
    assert_eq!(release.sample, Some(c));
}

#[test]
fn usual_held_brush_samples_are_unchanged() {
    let mut h = Harness::new();
    let (a, b) = (pos2(60.0, 80.0), pos2(100.0, 80.0));
    h.frame(vec![Event::PointerMoved(a)]);
    let press = h.frame(vec![button(a, PointerButton::Primary, true)]);
    assert_eq!(press.origin, Some(a));
    assert_eq!(press.sample, Some(a));
    assert_eq!(h.frame(vec![Event::PointerMoved(b)]).sample, Some(b));
    let release = h.frame(vec![button(b, PointerButton::Primary, false)]);
    assert!(release.stopped && release.released);
    assert_eq!(release.sample, Some(b));
}

#[test]
fn whole_frame_brush_release_then_hover_keeps_the_validated_event_pair() {
    let mut h = Harness::new();
    let (a, c, hover) = (pos2(60.0, 80.0), pos2(140.0, 80.0), pos2(200.0, 80.0));
    h.frame(vec![Event::PointerMoved(a)]);
    let release = h.frame(vec![
        button(a, PointerButton::Primary, true),
        Event::PointerMoved(c),
        button(c, PointerButton::Primary, false),
        Event::PointerMoved(hover),
    ]);
    assert_eq!(release.completed, Some((a, c)));
    assert_eq!(release.sample, Some(c));
}

// These checks use the actual overlay and image mapping, without rendering an application frame.
struct OverlayHarness {
    app: crate::LightcraftApp,
    view: HeadlessView,
    time: f64,
}
impl OverlayHarness {
    fn new(kind: &str) -> Self {
        let mut app = crate::LightcraftApp::new(lightcraft_engine::Session::with_demo(), crate::Services::default());
        app.ui.right = crate::state::RightPanel::Masking;
        app.ui.view = crate::state::ViewMode::Detail;
        app.run("mask.add", serde_json::json!({"kind": kind})).unwrap();
        app.ui.tool = kind.into();
        app.ui.brush_size = 0.0025;
        let mut h = Self { app, view: HeadlessView::new(), time: 0.0 };
        for _ in 0..3 {
            h.frame(vec![]);
        }
        h
    }
    fn frame(&mut self, events: Vec<Event>) {
        self.time += 0.02;
        let app = &mut self.app;
        self.view.run(HeadlessView::raw_input(vec2(400.0, 400.0), 1.0, self.time, events), |ui| {
            super::sync_mask_tools(app, ui.ctx());
            let (rect, resp) = ui.allocate_exact_size(ui.available_size(), Sense::click_and_drag());
            app.canvas_rect = Some(rect);
            let d = app.session.develop_of(app.session.active().unwrap()).unwrap();
            let frame = lightcraft_pipeline::geometry::Frame::new(400, 400, &d, true);
            let map = super::CanvasMap::new(&frame, rect);
            if !super::interrupted_mask_pointer(ui.ctx()) {
                super::mask_overlay(app, ui, &resp, &map, &d);
            }
            if ui.input(|i| i.pointer.primary_released()) {
                ui.data_mut(|d| d.remove::<super::MaskPointerTarget>(super::mask_pointer_target_id()));
            }
        });
        crate::widgets::take_registry(&self.view.ctx);
    }
    fn down(&mut self, at: Pos2) {
        self.frame(vec![Event::PointerMoved(at)]);
        self.frame(vec![button(at, PointerButton::Primary, true)]);
    }
    fn points(&self) -> Vec<lightcraft_geom::Point> {
        let d = self.app.session.develop_of(self.app.session.active().unwrap()).unwrap();
        let lightcraft_develop::MaskShape::Brush { strokes } = &d.masks[0].components[0].shape else { panic!("brush") };
        strokes.last().unwrap().points.clone()
    }
}

#[test]
fn brush_overlay_preserves_press_and_release_without_duplicate_dabs() {
    use lightcraft_geom::Point;
    for moved_release in [false, true] {
        let mut h = OverlayHarness::new("brush");
        let (a, b, c) = (pos2(100.0, 200.0), pos2(160.0, 200.0), pos2(if moved_release { 220.0 } else { 160.0 }, 200.0));
        h.down(a);
        h.frame(vec![Event::PointerMoved(b)]);
        h.frame(vec![Event::PointerMoved(c), button(c, PointerButton::Primary, false)]);
        let points = h.points();
        assert!(points.first().unwrap().dist(Point::new(0.25, 0.5)) < 1e-6);
        assert!(points.last().unwrap().dist(Point::new(c.x as f64 / 400.0, 0.5)) < 1e-6);
        assert_eq!(points.len(), if moved_release { 3 } else { 2 });
    }
    let mut h = OverlayHarness::new("brush");
    let p = pos2(100.0, 200.0);
    h.down(p);
    h.frame(vec![button(p, PointerButton::Primary, false)]);
    assert_eq!(h.points().len(), 1, "a click remains one dab");
}

#[test]
fn held_brush_release_then_hover_stores_only_the_pressed_stroke_endpoints() {
    use lightcraft_geom::Point;
    let mut h = OverlayHarness::new("brush");
    let (a, b, c, hover) = (pos2(100.0, 200.0), pos2(160.0, 200.0), pos2(220.0, 200.0), pos2(300.0, 200.0));
    h.down(a);
    h.frame(vec![Event::PointerMoved(b)]);
    h.frame(vec![Event::PointerMoved(c), button(c, PointerButton::Primary, false), Event::PointerMoved(hover)]);
    let points = h.points();
    assert_eq!(points.len(), 3);
    for (actual, expected) in points.iter().zip([Point::new(0.25, 0.5), Point::new(0.4, 0.5), Point::new(0.55, 0.5)]) {
        assert!(actual.dist(expected) < 1e-6);
    }
    assert!(h.app.gesture.is_none());
    h.frame(vec![Event::PointerMoved(pos2(340.0, 200.0))]);
    assert_eq!(h.points(), points, "unpressed movement does not extend the committed stroke");
}

#[test]
fn old_mask_handle_cannot_update_or_end_same_photo_replacement_interaction() {
    use serde_json::json;
    for switch_tool in [false, true] {
        let mut h = OverlayHarness::new("radial");
        h.down(pos2(280.0, 200.0));
        h.frame(vec![Event::PointerMoved(pos2(300.0, 200.0))]);
        assert!(matches!(h.app.gesture, Some(super::Gesture::MaskHandle { .. })));
        h.app.run("develop.beginInteraction", json!({"label": "Exposure"})).unwrap();
        h.app.run("develop.set", json!({"control": "light.exposure", "value": 0.5})).unwrap();
        let identity = h.app.session.interaction.as_ref().unwrap().identity();
        let before = h.app.session.develop_of(h.app.session.active().unwrap()).unwrap();
        if switch_tool {
            h.app.ui.tool = "brush".into();
        }
        h.frame(vec![Event::PointerMoved(pos2(320.0, 200.0))]);
        assert!(h.app.gesture.is_none());
        h.frame(vec![button(pos2(320.0, 200.0), PointerButton::Primary, false)]);
        assert!(std::sync::Arc::ptr_eq(&identity, &h.app.session.interaction.as_ref().unwrap().identity()));
        assert_eq!(h.app.session.develop_of(h.app.session.active().unwrap()).unwrap(), before);
        h.app.run("develop.endInteraction", json!({})).unwrap();
        h.app.run("edit.undo", json!({})).unwrap();
        assert_eq!(h.app.session.develop_of(h.app.session.active().unwrap()).unwrap().light.exposure, 0.0);
    }
}

#[test]
fn equal_mask_ids_in_another_library_retire_pressed_brush_and_picker_owners() {
    use lightcraft_catalog::MemStore;
    use lightcraft_engine::library::LibraryStores;
    use serde_json::json;
    for kind in ["brush", "colorRange"] {
        let mut h = OverlayHarness::new(kind);
        let photo = h.app.session.active().unwrap();
        let mask = h.app.session.active_mask.unwrap();
        if kind == "brush" {
            h.down(pos2(100.0, 200.0));
            h.frame(vec![Event::PointerMoved(pos2(160.0, 200.0))]);
        } else {
            h.app.mask_color_picker = super::super::masking::ColorPicker::capture(&h.app, 0);
        }
        h.app
            .session
            .open_library_in(
                LibraryStores {
                    dir: "mask-new-library".into(),
                    catalog: Box::new(MemStore::new()),
                    files: Box::new(MemStore::new()),
                    on_disk: false,
                },
                true,
            )
            .unwrap();
        h.app.run("mask.add", json!({"kind": kind})).unwrap();
        assert_eq!(h.app.session.active(), Some(photo));
        assert_eq!(h.app.session.active_mask, Some(mask));
        let before = h.app.session.develop_of(photo).unwrap();
        let undo = h.app.session.undo.len();
        h.frame(vec![]);
        h.frame(vec![button(pos2(220.0, 200.0), PointerButton::Primary, false)]);
        assert!(h.app.gesture.is_none());
        assert!(h.app.mask_color_picker.is_none());
        assert_eq!(h.app.session.develop_of(photo).unwrap(), before);
        assert_eq!(h.app.session.undo.len(), undo);
    }
}

#[test]
fn a_brush_press_move_release_in_one_frame_keeps_both_endpoints() {
    use lightcraft_geom::Point;
    let mut h = OverlayHarness::new("brush");
    let (a, b) = (pos2(100.0, 200.0), pos2(160.0, 200.0));
    h.frame(vec![Event::PointerMoved(a)]);
    h.frame(vec![button(a, PointerButton::Primary, true), Event::PointerMoved(b), button(b, PointerButton::Primary, false)]);
    let points = h.points();
    assert!(points.first().unwrap().dist(Point::new(0.25, 0.5)) < 1e-6);
    assert!(points.last().unwrap().dist(Point::new(0.4, 0.5)) < 1e-6);
    assert_eq!(points.len(), 2);
}

#[test]
fn a_batched_brush_press_outside_the_canvas_cannot_paint_on_release() {
    let mut h = OverlayHarness::new("brush");
    let (a, b) = (pos2(-10.0, 200.0), pos2(160.0, 200.0));
    h.frame(vec![Event::PointerMoved(a)]);
    h.frame(vec![button(a, PointerButton::Primary, true), Event::PointerMoved(b), button(b, PointerButton::Primary, false)]);
    let d = h.app.session.develop_of(h.app.session.active().unwrap()).unwrap();
    let lightcraft_develop::MaskShape::Brush { strokes } = &d.masks[0].components[0].shape else { panic!("brush") };
    assert!(strokes.is_empty());
}
