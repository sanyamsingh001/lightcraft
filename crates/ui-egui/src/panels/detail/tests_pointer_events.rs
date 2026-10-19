//! Actual egui responses used by the production endpoint helpers; no application facade.
use super::{brush_pointer_pos, completed_brush_drag, mask_pointer_press_origin};
use egui::{Context, Event, Modifiers, PointerButton, Pos2, RawInput, Rect, Sense, pos2, vec2};

#[derive(Default)]
struct Samples {
    press: Option<Pos2>,
    terminal: Option<Pos2>,
    pair: Option<(Pos2, Pos2)>,
    stopped: bool,
    clicked: bool,
}

struct EventFrames {
    ctx: Context,
    time: f64,
}

impl EventFrames {
    fn new() -> Self {
        let mut f = Self { ctx: Context::default(), time: 0.0 };
        for _ in 0..3 {
            f.frame(vec![]);
        }
        f
    }

    fn frame(&mut self, events: Vec<Event>) -> Samples {
        self.time += 0.02;
        let mut samples = Samples::default();
        let mut out = self.ctx.run_ui(
            RawInput { screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(330.0, 200.0))), time: Some(self.time), events, ..Default::default() },
            |ui| {
                let canvas = Rect::from_min_size(pos2(20.0, 20.0), vec2(240.0, 140.0));
                let resp = ui.interact(canvas, egui::Id::new("pointer-endpoint-canvas"), Sense::click_and_drag());
                samples.press = mask_pointer_press_origin(&resp);
                samples.pair = completed_brush_drag(&resp);
                samples.terminal = brush_pointer_pos(&resp, samples.pair);
                samples.clicked = resp.clicked();
                samples.stopped = resp.drag_stopped();
            },
        );
        out.textures_delta.clear();
        samples
    }
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
}

#[test]
fn batched_stationary_click_excludes_later_hover_from_both_endpoints() {
    let mut f = EventFrames::new();
    let (a, hover) = (pos2(80.0, 80.0), pos2(230.0, 80.0));
    f.frame(vec![Event::PointerMoved(a)]);
    let samples = f.frame(vec![button(a, true), button(a, false), Event::PointerMoved(hover)]);
    assert!(samples.clicked);
    assert_eq!(samples.press, Some(a));
    assert_eq!(samples.terminal, Some(a));
    assert_eq!(samples.pair, None);
}

#[test]
fn split_click_release_does_not_recapture_a_new_owner_at_hover() {
    let mut f = EventFrames::new();
    let (a, c, hover) = (pos2(80.0, 80.0), pos2(81.0, 80.0), pos2(230.0, 80.0));
    f.frame(vec![Event::PointerMoved(a)]);
    assert_eq!(f.frame(vec![button(a, true)]).press, Some(a));
    let samples = f.frame(vec![button(c, false), Event::PointerMoved(hover)]);
    assert!(samples.clicked);
    assert_eq!(samples.press, None);
    assert_eq!(samples.terminal, Some(c));
}

#[test]
fn batched_moved_release_recovers_the_primary_pair_before_hover() {
    let mut f = EventFrames::new();
    let (a, c, hover) = (pos2(80.0, 80.0), pos2(140.0, 80.0), pos2(230.0, 80.0));
    f.frame(vec![Event::PointerMoved(a)]);
    let samples = f.frame(vec![button(a, true), Event::PointerMoved(c), button(c, false), Event::PointerMoved(hover)]);
    assert_eq!(samples.pair, Some((a, c)));
    assert_eq!(samples.terminal, Some(c));
}

#[test]
fn held_drag_stops_at_the_release_position_even_without_a_move_event() {
    let mut f = EventFrames::new();
    let (a, b, c, hover) = (pos2(80.0, 80.0), pos2(110.0, 80.0), pos2(140.0, 80.0), pos2(230.0, 80.0));
    f.frame(vec![Event::PointerMoved(a)]);
    f.frame(vec![button(a, true)]);
    f.frame(vec![Event::PointerMoved(b)]);
    let samples = f.frame(vec![button(c, false), Event::PointerMoved(hover)]);
    assert!(samples.stopped);
    assert_eq!(samples.press, None);
    assert_eq!(samples.terminal, Some(c));
}

#[test]
fn outside_and_secondary_batched_presses_cannot_claim_the_canvas() {
    for secondary in [false, true] {
        let mut f = EventFrames::new();
        let (a, c) = (pos2(if secondary { 80.0 } else { 5.0 }, 80.0), pos2(140.0, 80.0));
        let button_kind = if secondary { PointerButton::Secondary } else { PointerButton::Primary };
        f.frame(vec![Event::PointerMoved(a)]);
        let samples = f.frame(vec![
            Event::PointerButton { pos: a, button: button_kind, pressed: true, modifiers: Modifiers::NONE },
            Event::PointerMoved(c),
            Event::PointerButton { pos: c, button: button_kind, pressed: false, modifiers: Modifiers::NONE },
        ]);
        assert_eq!(samples.press, None);
        assert_eq!(samples.pair, None);
    }
}
