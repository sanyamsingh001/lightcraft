//! Actual headless command/menu and Cancel route; pixels are deliberately held by a mock host.
use crate::render::{RenderOffload, Slot};
use crate::{LightcraftApp, Services, headless::Headless};
use lightcraft_engine::Session;
use lightcraft_engine::media::{RenderJob, RenderResult};
use serde_json::json;
use std::sync::atomic::Ordering;
use std::time::Duration;

struct Paused;
impl RenderOffload for Paused {
    fn try_start(&mut self, _: Slot, job: RenderJob) -> Option<RenderJob> {
        Some(job)
    }
    fn finished(&mut self) -> Vec<(Slot, RenderResult, f64)> {
        vec![]
    }
}

#[test]
fn headless_menu_build_shows_progress_and_cancel_dispatches_the_existing_command() {
    let mut app = LightcraftApp::new(Session::with_demo(), Services { png: None, ..Default::default() });
    app.ui.settings.gpu = false;
    app.renderer.set_offload(Box::new(Paused));
    let mut h = Headless::new(app, [1200.0, 800.0], 1.0);
    let timeout = Duration::from_secs(30);
    h.step();
    let menu = h.request("ui.menu.tree", json!({}), timeout);
    assert!(menu.to_string().contains("library.buildThumbnails"));
    let r = h.request("ui.menu.invoke", json!({"command": "library.buildThumbnails"}), timeout);
    assert_eq!(r["ok"], true, "{r}");
    assert!(h.step_until(timeout, |h| h.app.widgets.iter().any(|(id, _)| id == "button:thumbnailCancel")));
    let state = h.app.session.preview_build.as_ref().unwrap().clone();
    assert!(state.total > 1);
    let cancel = h.app.widgets.iter().find(|(id, _)| id == "button:thumbnailCancel").unwrap().1;
    assert!(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1200.0, 800.0)).contains_rect(cancel));
    assert!(h.busy());
    let r = h.request("ui.clickWidget", json!({"id": "button:thumbnailCancel"}), timeout);
    assert_eq!(r["ok"], true, "{r}");
    h.step();
    assert!(state.cancel.load(Ordering::Relaxed));
    assert!(state.finished.load(Ordering::Relaxed));
    assert!(!h.app.session.thumbnail_build_running());
    assert!(!h.app.widgets.iter().any(|(id, _)| id == "button:thumbnailCancel"));
}

#[test]
fn switching_a_memory_library_cancels_the_thumbnail_manifest_even_without_a_new_cache() {
    use lightcraft_catalog::MemStore;
    use lightcraft_engine::library::LibraryStores;
    let mut s = Session::with_demo();
    s.set_thumbnail_renderer(true);
    s.execute("library.buildThumbnails", &json!({})).unwrap();
    let job = s.next_thumbnail_job().unwrap();
    let state = s.preview_build.as_ref().unwrap().clone();
    let cache = s.media.rendered.clone();
    s.open_library_in(
        LibraryStores {
            dir: "thumbnail-memory-library".into(),
            catalog: Box::new(MemStore::default()),
            files: Box::new(MemStore::default()),
            on_disk: false,
        },
        false,
    )
    .unwrap();
    assert!(std::sync::Arc::ptr_eq(&cache, &s.media.rendered), "memory-library switch does not replace this cache");
    assert!(state.cancel.load(Ordering::Relaxed));
    assert!(state.finished.load(Ordering::Relaxed));
    let r = lightcraft_engine::memory::in_background(|| job.run());
    s.complete_thumbnail_job(&r);
    assert!(s.next_thumbnail_job().is_none());
    assert!(!s.thumbnail_build_running());
    assert_eq!(s.media.usage().0.count, 0);
}

fn visit(s: &egui::epaint::Shape, viewport: egui::Rect) {
    match s {
        egui::epaint::Shape::Vec(v) => {
            for s in v {
                visit(s, viewport)
            }
        }
        egui::epaint::Shape::Text(t) => assert!(
            viewport.expand(0.5).contains_rect(t.visual_bounding_rect()),
            "{}: {:?} outside {:?}",
            t.galley.text(),
            t.visual_bounding_rect(),
            viewport
        ),
        _ => {}
    }
}

#[test]
fn translated_thumbnail_progress_paints_within_the_headless_viewport() {
    use crate::{i18n, theme, widgets};
    for language in [i18n::Locale::En, i18n::Locale::De, i18n::Locale::PtBr, i18n::Locale::Es] {
        for scale in [1.0, 2.0] {
            for width in [320.0, 420.0, 1200.0] {
                i18n::set_language(language);
                let ctx = egui::Context::default();
                theme::install_fonts(&ctx);
                theme::apply(&ctx);
                let mut app = LightcraftApp::new(Session::new(), Services::default());
                app.session.preview_build = Some(std::sync::Arc::new(lightcraft_engine::cmd::previews::PreviewBuild {
                    total: 1300,
                    what: "thumbnails",
                    ..Default::default()
                }));
                app.session.preview_build.as_ref().unwrap().done.store(875, Ordering::Relaxed);
                let size = egui::vec2(width, 280.0);
                let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
                for frame in 0..3 {
                    let mut out = ctx.run_ui(crate::headless::HeadlessView::raw_input(size, scale, frame as f64 / 60.0, vec![]), |_| {
                        crate::panels::thumbnail_progress(&mut app, &ctx)
                    });
                    out.textures_delta.clear();
                    let registry = widgets::take_registry(&ctx);
                    if frame == 2 {
                        for shape in out.shapes {
                            visit(&shape.shape, viewport);
                        }
                        let cancel = registry.iter().find(|(id, _)| id == "button:thumbnailCancel").unwrap().1;
                        assert!(viewport.contains_rect(cancel));
                    }
                }
            }
        }
    }
    i18n::set_language(i18n::Locale::En);
}
