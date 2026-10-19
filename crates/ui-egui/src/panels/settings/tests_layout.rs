//! Render the complete production Settings dialog; move and click through actual egui input.
use crate::{LightcraftApp, Services, i18n, state::Dialog, theme, widgets};
use egui::{Event, Modifiers, MouseWheelUnit, PointerButton, Pos2, Rect, Vec2};

struct Scene {
    app: LightcraftApp,
    ctx: egui::Context,
    size: Vec2,
    frame: u32,
    rects: Vec<(String, Rect)>,
    text: Vec<(String, Rect, Rect)>,
}
impl Scene {
    fn new(size: [f32; 2], language: i18n::Locale, tab: &str) -> Self {
        let mut app = LightcraftApp::new(lightcraft_engine::Session::new(), Services::default());
        app.ui.language = language;
        app.ui.settings.gpu = false;
        app.ui.dialog = Some(Dialog::Settings { tab: tab.into() });
        i18n::set_language(language);
        let ctx = egui::Context::default();
        theme::apply(&ctx);
        theme::install_fonts(&ctx);
        let mut scene = Self { app, ctx, size: size.into(), frame: 0, rects: vec![], text: vec![] };
        scene.idle();
        scene
    }
    fn draw(&mut self, events: Vec<Event>) {
        i18n::set_language(self.app.ui.language);
        let input = crate::headless::HeadlessView::raw_input(self.size, 1.0, self.frame as f64 / 60.0, events);
        let mut out = self.ctx.run_ui(input, |ui| crate::panels::dialogs::show(&mut self.app, ui.ctx()));
        self.rects = widgets::take_registry(&self.ctx);
        self.text.clear();
        for shape in &out.shapes {
            collect_text(&shape.shape, shape.clip_rect, &mut self.text);
        }
        out.textures_delta.clear();
        self.frame += 1;
    }
    fn idle(&mut self) {
        for _ in 0..4 {
            self.draw(vec![]);
        }
    }
    fn rect(&self, id: &str) -> Rect {
        self.rects.iter().find(|(name, _)| name == id).unwrap_or_else(|| panic!("missing {id}")).1
    }
    fn viewport(&self) -> Rect {
        Rect::from_min_size(Pos2::ZERO, self.size)
    }
    fn content(&self) -> Rect {
        self.rect("scroll:settingsContent").intersect(self.viewport())
    }
    fn scroll(&mut self, delta: Vec2) {
        let pos = self.content().center();
        self.draw(vec![
            Event::PointerMoved(pos),
            Event::MouseWheel { unit: MouseWheelUnit::Point, delta, phase: egui::TouchPhase::Move, modifiers: Modifiers::NONE },
        ]);
        for _ in 0..20 {
            self.draw(vec![]);
        }
    }
    fn reveal(&mut self, id: &str) {
        for _ in 0..20 {
            let r = self.rect(id);
            let clip = self.content().expand(0.5);
            if clip.contains_rect(r) {
                return;
            }
            let x = if r.left() < clip.left() {
                clip.left() - r.left()
            } else if r.right() > clip.right() {
                clip.right() - r.right()
            } else {
                0.0
            }
            .clamp(-80.0, 80.0);
            let y = if r.top() < clip.top() {
                clip.top() - r.top()
            } else if r.bottom() > clip.bottom() {
                clip.bottom() - r.bottom()
            } else {
                0.0
            }
            .clamp(-80.0, 80.0);
            self.scroll(egui::vec2(x, y));
        }
        panic!("{:?}/{:?}: {id} at {:?} remains clipped by {:?}", self.app.ui.language, self.size, self.rect(id), self.content());
    }
    fn assert_painted(&self, label: &str) {
        let (_, bounds, clip) = self.text.iter().find(|(text, _, _)| text == label).unwrap_or_else(|| panic!("missing painted {label}"));
        assert!(self.viewport().intersect(*clip).expand(0.5).contains_rect(*bounds), "{label}: {bounds:?} clipped by {clip:?}");
    }
    fn assert_close(&self) {
        assert!(self.viewport().contains_rect(self.rect("button:dialogOk")));
        self.assert_painted(i18n::tr("Settings"));
        self.assert_painted(i18n::tr("Close"));
    }
    fn click(&mut self, id: &str) {
        let r = self.rect(id);
        assert!(self.viewport().contains_rect(r));
        if id != "button:dialogOk" {
            assert!(self.content().expand(0.5).contains_rect(r), "{id}: clipped click target");
        }
        let pos = r.center();
        self.draw(vec![Event::PointerMoved(pos)]);
        for pressed in [true, false] {
            self.draw(vec![Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }]);
        }
        self.idle();
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
            out.push((text.galley.text().to_string(), text.visual_bounding_rect(), clip))
        }
        _ => {}
    }
}

#[test]
fn all_settings_tabs_keep_primary_controls_and_close_reachable_in_all_languages() {
    for size in [[640.0, 480.0], [320.0, 480.0], [900.0, 700.0]] {
        for language in i18n::Locale::ALL {
            for (tab, label) in super::TABS {
                let mut scene = Scene::new(size, *language, tab);
                scene.assert_close();
                assert!(scene.rect("scroll:settingsContent").width() <= 560.5);
                let controls: &[&str] = match *tab {
                    "general" => &["button:settingsLanguage-7", "button:settingsStartup-0", "button:settingsStartup-2", "field:externalEditor"],
                    "import" => &[
                        "combo:settingsRawPreset",
                        "combo:settingsOtherPreset",
                        "field:settings.copyright",
                        "field:settings.creator",
                        "button:settingsXmpNaming-1",
                    ],
                    "performance" => &[
                        "button:settingsPreview-0",
                        "button:settingsPreview-4",
                        "button:settingsMemory-4",
                        "button:settingsCache-4",
                        "button:settingsForgetLocal-4",
                    ],
                    "interface" => &["check:settings.filmNames", "button:settingsGridBadges-2", "button:settingsInfo-2"],
                    // Session::new has no face-model directory. Verify the actual empty-state heading;
                    // don't invent model files or invoke downloads to exercise this tab.
                    "faces" => {
                        scene.assert_painted(i18n::tr("Face recognition"));
                        &[]
                    }
                    // Additional dynamically registered tabs still receive heading and Close
                    // coverage. Their model/setup controls need their own production fixtures.
                    _ => {
                        scene.assert_painted(i18n::tr(label));
                        &[]
                    }
                };
                for id in controls {
                    scene.reveal(id);
                    scene.assert_close();
                }
                scene.click("button:dialogOk");
                assert!(scene.app.ui.dialog.is_none());
            }
        }
    }
    i18n::set_language(i18n::Locale::En);
}

#[test]
fn every_retention_choice_is_painted_and_pointer_selectable_after_scrolling() {
    for size in [[640.0, 480.0], [320.0, 480.0], [900.0, 700.0]] {
        for language in i18n::Locale::ALL {
            let mut scene = Scene::new(size, *language, "performance");
            for (index, (days, label)) in
                [(0, "Never"), (7, "After 7 days"), (30, "After 30 days"), (90, "After 90 days"), (365, "After a year")].iter().enumerate()
            {
                let id = format!("button:settingsForgetLocal-{index}");
                scene.reveal(&id);
                scene.assert_painted(i18n::tr(label));
                scene.click(&id);
                assert_eq!(scene.app.session.forget_local_days, *days, "{language:?}/{size:?}: {id} did not apply");
                scene.assert_close();
            }
        }
    }
    i18n::set_language(i18n::Locale::En);
}

#[test]
fn fitting_choices_retain_one_row_and_normal_body_width_on_wide_viewports() {
    for language in i18n::Locale::ALL {
        for (tab, prefix, count) in [("general", "settingsStartup", 3), ("import", "settingsXmpNaming", 2), ("interface", "settingsInfo", 3)] {
            let mut scene = Scene::new([900.0, 700.0], *language, tab);
            scene.reveal(&format!("button:{prefix}-0"));
            let first = scene.rect(&format!("button:{prefix}-0"));
            for index in 1..count {
                let next = scene.rect(&format!("button:{prefix}-{index}"));
                assert!((next.top() - first.top()).abs() < 0.5, "{language:?}/{tab}: fitting choices wrapped");
                assert!(scene.content().expand(0.5).contains_rect(next));
            }
            assert!((scene.rect("scroll:settingsContent").width() - 560.0).abs() < 0.5);
        }
    }
    i18n::set_language(i18n::Locale::En);
}
