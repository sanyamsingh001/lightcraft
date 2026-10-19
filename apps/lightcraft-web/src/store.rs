//! Imported originals on the web. There is no filesystem: the bytes of every photo picked or
//! dropped in the browser are stored in browser storage as `originals/<content hash>`, and the
//! catalog refers to them by a synthetic path `web/<content hash>/<file name>` (so the decoders
//! still see the file's extension).
//!
//! The engine reads files synchronously ([`FileLoader`]/[`FileProbe`]), so the main thread keeps
//! the bytes it needs in a bounded memory cache ([`Originals`]). A miss (e.g. after a reload)
//! queues an asynchronous load from storage ([`Originals::take_wanted`]) and fails for now; the
//! host prefetches the active photo's original so commands that need its pixels (auto tone,
//! export) find it. Render workers read originals from storage themselves.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use lightcraft_engine::files::{load_bytes, probe_bytes};
use lightcraft_engine::media::{FileLoader, FileProbe};

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
        if !g.temporary && !g.loading.contains(hash) && !g.wanted.iter().any(|w| w == hash) {
            g.wanted.push(hash.to_string());
        }
        None
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

    pub fn load_failed(&self, hash: &str) {
        self.lock().loading.remove(hash);
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
            let bytes = s.get(path).ok_or_else(|| {
                if s.lock().temporary {
                    format!("{path}: the original is unavailable in this temporary session; import the original file again")
                } else {
                    format!("{path}: the original is still loading from browser storage")
                }
            })?;
            load_bytes(&bytes, max_edge)
        });
        let s = self.clone();
        let probe: FileProbe = Arc::new(move |path: &str| {
            let bytes = s.get(path).ok_or_else(|| format!("{path}: not loaded"))?;
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
    fn download_names() {
        assert_eq!(download_name("dir/x-lightcraft.png"), "x-lightcraft.png");
        assert_eq!(download_name("y.png"), "y.png");
        assert_eq!(mime_for("a.JPG"), "image/jpeg");
        assert_eq!(mime_for("a"), "application/octet-stream");
        assert_eq!(download_name(""), "export.png");
    }
}
