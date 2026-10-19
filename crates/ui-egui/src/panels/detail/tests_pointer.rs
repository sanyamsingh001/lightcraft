//! Real egui event frames through the canvas press helper used by mask_overlay.
use super::mask_pointer_press_origin;
use crate::headless::HeadlessView;
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, Sense, pos2, vec2};

#[derive(Default)]
struct Frame {
    origin: Option<Pos2>,
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
