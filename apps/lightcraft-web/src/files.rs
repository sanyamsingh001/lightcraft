//! The library's small files (catalog journal, presets, view state, prefs) on the web.
//!
//! Browser storage is asynchronous on the main thread (OPFS writable streams, IndexedDB), but the
//! catalog [`Store`] is synchronous. So the files are mirrored in memory: they are read once at
//! start-up ([`Files::preload`]), every write changes the mirror immediately and marks the file
//! dirty, and the host flushes dirty files to storage in the background ([`Files::take_dirty`],
//! about once per frame while anything is dirty).
//!
//! Durability: a write is in browser storage a few milliseconds after the command that made it
//! (one flush), not before the command returns as on the desktop. Each flushed file is replaced
//! atomically (OPFS swap file / one IndexedDB transaction), and files are flushed in the order they
//! were last modified, so the journal's recovery rules still hold: e.g. a new `catalog.snap` always
//! lands before the `catalog.log` reset that follows it.

use std::collections::BTreeMap;
use std::io;
use std::sync::{Arc, Mutex};

use lightcraft_engine::catalog::Store;

/// What has to be written for a dirty file.
#[derive(Clone, Debug, PartialEq)]
pub enum FlushOp {
    /// Replace the whole file.
    Write { name: String, data: Vec<u8> },
    /// Bytes appended since the last flush, starting at `offset` (the stored file's length).
    Append { name: String, offset: usize, data: Vec<u8> },
}

impl FlushOp {
    pub fn name(&self) -> &str {
        match self {
            FlushOp::Write { name, .. } | FlushOp::Append { name, .. } => name,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Dirty {
    Rewrite,
    /// Only appended since the last flush; the stored file has `from` bytes.
    Append {
        from: usize,
    },
}

#[derive(Default)]
struct Mirror {
    files: BTreeMap<String, Vec<u8>>,
    /// Dirty files, least recently modified first.
    dirty: Vec<(String, Dirty)>,
    /// Why the last flush to browser storage failed (quota, eviction…), until one succeeds.
    error: Option<String>,
}

impl Mirror {
    fn mark(&mut self, name: &str, d: Dirty) {
        let prev = self.dirty.iter().position(|(n, _)| n == name).map(|i| self.dirty.remove(i).1);
        let d = match (prev, d) {
            // appends after appends keep the original offset; anything else needs a rewrite
            (Some(Dirty::Append { from }), Dirty::Append { .. }) => Dirty::Append { from },
            (Some(Dirty::Rewrite), _) => Dirty::Rewrite,
            (_, d) => d,
        };
        self.dirty.push((name.to_string(), d));
    }
}

/// Shared in-memory mirror of the library files (cheap to clone).
#[derive(Clone, Default)]
pub struct Files(Arc<Mutex<Mirror>>);

impl Files {
    fn lock(&self) -> std::sync::MutexGuard<'_, Mirror> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// Set a file's stored content (start-up; not dirty).
    pub fn preload(&self, name: &str, data: Vec<u8>) {
        self.lock().files.insert(name.to_string(), data);
    }

    pub fn get(&self, name: &str) -> Option<Vec<u8>> {
        self.lock().files.get(name).cloned()
    }

    /// Replace a file.
    pub fn write(&self, name: &str, data: &[u8]) {
        let mut m = self.lock();
        m.files.insert(name.to_string(), data.to_vec());
        m.mark(name, Dirty::Rewrite);
    }

    pub fn append(&self, name: &str, data: &[u8]) {
        let mut m = self.lock();
        let f = m.files.entry(name.to_string()).or_default();
        let from = f.len();
        f.extend_from_slice(data);
        m.mark(name, Dirty::Append { from });
    }

    pub fn truncate(&self, name: &str, len: usize) {
        let mut m = self.lock();
        if let Some(f) = m.files.get_mut(name) {
            f.truncate(len);
            m.mark(name, Dirty::Rewrite);
        }
    }

    pub fn is_dirty(&self) -> bool {
        !self.lock().dirty.is_empty()
    }

    /// Everything to write, in modification order; the files count as clean afterwards (call
    /// [`Files::failed`] for ops that couldn't be written).
    pub fn take_dirty(&self) -> Vec<FlushOp> {
        let mut m = self.lock();
        let dirty = std::mem::take(&mut m.dirty);
        dirty
            .into_iter()
            .map(|(name, d)| {
                let data = m.files.get(&name).cloned().unwrap_or_default();
                match d {
                    Dirty::Append { from } if from <= data.len() => FlushOp::Append { offset: from, data: data[from..].to_vec(), name },
                    _ => FlushOp::Write { name, data },
                }
            })
            .collect()
    }

    /// A flush failed: write these files in full next time (keeps any newer modification order).
    pub fn failed(&self, ops: &[FlushOp]) {
        let mut m = self.lock();
        for op in ops.iter().rev() {
            if !m.dirty.iter().any(|(n, _)| n == op.name()) {
                m.dirty.insert(0, (op.name().to_string(), Dirty::Rewrite));
            } else if let Some(d) = m.dirty.iter_mut().find(|(n, _)| n == op.name()) {
                d.1 = Dirty::Rewrite;
            }
        }
    }

    /// Record the outcome of a flush to browser storage: `Some(why)` when it failed, `None` once
    /// one succeeds. While a flush is failing, the catalog [`Store`] refuses new writes, so the
    /// engine keeps later changes queued. The host also forwards this error through
    /// `Session::set_backing_save_error`: writes already accepted by the mirror (including the
    /// initial snapshot) must show the existing unsaved warning without another edit.
    pub fn set_error(&self, e: Option<String>) {
        self.lock().error = e;
    }

    pub fn error(&self) -> Option<String> {
        self.lock().error.clone()
    }

    /// A catalog [`Store`] over these files.
    pub fn store(&self) -> FilesStore {
        FilesStore(self.clone())
    }
}

/// [`Store`] implementation over [`Files`].
pub struct FilesStore(Files);

impl FilesStore {
    /// Writes fail while browser storage does (see [`Files::set_error`]).
    fn writable(&self) -> io::Result<()> {
        match self.0.error() {
            Some(e) => Err(io::Error::other(format!("browser storage: {e}"))),
            None => Ok(()),
        }
    }
}

impl Store for FilesStore {
    fn read(&mut self, name: &str) -> io::Result<Option<Vec<u8>>> {
        Ok(self.0.get(name))
    }
    fn write_atomic(&mut self, name: &str, data: &[u8]) -> io::Result<()> {
        self.writable()?;
        self.0.write(name, data);
        Ok(())
    }
    fn append(&mut self, name: &str, data: &[u8]) -> io::Result<()> {
        self.writable()?;
        self.0.append(name, data);
        Ok(())
    }
    fn truncate(&mut self, name: &str, len: u64) -> io::Result<()> {
        self.writable()?;
        self.0.truncate(name, len as usize);
        Ok(())
    }
    fn describe(&self) -> String {
        "browser storage".into()
    }
}

/// Isolate a live backup from the mirrored files and the open library's failed store.
pub fn backup_snapshot(session: &lightcraft_engine::Session) -> Result<Files, String> {
    if session.library.is_none() {
        return Err("the library could not be opened, so its current catalog cannot be backed up; reload to retry opening it".into());
    }
    let snapshot = session.snapshot_library().map_err(|e| e.to_string())?;
    let files = Files::default();
    for name in LIBRARY_FILES {
        if let Some(data) = snapshot.get(name) {
            files.preload(name, data);
        }
    }
    Ok(files)
}

/// Names of the library files read at start-up.
pub const LIBRARY_FILES: [&str; 6] = ["catalog.snap", "catalog.log", "presets.json", "view.json", "prefs.json", "ui.json"];

#[cfg(test)]
mod tests {
    use super::*;
    use lightcraft_engine::Session;
    use lightcraft_engine::library::LibraryStores;
    use serde_json::json;

    #[test]
    fn backup_restores_live_edits_while_the_store_is_failing() {
        use crate::backup::{ZipWriter, data_offset, find_central, parse_central, tail_len, verify};
        let files = Files::default();
        let mut session = Session::new();
        let stores = LibraryStores { dir: "browser".into(), catalog: Box::new(files.store()), files: Box::new(files.store()), on_disk: false };
        session.open_library_in(stores, true).unwrap();
        let id = session.selection.active.unwrap();
        files.set_error(Some("QuotaExceededError".into()));
        assert!(matches!(session.execute("photo.rate", &json!({"rating": 5})), Err(lightcraft_engine::EngineError::NotSaved(_))));
        assert!(matches!(
            session.execute("develop.set", &json!({"control": "light.exposure", "value": 1.25})),
            Err(lightcraft_engine::EngineError::NotSaved(_))
        ));
        assert!(session.set_cache_mb(17).is_err());
        let before: Vec<_> = LIBRARY_FILES.iter().map(|n| files.get(n)).collect();
        let pending = session.unsaved().unwrap().0;
        let snapshot = backup_snapshot(&session).unwrap();
        assert_eq!(session.unsaved().unwrap().0, pending, "backup must leave live changes queued");
        assert_eq!(files.error().as_deref(), Some("QuotaExceededError"));
        assert_eq!(LIBRARY_FILES.iter().map(|n| files.get(n)).collect::<Vec<_>>(), before, "failed store untouched");
        assert!(snapshot.get("catalog.log").unwrap().is_empty(), "live state is wholly in the snapshot");
        // Create and restore a healthy archive exclusively through production archive APIs.
        let mut zip = ZipWriter::default();
        let mut bytes = Vec::new();
        for name in LIBRARY_FILES {
            if let Some(data) = snapshot.get(name) {
                bytes.extend(zip.entry(&format!("library/{name}"), &data).unwrap());
                bytes.extend(data);
            }
        }
        bytes.extend(zip.finish().unwrap());
        let tail = &bytes[bytes.len() - tail_len(bytes.len() as u64) as usize..];
        let (offset, size) = find_central(tail).unwrap();
        let entries = parse_central(&bytes[offset as usize..(offset + size) as usize]).unwrap();
        let restored = Files::default();
        for entry in entries {
            let offset = data_offset(&entry, &bytes[entry.header as usize..entry.header as usize + 30]).unwrap() as usize;
            let data = &bytes[offset..offset + entry.size as usize];
            verify(&entry, data).unwrap();
            restored.preload(entry.name.strip_prefix("library/").unwrap(), data.to_vec());
        }
        let mut reopened = Session::new();
        reopened
            .open_library_in(
                LibraryStores { dir: "restored".into(), catalog: Box::new(restored.store()), files: Box::new(restored.store()), on_disk: false },
                true,
            )
            .unwrap();
        let photo = reopened.catalog.photo(id).unwrap();
        assert_eq!(photo.rating, 5);
        assert_eq!(photo.develop.light.exposure, 1.25);
        assert_eq!(reopened.selection.active, session.selection.active);
        assert_eq!(reopened.cache_mb, 17, "live preferences are included even when their write failed");
        assert_eq!(reopened.catalog.to_snapshot(), session.catalog.to_snapshot());
    }

    /// Apply flush ops to a "disk" map, as the backend would.
    fn flush(files: &Files, disk: &mut BTreeMap<String, Vec<u8>>) {
        for op in files.take_dirty() {
            match op {
                FlushOp::Write { name, data } => {
                    disk.insert(name, data);
                }
                FlushOp::Append { name, offset, data } => {
                    let f = disk.entry(name).or_default();
                    assert_eq!(f.len(), offset, "append offset matches the stored length");
                    f.extend_from_slice(&data);
                }
            }
        }
    }

    #[test]
    fn appends_coalesce_and_rewrites_win() {
        let f = Files::default();
        f.preload("log", b"ab".to_vec());
        f.append("log", b"c");
        f.append("log", b"d");
        assert_eq!(f.take_dirty(), vec![FlushOp::Append { name: "log".into(), offset: 2, data: b"cd".to_vec() }]);
        assert!(!f.is_dirty());
        f.write("snap", b"S");
        f.truncate("log", 1);
        f.append("log", b"x");
        let ops = f.take_dirty();
        assert_eq!(
            ops,
            vec![FlushOp::Write { name: "snap".into(), data: b"S".to_vec() }, FlushOp::Write { name: "log".into(), data: b"ax".to_vec() }]
        );
        f.failed(&ops);
        assert_eq!(f.take_dirty().len(), 2);
    }

    #[test]
    fn modification_order_puts_snapshot_before_log_reset() {
        let f = Files::default();
        f.append("catalog.log", b"1\n");
        f.write("catalog.snap", b"snap");
        f.write("catalog.log", b"");
        let names: Vec<_> = f.take_dirty().iter().map(|o| o.name().to_string()).collect();
        assert_eq!(names, ["catalog.snap", "catalog.log"]);
    }

    /// A failing flush (quota exceeded, storage evicted) is not silent: commands report the change
    /// as unsaved and the engine keeps it queued; once storage works again it is written.
    #[test]
    fn flush_failures_show_as_unsaved_and_are_retried() {
        let mut disk = BTreeMap::new();
        let files = Files::default();
        let mut s = Session::new();
        let stores = LibraryStores { dir: "browser".into(), catalog: Box::new(files.store()), files: Box::new(files.store()), on_disk: false };
        s.open_library_in(stores, true).unwrap();
        flush(&files, &mut disk);
        files.set_error(Some("QuotaExceededError".into()));
        let r = s.execute("photo.rate", &json!({"rating": 5}));
        assert!(matches!(r, Err(lightcraft_engine::EngineError::NotSaved(_))), "{r:?}");
        let (ops, why) = s.unsaved().unwrap();
        assert!(ops > 0 && why.contains("QuotaExceeded"), "{why}");
        files.set_error(None);
        s.persist_if_dirty();
        s.persist().unwrap();
        assert!(s.unsaved().is_none(), "written once storage works again");
        flush(&files, &mut disk);
        let id = s.selection.active.unwrap();
        let files2 = Files::default();
        for (k, v) in &disk {
            files2.preload(k, v.clone());
        }
        let mut s2 = Session::new();
        let stores = LibraryStores { dir: "browser".into(), catalog: Box::new(files2.store()), files: Box::new(files2.store()), on_disk: false };
        s2.open_library_in(stores, true).unwrap();
        assert_eq!(s2.catalog.photo(id).unwrap().rating, 5, "the rating survived");
    }

    #[test]
    fn queued_edits_keep_the_backing_warning_until_their_own_flush() {
        let files = Files::default();
        let mut disk = BTreeMap::new();
        let mut s = Session::new();
        s.open_library_in(
            LibraryStores { dir: "browser".into(), catalog: Box::new(files.store()), files: Box::new(files.store()), on_disk: false },
            true,
        )
        .unwrap();
        flush(&files, &mut disk);
        s.execute("photo.rate", &json!({"rating": 4})).unwrap();
        let ops = files.take_dirty();
        files.failed(&ops);
        files.set_error(Some("fixture backing write refusal".into()));
        s.set_backing_save_error(files.error());
        assert!(s.execute("photo.rate", &json!({"rating": 5})).is_err());
        flush(&files, &mut disk); // only the already mirrored rating has reached storage
        files.set_error(None);
        s.set_backing_save_error(None); // queued edits must keep the backing warning latched
        s.persist().unwrap(); // the queued rating is now in the mirror, still not in storage
        assert!(files.is_dirty());
        assert!(s.unsaved().is_some());
        flush(&files, &mut disk);
        s.set_backing_save_error(None);
        assert!(s.unsaved().is_none());
    }

    /// Both the initial snapshot and an already accepted append can fail asynchronously, with
    /// no subsequent edit to make the synchronous Store report an error.
    #[test]
    fn backing_failure_without_another_edit_survives_backup_and_retry() {
        for startup in [true, false] {
            let files = Files::default();
            let mut disk = BTreeMap::new();
            let mut s = Session::new();
            s.open_library_in(
                LibraryStores { dir: "browser".into(), catalog: Box::new(files.store()), files: Box::new(files.store()), on_disk: false },
                true,
            )
            .unwrap();
            let id = s.selection.active.unwrap();
            let expected_rating = if startup { s.catalog.photo(id).unwrap().rating } else { 4 };
            if !startup {
                flush(&files, &mut disk);
                s.execute("photo.rate", &json!({"rating": 4})).unwrap();
            }
            assert!(s.unsaved().is_none(), "the mirror has accepted all catalog writes");
            let failed = files.take_dirty();
            assert!(!failed.is_empty());
            files.failed(&failed);
            files.set_error(Some("fixture backing write refusal".into()));
            s.set_backing_save_error(files.error());
            let before: Vec<_> = LIBRARY_FILES.iter().map(|n| files.get(n)).collect();
            let warning = s.unsaved().map(|(n, e)| (n, e.to_string()));
            assert_eq!(warning, Some((1, "fixture backing write refusal".into())));
            let snapshot = backup_snapshot(&s).unwrap();
            assert!(snapshot.get("catalog.snap").is_some());
            assert_eq!(LIBRARY_FILES.iter().map(|n| files.get(n)).collect::<Vec<_>>(), before);
            assert!(files.is_dirty());
            assert_eq!(files.error().as_deref(), Some("fixture backing write refusal"));
            s.persist().unwrap(); // a successful mirror/no-op save is not a backing flush
            assert_eq!(s.unsaved().map(|(n, e)| (n, e.to_string())), warning);
            flush(&files, &mut disk);
            files.set_error(None);
            s.set_backing_save_error(files.error());
            assert!(s.unsaved().is_none());
            let reopened = Files::default();
            for (name, bytes) in disk {
                reopened.preload(&name, bytes);
            }
            let mut loaded = Session::new();
            loaded
                .open_library_in(
                    LibraryStores { dir: "browser".into(), catalog: Box::new(reopened.store()), files: Box::new(reopened.store()), on_disk: false },
                    false,
                )
                .unwrap();
            assert_eq!(loaded.catalog.photo(id).unwrap().rating, expected_rating);
        }
    }

    #[test]
    fn library_survives_reload_through_flushes() {
        let mut disk = BTreeMap::new();
        let open = |disk: &BTreeMap<String, Vec<u8>>| {
            let files = Files::default();
            for (k, v) in disk {
                files.preload(k, v.clone());
            }
            let mut s = Session::new();
            let stores = LibraryStores { dir: "browser".into(), catalog: Box::new(files.store()), files: Box::new(files.store()), on_disk: false };
            s.open_library_in(stores, true).unwrap();
            (s, files)
        };
        let (mut s, files) = open(&disk);
        assert!(!s.catalog.is_empty(), "seeded with the demo photos");
        flush(&files, &mut disk);
        let id = s.selection.active.unwrap();
        s.execute("develop.set", &json!({"control": "light.exposure", "value": 1.25})).unwrap();
        s.execute("photo.rate", &json!({"rating": 4})).unwrap();
        s.save_view();
        flush(&files, &mut disk);
        let (s2, _) = open(&disk);
        let p = s2.catalog.photo(id).unwrap();
        assert_eq!(p.rating, 4);
        assert_eq!(p.develop.light.exposure, 1.25);
        assert_eq!(s2.catalog.len(), s.catalog.len(), "not seeded twice");
        assert_eq!(s2.selection.active, s.selection.active);
    }
}
