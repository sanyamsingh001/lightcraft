//! Exercise production toolbar widgets with real egui input, without rendering the library.
use crate::i18n::Locale;
use crate::state::{Dialog, RightPanel};
use crate::{LightcraftApp, Services, theme, widgets};
use egui::{Context, Event, Id, Modifiers, PointerButton, Rect, pos2, vec2};

struct Bar {
    ctx: Context,
    app: LightcraftApp,
    width: f32,
    time: f64,
    rects: Vec<(String, Rect)>,
    access: Vec<(String, String, String)>,
}

impl Bar {
    fn new(language: Locale, width: f32) -> Self {
        crate::i18n::set_language(language);
        let ctx = Context::default();
        theme::apply(&ctx);
        theme::install_fonts(&ctx);
        let mut app = LightcraftApp::new(lightcraft_engine::Session::new(), Services::default());
        app.ui.language = language;
        let mut bar = Self { ctx, app, width, time: 0.0, rects: Vec::new(), access: Vec::new() };
        bar.draw(Vec::new());
        bar
    }

    fn draw(&mut self, events: Vec<Event>) {
        self.time += 1.0 / 60.0;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(pos2(0.0, 0.0), vec2(self.width, 720.0))),
            time: Some(self.time),
            events,
            ..Default::default()
        };
        let mut output = self.ctx.run_ui(input, |ui| super::show(&mut self.app, ui));
        output.textures_delta.clear();
        if let Some(update) = output.platform_output.accesskit_update.take() {
            self.access = update
                .nodes
                .iter()
                .map(|(id, node)| (format!("{id:?}"), format!("{:?}", node.role()), node.label().unwrap_or_default().to_string()))
                .collect();
        }
        self.rects = widgets::take_registry(&self.ctx);
    }

    fn settle(&mut self) {
        for _ in 0..4 {
            self.draw(Vec::new());
        }
    }

    fn rect(&self, id: &str) -> Rect {
        self.rects.iter().find(|(name, _)| name == id).unwrap_or_else(|| panic!("missing {id} at {}", self.width)).1
    }

    fn click_at(&mut self, at: egui::Pos2) {
        self.draw(vec![Event::PointerMoved(at)]);
        for pressed in [true, false] {
            self.draw(vec![Event::PointerButton { pos: at, button: PointerButton::Primary, pressed, modifiers: Modifiers::NONE }]);
        }
        self.settle();
    }

    fn click_action(&mut self, icon: &str, command: &str) {
        if self.rects.iter().any(|(id, _)| id == "icon:toolbarMore") {
            self.click_at(self.rect("icon:toolbarMore").center());
            let entries: Vec<_> = self.rects.iter().filter(|(id, _)| id.starts_with("toolbar:")).collect();
            assert_eq!(entries.len(), 4, "all actionable right icons remain available");
            for (_, rect) in entries {
                assert!(self.ctx.content_rect().contains_rect(*rect), "overflow entry outside viewport: {rect:?}");
                assert!(rect.top() >= self.rect("field:search").bottom(), "overflow covers the bar");
            }
            self.click_at(self.rect(&format!("toolbar:{command}")).center());
        } else {
            self.click_at(self.rect(icon).center());
        }
    }
}

#[test]
fn targets_fit_and_do_not_overlap_at_every_browser_width_and_language() {
    for language in Locale::ALL {
        let mut bar = Bar::new(*language, 320.0);
        for width in 320..=900 {
            bar.width = width as f32;
            bar.settle();
            let old = bar.rects.clone();
            bar.draw(Vec::new());
            assert_eq!(old, bar.rects, "unsettled {language:?} at {width}");
            let targets: Vec<_> =
                bar.rects.iter().filter(|(id, _)| id.starts_with("icon:") || id.starts_with("menu:") || id == "field:search").collect();
            assert!(targets.iter().any(|(id, _)| id == "menu:all"), "in-window menus stay accessible");
            for (i, (name, rect)) in targets.iter().enumerate() {
                assert!(bar.ctx.content_rect().contains_rect(*rect), "{language:?}/{width}: {name} outside viewport");
                for (other, r) in targets.iter().skip(i + 1) {
                    let overlap = rect.intersect(*r);
                    assert!(overlap.width() <= 0.0 || overlap.height() <= 0.0, "{language:?}/{width}: {name} overlaps {other}");
                }
            }
            let input = bar.ctx.read_response(Id::new("search-field")).expect("actual TextEdit");
            assert!(input.rect.width() >= 20.0, "{language:?}/{width}: unusable search target");
            assert!(bar.rect("field:search").contains_rect(input.rect), "TextEdit must stay inside Search");
        }
    }
    crate::i18n::set_language(Locale::En);
}

#[test]
fn fresh_clicks_reach_filter_search_activity_and_export() {
    for language in Locale::ALL {
        for width in [320.0, 400.0, 480.0, 520.0, 542.0, 622.0, 640.0, 700.0, 900.0] {
            let mut bar = Bar::new(*language, width);
            bar.settle();
            bar.click_at(bar.rect("icon:filter").center());
            assert!(bar.app.ui.filter_bar, "{language:?}/{width}: Filter did not toggle");
            assert_eq!(bar.app.ui.right, RightPanel::Edit, "Filter dispatched Activity");
            assert!(bar.app.ui.dialog.is_none(), "Filter dispatched Export");
            bar.click_at(bar.rect("icon:filter").center());
            assert!(!bar.app.ui.filter_bar);
            let input = bar.ctx.read_response(Id::new("search-field")).expect("actual TextEdit");
            // This right edge used to be under Activity at 520 points.
            bar.click_at(pos2(input.rect.right() - 1.0, input.rect.center().y));
            assert!(bar.ctx.memory(|m| m.has_focus(Id::new("search-field"))), "{language:?}/{width}: Search lost its click");
            bar.draw(vec![Event::Text("toolbar query".into())]);
            assert_eq!(bar.app.ui.search, "toolbar query");
            assert_eq!(bar.app.session.filter.text, "toolbar query", "Search must dispatch library.filter");
            assert_eq!(bar.app.ui.right, RightPanel::Edit);
            assert!(bar.app.ui.dialog.is_none());
            bar.click_action("icon:bell", "panel.activity");
            assert_eq!(bar.app.ui.right, RightPanel::Activity, "{language:?}/{width}: Activity inaccessible");
            assert!(!bar.app.ui.filter_bar);
            assert!(bar.app.ui.dialog.is_none());
            bar.click_action("icon:share", "dialog.export");
            assert!(matches!(bar.app.ui.dialog, Some(Dialog::Export { .. })), "{language:?}/{width}: Export inaccessible");
            assert!(!bar.app.ui.filter_bar);
            assert_eq!(bar.app.session.filter.text, "toolbar query");
        }
    }
    crate::i18n::set_language(Locale::En);
}

#[test]
fn more_has_a_translated_accessible_button_name_and_keeps_its_pointer_actions() {
    for language in Locale::ALL {
        let mut bar = Bar::new(*language, 320.0);
        bar.ctx.enable_accesskit();
        bar.settle();
        let id = format!("#{}", Id::new("toolbar-more").value());
        let (_, role, name) = bar.access.iter().find(|(node, _, _)| node == &id).expect("actual More node");
        assert_eq!(role, "Button", "{language:?}: More role");
        assert_eq!(name, crate::i18n::tr("More"), "{language:?}: More name");
        bar.click_at(bar.rect("icon:toolbarMore").center());
        assert_eq!(bar.rects.iter().filter(|(id, _)| id.starts_with("toolbar:")).count(), 4);
        bar.click_at(bar.rect("toolbar:panel.activity").center());
        assert_eq!(bar.app.ui.right, RightPanel::Activity);
        assert!(!egui::Popup::is_any_open(&bar.ctx));
        bar.click_action("icon:share", "dialog.export");
        assert!(matches!(bar.app.ui.dialog, Some(Dialog::Export { .. })));
    }
    crate::i18n::set_language(Locale::En);
}
