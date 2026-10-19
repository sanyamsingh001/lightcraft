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
