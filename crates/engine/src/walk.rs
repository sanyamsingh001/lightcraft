//! Bounded folder walks (issue #374): every recursive listing of the disk — import, Local's
//! Include Subfolders, Find Missing Photos, preset and LUT folders, MCP — goes through
//! [`files_in`], which stops at a depth and a number of directory entries instead of walking a
//! whole drive, follows a symbolic link or junction to a folder at most once (no loops), and
//! decides what is a folder from the directory listing itself (no extra open per entry, which
//! is what makes a large walk slow on Windows).
//!
//! It never chooses where to walk: callers pass a folder the user named. Nothing in the app walks
//! at startup or on the UI thread.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// How far a walk may go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    /// Folder levels below the starting folder that are listed (its own entries are level 1).
    pub max_depth: usize,
    /// Directory entries looked at in all (files and folders, hidden ones included).
    pub max_entries: usize,
}

impl Default for Limits {
    fn default() -> Self {
        Limits { max_depth: MAX_DEPTH, max_entries: MAX_ENTRIES }
    }
}

/// Folder levels a walk descends at most; photo trees are a handful of levels deep.
pub const MAX_DEPTH: usize = 32;
/// Directory entries a walk looks at at most: far more than any photo folder holds, far fewer
/// than a whole system drive.
pub const MAX_ENTRIES: usize = 1_000_000;

/// What a walk found.
#[derive(Debug, Default, PartialEq)]
pub struct Walked {
    /// The files `keep` accepted, in path order, folder by folder (depth first).
    pub files: Vec<PathBuf>,
    /// The walk stopped at a [`Limits`] bound: there may be more files.
    pub truncated: bool,
    /// Directory entries looked at.
    pub entries: usize,
}

fn hidden(p: &Path) -> bool {
    p.file_name().is_some_and(|n| n.to_string_lossy().starts_with('.'))
}

/// Optional import scope and worker cancellation. Defaults preserve [`files_in`]'s behavior.
pub struct Options<'a> {
    /// Read nested folders too; false omits them without reporting truncation.
    pub subfolders: bool,
    pub cancel: Option<&'a std::sync::atomic::AtomicBool>,
    /// A worker-resolved folder identity; its descendants and directory aliases are excluded.
    pub canonical_skip: Option<&'a Path>,
    /// Share across source roots to list each physical directory once. Ordinary child folder
    /// identities come from their canonical parent and entry name, without another metadata read.
    pub directories: Option<&'a mut HashSet<PathBuf>>,
}

impl Default for Options<'_> {
    fn default() -> Self {
        Self { subfolders: true, cancel: None, canonical_skip: None, directories: None }
    }
}

impl Options<'_> {
    fn cancelled(&self) -> bool {
        self.cancel.is_some_and(|c| c.load(std::sync::atomic::Ordering::Relaxed))
    }
}

/// The files below folder `root` that `keep` accepts. Hidden entries (names starting with `.`)
/// are left out, the folder `skip` (e.g. the library's own) is never entered, and the walk stops
/// at `limits` (see [`Walked::truncated`]). Unreadable folders are skipped.
pub fn files_in(root: &Path, skip: Option<&Path>, limits: Limits, keep: impl FnMut(&Path) -> bool) -> Walked {
    files_in_with(root, skip, limits, Options::default(), keep)
}

/// [`files_in`] with optional direct-folder scope, cancellation and shared directory identities.
/// Cancellation is checked before filesystem work, during listing and between queued entries;
/// it does not report a limit truncation. Call only on a filesystem worker.
pub fn files_in_with(root: &Path, skip: Option<&Path>, limits: Limits, mut options: Options<'_>, mut keep: impl FnMut(&Path) -> bool) -> Walked {
    let mut out = Walked::default();
    if options.cancelled() {
        return out;
    }
    let canonical = std::fs::canonicalize(root).ok();
    let identities = options.directories.is_some() || options.canonical_skip.is_some();
    // Default callers retain the existing link-only seen set. Import shares all directory
    // identities and checks them when popped in path order, preserving the first source spelling.
    let mut linked: HashSet<PathBuf> = HashSet::new();
    if let Some(c) = &canonical {
        linked.insert(c.clone());
    }
    // Include aliases to the library in the exclusion, not just its lexical path.
    let skip_canonical = skip.and_then(|p| std::fs::canonicalize(p).ok());
    // (path, depth of the folder it is in, is a folder, canonical folder identity)
    let mut stack = vec![(root.to_path_buf(), 0usize, true, canonical.filter(|_| identities))];
    while let Some((path, depth, is_dir, canonical)) = stack.pop() {
        if options.cancelled() {
            break;
        }
        if !is_dir {
            if keep(&path) {
                out.files.push(path);
            }
            continue;
        }
        if (depth > 0 && !options.subfolders)
            || skip.is_some_and(|s| path == s)
            || skip_canonical.as_ref().is_some_and(|s| std::fs::canonicalize(&path).is_ok_and(|p| p.starts_with(s)))
        {
            continue;
        }
        if identities {
            let Some(c) = &canonical else { continue };
            if options.canonical_skip.is_some_and(|s| c.starts_with(s)) || options.directories.as_ref().is_some_and(|seen| seen.contains(c)) {
                continue;
            }
        }
        if depth >= limits.max_depth {
            out.truncated = true;
            continue;
        }
        if options.cancelled() {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&path) else { continue };
        if let (Some(seen), Some(c)) = (options.directories.as_mut(), &canonical) {
            seen.insert(c.clone());
        }
        let mut children = Vec::new();
        for entry in rd {
            if options.cancelled() {
                break;
            }
            let Ok(e) = entry else { continue };
            if out.entries >= limits.max_entries {
                out.truncated = true;
                break;
            }
            out.entries = out.entries.saturating_add(1);
            let p = e.path();
            if hidden(&p) {
                continue;
            }
            let Ok(ft) = e.file_type() else { continue };
            let (dir, identity) = if ft.is_symlink() {
                // A link to a folder needs its target identity; regular entries use file_type.
                match std::fs::metadata(&p) {
                    Ok(m) if m.is_dir() => {
                        if !options.subfolders {
                            continue;
                        }
                        let Ok(c) = std::fs::canonicalize(&p) else { continue };
                        if options.directories.is_none() && !linked.insert(c.clone()) {
                            continue;
                        }
                        (true, identities.then_some(c))
                    }
                    _ => (false, None),
                }
            } else {
                let dir = ft.is_dir();
                if dir && !options.subfolders {
                    continue;
                }
                (dir, canonical.as_ref().filter(|_| dir).map(|c| c.join(e.file_name())))
            };
            children.push((p, dir, identity));
        }
        children.sort();
        let below = depth.saturating_add(1);
        stack.extend(children.into_iter().rev().map(|(p, d, c)| (p, below, d, c)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("lc-walk-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn files_come_in_path_order_without_hidden_ones_or_the_skipped_folder() {
        let d = tmp("order");
        for f in ["b/2.jpg", "a/1.jpg", "a/x/0.jpg", "c.jpg", ".hidden/h.jpg", "a/.h.jpg", "lib/l.jpg", "a/n.txt"] {
            let p = d.join(f);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        let lib = d.join("lib");
        let w = files_in(&d, Some(&lib), Limits::default(), |p| p.extension().is_some_and(|e| e == "jpg"));
        let rel: Vec<String> = w.files.iter().map(|p| p.strip_prefix(&d).unwrap().to_string_lossy().replace('\\', "/")).collect();
        assert_eq!(rel, ["a/1.jpg", "a/x/0.jpg", "b/2.jpg", "c.jpg"]);
        assert!(!w.truncated);
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Issue #374: a walk never runs on without end — a tree deeper than the limit, or with more
    /// entries than the limit, stops and says so.
    #[test]
    fn deep_and_wide_trees_stop_at_the_limits() {
        let d = tmp("bounds");
        let mut deep = d.join("deep");
        for i in 0..12 {
            deep = deep.join(format!("l{i}"));
        }
        std::fs::create_dir_all(&deep).unwrap();
        std::fs::write(deep.join("bottom.jpg"), b"x").unwrap();
        let wide = d.join("wide");
        std::fs::create_dir_all(&wide).unwrap();
        for i in 0..50 {
            std::fs::write(wide.join(format!("{i:02}.jpg")), b"x").unwrap();
        }
        let all = |_: &Path| true;
        let w = files_in(&d.join("deep"), None, Limits { max_depth: 5, max_entries: 1000 }, all);
        assert!(w.files.is_empty() && w.truncated, "{w:?}");
        let w = files_in(&d.join("deep"), None, Limits { max_depth: 13, max_entries: 1000 }, all);
        assert_eq!((w.files.len(), w.truncated), (1, false), "{w:?}");
        let w = files_in(&wide, None, Limits { max_depth: 5, max_entries: 20 }, all);
        assert_eq!((w.files.len(), w.entries, w.truncated), (20, 20, true));
        let w = files_in(&wide, None, Limits::default(), all);
        assert_eq!((w.files.len(), w.truncated), (50, false));
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Folder imports must not revisit the user's own library through symlinks
    /// or through an alias to the library's parent.
    #[cfg(unix)]
    #[test]
    fn skipped_library_stays_skipped_through_symlinks_and_ancestors() {
        let d = tmp("skip-alias");
        let root = d.join("incoming");
        let library = d.join("library");
        std::fs::create_dir_all(&root).unwrap();
        std::fs::create_dir_all(library.join("Originals")).unwrap();
        std::fs::write(root.join("ordinary.jpg"), b"photo").unwrap();
        std::fs::write(library.join("Originals").join("library-only.jpg"), b"original").unwrap();
        std::os::unix::fs::symlink(&library, root.join("library-link")).unwrap();
        std::os::unix::fs::symlink(&d, root.join("parent-link")).unwrap();

        let photos = files_in(&root, Some(&library), Limits::default(), |p| p.extension().is_some_and(|e| e == "jpg"));
        assert!(photos.files.contains(&root.join("ordinary.jpg")));
        assert!(!photos.files.iter().any(|p| p.ends_with("library-only.jpg")), "{photos:?}");

        // Scanning a subfolder already inside the excluded library is also forbidden.
        let inside = files_in(&library.join("Originals"), Some(&library), Limits::default(), |_| true);
        assert!(inside.files.is_empty(), "{inside:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// A link back up the tree (a loop) is followed at most once: the walk ends.
    #[cfg(unix)]
    #[test]
    fn link_loops_end() {
        let d = tmp("loop");
        std::fs::create_dir_all(d.join("a/b")).unwrap();
        std::fs::write(d.join("a/b/p.jpg"), b"x").unwrap();
        std::os::unix::fs::symlink(&d, d.join("a/b/up")).unwrap();
        std::os::unix::fs::symlink(d.join("a"), d.join("alias")).unwrap();
        let w = files_in(&d, None, Limits::default(), |_| true);
        assert!(!w.truncated, "{w:?}");
        assert!(w.files.len() <= 2, "{w:?}");
        assert!(w.files.iter().any(|p| p.ends_with("a/b/p.jpg")), "{w:?}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn direct_scope_omits_children_without_truncation_and_allows_named_children() {
        let d = tmp("direct-scope");
        std::fs::create_dir_all(d.join("child")).unwrap();
        std::fs::write(d.join("a.jpg"), b"healthy").unwrap();
        std::fs::write(d.join("child/b.jpg"), b"healthy").unwrap();
        let mut seen = HashSet::new();
        let limits = Limits { max_depth: 1, max_entries: 10 };
        let options = Options { subfolders: false, directories: Some(&mut seen), ..Default::default() };
        let direct = files_in_with(&d, None, limits, options, |_| true);
        assert_eq!(direct.files, [d.join("a.jpg")]);
        assert_eq!((direct.entries, direct.truncated), (2, false));
        let options = Options { subfolders: false, directories: Some(&mut seen), ..Default::default() };
        let child = files_in_with(&d.join("child"), None, limits, options, |_| true);
        assert_eq!(child.files, [d.join("child/b.jpg")]);
        assert!(!child.truncated);
        let _ = std::fs::remove_dir_all(d);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn pre_cancelled_walk_does_no_listing_or_filtering() {
        use std::sync::atomic::AtomicBool;
        let d = tmp("pre-cancel");
        std::fs::write(d.join("a.jpg"), b"healthy").unwrap();
        let cancel = AtomicBool::new(true);
        let options = Options { cancel: Some(&cancel), ..Default::default() };
        let w = files_in_with(&d, None, Limits::default(), options, |_| panic!("cancelled before filtering"));
        assert_eq!(w, Walked::default());
        let _ = std::fs::remove_dir_all(d);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn cancelled_walk_stops_between_files_and_folders() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let d = tmp("during-cancel");
        std::fs::create_dir_all(d.join("z-child")).unwrap();
        for f in ["a.jpg", "b.jpg", "z-child/c.jpg"] {
            std::fs::write(d.join(f), b"healthy").unwrap();
        }
        let cancel = AtomicBool::new(false);
        let mut seen = HashSet::new();
        let options = Options { cancel: Some(&cancel), directories: Some(&mut seen), ..Default::default() };
        let w = files_in_with(&d, None, Limits::default(), options, |_| {
            cancel.store(true, Ordering::Relaxed);
            true
        });
        assert_eq!(w.files, [d.join("a.jpg")]);
        assert_eq!((w.entries, w.truncated), (3, false));
        assert!(!seen.contains(&d.join("z-child").canonicalize().unwrap()));
        let _ = std::fs::remove_dir_all(d);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn shared_sources_choose_first_directory_alias_and_preserve_distinct_files() {
        let d = tmp("shared-sources");
        let real = d.join("real");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("a.jpg"), b"same healthy bytes").unwrap();
        std::fs::copy(real.join("a.jpg"), real.join("copy.jpg")).unwrap();
        std::os::unix::fs::symlink(real.join("a.jpg"), real.join("file-alias.jpg")).unwrap();
        let alias = d.join("alias");
        std::os::unix::fs::symlink(&real, &alias).unwrap();
        let mut seen = HashSet::new();
        let options = Options { directories: Some(&mut seen), ..Default::default() };
        let first = files_in_with(&alias, None, Limits::default(), options, |_| true);
        assert_eq!(first.files, [alias.join("a.jpg"), alias.join("copy.jpg"), alias.join("file-alias.jpg")]);
        assert!(!first.truncated);
        let options = Options { directories: Some(&mut seen), ..Default::default() };
        assert_eq!(files_in_with(&real, None, Limits::default(), options, |_| true), Walked::default());
        // Overlapping parents also omit already-listed directory identities, not file copies.
        let options = Options { directories: Some(&mut seen), ..Default::default() };
        let parent = files_in_with(&d, None, Limits::default(), options, |_| true);
        assert!(parent.files.is_empty() && !parent.truncated);
        let _ = std::fs::remove_dir_all(d);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn canonical_skip_excludes_real_alias_and_nested_directory_sources() {
        let d = tmp("canonical-skip");
        let lib = d.join("library");
        let outside = d.join("library-other");
        for dir in [lib.join("originals"), outside.clone()] {
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(dir.join("a.jpg"), b"healthy").unwrap();
        }
        std::os::unix::fs::symlink(&lib, d.join("alias")).unwrap();
        std::os::unix::fs::symlink(lib.join("originals"), d.join("nested-alias")).unwrap();
        let canonical_skip = lib.canonicalize().unwrap();
        let mut seen = HashSet::new();
        for root in [&lib, &lib.join("originals"), &d.join("alias"), &d.join("nested-alias")] {
            let options = Options { canonical_skip: Some(&canonical_skip), directories: Some(&mut seen), ..Default::default() };
            assert_eq!(files_in_with(root, Some(&lib), Limits::default(), options, |_| true), Walked::default());
        }
        let options = Options { canonical_skip: Some(&canonical_skip), directories: Some(&mut seen), ..Default::default() };
        let w = files_in_with(&d, Some(&lib), Limits::default(), options, |_| true);
        assert_eq!(w.files, [outside.join("a.jpg")]);
        assert!(!w.truncated);
        let _ = std::fs::remove_dir_all(d);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn scoped_walks_still_report_entry_and_depth_bounds() {
        let d = tmp("scoped-bounds");
        std::fs::create_dir_all(d.join("a/b")).unwrap();
        std::fs::write(d.join("a/b/deep.jpg"), b"healthy").unwrap();
        for i in 0..6 {
            std::fs::write(d.join(format!("{i}.jpg")), b"healthy").unwrap();
        }
        let mut seen = HashSet::new();
        let options = Options { directories: Some(&mut seen), ..Default::default() };
        let w = files_in_with(&d, None, Limits { max_depth: 1, max_entries: 20 }, options, |_| true);
        assert_eq!(w.files.len(), 6);
        assert!(w.truncated);
        let options = Options { subfolders: false, ..Default::default() };
        let w = files_in_with(&d, None, Limits { max_depth: 1, max_entries: 3 }, options, |_| true);
        assert_eq!(w.entries, 3);
        assert!(w.truncated);
        let _ = std::fs::remove_dir_all(d);
    }
}
