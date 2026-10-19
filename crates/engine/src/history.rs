//! Reveal cataloged photos during single-photo undo/redo.

use crate::{Browse, LibrarySource, Result, Session};
use lightcraft_catalog::{PhotoId, Source};

impl Session {
    /// Reveal a cataloged photo without scanning Local folders or creating a new undo step.
    pub(crate) fn reveal_history_photo(&mut self, id: PhotoId) -> Result<()> {
        if self.visible().contains(&id) || self.catalog.photo(id).is_none() {
            return Ok(());
        }
        if let Some(stack) = self.catalog.stack_of(id)
            && stack.collapsed
            && stack.top() != id
        {
            for op in self.catalog.set_collapsed_ops(&[stack.id], false) {
                self.catalog.apply(op.clone())?;
                self.pending_log.push(op);
            }
        }
        if self.visible().contains(&id) {
            return Ok(());
        }
        // If only the filter hides it, keep the current album/folder instead of leaving it.
        let filter = std::mem::take(&mut self.filter);
        if self.visible().contains(&id) {
            return Ok(());
        }
        self.filter = filter;
        let Some(photo) = self.catalog.photo(id) else { return Ok(()) };
        let deleted = photo.deleted;
        let folder = if photo.local {
            match &photo.source {
                Source::File { path } => std::path::Path::new(path).parent().map(|dir| {
                    let path = dir.to_string_lossy();
                    if path.is_empty() { ".".to_string() } else { path.into_owned() }
                }),
                _ => None,
            }
        } else {
            None
        };
        let leaving_folder = self.source == LibrarySource::Folder;
        self.source = if deleted {
            LibrarySource::RecentlyDeleted
        } else if folder.is_some() {
            LibrarySource::Folder
        } else {
            LibrarySource::All
        };
        if !deleted && let Some(folder) = &folder {
            self.browse = Some(Browse { path: folder.clone(), subfolders: false });
        } else if leaving_folder {
            self.browse = None;
        }
        self.library_folder = None;
        if !self.visible().contains(&id) {
            self.filter = Default::default();
        }
        if deleted && let Some(folder) = folder {
            self.filter.folder = Some(folder);
        }
        Ok(())
    }
}
