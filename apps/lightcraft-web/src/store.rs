//! Imported originals on the web. There is no filesystem: the bytes of every photo picked or
//! dropped in the browser are stored in browser storage as `originals/<content hash>`, and the
//! catalog refers to them by a synthetic path `web/<content hash>/<file name>` (so the decoders
//! still see the file's extension).
//!
//! The engine reads files synchronously ([`FileLoader`]/[`FileProbe`]), so the main thread keeps
//! the bytes it needs in a bounded memory cache ([`Originals`]). A miss (e.g. after a reload)
//! queues an asynchronous load from storage ([`Originals::take_wanted`]). Inline render hosts
//! defer pending file jobs via [`Originals::render_readiness`]; synchronous commands fail for now.
//! The host prefetches the active photo's original so commands that need its pixels (auto tone,
//! export) find it. Render workers read originals from storage themselves.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use lightcraft_engine::files::{load_bytes, probe_bytes};
use lightcraft_engine::media::{FileLoader, FileProbe, SourceRef};

/// Prefix of the catalog paths of files kept in browser storage.
pub const PATH_PREFIX: &str = "web/";

/// Memory budget for original bytes on the main thread.
const MEM_BUDGET: usize = 768 << 20;

/// Catalog path for a stored original.
pub fn original_path(hash: &str, name: &str) -> String {
    let name = name.rsplit(['/', '\\']).next().filter(|n| !n.is_empty()).unwrap_or("photo");
    format!("{PATH_PREFIX}{hash}/{name}")
}

/// The content hash in a path made by [`original_path`].
pub fn hash_of_path(path: &str) -> Option<&str> {
    let (hash, _) = path.strip_prefix(PATH_PREFIX)?.split_once('/')?;
    (!hash.is_empty()).then_some(hash)
}

/// Storage key of an original.
pub fn storage_key(hash: &str) -> String {
    format!("originals/{hash}")
}

/// Hash of a file's bytes (the same as the catalog's `content_hash`).
pub fn content_hash(bytes: &[u8]) -> String {
    lightcraft_preview::hash_bytes(bytes).to_string()
}

/// Main-thread original availability. Pending jobs stay queued on the render host; a failed
/// read must complete with its error instead of waiting or automatically loading forever.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OriginalReadiness {
    Ready,
    Pending,
    Failed(String),
}

#[derive(Default)]
struct Inner {
    /// hash → (bytes, last use).
    bytes: HashMap<String, (Arc<[u8]>, u64)>,
    total: usize,
    budget: usize,
    /// Temporary originals have no backing store and must never be evicted.
    temporary: bool,
    clock: u64,
    /// Catalog paths stored since the last [`Originals::take_pending`] (to import on the next frame).
    pending: Vec<String>,
    /// Hashes to load from storage.
    wanted: Vec<String>,
    /// Hashes being loaded.
    loading: std::collections::HashSet<String>,
    /// Terminal read failures, cleared only by successful insertion or a fresh page/session.
    failed: HashMap<String, String>,
}

impl Inner {
    fn queue_load(&mut self, hash: &str) {
        if !self.temporary && !self.failed.contains_key(hash) && !self.loading.contains(hash) && !self.wanted.iter().any(|w| w == hash) {
            self.wanted.push(hash.to_string());
        }
    }
}

/// Original bytes in memory, shared between the engine hooks and the host (cheap to clone).
#[derive(Clone)]
pub struct Originals(Arc<Mutex<Inner>>);

impl Default for Originals {
    fn default() -> Self {
        Self::with_budget(false, MEM_BUDGET)
    }
}

impl Originals {
    fn with_budget(temporary: bool, budget: usize) -> Self {
        Self(Arc::new(Mutex::new(Inner { temporary, budget, ..Default::default() })))
    }

    /// A bounded temporary session retains every accepted original until the page closes.
    pub fn temporary() -> Self {
        Self::with_budget(true, MEM_BUDGET)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Keep `bytes` in memory. Persistent originals may be evicted; temporary imports are
    /// rejected before any existing bytes or pending imports change if they exceed the budget.
    pub fn insert(&self, hash: &str, bytes: Arc<[u8]>) -> Result<(), String> {
        let mut g = self.lock();
        let old_len = g.bytes.get(hash).map_or(0, |v| v.0.len());
        let total = g.total.checked_sub(old_len).and_then(|n| n.checked_add(bytes.len()));
        let total = total.ok_or("original byte count exceeds the memory limit")?;
        if g.temporary && total > g.budget {
            return Err("the temporary session's original-photo memory limit is full. Existing photos are kept. Use a stored session to add more photos, and export your edits before closing this page".into());
        }
        g.clock = g.clock.saturating_add(1);
        let clock = g.clock;
        g.loading.remove(hash);
        g.failed.remove(hash);
        g.wanted.retain(|h| h != hash);
        g.bytes.insert(hash.to_string(), (bytes, clock));
        g.total = total;
        while !g.temporary && g.total > g.budget && g.bytes.len() > 1 {
            let Some(oldest) = g.bytes.iter().filter(|(h, _)| *h != hash).min_by_key(|(_, v)| v.1).map(|(h, _)| h.clone()) else { break };
            if let Some(v) = g.bytes.remove(&oldest) {
                g.total -= v.0.len();
            }
        }
        Ok(())
    }

    /// A file was accepted: retain its bytes, then queue its path for import.
    pub fn added(&self, name: &str, hash: &str, bytes: Arc<[u8]>) -> Result<String, String> {
        self.insert(hash, bytes)?;
        let path = original_path(hash, name);
        self.lock().pending.push(path.clone());
        Ok(path)
    }

    /// Bytes for a catalog path, if in memory. A miss for a stored original queues its load.
    pub fn get(&self, path: &str) -> Option<Arc<[u8]>> {
        let hash = hash_of_path(path)?;
        let mut g = self.lock();
        g.clock = g.clock.saturating_add(1);
        let clock = g.clock;
        if let Some(v) = g.bytes.get_mut(hash) {
            v.1 = clock;
            return Some(v.0.clone());
        }
        g.queue_load(hash);
        None
    }

    /// Queue a stored original once and report whether it can be read synchronously now.
    /// Failed reads remain terminal until bytes are inserted successfully or the page reloads.
    pub fn readiness(&self, path: &str) -> OriginalReadiness {
        let Some(hash) = hash_of_path(path) else {
            return OriginalReadiness::Failed(format!("{path}: not a browser original path; import the original file again"));
        };
        let mut g = self.lock();
        if g.bytes.contains_key(hash) {
            return OriginalReadiness::Ready;
        }
        if g.temporary {
            return OriginalReadiness::Failed(format!(
                "{path}: the original is unavailable in this temporary session; import the original file again"
            ));
        }
        if let Some(message) = g.failed.get(hash) {
            return OriginalReadiness::Failed(format!("{path}: {message}; reload the page to retry"));
        }
        g.queue_load(hash);
        OriginalReadiness::Pending
    }

    /// Only an undecoded file source requires main-thread original bytes. A loaded source,
    /// procedural scene or smart preview proceeds through its existing render path.
    pub fn render_readiness(&self, source: &SourceRef) -> OriginalReadiness {
        match source {
            SourceRef::File { path, .. } => self.readiness(path),
            _ => OriginalReadiness::Ready,
        }
    }

    fn unavailable_error(&self, path: &str) -> String {
        match self.readiness(path) {
            OriginalReadiness::Failed(message) => message,
            _ => format!("{path}: the original is still loading from browser storage"),
        }
    }

    pub fn contains(&self, path: &str) -> bool {
        hash_of_path(path).is_some_and(|h| self.lock().bytes.contains_key(h))
    }

    /// Ask for the original behind `path` to be loaded into memory (no-op if it's there).
    pub fn prefetch(&self, path: &str) {
        let _ = self.get(path);
    }

    /// Hashes to load from storage (they count as loading until [`Originals::insert`] or
    /// [`Originals::load_failed`]).
    pub fn take_wanted(&self) -> Vec<String> {
        let mut g = self.lock();
        let w = std::mem::take(&mut g.wanted);
        g.loading.extend(w.iter().cloned());
        w
    }

    /// Latch a terminal missing/read failure. Existing hosts may use this generic message.
    pub fn load_failed(&self, hash: &str) {
        self.load_failed_with_error(hash, "the original is missing or could not be read from browser storage");
    }

    /// Preserve the read error without allowing repeated frames to queue another read.
    pub fn load_failed_with_error(&self, hash: &str, message: &str) {
        let mut g = self.lock();
        g.loading.remove(hash);
        g.wanted.retain(|h| h != hash);
        // An import may have supplied the original while an older storage read was pending.
        if !g.bytes.contains_key(hash) {
            let message = message.trim();
            let message = if message.is_empty() { "could not read the original from browser storage" } else { message };
            g.failed.insert(hash.to_string(), message.chars().take(512).collect());
        }
    }

    /// Paths added since the last call.
    pub fn take_pending(&self) -> Vec<String> {
        std::mem::take(&mut self.lock().pending)
    }

    /// (originals in memory, bytes).
    pub fn usage(&self) -> (usize, usize) {
        let g = self.lock();
        (g.bytes.len(), g.total)
    }

    /// Engine hooks that decode/probe from memory.
    pub fn hooks(&self) -> (FileLoader, FileProbe) {
        let s = self.clone();
        let loader: FileLoader = Arc::new(move |path: &str, max_edge: usize| {
            let bytes = s.get(path).ok_or_else(|| s.unavailable_error(path))?;
            load_bytes(&bytes, max_edge)
        });
        let s = self.clone();
        let probe: FileProbe = Arc::new(move |path: &str| {
            let bytes = s.get(path).ok_or_else(|| s.unavailable_error(path))?;
            probe_bytes(path, &bytes)
        });
        (loader, probe)
    }

    /// Install the hooks into a session.
    pub fn install(&self, session: &mut lightcraft_engine::Session) {
        let (l, p) = self.hooks();
        session.media.file_loader = Some(l);
        session.media.file_probe = Some(p);
        let s = self.clone();
        session.media.preview_loader =
            Some(Arc::new(move |path: &str, max_edge: usize| lightcraft_engine::files::embedded_preview_srgb(&s.get(path)?, max_edge)));
    }
}

/// File name part of an export path (the browser download name).
pub fn download_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().filter(|n| !n.is_empty()).unwrap_or("export.png")
}

/// MIME type for a download, by extension.
pub fn mime_for(name: &str) -> &'static str {
    match name.rsplit_once('.').map(|(_, e)| e.to_ascii_lowercase()).as_deref() {
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("tif" | "tiff") => "image/tiff",
        Some("webp") => "image/webp",
        Some("avif") => "image/avif",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
pub(crate) fn png_bytes(w: usize, h: usize) -> Vec<u8> {
    let bytes: Vec<u8> = (0..w * h).flat_map(|i| [(i * 7) as u8, 128, 200, 255]).collect();
    let img = lightcraft_raster::Rgba8::from_bytes(w, h, &bytes).unwrap();
    lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&img), &lightcraft_codecs::EncodeMeta::default()).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn paths() {
        let p = original_path("abc123", "C:\\x\\a.png");
        assert_eq!(p, "web/abc123/a.png");
        assert_eq!(hash_of_path(&p), Some("abc123"));
        assert_eq!(hash_of_path("mem/1/a.png"), None);
        assert_eq!(storage_key("abc"), "originals/abc");
    }

    #[test]
    fn miss_queues_a_load_once() {
        let s = Originals::default();
        assert!(s.get("web/h1/a.png").is_none());
        assert!(s.get("web/h1/a.png").is_none());
        assert_eq!(s.take_wanted(), vec!["h1".to_string()]);
        assert!(s.get("web/h1/a.png").is_none(), "still loading");
        assert!(s.take_wanted().is_empty());
        s.insert("h1", Arc::from(vec![1u8, 2])).unwrap();
        assert_eq!(&*s.get("web/h1/b.jpg").unwrap(), &[1, 2]);
    }

    #[test]
    fn import_and_render_from_memory() {
        let store = Originals::temporary();
        let mut session = lightcraft_engine::Session::new();
        store.install(&mut session);
        let bytes = png_bytes(40, 30);
        let hash = content_hash(&bytes);
        let path = store.added("tiny.png", &hash, bytes.into()).unwrap();
        assert_eq!(store.take_pending(), vec![path.clone()]);
        let r = session.execute("library.import", &json!({"paths": [path]})).unwrap();
        let id = lightcraft_engine::catalog::PhotoId(r["imported"][0].as_u64().unwrap());
        let p = session.catalog.photo(id).unwrap();
        assert_eq!((p.width, p.height), (40, 30));
        assert_eq!(p.file_name, "tiny.png");
        assert_eq!(p.content_hash.as_deref(), Some(hash.as_str()), "storage key = catalog content hash");
        let out = session.render_now(id, 20, 20).unwrap();
        assert_eq!(out.image.width, 20);
    }

    #[test]
    fn temporary_budget_rejects_import_before_eviction_and_keeps_rendering() {
        let bytes = png_bytes(40, 30);
        let other = png_bytes(20, 10);
        let hash = content_hash(&bytes);
        let other_hash = content_hash(&other);
        let store = Originals::with_budget(true, bytes.len());
        let path = store.added("first.png", &hash, bytes.clone().into()).unwrap();
        assert_eq!(store.take_pending(), vec![path.clone()]);
        let error = store.added("second.png", &other_hash, other.into()).unwrap_err();
        assert!(error.contains("Existing photos are kept"));
        assert_eq!(store.usage(), (1, bytes.len()));
        assert_eq!(&*store.get(&path).unwrap(), bytes.as_slice());
        assert!(store.take_pending().is_empty());
        assert!(store.get("web/missing/photo.png").is_none());
        assert!(store.take_wanted().is_empty(), "temporary sessions have no storage to load from");
        // Re-picking the same original consumes no extra budget.
        store.added("again.png", &hash, bytes.clone().into()).unwrap();
        assert_eq!(store.usage(), (1, bytes.len()));
        let mut session = lightcraft_engine::Session::new();
        store.install(&mut session);
        let result = session.execute("library.import", &json!({"paths": [path]})).unwrap();
        let id = lightcraft_engine::catalog::PhotoId(result["imported"][0].as_u64().unwrap());
        let rendered = session.render_now(id, 20, 20).unwrap();
        assert_eq!((rendered.image.width, rendered.image.height), (20, 15));
        session.media.clear_sources();
        let options =
            lightcraft_engine::export::ExportOptions { format: lightcraft_engine::export::ExportFormat::Png, resize: None, ..Default::default() };
        let exported = lightcraft_engine::export::export_photo(&mut session, id, &options, 1).unwrap();
        assert!(!exported.bytes.is_empty(), "an uncached export still reads the accepted original");
    }

    #[test]
    fn temporary_oversize_first_import_is_rejected_without_queuing() {
        let bytes = png_bytes(8, 8);
        let store = Originals::with_budget(true, bytes.len().saturating_sub(1));
        assert!(store.added("photo.png", &content_hash(&bytes), bytes.into()).is_err());
        assert_eq!(store.usage(), (0, 0));
        assert!(store.take_pending().is_empty());
    }

    #[test]
    fn persistent_budget_can_evict_backed_originals() {
        let first = png_bytes(8, 8);
        let second = png_bytes(9, 9);
        let store = Originals::with_budget(false, second.len());
        let path = store.added("first.png", &content_hash(&first), first.into()).unwrap();
        store.added("second.png", &content_hash(&second), second.into()).unwrap();
        assert!(!store.contains(&path));
        assert!(store.get(&path).is_none());
        assert_eq!(store.take_wanted().len(), 1, "backed originals remain reloadable");
    }

    #[test]
    fn stored_readiness_waits_once_then_loads_and_renders() {
        use lightcraft_engine::develop::DevelopSettings;
        use lightcraft_engine::pipeline::{RenderRequest, render};

        let bytes = png_bytes(24, 16);
        let hash = content_hash(&bytes);
        let path = original_path(&hash, "healthy.png");
        let alias = original_path(&hash, "same-original.png");
        let store = Originals::default();
        let source = SourceRef::File { path: path.clone(), max_edge: 24, loader: Some(store.hooks().0), fallback: None };
        assert_eq!(store.render_readiness(&source), OriginalReadiness::Pending);
        assert_eq!(store.readiness(&alias), OriginalReadiness::Pending);
        assert_eq!(store.take_wanted(), vec![hash.clone()]);
        for _ in 0..8 {
            assert_eq!(store.render_readiness(&source), OriginalReadiness::Pending);
            assert!(store.take_wanted().is_empty(), "one read stays pending across frames and aliases");
        }
        store.insert(&hash, bytes.into()).unwrap();
        assert_eq!(store.render_readiness(&source), OriginalReadiness::Ready);
        assert!(store.take_wanted().is_empty());
        let decoded = source.load_source().unwrap();
        let rendered = render(&decoded.image, &decoded.info_or(Default::default()), &DevelopSettings::default(), &RenderRequest::fit(12, 12));
        assert_eq!((rendered.image.width, rendered.image.height), (12, 8));
        assert!(rendered.image.as_bytes().iter().any(|b| *b != 0));
    }

    #[test]
    fn failed_reads_are_terminal_until_successful_insert() {
        for reason in ["original not found in browser storage", "IndexedDB read failed"] {
            let bytes = png_bytes(12, 8);
            let hash = content_hash(&bytes);
            let path = original_path(&hash, "healthy.png");
            let store = Originals::default();
            assert_eq!(store.readiness(&path), OriginalReadiness::Pending);
            assert_eq!(store.take_wanted(), vec![hash.clone()]);
            store.load_failed_with_error(&hash, reason);
            for _ in 0..8 {
                let OriginalReadiness::Failed(message) = store.readiness(&path) else { panic!("failed read must stop deferring") };
                assert!(message.contains(reason) && message.contains("reload"));
                assert!(store.get(&path).is_none());
                store.prefetch(&path);
                assert!(store.take_wanted().is_empty(), "failed reads are not retried every frame");
            }
            let error = store.hooks().0(&path, 12).err().unwrap();
            assert!(error.contains(reason) && error.contains("reload"));
            let reloaded = Originals::default();
            assert_eq!(reloaded.readiness(&path), OriginalReadiness::Pending, "a page reload permits one fresh read");
            assert_eq!(reloaded.take_wanted(), vec![hash.clone()]);
            store.insert(&hash, bytes.into()).unwrap();
            assert_eq!(store.readiness(&path), OriginalReadiness::Ready);
            assert!(store.hooks().0(&path, 12).is_ok(), "a supplied healthy original recovers");
        }
    }

    #[test]
    fn generic_failure_is_terminal_and_queued_success_clears_the_request() {
        let store = Originals::default();
        let path = original_path("queued", "healthy.png");
        assert_eq!(store.readiness(&path), OriginalReadiness::Pending);
        store.load_failed("queued");
        assert!(matches!(store.readiness(&path), OriginalReadiness::Failed(_)));
        assert!(store.take_wanted().is_empty());
        assert!(store.lock().loading.is_empty());
        let bytes = png_bytes(8, 8);
        let other = original_path("supplied", "healthy.png");
        assert_eq!(store.readiness(&other), OriginalReadiness::Pending);
        store.insert("supplied", bytes.into()).unwrap();
        assert!(store.take_wanted().is_empty(), "an import already supplied the queued bytes");
        assert_eq!(store.readiness(&other), OriginalReadiness::Ready);
    }

    #[test]
    fn a_late_failure_does_not_poison_supplied_originals() {
        let bytes = png_bytes(8, 8);
        let hash = content_hash(&bytes);
        let path = original_path(&hash, "healthy.png");
        let store = Originals::default();
        assert_eq!(store.readiness(&path), OriginalReadiness::Pending);
        assert_eq!(store.take_wanted(), vec![hash.clone()]);
        store.insert(&hash, bytes.into()).unwrap();
        store.load_failed_with_error(&hash, "older read failed");
        assert_eq!(store.readiness(&path), OriginalReadiness::Ready);
        assert!(store.lock().failed.is_empty());
        assert!(store.take_wanted().is_empty());
    }

    #[test]
    fn loaded_sources_do_not_wait_for_original_bytes_and_temporary_misses_are_terminal() {
        use lightcraft_engine::media::DecodedSource;

        let store = Originals::default();
        let image = Arc::new(lightcraft_raster::Rgb32f::filled(8, 8, [0.2, 0.3, 0.4]));
        let loaded = SourceRef::Loaded(Box::new(DecodedSource::new(image, None)));
        assert_eq!(store.render_readiness(&loaded), OriginalReadiness::Ready);
        let demo = lightcraft_engine::media::MediaCache::default().origin_ref(&lightcraft_engine::catalog::Source::Demo { scene: 1 }, 8);
        assert!(matches!(demo, SourceRef::Demo { .. }));
        assert_eq!(store.render_readiness(&demo), OriginalReadiness::Ready);
        assert!(demo.load_source().is_ok());
        assert_eq!(store.render_readiness(&SourceRef::Smart { path: "unused.smart".into() }), OriginalReadiness::Ready);
        assert!(store.take_wanted().is_empty());
        assert!(matches!(store.readiness("not-a-browser-path"), OriginalReadiness::Failed(_)));
        let temporary = Originals::temporary();
        assert!(matches!(temporary.readiness("web/missing/healthy.png"), OriginalReadiness::Failed(_)));
        assert!(temporary.take_wanted().is_empty());
    }

    #[test]
    fn readiness_preserves_temporary_budget_admission() {
        let bytes = png_bytes(12, 8);
        let other = png_bytes(8, 8);
        let hash = content_hash(&bytes);
        let store = Originals::with_budget(true, bytes.len());
        let path = store.added("accepted.png", &hash, bytes.clone().into()).unwrap();
        store.take_pending();
        assert!(store.added("too-many.png", &content_hash(&other), other.into()).is_err());
        assert_eq!(store.readiness(&path), OriginalReadiness::Ready);
        assert_eq!(&*store.get(&path).unwrap(), bytes.as_slice());
        assert_eq!(store.usage(), (1, bytes.len()));
        assert!(store.take_pending().is_empty());
        assert!(store.take_wanted().is_empty());
        assert!(store.hooks().0(&path, 12).is_ok());
    }

    #[test]
    fn failure_messages_are_bounded_without_losing_utf8() {
        let store = Originals::default();
        let path = original_path("failed", "healthy.png");
        assert_eq!(store.readiness(&path), OriginalReadiness::Pending);
        store.load_failed_with_error("failed", &"讀".repeat(700));
        assert_eq!(store.lock().failed.get("failed").unwrap().chars().count(), 512);
        assert!(matches!(store.readiness(&path), OriginalReadiness::Failed(_)));
        assert!(store.take_wanted().is_empty());
    }

    #[test]
    fn download_names() {
        assert_eq!(download_name("dir/x-lightcraft.png"), "x-lightcraft.png");
        assert_eq!(download_name("y.png"), "y.png");
        assert_eq!(mime_for("a.JPG"), "image/jpeg");
        assert_eq!(mime_for("a"), "application/octet-stream");
        assert_eq!(download_name(""), "export.png");
    }
}
