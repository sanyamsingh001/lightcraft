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

#[test]
fn headless_colour_picker_samples_the_click_endpoint_before_later_hover() -> Result<(), String> {
    for batched in [false, true] {
        let mut f = AppFrames::new(RightPanel::Masking)?;
        let mask = f.add_mask("colorRange")?;
        let map = f.map()?;
        let a = map.screen(Point::new(0.2, 0.55));
        let c = a + vec2(1.0, 0.0); // a healthy click with a distinct release coordinate
        let hover = map.screen(Point::new(0.8, 0.55));
        let src = f.h.app.session.source_now(PHOTO, lightcraft_engine::media::SourceLevel::Thumb)?;
        let info = f.h.app.session.source_info(PHOTO);
        let d = f.settings()?;
        let req = lightcraft_pipeline::RenderRequest::fit(384, 384);
        let expected = lightcraft_pipeline::color_range_sample(&src, &info, &d, &req, map.norm(c)).unwrap();
        let wrong = lightcraft_pipeline::color_range_sample(&src, &info, &d, &req, map.norm(hover)).unwrap();
        assert_ne!(expected, wrong, "fixture must distinguish endpoint and hover colours");
        f.h.app.mask_color_picker = super::super::masking::ColorPicker::capture(&f.h.app, 0);
        f.h.app.ui.tool = "colorRange".into();
        f.frame(vec![Event::PointerMoved(a)]);
        if !batched {
            f.frame(vec![button(a, true)]);
        }
        let mut events = if batched { vec![button(a, true)] } else { vec![] };
        events.extend([button(c, false), Event::PointerMoved(hover)]);
        f.frame(events);
        let sampled = f.settings()?;
        let MaskShape::ColorRange { samples, .. } = &sampled.masks[0].components[0].shape else { panic!("colour range") };
        assert_eq!(samples, &[expected]);
        assert_eq!(f.h.app.session.active_mask, Some(mask));
        assert!(f.h.app.mask_color_picker.is_some(), "own sample refreshes the picker");
        f.frame(vec![Event::PointerMoved(hover + vec2(10.0, 0.0))]);
        assert_eq!(f.settings()?, sampled);
    }
    Ok(())
}

#[test]
fn headless_colour_picker_latches_append_at_press_through_release_and_hover() -> Result<(), String> {
    for panel in [RightPanel::Masking, RightPanel::Edit] {
        for batched in [false, true] {
            for (press_shift, release_shift, later_shift) in [(true, false, false), (false, true, true), (true, true, false), (false, false, true)] {
                let mut f = AppFrames::new(panel)?;
                let mask = f.add_mask("colorRange")?;
                f.h.app.run("mask.sampleColor", json!({"id": mask, "component": 0, "x": 0.8, "y": 0.55}))?;
                f.frame(vec![]);
                let map = f.map()?;
                let a = map.screen(Point::new(0.2, 0.55));
                let c = a + vec2(1.0, 0.0);
                let hover = map.screen(Point::new(0.8, 0.55));
                let before = f.settings()?;
                let MaskShape::ColorRange { samples: original, .. } = &before.masks[0].components[0].shape else { panic!("colour range") };
                assert_eq!(original.len(), 1);
                let src = f.h.app.session.source_now(PHOTO, lightcraft_engine::media::SourceLevel::Thumb)?;
                let info = f.h.app.session.source_info(PHOTO);
                let req = lightcraft_pipeline::RenderRequest::fit(384, 384);
                let expected = lightcraft_pipeline::color_range_sample(&src, &info, &before, &req, map.norm(c)).unwrap();
                assert_ne!(original[0], expected, "fixture distinguishes old and new colours");
                f.h.app.mask_color_picker = super::super::masking::ColorPicker::capture(&f.h.app, 0);
                f.h.app.ui.tool = "colorRange".into();
                f.frame(vec![Event::PointerMoved(a)]);
                let mods = |shift| if shift { Modifiers::SHIFT } else { Modifiers::NONE };
                let press = Event::PointerButton { pos: a, button: PointerButton::Primary, pressed: true, modifiers: mods(press_shift) };
                if !batched {
                    // Change Shift after press but before drawing, so both later frames and
                    // same-frame modifier changes must leave the button's intent intact.
                    f.frame(vec![Event::ModifiersChanged(mods(press_shift)), press.clone(), Event::ModifiersChanged(mods(release_shift))]);
                }
                let mut events = if batched { vec![Event::ModifiersChanged(mods(press_shift)), press] } else { vec![] };
                events.extend([
                    Event::ModifiersChanged(mods(release_shift)),
                    Event::PointerButton { pos: c, button: PointerButton::Primary, pressed: false, modifiers: mods(release_shift) },
                    Event::ModifiersChanged(mods(later_shift)),
                    Event::PointerMoved(hover),
                ]);
                let undo_before = f.h.app.session.undo.len();
                f.frame(events);
                let sampled = f.settings()?;
                let MaskShape::ColorRange { samples, .. } = &sampled.masks[0].components[0].shape else { panic!("colour range") };
                let mut wanted = if press_shift { original.clone() } else { vec![] };
                wanted.push(expected);
                assert_eq!(samples, &wanted, "{panel:?} batched={batched} press={press_shift} release={release_shift} later={later_shift}");
                assert_eq!(f.h.app.session.undo.len(), undo_before + 1, "one sample edit");
                let pick = f.h.app.mask_color_picker.as_ref().unwrap();
                assert!(pick.is_current(&f.h.app), "successful sample refreshes the armed snapshot");
                assert_eq!(pick.append_on_press, None, "the completed click is consumed");
                f.frame(vec![Event::PointerMoved(hover + vec2(10.0, 0.0)), Event::ModifiersChanged(mods(!later_shift))]);
                assert_eq!(f.settings()?, sampled, "unpressed hover cannot sample twice");
            }
        }
    }
    Ok(())
}

#[test]
fn headless_colour_picker_fresh_press_replaces_previous_append_intent() -> Result<(), String> {
    let mut f = AppFrames::new(RightPanel::Masking)?;
    f.add_mask("colorRange")?;
    f.h.app.mask_color_picker = super::super::masking::ColorPicker::capture(&f.h.app, 0);
    f.h.app.ui.tool = "colorRange".into();
    let map = f.map()?;
    let a = map.screen(Point::new(0.2, 0.55));
    let b = map.screen(Point::new(0.8, 0.55));
    let undo_before = f.h.app.session.undo.len();
    f.frame(vec![Event::PointerMoved(a)]);
    f.frame(vec![
        Event::ModifiersChanged(Modifiers::SHIFT),
        Event::PointerButton { pos: a, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::SHIFT },
    ]);
    assert_eq!(f.h.app.mask_color_picker.as_ref().unwrap().append_on_press, Some(true));
    f.frame(vec![
        Event::PointerMoved(b),
        Event::PointerButton { pos: b, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE },
        Event::ModifiersChanged(Modifiers::NONE),
    ]);
    assert_eq!(f.h.app.session.undo.len(), undo_before, "a moved gesture is not a sample click");
    assert_eq!(f.h.app.mask_color_picker.as_ref().unwrap().append_on_press, None, "unclicked release consumes intent");
    f.click(a, b, true);
    let first = f.settings()?;
    let MaskShape::ColorRange { samples, .. } = &first.masks[0].components[0].shape else { panic!("colour range") };
    assert_eq!(samples.len(), 1, "fresh ordinary click replaces");
    f.frame(vec![Event::PointerMoved(b)]);
    f.frame(vec![Event::PointerButton { pos: b, button: PointerButton::Primary, pressed: true, modifiers: Modifiers::SHIFT }]);
    f.frame(vec![
        Event::ModifiersChanged(Modifiers::NONE),
        Event::PointerButton { pos: b, button: PointerButton::Primary, pressed: false, modifiers: Modifiers::NONE },
    ]);
    let second = f.settings()?;
    let MaskShape::ColorRange { samples, .. } = &second.masks[0].components[0].shape else { panic!("colour range") };
    assert_eq!(samples.len(), 2, "a fresh Shift press adds despite release modifiers");
    Ok(())
}
