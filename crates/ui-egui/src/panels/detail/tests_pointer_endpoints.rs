//! Healthy input through the full Headless app UI, including several events in one frame.
use super::{CanvasMap, photo_frame};
use crate::headless::{Headless, HeadlessView};
use crate::state::{RightPanel, ViewMode};
use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use lightcraft_catalog::{Op, Photo, PhotoId, Source};
use lightcraft_develop::{DevelopSettings, MaskShape};
use lightcraft_geom::Point;
use serde_json::json;
use std::sync::Arc;

const PHOTO: PhotoId = PhotoId(1);

struct AppFrames {
    h: Headless,
    time: f64,
}

impl AppFrames {
    fn new(panel: RightPanel) -> Result<Self, String> {
        let mut session = lightcraft_engine::Session::new();
        session
            .catalog
            .apply(Op::AddPhoto { photo: Box::new(Photo::new(PHOTO, Source::Demo { scene: 0 }, "pointer.jpg", "JPEG", 80, 60, "2026-10-09")) })
            .map_err(|e| e.to_string())?;
        session.execute("library.select", &json!({"ids": [PHOTO.0]})).map_err(|e| e.to_string())?;
        let mut app = crate::LightcraftApp::new(session, crate::Services::default());
        app.ui.view = ViewMode::Detail;
        app.ui.right = panel;
        app.ui.left_panel = false;
        let mut f = Self { h: Headless::new(app, [1200.0, 1000.0], 1.0), time: 0.0 };
        for _ in 0..3 {
            f.frame(vec![]);
        }
        Ok(f)
    }

    fn frame(&mut self, events: Vec<Event>) {
        // Headless::step serializes control-channel synthetic input. Feed a native RawInput
        // batch through the same logic + ui path to cover actual coalesced viewport events.
        self.time += 0.02;
        let mut raw = HeadlessView::raw_input(self.h.size, self.h.pixels_per_point, self.time, events);
        self.h.app.raw_input_hook(&mut raw);
        let app = &mut self.h.app;
        self.h.view.run(raw, |ui| {
            app.logic(ui.ctx());
            app.ui(ui);
        });
    }

    fn settings(&self) -> Result<Arc<DevelopSettings>, String> {
        self.h.app.session.develop_of(PHOTO).ok_or_else(|| "missing fixture settings".into())
    }

    fn map(&self) -> Result<CanvasMap, String> {
        let photo = self.h.app.session.catalog.photo(PHOTO).ok_or_else(|| "missing fixture photo".to_owned())?;
        let rect = self.h.app.image_rect.ok_or_else(|| "missing image rectangle".to_owned())?;
        Ok(CanvasMap::new(&photo_frame(photo, true), rect))
    }

    fn widget(&self, id: &str) -> Result<Pos2, String> {
        self.h.app.widgets.iter().find(|(key, _)| key == id).map(|(_, rect)| rect.center()).ok_or_else(|| format!("missing {id}"))
    }

    fn add_mask(&mut self, kind: &str) -> Result<u32, String> {
        self.h.app.run("mask.add", json!({"kind": kind, "center": [0.5, 0.5], "rx": 0.13, "ry": 0.09, "start": [0.3, 0.4], "end": [0.7, 0.6]}))?;
        self.frame(vec![]);
        self.h.app.session.active_mask.ok_or_else(|| "missing mask".into())
    }

    fn click(&mut self, at: Pos2, hover: Pos2, batched: bool) {
        self.frame(vec![Event::PointerMoved(at)]);
        if batched {
            self.frame(vec![button(at, true), button(at, false), Event::PointerMoved(hover)]);
        } else {
            self.frame(vec![button(at, true)]);
            self.frame(vec![button(at, false), Event::PointerMoved(hover)]);
        }
    }

    fn drag(&mut self, a: Pos2, b: Pos2, c: Pos2, hover: Pos2, batched: bool) {
        self.frame(vec![Event::PointerMoved(a)]);
        if batched {
            self.frame(vec![button(a, true), Event::PointerMoved(b), button(c, false), Event::PointerMoved(hover)]);
        } else {
            self.frame(vec![button(a, true)]);
            self.frame(vec![Event::PointerMoved(b)]);
            self.frame(vec![button(c, false), Event::PointerMoved(hover)]);
        }
        assert!(self.h.app.gesture.is_none());
        assert!(self.h.app.session.interaction.is_none());
    }
}

fn button(pos: Pos2, pressed: bool) -> Event {
    Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
}

#[test]
fn headless_stationary_brush_click_keeps_one_dab_at_the_press() -> Result<(), String> {
    for batched in [false, true] {
        let mut f = AppFrames::new(RightPanel::Masking)?;
        f.add_mask("brush")?;
        f.h.app.ui.tool = "brush".into();
        let map = f.map()?;
        let a = map.screen(Point::new(0.25, 0.55));
        let hover = map.screen(Point::new(0.8, 0.55));
        f.click(a, hover, batched);
        let d = f.settings()?;
        let MaskShape::Brush { strokes } = &d.masks[0].components[0].shape else { panic!("brush") };
        assert_eq!(strokes.len(), 1);
        assert_eq!(strokes[0].points.len(), 1);
        assert!(strokes[0].points[0].dist(map.norm(a)) < 1e-6);
        f.frame(vec![Event::PointerMoved(hover + vec2(10.0, 0.0))]);
        assert_eq!(f.settings()?, d);
    }
    Ok(())
}

#[test]
fn headless_batched_pin_click_selects_the_pressed_pin_before_later_hover() -> Result<(), String> {
    for batched in [false, true] {
        let mut f = AppFrames::new(RightPanel::Masking)?;
        let first = f.add_mask("radial")?;
        let shape = MaskShape::Radial { center: Point::new(0.25, 0.5), rx: 0.1, ry: 0.1, angle: 0.0, feather: 50.0, invert: false };
        f.h.app.run("mask.update", json!({"id": first, "shape": shape}))?;
        let second = f.add_mask("radial")?;
        f.frame(vec![]);
        let a = f.widget(&format!("maskPin:{first}:0"))?;
        let hover = f.widget(&format!("maskPin:{second}:0"))?;
        let before = f.settings()?;
        f.click(a, hover, batched);
        assert_eq!(f.h.app.session.active_mask, Some(first));
        assert_eq!(f.settings()?, before);
    }
    Ok(())
}

#[test]
fn headless_mask_pins_and_handles_apply_the_recorded_terminal_movement() -> Result<(), String> {
    for (kind, handle) in [("radial", None), ("radial", Some(1)), ("linear", None), ("linear", Some(1))] {
        for batched in [false, true] {
            let mut f = AppFrames::new(RightPanel::Masking)?;
            let mask = f.add_mask(kind)?;
            let id = handle.map_or_else(|| format!("maskPin:{mask}:0"), |h| format!("maskHandle:{mask}:0:{h}"));
            let a = f.widget(&id)?;
            let (b, c, hover) = (a + vec2(20.0, 0.0), a + vec2(40.0, 0.0), a + vec2(80.0, 0.0));
            let before = f.settings()?;
            let map = f.map()?;
            let delta = map.norm(c).x - map.norm(a).x;
            let undo = f.h.app.session.undo.len();
            f.drag(a, b, c, hover, batched);
            let after = f.settings()?;
            match (&before.masks[0].components[0].shape, &after.masks[0].components[0].shape) {
                (MaskShape::Radial { center: old, rx: old_rx, .. }, MaskShape::Radial { center, rx, .. }) => {
                    if handle.is_none() {
                        assert!((center.x - old.x - delta).abs() < 1e-6);
                        assert_eq!(rx, old_rx);
                    } else {
                        assert!((rx - old_rx - delta).abs() < 1e-6);
                        assert_eq!(center, old);
                    }
                }
                (MaskShape::Linear { start: old_start, end: old_end }, MaskShape::Linear { start, end }) => {
                    if handle.is_none() {
                        assert!((start.x - old_start.x - delta).abs() < 1e-6);
                        assert!((end.x - old_end.x - delta).abs() < 1e-6);
                    } else {
                        assert_eq!(start, old_start);
                        assert!(end.dist(map.norm(c)) < 1e-6);
                    }
                }
                _ => panic!("wrong mask shape"),
            }
            assert_eq!(f.h.app.session.undo.len(), undo + 1);
            f.frame(vec![Event::PointerMoved(hover + vec2(10.0, 0.0))]);
            assert_eq!(f.settings()?, after);
            f.h.app.run("edit.undo", json!({}))?;
            assert_eq!(f.settings()?, before);
        }
    }
    Ok(())
}
