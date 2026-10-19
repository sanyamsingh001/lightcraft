//! Lazy, thumbnail-only library builds. Hosts feed one job at a time to their normal render
//! scheduler, below interactive work. The manifest retains ids, not photos, sources or jobs.

use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use std::sync::{Arc, Weak};

use lightcraft_catalog::{Photo, PhotoId};
use lightcraft_preview::PreviewCache;
use serde_json::{Value, json};

use super::previews::PreviewBuild;
use super::{CommandSpec, always, bad, cmd};
use crate::media::{RenderJob, RenderResult, THUMB_SIZES};
use crate::{Result, Session};

/// Library opening and cache epoch which own a thumbnail build or delayed import trigger.
/// Ordinary catalog edits do not change this identity. Successful open/close and cache clear do.
#[derive(Clone, Debug)]
pub struct ThumbnailLibrary {
    cache: Weak<PreviewCache>,
    generation: u64,
    identity: Weak<()>,
}

impl ThumbnailLibrary {
    pub fn matches(&self, s: &Session) -> bool {
        self.identity.as_ptr() == Arc::as_ptr(&s.library_identity)
            && self.cache.as_ptr() == Arc::as_ptr(&s.media.rendered)
            && self.generation == s.media.rendered.generation()
    }
}

#[derive(Debug)]
struct Current {
    id: PhotoId,
    photo: Weak<Photo>,
    edge: usize,
    key: Option<u64>,
}

#[derive(Debug)]
pub(crate) struct ThumbnailBuild {
    ids: VecDeque<PhotoId>,
    current: Option<Current>,
    library: ThumbnailLibrary,
    state: Arc<PreviewBuild>,
}

impl Drop for ThumbnailBuild {
    fn drop(&mut self) {
        if !self.state.finished.load(Ordering::Relaxed) {
            self.state.cancel.store(true, Ordering::Relaxed);
            self.state.finished.store(true, Ordering::Relaxed);
        }
    }
}

impl ThumbnailBuild {
    fn bound_to(&self, s: &Session) -> bool {
        self.library.matches(s) && s.preview_build.as_ref().is_some_and(|b| Arc::ptr_eq(b, &self.state))
    }

    fn current_valid(&self, s: &Session) -> bool {
        self.current.as_ref().is_some_and(|c| s.catalog.photo(c.id).is_some_and(|p| !p.deleted && !p.local && Arc::as_ptr(p) == c.photo.as_ptr()))
    }

    fn done(&mut self, skipped: bool) {
        self.current = None;
        self.state.done.fetch_add(1, Ordering::Relaxed);
        if skipped {
            self.state.skipped.fetch_add(1, Ordering::Relaxed);
        }
    }
}

impl Session {
    /// Capture an import's thumbnail destination without retaining images or doing file I/O.
    pub fn thumbnail_library(&self) -> ThumbnailLibrary {
        ThumbnailLibrary {
            cache: Arc::downgrade(&self.media.rendered),
            generation: self.media.rendered.generation(),
            identity: Arc::downgrade(&self.library_identity),
        }
    }

    /// Register a host that pumps the lazy thumbnail plan on its normal render scheduler.
    /// Without a host, the command defaults to synchronous work (CLI/MCP); the UI registers
    /// during app construction so its default command path never opens a file on the UI thread.
    pub fn set_thumbnail_renderer(&mut self, available: bool) {
        if !available {
            self.cancel_thumbnail_build();
        }
        self.thumbnail_renderer = available;
    }

    /// Includes catalog-only advancement and a coalesced import waiting for other preview work.
    pub fn thumbnail_build_running(&self) -> bool {
        self.thumbnail_build.is_some() || self.thumbnail_import_pending.is_some()
    }

    /// Stop a thumbnail plan and its deferred import trigger. A running decode may finish its
    /// correctly keyed cache write; its result will not update the session or progress.
    pub fn cancel_thumbnail_build(&mut self) {
        self.thumbnail_import_pending = None;
        self.thumbnail_build = None;
    }

    /// Fence queued/results after cancel, cache replacement/clear, deletion, undo or edits.
    /// Returns whether this plan is awaiting a result. Hosts drop an obsolete queued job.
    pub fn thumbnail_job_pending(&mut self) -> bool {
        let Some(mut build) = self.thumbnail_build.take() else { return false };
        if !build.bound_to(self) || build.state.cancel.load(Ordering::Relaxed) {
            self.thumbnail_import_pending = None;
            return false;
        }
        if build.current.is_some() && !build.current_valid(self) {
            build.done(true);
        }
        let pending = build.current.as_ref().is_some_and(|c| c.key.is_some());
        self.thumbnail_build = Some(build);
        pending
    }

    /// Build at most one thumbnail job, without opening a file or reading the disk cache.
    /// Hosts must call `complete_thumbnail_job` before asking for another. All normal buckets
    /// are warmed, so both the grid (including HiDPI) and the filmstrip reuse the exact keys.
    pub fn next_thumbnail_job(&mut self) -> Option<RenderJob> {
        self.thumbnail_job_pending();
        if self.thumbnail_build.is_none()
            && let Some(library) = &self.thumbnail_import_pending
        {
            if !library.matches(self) {
                self.thumbnail_import_pending = None;
            } else if self.preview_build.as_ref().is_none_or(|b| b.finished.load(Ordering::Relaxed)) {
                self.thumbnail_import_pending = None;
                // The import already dispatched library.buildThumbnails with defer=true.
                let _ = build(self, &Value::Null);
            }
        }
        let mut build = self.thumbnail_build.take()?;
        // Bound catalog-only work per frame too (a large import may have just been undone).
        for _ in 0..64 {
            if build.current.is_none() {
                let Some(id) = build.ids.pop_front() else {
                    build.state.finished.store(true, Ordering::Relaxed);
                    return None;
                };
                let Some(photo) = self.catalog.photo(id).filter(|p| !p.deleted && !p.local) else {
                    build.done(true);
                    continue;
                };
                build.current = Some(Current { id, photo: Arc::downgrade(photo), edge: 0, key: None });
            }
            let Some(current) = &build.current else { break };
            if current.key.is_some() {
                break;
            }
            let Some(&edge) = THUMB_SIZES.get(current.edge) else {
                build.done(false);
                continue;
            };
            let Some(job) = self.thumb_job(current.id, edge) else {
                build.done(true);
                continue;
            };
            if let Some(current) = &mut build.current {
                current.key = Some(job.key);
            }
            self.thumbnail_build = Some(build);
            return Some(job);
        }
        self.thumbnail_build = Some(build);
        None
    }

    /// A cache-only result: accept only the current thumbnail source into the existing bounded
    /// source LRU. Never retain an offscreen texture, loupe render or full-resolution source.
    pub fn complete_thumbnail_job(&mut self, r: &RenderResult) {
        self.thumbnail_job_pending();
        let Some(mut build) = self.thumbnail_build.take() else { return };
        if build.current.as_ref().is_some_and(|c| c.id == r.photo && c.key == Some(r.key)) {
            if r.rendered.is_err() {
                build.state.failed.fetch_add(1, Ordering::Relaxed);
                build.done(false);
            } else {
                self.accept(r);
                if let Some(current) = &mut build.current {
                    current.key = None;
                    current.edge += 1;
                    if current.edge == THUMB_SIZES.len() {
                        build.done(false);
                    }
                }
            }
        }
        self.thumbnail_build = Some(build);
    }
}

fn build(s: &mut Session, p: &Value) -> Result<Value> {
    const C: &str = "library.buildThumbnails";
    let wait = p.get("wait").and_then(Value::as_bool).unwrap_or(!s.thumbnail_renderer);
    let defer = p.get("defer").and_then(Value::as_bool).unwrap_or(false);
    if !wait && !s.thumbnail_renderer {
        return Err(bad(C, "background thumbnails require a registered renderer; use wait=true"));
    }
    if defer && wait {
        return Err(bad(C, "wait and defer cannot be combined"));
    }
    if s.preview_build.as_ref().is_some_and(|b| !b.finished.load(Ordering::Relaxed)) {
        if defer {
            s.thumbnail_import_pending = Some(s.thumbnail_library());
            return Ok(json!({"deferred": true}));
        }
        return Err(bad(C, "a preview build is already running"));
    }
    s.thumbnail_import_pending = None;
    let ids: VecDeque<_> = s.catalog.photos().filter(|p| !p.deleted && !p.local).map(|p| p.id).collect();
    let state = Arc::new(PreviewBuild { total: ids.len(), what: "thumbnails", ..Default::default() });
    s.preview_build = Some(state.clone());
    s.thumbnail_build = Some(ThumbnailBuild { ids, current: None, library: s.thumbnail_library(), state: state.clone() });
    if state.total == 0 {
        state.finished.store(true, Ordering::Relaxed);
        s.thumbnail_build = None;
    }
    if wait {
        while s.thumbnail_build.is_some() {
            if let Some(job) = s.next_thumbnail_job() {
                let r = crate::memory::in_background(|| job.run());
                s.complete_thumbnail_job(&r);
            }
        }
    }
    Ok(state.json())
}

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "library.buildThumbnails",
        "Build All Library Thumbnails",
        [],
        None,
        "{wait?: bool, defer?: bool} — warm all normal thumbnail size buckets for every imported, non-deleted library photo, regardless of selection/filter/scroll. The app feeds one cache-only job at a time to its normal renderer below interactive work; other render hosts register with Session::set_thumbnail_renderer and drive next_thumbnail_job/complete_thumbnail_job. Without a render host (CLI/MCP), wait defaults to true. defer=true coalesces an import trigger behind an active preview build. Reuse valid cached thumbnails; after library.clearPreviews, rebuild them. No loupe or full-size previews. Progress: library.previewProgress; stop: library.cancelPreviews → {total, done, failed, skipped, running, cancelled, what, error}",
        always,
        build
    )]
}

#[cfg(test)]
#[path = "thumbnails/tests.rs"]
mod tests;
