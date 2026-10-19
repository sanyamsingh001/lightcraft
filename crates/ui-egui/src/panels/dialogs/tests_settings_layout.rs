//! Complete Settings dialog geometry and pointer/wheel interaction with actual egui input.
use egui::{Event, Modifiers, MouseWheelUnit, PointerButton, Pos2, Rect, Vec2};

use crate::{LightcraftApp, Services, i18n, state::Dialog, theme, widgets};

struct SettingsScene {
    app: LightcraftApp,
    ctx: egui::Context,
    size: Vec2,
    frames: usize,
    language: i18n::Locale,
    registry: Vec<(String, Rect)>,
    text: Vec<(String, Rect, Rect)>,
}

impl SettingsScene {
    fn new(size: [f32; 2], language: i18n::Locale) -> Self {
        let mut app = LightcraftApp::new(lightcraft_engine::Session::new(), Services::default());
        app.ui.language = language;
        app.ui.dialog = Some(Dialog::Settings { tab: "general".into() });
        i18n::set_language(language);
        let ctx = egui::Context::default();
        theme::install_fonts(&ctx);
        theme::apply(&ctx);
        let mut scene = Self { app, ctx, size: size.into(), frames: 0, language, registry: vec![], text: vec![] };
        scene.idle();
        scene
    }

    fn frame(&mut self, events: Vec<Event>) {
        i18n::set_language(self.app.ui.language);
        if self.language != self.app.ui.language {
            theme::install_fonts(&self.ctx);
            self.language = self.app.ui.language;
        }
        let input = crate::headless::HeadlessView::raw_input(self.size, 1.0, self.frames as f64 / 60.0, events);
        let mut output = self.ctx.run_ui(input, |ui| super::show(&mut self.app, ui.ctx()));
        self.registry = widgets::take_registry(&self.ctx);
        self.text.clear();
        for shape in &output.shapes {
            collect_text(&shape.shape, shape.clip_rect, &mut self.text);
        }
        output.textures_delta.clear();
        self.frames += 1;
    }

    fn idle(&mut self) {
        for _ in 0..4 {
            self.frame(vec![]);
        }
    }

    fn widget(&self, id: &str) -> Rect {
        self.registry.iter().find(|(widget, _)| widget == id).unwrap_or_else(|| panic!("missing {id}")).1
    }

    fn viewport(&self) -> Rect {
        Rect::from_min_size(Pos2::ZERO, self.size)
    }

    fn assert_close_visible(&self) {
        let viewport = self.viewport();
        let close = self.widget("button:dialogOk");
        assert!(viewport.contains_rect(close), "{:?} {:?}: Close at {close:?}", self.app.ui.language, self.size);
        let title = i18n::tr("Settings");
        let label = i18n::tr("Close");
        for expected in [title, label] {
            let (_, bounds, clip) = self.text.iter().find(|(text, _, _)| text == expected).unwrap_or_else(|| panic!("missing painted {expected}"));
            assert!(viewport.intersect(*clip).expand(0.5).contains_rect(*bounds), "{expected}: {bounds:?} outside {clip:?}");
        }
    }

    fn click(&mut self, id: &str) {
        let pos = self.widget(id).center();
        assert!(self.viewport().contains(pos), "{id} is outside the viewport");
        self.frame(vec![Event::PointerMoved(pos)]);
        for pressed in [true, false] {
            self.frame(vec![Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }]);
        }
        self.idle();
    }

    fn scroll(&mut self, delta: Vec2) {
        let pos = self.widget("scroll:settingsContent").center();
        self.frame(vec![
            Event::PointerMoved(pos),
            Event::MouseWheel { unit: MouseWheelUnit::Point, delta, phase: egui::TouchPhase::Move, modifiers: Modifiers::NONE },
        ]);
        for _ in 0..20 {
            self.frame(vec![]);
        }
    }
}

fn collect_text(shape: &egui::epaint::Shape, clip: Rect, out: &mut Vec<(String, Rect, Rect)>) {
    match shape {
        egui::epaint::Shape::Vec(shapes) => {
            for shape in shapes {
                collect_text(shape, clip, out);
            }
        }
        egui::epaint::Shape::Text(text) if !text.galley.text().is_empty() => {
            out.push((text.galley.text().to_string(), text.visual_bounding_rect(), clip));
        }
        _ => {}
    }
}

#[test]
fn settings_close_fits_and_clicks_across_languages_and_small_viewports() {
    for size in [[640.0, 480.0], [640.0, 320.0], [600.0, 320.0], [420.0, 360.0], [320.0, 480.0], [900.0, 560.0]] {
        for language in i18n::Locale::ALL {
            let mut scene = SettingsScene::new(size, *language);
            scene.assert_close_visible();
            scene.click("button:dialogOk");
            assert!(scene.app.ui.dialog.is_none());
        }
    }
    i18n::set_language(i18n::Locale::En);
}

#[test]
fn language_changes_and_resizing_keep_close_visible() {
    let mut scene = SettingsScene::new([640.0, 480.0], i18n::Locale::En);
    for (index, language) in i18n::Locale::ALL.iter().enumerate() {
        let id = format!("button:settingsLanguage-{index}");
        assert!(scene.viewport().contains_rect(scene.widget(&id)));
        scene.click(&id);
        assert_eq!(scene.app.ui.language, *language);
        scene.assert_close_visible();
    }
    for size in [[640.0, 320.0], [320.0, 480.0], [900.0, 560.0], [640.0, 480.0]] {
        scene.size = size.into();
        scene.idle();
        scene.assert_close_visible();
    }
    scene.size = [900.0, 700.0].into();
    scene.idle();
    scene.assert_close_visible();
    assert!(scene.widget("scroll:settingsContent").contains_rect(scene.widget("field:externalEditor")), "large viewport should show the lower field");
    scene.click("button:dialogOk");
    assert!(scene.app.ui.dialog.is_none());
    i18n::set_language(i18n::Locale::En);
}

#[test]
fn settings_body_scrolls_to_lower_controls_without_moving_close() {
    let mut scene = SettingsScene::new([640.0, 320.0], i18n::Locale::De);
    let close = scene.widget("button:dialogOk");
    let field_before = scene.widget("field:externalEditor");
    let content = scene.widget("scroll:settingsContent");
    assert!(!content.contains_rect(field_before), "fixture must need scrolling");
    scene.scroll(egui::vec2(0.0, -2000.0));
    let field = scene.widget("field:externalEditor");
    assert!(field.top() < field_before.top());
    assert!(scene.widget("scroll:settingsContent").contains_rect(field), "lower field remains clipped: {field:?}");
    assert_eq!(scene.widget("button:dialogOk"), close);
    scene.assert_close_visible();
    scene.click("field:externalEditor");
    scene.frame(vec![Event::Text("editor-test".into())]);
    assert_eq!(scene.app.ui.settings.external_editor, "editor-test");
    scene.click("button:dialogOk");
    assert!(scene.app.ui.dialog.is_none());
    i18n::set_language(i18n::Locale::En);
}

#[test]
fn settings_body_fits_without_scrolling_on_large_viewports() {
    for language in i18n::Locale::ALL {
        let scene = SettingsScene::new([900.0, 700.0], *language);
        scene.assert_close_visible();
        let content = scene.widget("scroll:settingsContent");
        assert!(content.contains_rect(scene.widget("field:externalEditor")), "{:?}: lower field should fit without scrolling", language);
        assert!(content.width() <= 560.5, "normal body should retain its width: {content:?}");
    }
    i18n::set_language(i18n::Locale::En);
}
