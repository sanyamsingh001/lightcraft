//! Healthy input through the full Headless app UI, including several events in one frame.
use super::CanvasMap;
use crate::headless::{Headless, HeadlessView};
use crate::state::{RightPanel, ViewMode};
use egui::{Event, Modifiers, PointerButton, Pos2, vec2};
use lightcraft_catalog::{Op, Photo, PhotoId, Source};
use lightcraft_develop::DevelopSettings;
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
        Ok(CanvasMap::new(
            &lightcraft_pipeline::geometry::Frame::new(photo.width.max(1) as usize, photo.height.max(1) as usize, &photo.develop, true),
            rect,
        ))
    }

    fn widget(&self, id: &str) -> Result<Pos2, String> {
        self.h.app.widgets.iter().find(|(key, _)| key == id).map(|(_, rect)| rect.center()).ok_or_else(|| format!("missing {id}"))
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
fn headless_remove_target_and_source_apply_the_terminal_delta() -> Result<(), String> {
    for source in [false, true] {
        for batched in [false, true] {
            let mut f = AppFrames::new(RightPanel::Remove)?;
            f.h.app.ui.tool = "heal".into();
            f.h.app.run("spot.add", json!({"mode": "heal", "points": [[0.3, 0.5]], "size": 0.015, "source": [0.15, 0.0]}))?;
            f.frame(vec![]);
            let a = f.widget(if source { "spotSource:0" } else { "spotPin:0" })?;
            let (b, c, hover) = (a + vec2(20.0, 0.0), a + vec2(40.0, 0.0), a + vec2(80.0, 0.0));
            let before = f.settings()?;
            let map = f.map()?;
            let delta = map.norm(c).x - map.norm(a).x;
            let undo = f.h.app.session.undo.len();
            f.drag(a, b, c, hover, batched);
            let after = f.settings()?;
            assert_eq!(after.spots.len(), 1);
            if source {
                assert_eq!(after.spots[0].points, before.spots[0].points);
                assert!((after.spots[0].source_offset.unwrap().x - before.spots[0].source_offset.unwrap().x - delta).abs() < 1e-6);
            } else {
                assert!((after.spots[0].points[0].x - before.spots[0].points[0].x - delta).abs() < 1e-6);
                assert_eq!(after.spots[0].source_offset, before.spots[0].source_offset);
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
