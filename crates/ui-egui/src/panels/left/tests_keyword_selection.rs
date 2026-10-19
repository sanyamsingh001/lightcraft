//! Keyword selection through the real sidebar rows and engine commands, without photo rendering.

use lightcraft_catalog::{Op, Photo, PhotoId, Source};
use lightcraft_engine::{LibrarySource, Session};
use serde_json::json;

use super::keyword_rows;
use crate::{LightcraftApp, Services};

type TestResult<T = ()> = Result<T, Box<dyn std::error::Error>>;

struct KeywordUi {
    app: LightcraftApp,
    ctx: egui::Context,
    frames: u64,
}

impl KeywordUi {
    fn new(keywords: &[&str]) -> TestResult<Self> {
        let mut session = Session::new();
        for (index, keyword) in keywords.iter().enumerate() {
            let id = PhotoId(index as u64 + 1);
            let mut photo = Photo::new(id, Source::Demo { scene: 0 }, &format!("{}.jpg", id.0), "JPEG", 4, 3, "2026-04-01T12:00:00");
            photo.meta.keywords = vec![(*keyword).into()];
            photo.rating = if id == PhotoId(4) { 1 } else { 5 };
            session.catalog.apply(Op::AddPhoto { photo: Box::new(photo) })?;
        }
        let ctx = egui::Context::default();
        crate::theme::install_fonts(&ctx);
        crate::theme::apply(&ctx);
        Ok(Self { app: LightcraftApp::new(session, Services::default()), ctx, frames: 0 })
    }

    fn frame(&mut self, events: Vec<egui::Event>) -> Vec<(String, egui::Rect)> {
        let _ = crate::widgets::take_registry(&self.ctx);
        let raw = crate::headless::HeadlessView::raw_input(egui::vec2(360.0, 600.0), 1.0, self.frames as f64 / 60.0, events);
        let app = &mut self.app;
        let _ = self.ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let tree = app.caches.keyword_tree(&app.session.catalog);
                keyword_rows(app, ui, &tree, 0.0);
            });
        });
        self.frames += 1;
        crate::widgets::take_registry(&self.ctx)
    }

    fn click(&mut self, id: &str) -> TestResult {
        self.frame(vec![]);
        let widgets = self.frame(vec![]);
        let pos = widgets.iter().find(|(name, _)| name == id).ok_or_else(|| format!("missing {id}: {widgets:?}"))?.1.center();
        self.frame(vec![egui::Event::PointerMoved(pos)]);
        for pressed in [true, false] {
            self.frame(vec![egui::Event::PointerButton { pos, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() }]);
        }
        Ok(())
    }

    fn highlights(&mut self) -> Vec<String> {
        // These rectangles are registered only when row_named paints the selected background.
        self.frame(vec![]).into_iter().filter(|(name, _)| name.starts_with("highlight:keyword:")).map(|(name, _)| name).collect()
    }

    fn visible(&mut self) -> Vec<u64> {
        let mut ids: Vec<_> = self.app.session.visible().iter().map(|id| id.0).collect();
        ids.sort_unstable();
        ids
    }
}

#[test]
fn merged_keyword_filter_keeps_highlight_and_next_click_clears() -> TestResult {
    for (tree_spelling, filter_spelling) in [("STRAẞE", "Straße"), ("Straße", "STRAẞE"), ("COAST", "coast")] {
        let target = format!("Target|{tree_spelling}");
        let before = format!("Before|{filter_spelling}");
        let followed = format!("Target|{filter_spelling}");
        let mut h = KeywordUi::new(&[&target, &before, "Target|Other", "Unrelated"])?;
        h.app.run("library.filter", json!({"rating": 5}))?;
        h.click("keywordToggle:Before")?;
        h.click(&format!("source:keyword:{before}"))?;
        assert_eq!(h.app.session.filter.keyword.as_deref(), Some(before.as_str()));
        assert_eq!(h.visible(), [2]);
        assert_eq!(h.highlights(), [format!("highlight:keyword:{before}")]);
        let selection = h.app.session.selection.clone();
        assert_eq!(selection.active, Some(PhotoId(2)), "the merge starts with a selected matching photo");
        let before_merge = h.app.session.catalog.to_snapshot();
        h.app.run("keyword.merge", json!({"from": ["Before"], "into": "Target"}))?;
        let after_merge = h.app.session.catalog.to_snapshot();
        assert_eq!(h.app.session.filter.keyword.as_deref(), Some(followed.as_str()));
        assert_eq!(h.app.session.selection, selection, "the global merge keeps photo context");
        assert_eq!(h.visible(), [1, 2]);
        h.click("keywordToggle:Target")?;
        assert_eq!(h.app.session.filter.keyword.as_deref(), Some(followed.as_str()), "disclosure must not navigate");
        assert_eq!(h.highlights(), [format!("highlight:keyword:{target}")], "the first tree spelling still represents the followed filter");
        h.app.run("edit.undo", json!({}))?;
        assert_eq!(h.app.session.catalog.to_snapshot(), before_merge);
        assert_eq!(h.app.session.filter.keyword.as_deref(), Some(before.as_str()));
        assert_eq!(h.highlights(), [format!("highlight:keyword:{before}")]);
        h.app.run("edit.redo", json!({}))?;
        assert_eq!(h.app.session.catalog.to_snapshot(), after_merge);
        assert_eq!(h.app.session.filter.keyword.as_deref(), Some(followed.as_str()));
        assert_eq!(h.highlights(), [format!("highlight:keyword:{target}")]);
        h.click(&format!("source:keyword:{target}"))?;
        assert!(h.app.session.filter.keyword.is_none(), "one click must clear the matching global filter");
        assert!(h.highlights().is_empty());
        assert_eq!(h.app.session.filter.rating, 5, "clearing the keyword preserves independent filters");
        assert_eq!(h.visible(), [1, 2, 3]);
        assert_eq!(h.app.session.catalog.to_snapshot(), after_merge, "filter navigation does not rewrite keywords");
        assert!(h.frames < 48, "bounded row-only UI frames, without image rendering");
    }
    Ok(())
}

#[test]
fn keyword_highlights_match_only_the_exact_global_hierarchy_path() -> TestResult {
    let mut h = KeywordUi::new(&["Straße", "Straße|Berlin", "Straßenfest", "Work"])?;
    h.click("keywordToggle:Straße")?;
    for (filter, highlight, visible) in [
        ("STRAẞE", Some("Straße"), vec![1, 2]),
        ("STRAẞE|BERLIN", Some("Straße|Berlin"), vec![2]),
        ("Straßenfest", Some("Straßenfest"), vec![3]),
        ("STRASSE", None, vec![]),
        ("work", Some("Work"), vec![4]),
        ("Unrelated", None, vec![]),
    ] {
        h.app.run("library.filter", json!({"keyword": filter}))?;
        let expected: Vec<_> = highlight.into_iter().map(|path| format!("highlight:keyword:{path}")).collect();
        assert_eq!(h.highlights(), expected, "exact path selection for {filter}");
        assert_eq!(h.visible(), visible);
    }
    h.click("source:keyword:Straße")?;
    assert_eq!(h.app.session.filter.keyword.as_deref(), Some("Straße"), "an unrelated filter is replaced rather than cleared");
    assert_eq!(h.highlights(), ["highlight:keyword:Straße"]);
    h.app.run("library.source", json!({"kind": "picks"}))?;
    h.app.run("library.filter", json!({"keyword": "STRAẞE"}))?;
    assert!(h.highlights().is_empty(), "a scoped keyword filter is not a selected global row");
    h.click("source:keyword:Straße")?;
    assert_eq!(h.app.session.source, LibrarySource::All);
    assert_eq!(h.app.session.filter.keyword.as_deref(), Some("Straße"), "a scoped matching filter navigates globally before it can be cleared");
    assert_eq!(h.highlights(), ["highlight:keyword:Straße"]);
    assert_eq!(h.visible(), [1, 2]);
    h.click("source:keyword:Straße")?;
    assert!(h.app.session.filter.keyword.is_none());
    assert!(h.highlights().is_empty());
    assert_eq!(h.visible(), [1, 2, 3, 4]);
    assert!(h.frames < 40);
    Ok(())
}
