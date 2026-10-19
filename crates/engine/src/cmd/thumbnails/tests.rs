use super::*;
use lightcraft_catalog::{Op, Source};
use lightcraft_pipeline::SourceInfo;
use lightcraft_raster::Rgb32f;
use std::sync::atomic::AtomicUsize;

fn originals(n: u64) -> (Session, Arc<AtomicUsize>) {
    let mut s = Session::new();
    s.set_thumbnail_renderer(true);
    for i in 1..=n {
        let p = Photo::new(PhotoId(i), Source::File { path: format!("original-{i}.jpg") }, "original", "JPEG", 6000, 4000, "2026-01-01");
        s.commit("Import", Op::AddPhoto { photo: Box::new(p) }).unwrap();
    }
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    s.media.file_loader = Some(Arc::new(move |_, edge| {
        assert!(crate::memory::is_background());
        assert!(edge <= 512, "no preview/full source requested");
        count.fetch_add(1, Ordering::Relaxed);
        Ok((Rgb32f::filled(24, 16, [0.15, 0.3, 0.5]), SourceInfo::default()))
    }));
    (s, calls)
}

fn drain(s: &mut Session) {
    for _ in 0..1024 {
        if let Some(job) = s.next_thumbnail_job() {
            let r = std::thread::spawn(move || crate::memory::in_background(|| job.run())).join().unwrap();
            s.complete_thumbnail_job(&r);
        }
        if s.preview_build.as_ref().unwrap().finished.load(Ordering::Relaxed) {
            return;
        }
    }
    panic!("thumbnail plan did not finish");
}

#[test]
fn offscreen_originals_warm_the_normal_grid_keys_without_caller_io() {
    let (mut s, calls) = originals(4);
    let caller = std::thread::current().id();
    let original_loader = s.media.file_loader.clone().unwrap();
    s.media.file_loader = Some(Arc::new(move |path, edge| {
        assert_ne!(std::thread::current().id(), caller, "no file I/O on the UI thread");
        original_loader(path, edge)
    }));
    s.selection.ids = vec![PhotoId(1)];
    build(&mut s, &Value::Null).unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), 0);
    let first = s.next_thumbnail_job().unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), 0, "constructing a job does not open the original");
    assert!(s.next_thumbnail_job().is_none(), "only one job may be outstanding");
    let r = std::thread::spawn(move || crate::memory::in_background(|| first.run())).join().unwrap();
    s.complete_thumbnail_job(&r);
    drain(&mut s);
    for i in 1..=4 {
        for edge in [90, 220, 330, 480, 960] {
            let job = s.thumb_job(PhotoId(i), edge).unwrap();
            let (cache, key) = job.cache.as_ref().unwrap();
            assert!(cache.get_at(job.cache_generation, *key).is_some(), "offscreen {i}/{edge}");
        }
    }
    assert_eq!(calls.load(Ordering::Relaxed), 4, "one bounded thumbnail source per photo");
    let (_, previews, full) = s.media.usage();
    assert_eq!((previews.count, full.count), (0, 0));
    let state = s.preview_build.as_ref().unwrap();
    assert_eq!(state.done.load(Ordering::Relaxed), 4);
    assert_eq!(state.failed.load(Ordering::Relaxed), 0);
}

#[test]
fn errors_do_not_stop_later_originals_or_repeat_all_four_sizes() {
    let (mut s, calls) = originals(2);
    let good = s.media.file_loader.clone().unwrap();
    s.media.file_loader =
        Some(Arc::new(move |path, edge| if path.ends_with("1.jpg") { Err("original unavailable".into()) } else { good(path, edge) }));
    build(&mut s, &Value::Null).unwrap();
    drain(&mut s);
    let state = s.preview_build.as_ref().unwrap();
    assert_eq!(state.done.load(Ordering::Relaxed), 2);
    assert_eq!(state.failed.load(Ordering::Relaxed), 1);
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    let job = s.thumb_job(PhotoId(2), 220).unwrap();
    assert!(job.cache.as_ref().unwrap().0.get(job.cache.as_ref().unwrap().1).is_some());
}

#[test]
fn cancel_rejects_a_completed_result_and_drops_the_remaining_manifest() {
    let (mut s, _) = originals(3);
    build(&mut s, &Value::Null).unwrap();
    let job = s.next_thumbnail_job().unwrap();
    let state = s.preview_build.as_ref().unwrap().clone();
    s.cancel_thumbnail_build();
    let r = crate::memory::in_background(|| job.run());
    s.complete_thumbnail_job(&r);
    assert!(s.next_thumbnail_job().is_none());
    assert!(state.cancel.load(Ordering::Relaxed));
    assert!(state.finished.load(Ordering::Relaxed));
    assert_eq!(state.done.load(Ordering::Relaxed), 0);
    assert_eq!(s.media.usage().0.count, 0, "late source not accepted");
}

#[test]
fn cache_replacement_and_clear_fence_results_and_deferred_imports() {
    for replace in [false, true] {
        let (mut s, _) = originals(2);
        build(&mut s, &Value::Null).unwrap();
        let job = s.next_thumbnail_job().unwrap();
        build(&mut s, &json!({"defer": true})).unwrap();
        let state = s.preview_build.as_ref().unwrap().clone();
        if replace {
            s.media.rendered.retire();
            s.media.rendered = Arc::new(PreviewCache::memory(1 << 20));
        } else {
            s.media.rendered.clear();
        }
        let r = crate::memory::in_background(|| job.run());
        s.complete_thumbnail_job(&r);
        assert!(s.next_thumbnail_job().is_none());
        assert!(state.cancel.load(Ordering::Relaxed));
        assert_eq!(s.media.usage().0.count, 0);
        assert_eq!(s.media.rendered.mem_usage().0, 0);
    }
}

#[test]
fn undo_delete_and_edits_skip_obsolete_targets_without_retaining_their_sources() {
    let (mut s, _) = originals(3);
    build(&mut s, &Value::Null).unwrap();
    let job = s.next_thumbnail_job().unwrap();
    s.catalog.apply(Op::SetRating { id: PhotoId(1), rating: 3 }).unwrap();
    let r = crate::memory::in_background(|| job.run());
    s.complete_thumbnail_job(&r);
    assert_eq!(s.media.usage().0.count, 0);
    s.catalog.apply(Op::SetDeleted { id: PhotoId(2), deleted: true }).unwrap();
    s.undo_step().unwrap(); // undo import of 3
    drain(&mut s);
    let state = s.preview_build.as_ref().unwrap();
    assert_eq!(state.done.load(Ordering::Relaxed), 3);
    assert_eq!(state.skipped.load(Ordering::Relaxed), 3);
    assert_eq!(state.failed.load(Ordering::Relaxed), 0);
}

#[test]
fn repeated_build_reuses_cache_and_edit_or_content_change_has_a_new_key() {
    let (mut s, calls) = originals(1);
    build(&mut s, &json!({"wait": true})).unwrap();
    s.media.clear_sources();
    build(&mut s, &json!({"wait": true})).unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), 1, "manual trigger reuses existing thumbnails");
    let old = s.thumb_job(PhotoId(1), 220).unwrap().cache.unwrap().1;
    s.execute("library.select", &json!({"ids": [1]})).unwrap();
    s.execute("develop.set", &json!({"id": 1, "control": "light.exposure", "value": 1.0})).unwrap();
    let edited = s.thumb_job(PhotoId(1), 220).unwrap().cache.unwrap().1;
    assert_ne!(old, edited);
    assert!(s.media.rendered.get(edited).is_none());
    build(&mut s, &json!({"wait": true})).unwrap();
    assert!(s.media.rendered.get(edited).is_some());
    let mut p = (**s.catalog.photo(PhotoId(1)).unwrap()).clone();
    p.content_hash = Some("replacement content".into());
    s.catalog.apply(Op::RemovePhoto { id: PhotoId(1) }).unwrap();
    s.catalog.apply(Op::AddPhoto { photo: Box::new(p) }).unwrap();
    let replaced = s.thumb_job(PhotoId(1), 220).unwrap().cache.unwrap().1;
    assert_ne!(edited, replaced);
    assert!(s.media.rendered.get(replaced).is_none());
}

#[test]
fn import_trigger_coalesces_behind_existing_preview_work_and_excludes_local_or_deleted() {
    let (mut s, _) = originals(3);
    s.catalog.apply(Op::SetLocal { id: PhotoId(2), local: true }).unwrap();
    s.catalog.apply(Op::SetDeleted { id: PhotoId(3), deleted: true }).unwrap();
    let prior = Arc::new(PreviewBuild::default());
    s.preview_build = Some(prior.clone());
    assert!(build(&mut s, &Value::Null).is_err());
    build(&mut s, &json!({"defer": true})).unwrap();
    build(&mut s, &json!({"defer": true})).unwrap();
    assert!(s.next_thumbnail_job().is_none());
    prior.finished.store(true, Ordering::Relaxed);
    let job = s.next_thumbnail_job().unwrap();
    assert_eq!(s.preview_build.as_ref().unwrap().total, 1);
    let r = crate::memory::in_background(|| job.run());
    s.complete_thumbnail_job(&r);
    drain(&mut s);
    assert!(s.thumbnail_import_pending.is_none());
    assert!(build(&mut s, &json!({"wait": true, "defer": true})).is_err());
}

#[test]
fn a_large_manifest_retains_no_photos_and_thumbnail_sources_obey_the_existing_budget() {
    let (mut s, _) = originals(32);
    let p = s.catalog.photo(PhotoId(1)).unwrap().clone();
    let references = Arc::strong_count(&p);
    s.media.set_budget(16 << 10);
    s.media.rendered.set_mem_budget(16 << 10);
    build(&mut s, &Value::Null).unwrap();
    assert_eq!(Arc::strong_count(&p), references, "manifest holds ids, not photo histories");
    for _ in 0..256 {
        if let Some(job) = s.next_thumbnail_job() {
            assert_eq!(Arc::strong_count(&p), references);
            let r = crate::memory::in_background(|| job.run());
            s.complete_thumbnail_job(&r);
            let (thumbs, previews, full) = s.media.usage();
            assert!(thumbs.bytes <= 16 << 10);
            assert!(thumbs.count < 4, "source LRU evicts earlier originals");
            assert_eq!((previews.count, full.count), (0, 0));
            // The existing LRU keeps one oversized entry; even that is a normal 512 px thumbnail.
            assert!(s.media.rendered.mem_usage().0 <= 1);
            assert!(s.media.rendered.mem_usage().1 <= 512 * 512 * 4 + 64);
        }
        if s.preview_build.as_ref().unwrap().finished.load(Ordering::Relaxed) {
            return;
        }
    }
    panic!("bounded build did not finish");
}

#[test]
fn offscreen_originals_survive_memory_eviction_in_the_library_disk_cache() {
    let dir = std::env::temp_dir().join(format!("lc-background-thumbnails-{}-{}", std::process::id(), lightcraft_preview::next_tick()));
    let (mut s, calls) = originals(3);
    s.media.attach_disk_cache(&dir, 1 << 20);
    build(&mut s, &json!({"wait": true})).unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), 3);
    s.media.clear_sources();
    s.media.attach_disk_cache(&dir, 1 << 20); // new memory cache, same library files
    for i in 1..=3 {
        for edge in THUMB_SIZES {
            let job = s.thumb_job(PhotoId(i), edge).unwrap();
            let r = std::thread::spawn(move || crate::memory::in_background(|| job.run())).join().unwrap();
            assert!(r.rendered.is_ok());
            assert!(r.loaded.is_none(), "reused the exact disk thumbnail, not the original");
        }
    }
    assert_eq!(calls.load(Ordering::Relaxed), 3);
    s.media.rendered.retire();
    std::fs::remove_dir_all(dir).unwrap();
}

#[test]
fn command_defaults_finish_without_a_renderer_and_reject_unserviced_background_work() {
    let (mut s, calls) = originals(2);
    s.set_thumbnail_renderer(false);
    assert!(build(&mut s, &json!({"wait": false})).is_err());
    assert!(!s.thumbnail_build_running());
    let progress = build(&mut s, &Value::Null).unwrap();
    assert_eq!(progress["running"], false);
    assert_eq!(progress["done"], 2);
    assert_eq!(calls.load(Ordering::Relaxed), 2);
}

#[test]
fn library_open_identity_fences_jobs_and_deferred_imports_without_cache_or_catalog_replacement() {
    for deferred in [false, true] {
        let (mut s, _) = originals(1);
        let token = s.thumbnail_library();
        let cache = s.media.rendered.clone();
        let revision = s.catalog.revision;
        let prior = Arc::new(PreviewBuild::default());
        let job = if deferred {
            s.preview_build = Some(prior.clone());
            build(&mut s, &json!({"defer": true})).unwrap();
            None
        } else {
            build(&mut s, &Value::Null).unwrap();
            s.next_thumbnail_job()
        };
        let state = s.preview_build.as_ref().unwrap().clone();
        // Successful open/close refreshes this upstream identity even when cache/revision stay.
        s.library_identity = Arc::new(());
        assert!(!token.matches(&s));
        assert_eq!(s.catalog.revision, revision);
        assert!(Arc::ptr_eq(&cache, &s.media.rendered));
        prior.finished.store(true, Ordering::Relaxed);
        if let Some(job) = job {
            let r = crate::memory::in_background(|| job.run());
            s.complete_thumbnail_job(&r);
            assert!(state.cancel.load(Ordering::Relaxed));
            assert!(state.finished.load(Ordering::Relaxed));
        }
        assert!(s.next_thumbnail_job().is_none());
        assert!(!s.thumbnail_build_running());
        assert_eq!(s.media.usage().0.count, 0, "old opening cannot accept an original");
    }
}

#[test]
fn targets_moved_to_local_after_the_manifest_was_created_are_skipped() {
    let (mut s, calls) = originals(2);
    build(&mut s, &Value::Null).unwrap();
    s.catalog.apply(Op::SetLocal { id: PhotoId(1), local: true }).unwrap();
    drain(&mut s);
    let state = s.preview_build.as_ref().unwrap();
    assert_eq!(state.done.load(Ordering::Relaxed), 2);
    assert_eq!(state.skipped.load(Ordering::Relaxed), 1);
    assert_eq!(calls.load(Ordering::Relaxed), 1, "the Local original was never opened");
}
