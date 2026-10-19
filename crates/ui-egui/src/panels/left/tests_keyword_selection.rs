//! Keyword selection through the real sidebar rows and engine commands, without photo rendering.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lightcraft_catalog::{MemStore, Op, Photo, PhotoId, Source, Store};
use lightcraft_engine::library::LibraryStores;
use lightcraft_engine::{LibrarySource, Session};
use serde_json::json;

use super::{keyword_rows, library_filter};
use crate::headless::Headless;
use crate::state::{RightPanel, ViewMode};
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
        let mut output = self.ctx.run_ui(raw, |ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                let tree = app.caches.keyword_tree(&app.session.catalog);
                keyword_rows(app, ui, &tree, 0.0);
            });
        });
        output.textures_delta.clear();
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

#[test]
fn followed_legacy_keyword_alias_highlights_and_first_click_clears() -> TestResult {
    let mut h = KeywordUi::new(&["Target|CAFÉ", "Before || CAFÉ | ", "Unrelated"])?;
    h.app.run("library.filter", json!({"rating": 5, "keyword": "Before || CAFÉ | "}))?;
    h.app.run("library.select", json!({"ids": [2]}))?;
    h.app.run("keyword.merge", json!({"from": ["Before"], "into": "Target"}))?;
    assert_eq!(h.app.session.filter.keyword.as_deref(), Some("Target|| CAFÉ | "));
    assert_eq!(h.visible(), [1, 2]);
    assert_eq!(h.app.session.active(), Some(PhotoId(2)));
    let metadata = h.app.session.catalog.to_snapshot();
    h.click("keywordToggle:Target")?;
    assert_eq!(h.highlights(), ["highlight:keyword:Target|CAFÉ"]);
    h.click("source:keyword:Target|CAFÉ")?;
    assert!(h.app.session.filter.keyword.is_none(), "the first click clears the matching legacy alias");
    assert!(h.highlights().is_empty());
    assert_eq!(h.visible(), [1, 2, 3]);
    assert_eq!(h.app.session.filter.rating, 5);
    assert_eq!(h.app.session.catalog.to_snapshot(), metadata, "navigation does not normalize stored metadata");
    assert!(h.frames < 16, "bounded existing row-only harness without image rendering");
    Ok(())
}

// Existing unsaved-UI test Store: deterministic append refusal, no filesystem.
#[derive(Clone, Default)]
struct Flaky {
    files: MemStore,
    fail: Arc<AtomicBool>,
}

impl Store for Flaky {
    fn read(&mut self, name: &str) -> std::io::Result<Option<Vec<u8>>> {
        self.files.read(name)
    }
    fn write_atomic(&mut self, name: &str, data: &[u8]) -> std::io::Result<()> {
        self.files.write_atomic(name, data)
    }
    fn append(&mut self, name: &str, data: &[u8]) -> std::io::Result<()> {
        if self.fail.load(Ordering::SeqCst) {
            return Err(std::io::Error::other("disk full"));
        }
        self.files.append(name, data)
    }
    fn truncate(&mut self, name: &str, len: u64) -> std::io::Result<()> {
        self.files.truncate(name, len)
    }
    fn describe(&self) -> String {
        "flaky".into()
    }
}

// Hold jobs exactly as existing navigation layout tests do; no decoder or settle work.
struct HeldSourceRenders;

impl crate::render::RenderOffload for HeldSourceRenders {
    fn try_start(&mut self, _: crate::render::Slot, job: lightcraft_engine::media::RenderJob) -> Option<lightcraft_engine::media::RenderJob> {
        Some(job)
    }

    fn finished(&mut self) -> Vec<(crate::render::Slot, lightcraft_engine::media::RenderResult, f64)> {
        Vec::new()
    }
}

#[test]
fn applied_unsaved_source_navigation_invalidates_follow_for_the_kept_active_photo() -> TestResult {
    let store = Flaky::default();
    let mut session = Session::new();
    let stores = LibraryStores { dir: "flaky".into(), catalog: Box::new(store.clone()), files: Box::new(MemStore::new()), on_disk: false };
    session.open_library_in(stores, false)?;
    for id in 1..=40 {
        let mut photo = Photo::new(PhotoId(id), Source::Demo { scene: 0 }, &format!("{id}.jpg"), "JPEG", 4, 3, "2026-04-20T12:00:00");
        photo.captured = Some(if id == 1 { "2026-04-01T12:00:00" } else { "2026-04-20T12:00:00" }.into());
        photo.meta.keywords = vec!["Target".into()];
        session.commit("fixture", Op::AddPhoto { photo: Box::new(photo) })?;
    }
    let album = session.execute("album.create", &json!({"name": "One photo"}))?["id"].as_u64().ok_or("missing album id")?;
    session.execute("library.select", &json!({"ids": [1]}))?;
    session.execute("album.addPhotos", &json!({"id": album, "ids": [1]}))?;
    session.execute("library.source", &json!({"kind": "album", "id": album}))?;
    session.execute("library.select", &json!({"ids": [1]}))?;
    session.sort.group = lightcraft_catalog::GroupBy::None;
    let mut app = LightcraftApp::new(session, Services::default());
    app.ui.view = ViewMode::PhotoGrid;
    app.ui.left_panel = false;
    app.ui.right = RightPanel::None;
    app.ui.filter_bar = false;
    app.ui.thumb_size = 240.0;
    app.renderer = crate::render::Renderer::with_worker_threads(1);
    app.renderer.set_offload(Box::new(HeldSourceRenders));
    let mut h = Headless::new(app, [1000.0, 600.0], 1.0);
    for _ in 0..24 {
        h.step();
    }
    assert_eq!(h.app.session.visible(), [PhotoId(1)]);
    assert_eq!(h.app.grid_scroll, Some(0.0));
    let ctx = h.view.ctx.clone();
    let key = egui::Id::new("grid-follow-active");
    assert!(ctx.data(|data| data.get_temp::<(Option<PhotoId>, u64)>(key)).is_some(), "real grid primed follow state");
    let undo_before = h.app.session.undo.len();
    h.app.run("develop.beginInteraction", json!({"label": "Exposure"}))?;
    h.app.run("develop.set", json!({"control": "light.exposure", "value": 1.0}))?;
    assert!(h.app.session.unsaved().is_none());
    store.fail.store(true, Ordering::SeqCst);
    let error = library_filter(&mut h.app, &ctx, json!({"keyword": "Target"})).unwrap_err();
    assert!(error.contains("saved in memory but not written to disk"), "{error}");
    assert_eq!(h.app.session.source, LibrarySource::All);
    assert_eq!(h.app.session.filter.keyword.as_deref(), Some("Target"));
    assert_eq!(h.app.session.visible().len(), 40);
    assert_eq!(h.app.session.visible().last(), Some(&PhotoId(1)), "the retained active photo moves to the last row");
    assert_eq!(h.app.session.selection, lightcraft_engine::Selection::single(PhotoId(1)));
    assert!(h.app.session.interaction.is_none());
    assert_eq!(h.app.session.undo.len(), undo_before + 1);
    assert!(h.app.session.unsaved().is_some_and(|(n, _)| n > 0), "the failed append keeps the edit queued");
    assert_eq!(h.app.session.develop_of(PhotoId(1)).unwrap().light.exposure, 1.0);
    assert!(ctx.data(|data| data.get_temp::<(Option<PhotoId>, u64)>(key)).is_none(), "applied NotSaved navigation must invalidate follow");
    for _ in 0..24 {
        h.step();
    }
    assert!(h.app.grid_scroll.unwrap() > 500.0, "actual Headless grid follows the retained active photo");
    let cell = h.app.widgets.iter().find(|(id, _)| id == "thumb:1").ok_or("active cell was not drawn")?.1;
    assert!(cell.intersects(h.app.canvas_rect.ok_or("grid canvas missing")?), "active cell is on screen");
    store.fail.store(false, Ordering::SeqCst);
    h.app.session.persist()?;
    assert!(h.app.session.unsaved().is_none());
    h.app.run("edit.undo", json!({}))?;
    assert_eq!(h.app.session.develop_of(PhotoId(1)).unwrap().light.exposure, 0.0);
    h.app.run("edit.redo", json!({}))?;
    assert_eq!(h.app.session.develop_of(PhotoId(1)).unwrap().light.exposure, 1.0);
    assert_eq!(h.app.session.source, LibrarySource::All);
    assert_eq!(h.app.session.active(), Some(PhotoId(1)));
    assert!(h.frames() < 50, "fixed frames, no settle loops or image decoding");
    Ok(())
}
