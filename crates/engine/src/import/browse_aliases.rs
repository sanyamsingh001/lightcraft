//! Reconnect a saved Local folder view to existing file identities on a filesystem worker.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use lightcraft_catalog::{PhotoId, Source};

use crate::{Browse, LibrarySource, Session};

/// Stored sources at library opening, independent of rating/keywords/develop revisions.
/// Path, photo ID and Local status; no edits/history or filesystem facts are retained.
pub(crate) type BrowseAliasRecords = Arc<[(String, PhotoId, bool)]>;

/// The particular library opening and folder view that requested reconstruction.
#[derive(Clone)]
pub struct BrowseAliasScope {
    library: Arc<()>,
    browse: Browse,
}

impl BrowseAliasScope {
    /// Checking the owner is entirely in memory; reopening the same library is a new owner.
    pub fn is_current(&self, s: &Session) -> bool {
        Arc::ptr_eq(&self.library, &s.library_identity) && s.source == LibrarySource::Folder && s.browse.as_ref() == Some(&self.browse)
    }
}

/// An O(1) handoff in record count from the current library opening's source snapshot.
/// Construction does no catalog iteration, path copying or filesystem work.
pub struct BrowseAliasInput {
    scope: BrowseAliasScope,
    records: BrowseAliasRecords,
}

impl BrowseAliasInput {
    /// Capture once after opening a saved Local view, never on catalog revision/frame changes.
    /// This bounded memory-only pass excludes edits/history; all identity work stays on the worker.
    pub(crate) fn library_records(s: &Session) -> Option<BrowseAliasRecords> {
        if s.source != LibrarySource::Folder || s.browse.is_none() {
            return None;
        }
        Some(
            s.catalog
                .photos()
                .take(crate::walk::MAX_ENTRIES)
                .filter_map(|p| match &p.source {
                    Source::File { path } if !p.deleted => Some((path.clone(), p.id, p.local)),
                    _ => None,
                })
                .collect(),
        )
    }

    pub fn new(s: &Session) -> Option<Self> {
        if s.source != LibrarySource::Folder {
            return None;
        }
        let lib = s.library.as_ref().filter(|lib| lib.on_disk)?;
        let records = lib.browse_alias_records.as_ref()?.clone();
        let browse = s.browse.clone()?;
        Some(Self { scope: BrowseAliasScope { library: s.library_identity.clone(), browse }, records })
    }

    pub fn scope(&self) -> BrowseAliasScope {
        self.scope.clone()
    }

    /// Filesystem half: bounded folder listing and identity checks, without probing or import.
    /// A disconnected/offline folder simply has no aliases to reconnect.
    pub fn restore(self, cancel: &AtomicBool) -> BrowseAliasOutput {
        if cancel.load(Ordering::Relaxed) || !std::path::Path::new(&self.scope.browse.path).is_dir() {
            return BrowseAliasOutput { scope: self.scope, aliases: Vec::new(), cancelled: cancel.load(Ordering::Relaxed) };
        }
        let files = super::expand_cancellable(std::slice::from_ref(&self.scope.browse.path), None, self.scope.browse.subfolders, Some(cancel));
        let mut records = Vec::new();
        if !files.is_empty() {
            for record in self.records.iter() {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                records.push(record);
            }
        }
        // Prefer an imported record, then the first ID, without merging stored records.
        records.sort_by_key(|(_, id, local)| (*local, *id));
        let paths: Vec<_> = records.iter().map(|(path, _, _)| path.clone()).collect();
        let native = super::native_paths(&paths, cancel);
        let mut known = HashMap::new();
        for (path, id, _) in records {
            known.entry(path.as_str()).or_insert_with(Vec::new).push(*id);
        }
        let mut aliases = Vec::new();
        for path in files {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let Some(source) = super::native_match(&path, &native) else { continue };
            if source != path
                && let Some(ids) = known.remove(source)
            {
                // Resolve each source group once, even when the folder contains many links.
                // Each copy is fenced against its own stored source on owner-side apply.
                aliases.extend(ids.into_iter().take_while(|_| !cancel.load(Ordering::Relaxed)).map(|id| (path.clone(), id, source.to_string())));
            }
        }
        BrowseAliasOutput { scope: self.scope, aliases, cancelled: cancel.load(Ordering::Relaxed) }
    }
}

/// Worker result: only alternate spellings of records already in the snapshot.
pub struct BrowseAliasOutput {
    scope: BrowseAliasScope,
    aliases: Vec<(String, PhotoId, String)>,
    cancelled: bool,
}

impl BrowseAliasOutput {
    /// Owner half: reconnect only unchanged stored sources in the still-current view.
    /// Never adds photos, changes edits/selection/history, or accesses the filesystem.
    pub fn apply(self, s: &mut Session) -> bool {
        if self.cancelled || !self.scope.is_current(s) {
            return false;
        }
        let aliases: Vec<_> = self
            .aliases
            .into_iter()
            .filter_map(|(path, id, source)| {
                s.catalog.photo(id).filter(|p| matches!(&p.source, Source::File { path } if path == &source)).map(|_| (path, id))
            })
            .collect();
        s.set_browse_aliases(aliases);
        true
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use lightcraft_catalog::{Op, Photo};

    fn fixture(tag: &str) -> (Session, std::path::PathBuf, PhotoId) {
        let dir = std::env::temp_dir().join(format!("lc-saved-alias-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("real/nested")).unwrap();
        let image = lightcraft_raster::Rgba8::from_fn(2, 2, |_, _| [32, 64, 128, 255]);
        let bytes = lightcraft_codecs::encode_png(&lightcraft_codecs::EncodeImage::rgba8(&image), &Default::default()).unwrap();
        std::fs::write(dir.join("real/photo.png"), bytes).unwrap();
        std::os::unix::fs::symlink(dir.join("real"), dir.join("alias")).unwrap();
        let mut s = Session::new().with_fs();
        s.open_library(dir.join("library-a"), false).unwrap();
        let id = PhotoId(7);
        let path = dir.join("real/photo.png").to_string_lossy().into_owned();
        let mut photo = Photo::new(id, Source::File { path }, "photo.png", "PNG", 2, 2, "2026-10-09T00:00:00");
        photo.local = true;
        photo.rating = 3; // A touched Local record survives the library's normal cleanup.
        Arc::make_mut(&mut photo.develop).light.exposure = 1.25;
        s.commit("fixture", Op::AddPhoto { photo: Box::new(photo) }).unwrap();
        s.source = LibrarySource::Folder;
        s.browse = Some(Browse { path: dir.join("alias").to_string_lossy().into_owned(), subfolders: false });
        s.close_library().unwrap();
        s.open_library(dir.join("library-a"), false).unwrap();
        (s, dir, id)
    }

    #[test]
    fn saved_alias_view_reconnects_after_actual_library_reopen_without_changing_records() {
        let (mut s, dir, id) = fixture("reopen");
        let before = s.catalog.photo(id).unwrap().clone();
        s.close_library().unwrap();
        s.open_library(dir.join("library-b"), false).unwrap();
        s.close_library().unwrap();
        s.open_library(dir.join("library-a"), false).unwrap();
        assert_eq!(s.source, LibrarySource::Folder);
        assert_eq!(s.browse.as_ref().unwrap().path, dir.join("alias").to_string_lossy());
        assert!(s.visible().is_empty(), "stored source spelling does not lexically match the alias");
        let input = BrowseAliasInput::new(&s).unwrap();
        let out = std::thread::spawn(move || input.restore(&AtomicBool::new(false))).join().unwrap();
        assert!(out.apply(&mut s));
        assert_eq!(s.visible(), &[id]);
        assert_eq!(s.source_total(), Some(1));
        let after = s.catalog.photo(id).unwrap();
        assert_eq!(after.source, before.source);
        assert_eq!(after.develop, before.develop);
        assert_eq!(after.history, before.history);
        assert_eq!(after.rating, before.rating);
        assert_eq!(s.catalog.len(), 1);
        drop(s);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn ordinary_edits_keep_the_source_snapshot_and_cached_alias_view_without_reconstruction() {
        let (mut s, dir, id) = fixture("edit-cost");
        let input = BrowseAliasInput::new(&s).unwrap();
        let records = input.records.clone();
        let scope = input.scope();
        assert!(input.restore(&AtomicBool::new(false)).apply(&mut s));
        let mut meta = s.catalog.photo(id).unwrap().meta.clone();
        meta.keywords = vec!["healthy workflow".into()];
        let mut settings = s.catalog.photo(id).unwrap().develop.as_ref().clone();
        settings.light.exposure = 1.5;
        for op in [
            Op::SetRating { id, rating: 4 },
            Op::SetMeta { id, meta: Box::new(meta) },
            Op::SetDevelop { id, settings: Arc::new(settings), label: "edit".into(), edited: None },
        ] {
            s.commit("ordinary edit", op).unwrap();
            assert!(scope.is_current(&s));
            assert!(Arc::ptr_eq(&records, &BrowseAliasInput::new(&s).unwrap().records));
            assert!(s.browse_aliases_current());
            assert_eq!(s.visible(), &[id]);
            assert_eq!(s.source_total(), Some(1));
            let generation = s.visible_shared().0;
            for _ in 0..32 {
                assert_eq!(s.visible_shared().0, generation, "quiet frames reuse the cached view");
                assert_eq!(s.source_total(), Some(1));
            }
        }
        drop(s);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reconstruction_rejects_cancelled_changed_folder_library_and_stored_source_results() {
        let (mut s, dir, id) = fixture("fences");
        let input = BrowseAliasInput::new(&s).unwrap();
        assert!(!input.restore(&AtomicBool::new(true)).apply(&mut s));
        assert!(s.visible().is_empty());
        let input = BrowseAliasInput::new(&s).unwrap();
        let out = input.restore(&AtomicBool::new(false));
        s.browse.as_mut().unwrap().subfolders = true;
        assert!(!out.apply(&mut s));
        let input = BrowseAliasInput::new(&s).unwrap();
        let out = input.restore(&AtomicBool::new(false));
        s.close_library().unwrap();
        s.open_library(dir.join("library-a"), false).unwrap();
        assert!(!out.apply(&mut s), "reopening the same path changes the library owner");
        let input = BrowseAliasInput::new(&s).unwrap();
        let out = input.restore(&AtomicBool::new(false));
        let changed = Source::File { path: dir.join("other.png").to_string_lossy().into_owned() };
        s.commit("change source", Op::Relink { id, file_name: "other.png".into(), source: changed.clone(), format: None }).unwrap();
        assert!(out.apply(&mut s));
        assert!(s.visible().is_empty());
        assert_eq!(s.catalog.photo(id).unwrap().source, changed);
        drop(s);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
