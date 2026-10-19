//! Pointer input reaches the real grid cell, App dispatcher, Session commands and history.
use super::*;
use egui::{Event, Modifiers, PointerButton};
use lightcraft_catalog::{Op, Photo, Source};

struct PainterScene {
    app: LightcraftApp,
    ctx: egui::Context,
    frames: usize,
}
impl PainterScene {
    fn new(keywords: &[&str], brush: &str) -> Self {
        let mut s = lightcraft_engine::Session::new();
        let mut p = Photo::new(PhotoId(1), Source::Demo { scene: 0 }, "fixture.jpg", "JPEG", 32, 24, "");
        p.meta.keywords = keywords.iter().map(|k| k.to_string()).collect();
        p.meta.title = "Unrelated metadata".into();
        s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
        s.selection = lightcraft_engine::Selection::single(PhotoId(1));
        let mut app = LightcraftApp::new(s, crate::Services::default());
        app.ui.settings.gpu = false;
        let ctx = egui::Context::default();
        crate::i18n::set_language(crate::i18n::Locale::En);
        crate::theme::install_fonts(&ctx);
        crate::theme::apply(&ctx);
        let mut scene = Self { app, ctx, frames: 0 };
        scene.frame(vec![]);
        scene.app.run("tool.keywordPainter", json!({"keyword": brush})).unwrap();
        scene.frame(vec![]);
        scene
    }
    fn frame(&mut self, events: Vec<Event>) {
        let input = crate::headless::HeadlessView::raw_input(vec2(420.0, 320.0), 1.0, self.frames as f64 / 60.0, events);
        let mut output = self.ctx.run_ui(input, |ui| {
            cell(&mut self.app, ui, PhotoId(1), Rect::from_min_size(pos2(50.0, 50.0), vec2(200.0, 180.0)), true, true, 1.0);
        });
        crate::widgets::take_registry(&self.ctx);
        output.textures_delta.clear();
        self.frames += 1;
    }
    fn click(&mut self) {
        let pos = pos2(150.0, 140.0);
        self.frame(vec![Event::PointerMoved(pos)]);
        for pressed in [true, false] {
            self.frame(vec![Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }]);
        }
        self.frame(vec![]);
    }
    fn sidebar_frame(&mut self, events: Vec<Event>) -> (Vec<(String, Rect)>, Vec<egui::epaint::ClippedShape>) {
        self.app.ui.right = crate::state::RightPanel::Keywords;
        let input = crate::headless::HeadlessView::raw_input(vec2(700.0, 700.0), 1.0, self.frames as f64 / 60.0, events);
        let mut output = self.ctx.run_ui(input, |ui| crate::panels::right::show(&mut self.app, ui));
        let registry = crate::widgets::take_registry(&self.ctx);
        output.textures_delta.clear();
        self.frames += 1;
        (registry, output.shapes)
    }
    fn set_button(&mut self, index: usize) -> Rect {
        self.sidebar_frame(vec![]).0.into_iter().find(|(id, _)| id == &format!("kwSet:{index}")).unwrap().1
    }
    fn click_set_button(&mut self, index: usize) {
        let pos = self.set_button(index).center();
        self.sidebar_frame(vec![Event::PointerMoved(pos)]);
        for pressed in [true, false] {
            self.sidebar_frame(vec![Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }]);
        }
        self.sidebar_frame(vec![]);
    }
    fn assert_set_button_selected(&mut self, index: usize, selected: bool) {
        // Settle the Grid row height before comparing its response rect and painted shape.
        self.sidebar_frame(vec![]);
        // Inspect the actual egui selected fill, with the pointer away from the button.
        let (registry, shapes) = self.sidebar_frame(vec![Event::PointerMoved(pos2(1.0, 1.0))]);
        let rect = registry.into_iter().find(|(id, _)| id == &format!("kwSet:{index}")).unwrap().1;
        let fill = shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Rect(shape) if shape.rect == rect => Some(shape.fill),
                _ => None,
            })
            .expect("keyword-set button must paint its registered rectangle");
        assert_eq!(
            fill == self.ctx.global_style().visuals.selection.bg_fill,
            selected,
            "keyword-set button {index} must show its exact identity state"
        );
    }
    fn keywords(&self) -> Vec<String> {
        self.app.session.catalog.photo(PhotoId(1)).unwrap().meta.keywords.clone()
    }
}

#[test]
fn painter_pointer_click_removes_unicode_equivalent_keyword_and_round_trips_history() {
    for (stored, brush) in [("CAFÉ", "café"), ("TARGET|STRAẞE", "Target|Straße"), ("ΑΘΉΝΑ", "Αθήνα"), ("TRAVEL", "travel")] {
        let mut scene = PainterScene::new(&[stored, "neighbor"], brush);
        let before = scene.app.session.catalog.to_snapshot();
        scene.click();
        assert_eq!(scene.keywords(), ["neighbor"], "actual pointer click must remove {stored}");
        assert_eq!(scene.app.session.catalog.photo(PhotoId(1)).unwrap().meta.title, "Unrelated metadata");
        let removed = scene.app.session.catalog.to_snapshot();
        scene.app.run("edit.undo", json!({})).unwrap();
        assert_eq!(scene.app.session.catalog.to_snapshot(), before, "Undo restores exact original spelling");
        scene.app.run("edit.redo", json!({})).unwrap();
        assert_eq!(scene.app.session.catalog.to_snapshot(), removed);
        scene.click();
        assert_eq!(scene.keywords(), ["neighbor", brush]);
    }
}

#[test]
fn painter_pointer_click_toggles_only_the_exact_keyword_not_descendants_or_neighbors() {
    let mut scene = PainterScene::new(&["CAFÉ|Street", "caféhouse"], "café");
    scene.click();
    assert_eq!(scene.keywords(), ["CAFÉ|Street", "caféhouse", "café"]);
    scene.click();
    assert_eq!(scene.keywords(), ["CAFÉ|Street", "caféhouse"]);
}

/// The real App/pointer boundary must choose removal before the engine's equivalent-add no-op.
fn assert_legacy_painter_round_trip(stored: &str, brush: &str, added: &str) {
    let child = format!("{stored}|Child");
    let preserved = [child.as_str(), "RAW | Other", "raw|other", " | | ", "旅行|Caféhouse"];
    let mut original = vec![stored];
    original.extend(preserved);
    let mut scene = PainterScene::new(&original, brush);
    let before = scene.app.session.catalog.to_snapshot();
    scene.click();
    assert_eq!(scene.keywords(), preserved, "pointer must remove the equivalent legacy spelling {stored:?}");
    assert_eq!(scene.app.session.catalog.photo(PhotoId(1)).unwrap().meta.title, "Unrelated metadata");
    let removed = scene.app.session.catalog.to_snapshot();
    scene.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(scene.app.session.catalog.to_snapshot(), before, "Undo restores every raw metadata byte");
    scene.app.run("edit.redo", json!({})).unwrap();
    assert_eq!(scene.app.session.catalog.to_snapshot(), removed);
    scene.click();
    let mut expected = preserved.to_vec();
    expected.push(added);
    assert_eq!(scene.keywords(), expected, "adding uses the normalized display spelling while preserving children and unrelated legacy duplicates");
    let added_snapshot = scene.app.session.catalog.to_snapshot();
    scene.app.run("edit.undo", json!({})).unwrap();
    assert_eq!(scene.app.session.catalog.to_snapshot(), removed);
    scene.app.run("edit.redo", json!({})).unwrap();
    assert_eq!(scene.app.session.catalog.to_snapshot(), added_snapshot);
    scene.click();
    assert_eq!(scene.app.session.catalog.to_snapshot(), removed);
}

#[test]
fn painter_pointer_removes_legacy_spaced_hierarchy_and_preserves_raw_neighbors() {
    assert_legacy_painter_round_trip("旅行 | Café", "旅行|Café", "旅行|Café");
}

#[test]
fn painter_pointer_removes_legacy_empty_hierarchy_levels() {
    assert_legacy_painter_round_trip(" | 旅行 | | Café | ", "旅行|Café", "旅行|Café");
}

#[test]
fn painter_pointer_matches_expanding_unicode_lowercase_in_legacy_hierarchy() {
    assert_legacy_painter_round_trip(" | İ | | CAFÉ | ", "i\u{307}|café", "i\u{307}|café");
}

#[test]
fn painter_pointer_normalizes_the_target_identity_and_added_display() {
    assert_legacy_painter_round_trip("旅行|Café", " | 旅行 | | CAFÉ | ", "旅行|CAFÉ");
}

#[test]
fn right_keyword_set_shows_exact_legacy_identity_and_pointer_history() {
    for (stored, target) in [
        ("旅行 | Café", "旅行|Café"),
        (" | 旅行 | | Café | ", "旅行|Café"),
        (" | İ | | CAFÉ | ", "i\u{307}|café"),
        ("旅行|Café", " | 旅行 | | CAFÉ | "),
    ] {
        let child = format!("{stored}|Child");
        let preserved = [child.as_str(), "RAW | Other", "raw|other", " | | "];
        let mut original = vec![stored];
        original.extend(preserved);
        let mut scene = PainterScene::new(&original, target);
        scene.app.run("keyword.saveSet", json!({"name": "Fixture", "keywords": [target]})).unwrap();
        let before = scene.app.session.catalog.to_snapshot();
        scene.assert_set_button_selected(1, true);
        scene.click_set_button(1);
        assert_eq!(scene.keywords(), preserved);
        scene.assert_set_button_selected(1, false);
        let removed = scene.app.session.catalog.to_snapshot();
        scene.app.run("edit.undo", json!({})).unwrap();
        assert_eq!(scene.app.session.catalog.to_snapshot(), before);
        scene.assert_set_button_selected(1, true);
        scene.app.run("edit.redo", json!({})).unwrap();
        assert_eq!(scene.app.session.catalog.to_snapshot(), removed);
        scene.assert_set_button_selected(1, false);
        scene.click_set_button(1);
        scene.assert_set_button_selected(1, true);
        let added = scene.app.session.catalog.to_snapshot();
        scene.app.run("edit.undo", json!({})).unwrap();
        assert_eq!(scene.app.session.catalog.to_snapshot(), removed);
        scene.assert_set_button_selected(1, false);
        scene.app.run("edit.redo", json!({})).unwrap();
        assert_eq!(scene.app.session.catalog.to_snapshot(), added);
        scene.assert_set_button_selected(1, true);
    }
    // A child alone must neither check its parent button nor be removed by that button.
    let mut scene = PainterScene::new(&["旅行 | Café | Street", "RAW | Other", "raw|other"], "旅行|Café");
    scene.app.run("keyword.saveSet", json!({"name": "Fixture", "keywords": ["旅行|Café"]})).unwrap();
    scene.assert_set_button_selected(1, false);
    scene.click_set_button(1);
    assert_eq!(scene.keywords(), ["旅行 | Café | Street", "RAW | Other", "raw|other", "旅行|Café"]);
    scene.assert_set_button_selected(1, true);
    scene.click_set_button(1);
    assert_eq!(scene.keywords(), ["旅行 | Café | Street", "RAW | Other", "raw|other"]);
    scene.assert_set_button_selected(1, false);
}
