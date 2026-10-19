use super::*;
use lightcraft_catalog::{Op, Photo, Source};
use lightcraft_pipeline::SourceInfo;
use lightcraft_raster::Rgb32f;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::sync::Mutex;
use std::sync::atomic::Ordering;

fn originals(n: u64) -> Session {
    let mut s = Session::new();
    s.set_thumbnail_renderer(true);
    for i in 1..=n {
        let p = Photo::new(PhotoId(i), Source::File { path: format!("original-{i}.jpg") }, "original", "JPEG", 6000, 4000, "2026-01-01");
        s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    s.media.file_loader = Some(Arc::new(|_, _| Ok((Rgb32f::filled(24, 16, [0.2, 0.3, 0.4]), SourceInfo::default()))));
    s
}

#[test]
fn foreground_requests_run_before_the_single_offscreen_job_and_eviction_keeps_it() {
    let mut s = originals(20);
    let order = Arc::new(Mutex::new(Vec::new()));
    let seen = order.clone();
    s.media.file_loader = Some(Arc::new(move |path, edge| {
        seen.lock().unwrap().push((path.to_string(), lightcraft_engine::memory::is_background()));
        assert!(edge <= 512);
        Ok((Rgb32f::filled(24, 16, [0.2, 0.3, 0.4]), SourceInfo::default()))
    }));
    let mut renderer = Renderer::with_worker_threads(1);
    renderer.pool = JobPool::new(0);
    let ctx = egui::Context::default();
    s.execute("library.buildThumbnails", &Value::Null).unwrap();
    renderer.build_thumbnails(&mut s);
    renderer.build_thumbnails(&mut s);
    assert_eq!(renderer.queued(), 1);
    renderer.evict_thumbs(&HashSet::new(), 96);
    assert_eq!(renderer.queued(), 1, "scrolling does not drop the library job");
    renderer.request(Slot::Main, s.thumb_job(PhotoId(20), 128).unwrap(), 100);
    renderer.pool.run_inline(1);
    renderer.poll(&ctx, &mut s);
    assert_eq!(order.lock().unwrap().as_slice(), &[("original-20.jpg".into(), false)]);
    assert_eq!(s.preview_build.as_ref().unwrap().done.load(Ordering::Relaxed), 0);
    renderer.pool.run_inline(1);
    renderer.poll(&ctx, &mut s);
    assert_eq!(order.lock().unwrap().get(1), Some(&("original-1.jpg".into(), true)));
    assert_eq!(renderer.textures.len(), 1, "only the foreground view owns a texture");
    assert_eq!(renderer.thumb_textures(), 0);
}

#[test]
fn cancellation_drops_queued_io_and_library_switch_rejects_late_results() {
    let mut s = originals(3);
    let calls = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let seen = calls.clone();
    s.media.file_loader = Some(Arc::new(move |_, _| {
        seen.fetch_add(1, Ordering::Relaxed);
        Ok((Rgb32f::filled(24, 16, [0.2, 0.3, 0.4]), SourceInfo::default()))
    }));
    let mut renderer = Renderer::with_worker_threads(1);
    renderer.pool = JobPool::new(0);
    let ctx = egui::Context::default();
    s.execute("library.buildThumbnails", &Value::Null).unwrap();
    renderer.build_thumbnails(&mut s);
    s.execute("library.cancelPreviews", &Value::Null).unwrap();
    renderer.poll(&ctx, &mut s);
    renderer.pool.run_inline(1);
    assert_eq!(calls.load(Ordering::Relaxed), 0, "cancelled queued work never opens a file");
    s.execute("library.buildThumbnails", &Value::Null).unwrap();
    renderer.build_thumbnails(&mut s);
    renderer.pool.run_inline(1); // completed but not accepted yet
    let old = s.preview_build.as_ref().unwrap().clone();
    s.media.rendered.retire();
    s.media.rendered = Arc::new(lightcraft_preview::PreviewCache::memory(1 << 20));
    renderer.poll(&ctx, &mut s);
    assert!(old.cancel.load(Ordering::Relaxed));
    assert_eq!(s.media.usage().0.count, 0);
    assert!(renderer.textures.is_empty());
    assert_eq!(renderer.in_flight(), 0);
    assert!(s.next_thumbnail_job().is_none());
}

#[test]
fn native_offscreen_build_has_no_ui_thread_io_and_retains_no_textures() {
    let mut s = originals(4);
    let caller = std::thread::current().id();
    s.media.file_loader = Some(Arc::new(move |_, edge| {
        assert_ne!(std::thread::current().id(), caller);
        assert!(lightcraft_engine::memory::is_background());
        assert!(edge <= 512);
        Ok((Rgb32f::filled(24, 16, [0.2, 0.3, 0.4]), SourceInfo::default()))
    }));
    let mut renderer = Renderer::with_worker_threads(1);
    let ctx = egui::Context::default();
    s.execute("library.buildThumbnails", &json!({})).unwrap();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !s.preview_build.as_ref().unwrap().finished.load(Ordering::Relaxed) {
        renderer.poll(&ctx, &mut s);
        renderer.build_thumbnails(&mut s);
        assert!(renderer.queued() <= 1);
        assert!(renderer.in_flight() <= 1);
        assert!(renderer.textures.is_empty());
        assert!(std::time::Instant::now() < until);
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    let job = s.thumb_job(PhotoId(4), 220).unwrap();
    assert!(job.cache.as_ref().unwrap().0.get(job.cache.as_ref().unwrap().1).is_some());
    let (_, previews, full) = s.media.usage();
    assert_eq!((previews.count, full.count), (0, 0));
}

#[test]
fn restarting_after_cancel_waits_for_the_old_decode_and_leaves_a_worker_for_foreground() {
    use std::sync::mpsc::channel;
    use std::time::{Duration, Instant};
    let mut s = originals(3);
    let (release, wait) = channel();
    let wait = Arc::new(Mutex::new(Some(wait)));
    let (started, began) = channel();
    s.media.file_loader = Some(Arc::new(move |_, _| {
        let blocked = { wait.lock().unwrap().take() };
        if let Some(wait) = blocked {
            let _ = started.send(());
            let _ = wait.recv();
        }
        Ok((Rgb32f::filled(24, 16, [0.2, 0.3, 0.4]), SourceInfo::default()))
    }));
    let mut renderer = Renderer::with_worker_threads(2);
    let ctx = egui::Context::default();
    s.execute("library.buildThumbnails", &Value::Null).unwrap();
    renderer.build_thumbnails(&mut s);
    began.recv_timeout(Duration::from_secs(10)).unwrap();
    for _ in 0..8 {
        s.execute("library.cancelPreviews", &Value::Null).unwrap();
        renderer.poll(&ctx, &mut s);
        s.execute("library.buildThumbnails", &Value::Null).unwrap();
        renderer.build_thumbnails(&mut s);
        assert_eq!(renderer.queued(), 0);
        assert_eq!(renderer.in_flight(), 1, "only the original physical job is retained");
    }
    renderer.request(Slot::Main, s.thumb_job(PhotoId(3), 128).unwrap(), 100);
    let until = Instant::now() + Duration::from_secs(10);
    while !renderer.textures.contains_key(&Slot::Main) {
        renderer.poll(&ctx, &mut s);
        assert!(Instant::now() < until, "foreground must finish while the old background decode waits");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert!(renderer.thumbnail_request.is_some());
    release.send(()).unwrap();
    let until = Instant::now() + Duration::from_secs(10);
    while renderer.thumbnail_request.is_some() {
        renderer.poll(&ctx, &mut s);
        assert!(Instant::now() < until);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(s.preview_build.as_ref().unwrap().done.load(Ordering::Relaxed), 0, "old completion cannot count toward the restart");
    renderer.build_thumbnails(&mut s);
    assert!(renderer.thumbnail_request.is_some());
}

#[test]
fn discarding_the_render_host_queue_cancels_the_plan_instead_of_leaving_it_pending() {
    let mut s = originals(2);
    let mut renderer = Renderer::with_worker_threads(1);
    renderer.pool = JobPool::new(0);
    s.execute("library.buildThumbnails", &Value::Null).unwrap();
    renderer.build_thumbnails(&mut s);
    let state = s.preview_build.as_ref().unwrap().clone();
    renderer.forget_all();
    renderer.build_thumbnails(&mut s);
    assert!(state.cancel.load(Ordering::Relaxed));
    assert!(state.finished.load(Ordering::Relaxed));
    assert_eq!(renderer.queued(), 0);
    assert!(!s.thumbnail_build_running());
}
