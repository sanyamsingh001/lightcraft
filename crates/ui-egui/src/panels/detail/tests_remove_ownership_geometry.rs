//! Real egui frames and app commands around the production Remove overlay. Full-app wiring
//! checks below also exercise logic + ui; bounded extraction runs explicitly exclude that module.
use super::{CanvasMap, Gesture, remove_overlay, remove_pointer_target_id, sync_remove_tools};
use crate::state::{RightPanel, ViewMode};
use crate::{LightcraftApp, Services};
use egui::{Event, Modifiers, PointerButton, Pos2, Rect, Sense, pos2, vec2};
use lightcraft_catalog::{Op, Photo, PhotoId, Source};
use lightcraft_develop::{BrushStroke, DevelopSettings};
use lightcraft_geom::{Orientation, Point};
use lightcraft_pipeline::geometry::Frame;
use serde_json::json;
use std::sync::Arc;

const A: PhotoId = PhotoId(1);
const B: PhotoId = PhotoId(2);

fn app(tool: &str) -> LightcraftApp {
    let mut session = lightcraft_engine::Session::new();
    for id in [A, B] {
        session
            .catalog
            .apply(Op::AddPhoto { photo: Box::new(Photo::new(id, Source::Demo { scene: 0 }, "remove-owner.jpg", "JPEG", 80, 60, "2026-10-09")) })
            .unwrap();
    }
    session.execute("library.select", &json!({"ids": [A.0]})).unwrap();
    let mut app = LightcraftApp::new(session, Services::default());
    app.ui.right = RightPanel::Remove;
    app.ui.view = ViewMode::Detail;
    app.ui.tool = tool.into();
    app
}

fn button(at: Pos2, pressed: bool) -> Event {
    Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }
}

struct Frames {
    app: LightcraftApp,
    ctx: egui::Context,
    time: f64,
    focused: bool,
    cover: bool,
}

impl Frames {
    fn new(tool: &str) -> Self {
        let mut f = Self { app: app(tool), ctx: egui::Context::default(), time: 0.0, focused: true, cover: false };
        for _ in 0..3 {
            f.frame(vec![]);
        }
        f
    }

    fn frame(&mut self, events: Vec<Event>) -> egui::FullOutput {
        self.time += 0.02;
        let app = &mut self.app;
        let cover = self.cover;
        let mut output = self.ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(420.0, 340.0))),
                time: Some(self.time),
                focused: self.focused,
                events,
                ..Default::default()
            },
            |ui| {
                // This is the same cancellation helper called at the start of the whole UI.
                sync_remove_tools(app, ui.ctx());
                egui::CentralPanel::default().frame(egui::Frame::NONE).show(ui, |ui| {
                    let rect = Rect::from_min_size(pos2(20.0, 20.0), vec2(360.0, 270.0));
                    let resp = ui.interact(rect, egui::Id::new("loupe"), Sense::click_and_drag());
                    app.canvas_rect = Some(rect);
                    if let Some(id) = app.session.active() {
                        let d = app.session.develop_of(id).unwrap();
                        let frame = Frame::new(4000, 3000, &d, true);
                        let map = CanvasMap::new(&frame, rect);
                        remove_overlay(app, ui, &resp, &map, &d);
                    }
                });
                if cover {
                    egui::Area::new(egui::Id::new("remove-test-cover"))
                        .constrain(false)
                        .order(egui::Order::Foreground)
                        .fixed_pos(pos2(20.0, 20.0))
                        .show(ui.ctx(), |ui| {
                            ui.allocate_exact_size(vec2(360.0, 270.0), Sense::click_and_drag());
                        });
                }
            },
        );
        // These overlay frames inspect shapes, without a texture host or rasterization.
        output.textures_delta.clear();
        output
    }

    fn settings(&self, photo: PhotoId) -> Arc<DevelopSettings> {
        self.app.session.develop_of(photo).unwrap()
    }

    fn down(&mut self, at: Pos2) {
        self.frame(vec![Event::PointerMoved(at)]);
        self.frame(vec![button(at, true)]);
    }

    fn release(&mut self, at: Pos2) {
        self.frame(vec![button(at, false), Event::PointerMoved(at + vec2(20.0, 0.0))]);
        self.frame(vec![]);
        assert!(self.app.gesture.is_none());
    }

    fn spot(&mut self, photo: PhotoId) {
        self.app.run("library.select", json!({"ids": [photo.0]})).unwrap();
        self.app.run("spot.add", json!({"mode": "clone", "points": [[0.4, 0.5]], "size": 0.03, "source": [0.1, 0.0]})).unwrap();
        self.frame(vec![]);
    }
}

#[test]
fn remove_press_and_started_stroke_cannot_follow_photo_selection() {
    for tool in ["remove", "heal", "clone"] {
        for drag in [false, true] {
            let mut f = Frames::new(tool);
            let before_a = f.settings(A);
            let before_b = f.settings(B);
            f.down(pos2(100.0, 150.0));
            if drag {
                f.frame(vec![Event::PointerMoved(pos2(150.0, 150.0))]);
                assert!(matches!(f.app.gesture, Some(Gesture::Spot { .. })));
            }
            f.app.run("library.select", json!({"ids": [B.0]})).unwrap();
            let undo = f.app.session.undo.len();
            f.frame(vec![]);
            assert!(f.app.gesture.is_none());
            f.release(pos2(200.0, 150.0));
            assert_eq!(f.settings(A), before_a);
            assert_eq!(f.settings(B), before_b);
            assert_eq!(f.app.session.undo.len(), undo);
            // A fresh same-photo click must still work after the stale release.
            f.down(pos2(240.0, 150.0));
            f.release(pos2(240.0, 150.0));
            assert_eq!(f.settings(B).spots.len(), 1);
        }
    }
}

#[test]
fn valid_remove_strokes_keep_both_endpoints_and_ignore_later_hover() {
    for tool in ["remove", "heal", "clone"] {
        for batched in [false, true] {
            let mut f = Frames::new(tool);
            let (a, b) = (pos2(100.0, 150.0), pos2(180.0, 150.0));
            let undo = f.app.session.undo.len();
            f.frame(vec![Event::PointerMoved(a)]);
            if batched {
                f.frame(vec![button(a, true), Event::PointerMoved(b), button(b, false), Event::PointerMoved(pos2(300.0, 150.0))]);
            } else {
                f.down(a);
                f.frame(vec![Event::PointerMoved(pos2(140.0, 150.0))]);
                f.release(b);
            }
            let d = f.settings(A);
            assert_eq!(d.spots.len(), 1);
            let points = &d.spots[0].points;
            let map =
                CanvasMap::new(&Frame::new(4000, 3000, &DevelopSettings::default(), true), Rect::from_min_size(pos2(20.0, 20.0), vec2(360.0, 270.0)));
            assert!(points.first().unwrap().dist(map.norm(a)) < 1e-6);
            assert!(points.last().unwrap().dist(map.norm(b)) < 1e-6);
            assert_eq!(f.app.session.undo.len(), undo + 1);
            assert!(f.settings(B).spots.is_empty());
        }
    }
}

#[test]
fn remove_context_loss_retires_focus_settings_tool_panel_view_and_layer() {
    for loss in 0..7 {
        for started in [false, true] {
            let mut f = Frames::new("clone");
            f.down(pos2(100.0, 150.0));
            if started {
                f.frame(vec![Event::PointerMoved(pos2(150.0, 150.0))]);
            }
            match loss {
                0 => f.focused = false,
                1 => {
                    f.app.run("develop.set", json!({"control": "light.exposure", "value": 1.0})).unwrap();
                }
                2 => f.app.ui.tool = "heal".into(),
                3 => f.app.ui.right = RightPanel::Edit,
                4 => f.app.ui.fullscreen = true,
                5 => f.app.ui.view = ViewMode::PhotoGrid,
                _ => f.cover = true,
            }
            let before = f.settings(A);
            f.frame(vec![]);
            f.frame(vec![]);
            f.frame(vec![]); // allow the new Area to finish measuring before layer hit-testing
            assert!(f.app.gesture.is_none(), "loss {loss}, started {started}");
            f.focused = true;
            f.release(pos2(200.0, 150.0));
            assert_eq!(f.settings(A), before, "loss {loss}, started {started}");
            assert!(f.settings(B).spots.is_empty());
        }
    }
}

#[test]
fn remove_library_replacement_with_equal_photo_ids_retires_press() {
    use lightcraft_catalog::MemStore;
    use lightcraft_engine::library::LibraryStores;
    let mut f = Frames::new("clone");
    f.down(pos2(100.0, 150.0));
    f.app
        .session
        .open_library_in(
            LibraryStores {
                dir: "remove-new-memory-library".into(),
                catalog: Box::new(MemStore::new()),
                files: Box::new(MemStore::new()),
                on_disk: false,
            },
            false,
        )
        .unwrap();
    f.app
        .session
        .catalog
        .apply(Op::AddPhoto { photo: Box::new(Photo::new(A, Source::Demo { scene: 0 }, "replacement.jpg", "JPEG", 80, 60, "2026-10-09")) })
        .unwrap();
    f.app.run("library.select", json!({"ids": [A.0]})).unwrap();
    let before = f.settings(A);
    f.frame(vec![]);
    f.release(pos2(200.0, 150.0));
    assert_eq!(f.settings(A), before);
}

#[test]
fn pressed_spot_index_and_live_move_cannot_follow_selection_or_replacement() {
    for source in [false, true] {
        for started in [false, true] {
            let mut f = Frames::new("clone");
            f.spot(B);
            f.spot(A);
            let at = pos2(if source { 200.0 } else { 164.0 }, 155.0);
            f.down(at);
            if started {
                f.frame(vec![Event::PointerMoved(at + vec2(25.0, 0.0))]);
                assert!(matches!(f.app.gesture, Some(Gesture::SpotMove { .. })));
            }
            let a = f.settings(A);
            let b = f.settings(B);
            f.app.run("library.select", json!({"ids": [B.0]})).unwrap();
            f.frame(vec![]);
            f.release(at + vec2(60.0, 0.0));
            assert_eq!(f.settings(A), a);
            assert_eq!(f.settings(B), b);
        }
    }
    let mut f = Frames::new("clone");
    f.spot(A);
    f.down(pos2(164.0, 155.0));
    f.app.run("spot.delete", json!({"index": 0})).unwrap();
    f.app.run("spot.add", json!({"mode": "clone", "points": [[0.4, 0.5]], "size": 0.03, "source": [0.1, 0.0]})).unwrap();
    let replacement = f.settings(A);
    f.frame(vec![Event::PointerMoved(pos2(220.0, 155.0))]);
    f.release(pos2(240.0, 155.0));
    assert_eq!(f.settings(A), replacement, "an identical replacement at index zero has a new settings owner");
}

#[test]
fn pressed_pin_is_retired_when_selected_spot_changes_without_a_settings_edit() {
    let mut f = Frames::new("clone");
    f.spot(A);
    f.app.run("spot.add", json!({"mode": "clone", "points": [[0.7, 0.7]], "size": 0.03, "source": [-0.1, 0.0]})).unwrap();
    f.app.run("spot.select", json!({"index": 0})).unwrap();
    f.frame(vec![]);
    let before = f.settings(A);
    f.down(pos2(164.0, 155.0));
    f.app.run("spot.select", json!({"index": 1})).unwrap();
    f.frame(vec![Event::PointerMoved(pos2(220.0, 155.0))]);
    f.release(pos2(240.0, 155.0));
    assert_eq!(f.app.session.active_spot, Some(1));
    assert!(Arc::ptr_eq(&before, &f.settings(A)));
}

#[test]
fn valid_spot_moves_refresh_their_own_snapshot_and_end_only_their_interaction() {
    for source in [false, true] {
        let mut f = Frames::new("clone");
        f.spot(A);
        let original = f.settings(A).spots[0].clone();
        let undo = f.app.session.undo.len();
        let at = pos2(if source { 200.0 } else { 164.0 }, 155.0);
        f.down(at);
        for dx in [20.0, 40.0, 60.0] {
            f.frame(vec![Event::PointerMoved(at + vec2(dx, 0.0))]);
            assert!(matches!(f.app.gesture, Some(Gesture::SpotMove { .. })));
        }
        f.release(at + vec2(80.0, 0.0));
        let spot = f.settings(A).spots[0].clone();
        let moved = if source { spot.source_offset.unwrap().x - original.source_offset.unwrap().x } else { spot.points[0].x - original.points[0].x };
        assert!((moved - 80.0 / 360.0).abs() < 1e-6);
        assert_eq!(f.app.session.undo.len(), undo + 1);
        assert!(f.app.session.interaction.is_none());
    }
    let mut f = Frames::new("clone");
    f.spot(A);
    f.down(pos2(164.0, 155.0));
    f.frame(vec![Event::PointerMoved(pos2(190.0, 155.0))]);
    f.app.run("develop.endInteraction", json!({})).unwrap();
    f.app.run("develop.beginInteraction", json!({"label": "Foreign caller"})).unwrap();
    let owner = f.app.session.interaction.as_ref().unwrap().identity();
    let before = f.settings(A);
    f.frame(vec![Event::PointerMoved(pos2(230.0, 155.0))]);
    f.release(pos2(250.0, 155.0));
    assert!(Arc::ptr_eq(&owner, &f.app.session.interaction.as_ref().unwrap().identity()));
    assert_eq!(f.settings(A), before);
    f.app.run("develop.endInteraction", json!({})).unwrap();
}

#[test]
fn canonical_radius_matches_production_dabs_under_crop_geometry_zoom_and_orientation() {
    for orientation in [Orientation::Normal, Orientation::Rotate90] {
        for crop in [1.0, 0.5, 0.25] {
            for geometry in [false, true] {
                for width in [360.0, 800.0, 1600.0] {
                    let mut d = DevelopSettings { orientation, ..Default::default() };
                    let lo = (1.0 - crop) / 2.0;
                    d.crop.geometry.rect = lightcraft_geom::Rect::new(lo, lo, lo + crop, lo + crop);
                    d.crop.geometry.angle = if geometry { 13.0 } else { 0.0 };
                    if geometry {
                        d.geometry.vertical = 35.0;
                        d.geometry.horizontal = -20.0;
                        d.geometry.constrain_crop = true;
                    }
                    let frame = Frame::new(4000, 3000, &d, true);
                    let height = width as f64 / frame.aspect();
                    let rect = Rect::from_min_size(pos2(20.0, 20.0), vec2(width, height as f32));
                    let map = CanvasMap::new(&frame, rect);
                    let stroke = BrushStroke { points: vec![Point::new(0.5, 0.5)], size: 0.03, feather: 0.0, ..Default::default() };
                    let scale = 2.0;
                    let w = (rect.width() * scale).round() as usize;
                    let h = (rect.height() * scale).round() as usize;
                    let rendered = lightcraft_pipeline::masks::brush_dabs(&stroke, &frame, w, h).r / scale as f64;
                    let visible = stroke.size * map.points_per_long();
                    assert!((visible - rendered).abs() / rendered < 0.002, "{orientation:?} {crop} {geometry} {width}: {visible} vs {rendered}");
                    if crop == 1.0 && !geometry {
                        assert!((visible - 0.03 * rect.width().max(rect.height()) as f64).abs() < 0.01, "uncropped radius is unchanged");
                    } else if crop < 1.0 {
                        assert!(visible > 0.03 * rect.width().max(rect.height()) as f64 * 1.9, "a crop is not the source long edge");
                    }
                }
            }
        }
    }
}

fn circles(shape: &egui::Shape, out: &mut Vec<egui::epaint::CircleShape>) {
    match shape {
        egui::Shape::Circle(c) => out.push(*c),
        egui::Shape::Vec(v) => v.iter().for_each(|s| circles(s, out)),
        _ => {}
    }
}

#[test]
fn cropped_remove_outline_and_hit_radius_match_the_applied_size() {
    let mut f = Frames::new("clone");
    f.spot(A);
    let mut d = (*f.settings(A)).clone();
    d.crop.geometry.rect = lightcraft_geom::Rect::new(0.25, 0.25, 0.75, 0.75);
    f.app.session.set_develop(A, d, "Fixture crop").unwrap();
    let map = CanvasMap::new(&Frame::new(4000, 3000, &f.settings(A), true), Rect::from_min_size(pos2(20.0, 20.0), vec2(360.0, 270.0)));
    let center = map.screen(Point::new(0.4, 0.5));
    let radius = 0.03 * map.points_per_long() as f32;
    let output = f.frame(vec![]);
    let mut painted = Vec::new();
    output.shapes.iter().for_each(|s| circles(&s.shape, &mut painted));
    assert!(painted.iter().any(|c| c.center.distance(center) < 0.01 && (c.radius - radius).abs() < 0.01));
    // This point is outside the old 10.8-point outline and inside the correct 21.6-point one.
    let at = center + vec2(16.0, 0.0);
    f.down(at);
    f.frame(vec![Event::PointerMoved(at + vec2(20.0, 0.0))]);
    assert!(matches!(f.app.gesture, Some(Gesture::SpotMove { spot: 0, source: false })));
    f.release(at + vec2(30.0, 0.0));
    assert_eq!(f.settings(A).spots.len(), 1, "the applied footprint remains a hit target rather than a new stroke");
    assert!(f.ctx.data(|d| d.get_temp::<super::RemovePointerTarget>(remove_pointer_target_id())).is_none());
}

// Full-app wiring is authored for the parent's native tests. The bounded extraction harness
// excludes this module because cached LightcraftApp::ui still contains the baseline wiring.
mod full_app {
    use super::*;
    use crate::headless::{Headless, HeadlessView};

    #[test]
    fn full_ui_cancels_remove_on_controller_selection_before_release() {
        let mut app = app("clone");
        app.ui.left_panel = false;
        let mut h = Headless::new(app, [1200.0, 1000.0], 1.0);
        for _ in 0..3 {
            h.step();
        }
        let img = h.app.image_rect.unwrap();
        let a = img.center() - vec2(50.0, 0.0);
        let b = a + vec2(30.0, 0.0);
        let mut time = 1.0;
        let mut frame = |h: &mut Headless, events: Vec<Event>| {
            time += 0.02;
            let raw = HeadlessView::raw_input(h.size, h.pixels_per_point, time, events);
            let app = &mut h.app;
            h.view.run(raw, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
        };
        frame(&mut h, vec![Event::PointerMoved(a)]);
        frame(&mut h, vec![button(a, true)]);
        frame(&mut h, vec![Event::PointerMoved(b)]);
        assert!(matches!(h.app.gesture, Some(Gesture::Spot { .. })));
        // The controller uses this same app command path; no pointer blur is needed.
        h.app.run("library.select", json!({"ids": [B.0]})).unwrap();
        frame(&mut h, vec![]);
        frame(&mut h, vec![button(b, false)]);
        assert!(h.app.gesture.is_none());
        assert!(h.app.session.develop_of(A).unwrap().spots.is_empty());
        assert!(h.app.session.develop_of(B).unwrap().spots.is_empty());
    }

    #[test]
    fn full_ui_accepts_fresh_same_photo_stroke_after_an_idle_hidden_panel() {
        let mut app = app("clone");
        app.ui.left_panel = false;
        let mut h = Headless::new(app, [1200.0, 1000.0], 1.0);
        for _ in 0..3 {
            h.step();
        }
        let a = h.app.image_rect.unwrap().center() - vec2(50.0, 0.0);
        let b = a + vec2(30.0, 0.0);
        let mut time = 1.0;
        let mut frame = |h: &mut Headless, events: Vec<Event>| {
            time += 0.02;
            let raw = HeadlessView::raw_input(h.size, h.pixels_per_point, time, events);
            let app = &mut h.app;
            h.view.run(raw, |ui| {
                app.logic(ui.ctx());
                app.ui(ui);
            });
        };
        frame(&mut h, vec![Event::PointerMoved(a)]);
        frame(&mut h, vec![button(a, true)]);
        frame(&mut h, vec![Event::PointerMoved(b)]);
        assert!(matches!(h.app.gesture, Some(Gesture::Spot { .. })));
        h.app.ui.right = RightPanel::Edit;
        frame(&mut h, vec![]);
        frame(&mut h, vec![button(b, false)]);
        frame(&mut h, vec![]); // no Remove overlay consumes this idle frame
        assert!(h.app.session.develop_of(A).unwrap().spots.is_empty());
        h.app.ui.right = RightPanel::Remove;
        h.app.ui.tool = "clone".into();
        frame(&mut h, vec![Event::PointerMoved(a), button(a, true)]);
        frame(&mut h, vec![Event::PointerMoved(b)]);
        frame(&mut h, vec![button(b, false)]);
        assert_eq!(h.app.session.develop_of(A).unwrap().spots.len(), 1);
        assert!(h.app.session.develop_of(B).unwrap().spots.is_empty());
    }
}
