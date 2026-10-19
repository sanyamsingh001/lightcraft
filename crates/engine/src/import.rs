//! Import: files and folders (recursive) → photos.
//!
//! 1. Expand folders recursively into supported files (hidden entries and the library's own
//!    folder are skipped), sorted for a stable order.
//! 2. Skip paths already in the catalog.
//! 3. Probe the rest — dimensions, metadata, and the **content hash** of the bytes — in parallel
//!    on native targets.
//! 4. Skip **duplicates by content** (same bytes already in the library, or twice in this batch).
//! 5. *Add* in place (the photo points at the original file), *copy* into the library's
//!    `Originals/YYYY/YYYY-MM-DD/` folder (names made unique) and point at the copy, or *move*
//!    there: placed like a copy (with its XMP sidecars), and the original removed only once its
//!    catalog record is committed and saved (see [`crate::import_move`] for the safety rules).
//! 6. Apply each photo's XMP sidecar (or a raw/DNG file's embedded XMP): the sidecar wins for
//!    metadata and develop settings are restored (see [`crate::sidecar`]). Its capture time fills
//!    in when the file has none (read before step 5, so it also files and names the copy).
//! 7. Optionally apply a preset and add keywords (the import dialog's options).
//! 8. Commit all new photos as one undoable op. (Undoing a move removes the catalog records;
//!    the files stay at the destination — undo never moves files back over a card.)
//!
//! The import dialog first calls [`scan`] (`library.importPreview`): the same expansion, probing
//! and duplicate detection without adding anything, so the user can review the candidates. The
//! probes are kept and reused by the import that follows.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use lightcraft_catalog::{MediaKind, Op, Photo, PhotoId, Source};
use serde::{Deserialize, Serialize};

use crate::Session;
use crate::media::ProbeInfo;

mod browse_aliases;
pub(crate) use browse_aliases::BrowseAliasRecords;
pub use browse_aliases::{BrowseAliasInput, BrowseAliasOutput, BrowseAliasScope};

/// File extensions LightCraft imports (lower case).
pub const EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "png", "tif", "tiff", "webp", "dng", "cr2", "cr3", "nef", "nrw", "arw", "raf", "orf", "rw2", "rwl", "raw", "pef", "psd", "jxl",
    "gif", "bmp", "heic", "avif", // containers LightCraft cannot decode but imports as preview only (their embedded JPEG)
    "iiq", "crw", "mrw", "x3f", "kdc", "mos", "erf",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ImportMode {
    /// Reference the files where they are.
    #[default]
    Add,
    /// Copy the files into the library's `Originals/` folder.
    Copy,
    /// Move the files there: as Copy, then each original (and its sidecars) is removed from the
    /// source once the copy is verified and catalogued.
    Move,
}

/// What an import does with a file the library already has in Recently Deleted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum OnDeleted {
    /// Leave it there; the report says so ([`Duplicate::existing_deleted`]).
    #[default]
    Skip,
    /// Bring the trashed photo back, with its edits ([`ImportReport::restored`]).
    Restore,
    /// Delete the trashed record permanently and import the file as a new photo.
    Fresh,
}

impl OnDeleted {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "skip" => Some(Self::Skip),
            "restore" => Some(Self::Restore),
            "fresh" => Some(Self::Fresh),
            _ => None,
        }
    }
}

/// What an import does besides adding the photos.
#[derive(Clone, Debug, Default)]
pub struct ImportOptions {
    /// A file that is in Recently Deleted: skip (default), restore, or import afresh.
    pub on_deleted: OnDeleted,
    pub mode: ImportMode,
    /// Applied to every imported photo (one History entry).
    pub preset: Option<lightcraft_develop::Preset>,
    /// Added to every imported photo.
    pub keywords: Vec<String>,
    /// Browsing a folder: photos come in as `local` (not in the library), and a file with the
    /// same content as one already known is still listed.
    pub local: bool,
    /// Copy: where the copies go (default: the library's `Originals/`).
    pub destination: Option<String>,
    /// Copy: the folders inside the destination.
    pub organize: Organize,
    /// Copy: a file-name template for the copies ([`crate::rename::expand`] tokens: `{name}`,
    /// `{seq:N}`, `{date:%Y%m%d}`, `{camera}`…), numbered from `rename_start`.
    pub rename: Option<String>,
    pub rename_start: usize,
    /// A metadata preset (by name) applied to every imported photo.
    pub metadata_preset: Option<String>,
    /// Copy: raws are copied as DNG (Copy as DNG).
    pub convert_dng: bool,
}

/// How copies are filed in the destination. The date is the capture time, else the time of the
/// import.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum Organize {
    /// `YYYY/YYYY-MM-DD/` by capture date.
    #[default]
    ByDay,
    /// `YYYY/YYYY-MM/`.
    ByMonth,
    /// All in the destination itself.
    Flat,
    /// A folder template, e.g. `{date:%Y}/{date:%Y%m%d}` → `2026/20260114/`: the template's own
    /// `/` make the levels, each expanded with the file-name tokens ([`crate::rename::expand_folder`];
    /// never outside the destination).
    Template(String),
}

impl Organize {
    /// `date` | `month` | `flat`, or a folder template (anything with a `{`, `/` or `\`).
    pub fn parse(s: &str) -> Option<Organize> {
        match s {
            "date" | "day" | "byDay" => Some(Organize::ByDay),
            "month" | "byMonth" => Some(Organize::ByMonth),
            "flat" | "none" | "intoOneFolder" => Some(Organize::Flat),
            t if t.contains(['{', '/', '\\']) => Some(Organize::Template(t.trim().to_string())),
            _ => None,
        }
    }

    /// The folders (inside the destination) a copy of `p` goes to.
    pub fn folders(&self, p: &Photo) -> Vec<String> {
        let date = p.date();
        let day = date.get(..10).filter(|d| d.len() == 10).unwrap_or("undated");
        let year = day.get(..4).unwrap_or("undated");
        match self {
            Organize::ByDay => vec![year.into(), day.into()],
            Organize::ByMonth => vec![year.into(), day.get(..7).unwrap_or("undated").into()],
            Organize::Flat => Vec::new(),
            Organize::Template(t) => crate::rename::expand_folder(t, p, 1),
        }
    }
}

/// A file found by [`scan`], for the import review.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ImportCandidate {
    pub path: String,
    /// Stable filesystem identity, computed on the scan worker for checkbox persistence.
    #[serde(skip)]
    pub selection_identity: String,
    pub name: String,
    pub format: String,
    pub kind: MediaKind,
    pub width: u32,
    pub height: u32,
    pub file_size: u64,
    pub captured: Option<String>,
    /// `"path"` (already in the library), `"content"` (same bytes in the library or earlier in this
    /// list): skipped by the import.
    pub duplicate: Option<String>,
    /// The photo that already has it.
    pub existing: Option<u64>,
    /// Not readable.
    pub error: Option<String>,
    /// A raw variant that can't be decoded yet (why): it imports as preview only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preview_only: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct Duplicate {
    pub path: String,
    /// The photo that already has these bytes (or this path).
    pub existing: Option<u64>,
    /// `"path"` (already imported from there) or `"content"` (same bytes elsewhere).
    pub reason: &'static str,
    /// That photo is in Recently Deleted (see [`OnDeleted`]).
    #[serde(rename = "existingDeleted", skip_serializing_if = "std::ops::Not::not")]
    pub existing_deleted: bool,
}

#[derive(Clone, Debug, Default, Serialize)]
pub struct ImportReport {
    pub imported: Vec<u64>,
    pub duplicates: Vec<Duplicate>,
    /// [`OnDeleted::Restore`]: photos brought back from Recently Deleted instead of imported.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub restored: Vec<u64>,
    /// Move: the originals that were moved (and removed from the source).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub moved: Vec<Moved>,
    /// Move: originals (or sidecars) left at the source, and why — the photo may still have been
    /// imported (from its copy, or in place).
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub kept: Vec<Kept>,
    /// (path, error)
    pub failed: Vec<(String, String)>,
    /// Files found after expanding folders.
    pub scanned: usize,
    /// Photos whose metadata/develop settings were read from an XMP sidecar (or embedded XMP).
    pub sidecars: usize,
}

/// A file moved by an import.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Moved {
    pub from: String,
    pub to: String,
    /// Its sidecars' new paths.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub sidecars: Vec<String>,
}

/// A source a Move left where it was.
#[derive(Clone, Debug, Default, Serialize)]
pub struct Kept {
    pub path: String,
    pub reason: String,
}

/// The develop settings a photo gets on import: raws start from their as-shot white balance with
/// default sharpening / colour noise reduction, and file-embedded lens corrections on (as the
/// camera intended); a user default preset ([`ImportDefaults`]) goes on top.
pub fn import_defaults(p: &Photo) -> lightcraft_develop::DevelopSettings {
    p.import_defaults()
}

/// Does the photo still look as the camera rendered it (its embedded camera preview is then a
/// fair stand-in)? Not when a default preset changed it on import.
pub fn has_import_look(p: &Photo) -> bool {
    *p.develop == p.camera_defaults()
}

/// User defaults applied on import (Settings → Import; saved in the library's `prefs.json`).
/// Presets are referenced by id; a preset that no longer exists is ignored.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ImportDefaults {
    /// Preset applied to raw files (`None` = the LightCraft default).
    pub raw_preset: Option<String>,
    /// Use a camera's own default (below) when the photo's camera has one.
    pub per_camera: bool,
    /// Per-camera raw defaults, keyed by the camera's make + model as shown in Info
    /// (`"Canon EOS R5"`).
    pub cameras: Vec<CameraDefault>,
    /// Preset applied to non-raw images (JPEG, PNG, TIFF, HEIC…; `None` = none).
    pub other_preset: Option<String>,
    /// Copyright notice given to imported photos that don't carry one (empty = none).
    pub copyright: String,
    /// Creator given to imported photos that don't name one (empty = none).
    pub creator: String,
    /// Metadata preset applied to every imported photo (`metadata.*`; `None` = none).
    pub metadata_preset: Option<String>,
    /// Auto import: a watched folder whose new photos are added as they arrive (`None` = off),
    /// copied into the library's Originals instead of added in place when `auto_copy`, and an
    /// album they go to.
    pub auto_folder: Option<String>,
    pub auto_copy: bool,
    pub auto_album: Option<String>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CameraDefault {
    /// Make + model (`Meta::camera`).
    pub camera: String,
    /// Preset id; `None` = the LightCraft default for this camera.
    pub preset: Option<String>,
}

impl ImportDefaults {
    /// The preset id that applies to a photo of this kind and camera, if any.
    pub fn preset_for(&self, raw: bool, camera: &str) -> Option<&str> {
        if !raw {
            return self.other_preset.as_deref();
        }
        if self.per_camera
            && let Some(c) = self.cameras.iter().find(|c| !camera.is_empty() && c.camera.eq_ignore_ascii_case(camera))
        {
            return c.preset.as_deref();
        }
        self.raw_preset.as_deref()
    }
}

/// Give a freshly imported photo its default settings: the camera defaults, then the matching
/// default preset ([`ImportDefaults::preset_for`]), remembered as the photo's import look.
pub fn apply_import_defaults(s: &Session, p: &mut Photo) {
    // metadata defaults fill gaps only: the file's own copyright / creator win
    let d = &s.import_defaults;
    if p.meta.copyright.trim().is_empty() && !d.copyright.trim().is_empty() {
        p.meta.copyright = d.copyright.trim().to_string();
    }
    if p.meta.creator.trim().is_empty() && !d.creator.trim().is_empty() {
        p.meta.creator = d.creator.trim().to_string();
    }
    if let Some(mp) = d.metadata_preset.as_ref().and_then(|n| s.metadata_presets.iter().find(|m| m.name.eq_ignore_ascii_case(n))) {
        crate::cmd::metadata::apply_to(&mut p.meta, &mp.fields);
    }
    let base = p.camera_defaults();
    // a raw shown from its embedded JPEG gets the rendered-file default, not the raw one
    let raw = p.develops_raw();
    let preset = s.import_defaults.preset_for(raw, &p.meta.camera).and_then(|id| s.presets.iter().find(|x| x.id == id));
    match preset {
        Some(pr) => {
            let look = std::sync::Arc::new(pr.apply(&base, 1.0));
            p.develop = look.clone();
            p.import_look = (*look != base).then_some(look);
        }
        None => {
            p.develop = std::sync::Arc::new(base);
            p.import_look = None;
        }
    }
}

pub fn is_supported(path: &Path) -> bool {
    path.extension().is_some_and(|e| EXTENSIONS.contains(&e.to_string_lossy().to_lowercase().as_str()))
}

/// Expand files and folders (recursively) into supported files. `skip` (e.g. the library folder)
/// is never descended into, including directory aliases. Each folder's walk is bounded
/// ([`crate::walk::Limits::default`]); a walk that stops at a bound is logged.
pub fn expand(paths: &[String], skip: Option<&Path>) -> Vec<String> {
    expand_within(paths, skip, crate::walk::Limits::default()).0
}

/// [`expand`] with the walk bounded by `limits` → (files, whether a walk stopped at a bound).
pub fn expand_within(paths: &[String], skip: Option<&Path>, limits: crate::walk::Limits) -> (Vec<String>, bool) {
    expand_scoped(paths, skip, limits, true, None)
}

/// As [`expand`], optionally reading only the directly named folders' files.
pub fn expand_with_subfolders(paths: &[String], skip: Option<&Path>, subfolders: bool) -> Vec<String> {
    expand_cancellable(paths, skip, subfolders, None)
}

fn expand_cancellable(paths: &[String], skip: Option<&Path>, subfolders: bool, cancel: Option<&std::sync::atomic::AtomicBool>) -> Vec<String> {
    expand_scoped(paths, skip, crate::walk::Limits::default(), subfolders, cancel).0
}

fn expand_scoped(
    paths: &[String],
    skip: Option<&Path>,
    limits: crate::walk::Limits,
    subfolders: bool,
    cancel: Option<&std::sync::atomic::AtomicBool>,
) -> (Vec<String>, bool) {
    let cancelled = || cancel.is_some_and(|flag| flag.load(std::sync::atomic::Ordering::Relaxed));
    if cancelled() {
        return (Vec::new(), false);
    }
    // Identity resolution belongs to the filesystem half, never ScanInput/ImportJob snapshots.
    let canonical_skip = skip.and_then(|p| p.canonicalize().ok());
    let mut directories = std::collections::HashSet::new();
    let mut out = Vec::new();
    let mut truncated = false;
    for p in paths {
        if cancelled() {
            break;
        }
        let p = Path::new(p);
        if skip.is_some_and(|s| p == s) {
            continue;
        }
        if p.is_dir() {
            let options = crate::walk::Options { subfolders, cancel, canonical_skip: canonical_skip.as_deref(), directories: Some(&mut directories) };
            let w = crate::walk::files_in_with(p, skip, limits, options, is_supported);
            if w.truncated {
                log::warn!(
                    "import: stopped looking in {} after {} entries (limits: {} entries, {} folder levels)",
                    p.display(),
                    w.entries,
                    limits.max_entries,
                    limits.max_depth
                );
            }
            truncated |= w.truncated;
            out.extend(w.files.into_iter().map(|f| f.to_string_lossy().to_string()));
        } else {
            // explicitly named files are attempted even with an unknown extension (sniffed)
            out.push(p.to_string_lossy().to_string());
        }
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|p| seen.insert(p.clone()));
    (out, truncated)
}

/// What a path given on the command line at launch becomes ([`launch_path`]).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LaunchPath {
    /// Import this (absolute) file or folder, in the background.
    Import(String),
    /// Not imported: `path` (absolute) is a whole drive or the whole home folder, which is never
    /// walked without being chosen in the app (issue #374).
    Refused { path: String, why: String },
}

/// Resolve a launch argument for importing (issue #374). An empty argument is ignored (`None`);
/// a relative one (`.`, `..`, `photos`; `/` or `\` on Windows, the current drive's root) is
/// resolved against `cwd` — the working folder a launcher picked, often a drive root or
/// `C:\Windows\System32` — and refused when that leaves no folder (`cwd` unknown). A drive
/// or file-system root is replaced by its camera folder (`DCIM`, from `dcim`: a memory card) or
/// refused, and so is the home folder `home`: either would walk most of the disk.
pub fn launch_path(arg: &str, cwd: Option<&Path>, home: Option<&Path>, dcim: impl Fn(&Path) -> Option<PathBuf>) -> Option<LaunchPath> {
    if arg.trim().is_empty() {
        return None;
    }
    let given = Path::new(arg);
    let joined = if given.is_absolute() {
        given.to_path_buf()
    } else {
        match cwd {
            Some(c) => c.join(given),
            None => return Some(LaunchPath::Refused { path: arg.to_string(), why: "a relative path, and the working folder is unknown".into() }),
        }
    };
    // `.` and `..` resolved by name (no file-system access)
    let mut abs = PathBuf::new();
    for c in joined.components() {
        match c {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                abs.pop();
            }
            other => abs.push(other.as_os_str()),
        }
    }
    let shown = abs.to_string_lossy().to_string();
    if abs.parent().is_none() {
        return Some(match dcim(&abs) {
            Some(d) => LaunchPath::Import(d.to_string_lossy().to_string()),
            None => LaunchPath::Refused { path: shown, why: "a whole drive".into() },
        });
    }
    let key = lightcraft_catalog::query::folder_key;
    if home.is_some_and(|h| key(&h.to_string_lossy()) == key(&shown)) {
        return Some(LaunchPath::Refused { path: shown, why: "the whole home folder".into() });
    }
    Some(LaunchPath::Import(shown))
}

/// Probe files (headers + content hash) as an import would.
pub(crate) fn probe_paths(s: &Session, paths: &[String]) -> Vec<Result<ProbeInfo, String>> {
    let progress = ScanProgress::default();
    probe_all(s.media.file_probe.as_ref(), paths, &progress, &progress.cancel)
}

fn probe_all(
    probe: Option<&crate::media::FileProbe>,
    paths: &[String],
    progress: &ScanProgress,
    cancel: &std::sync::atomic::AtomicBool,
) -> Vec<Result<ProbeInfo, String>> {
    use std::sync::atomic::Ordering::Relaxed;
    let Some(probe) = probe.cloned() else {
        return paths
            .iter()
            .map(|p| {
                Ok(ProbeInfo {
                    format: Path::new(p).extension().map(|e| e.to_string_lossy().to_uppercase()).unwrap_or_default(),
                    ..Default::default()
                })
            })
            .collect();
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        let n = workers(paths.len());
        if n > 1 {
            let mut results: Vec<Option<Result<ProbeInfo, String>>> = vec![None; paths.len()];
            let next = std::sync::atomic::AtomicUsize::new(0);
            let out = std::sync::Mutex::new(&mut results);
            std::thread::scope(|sc| {
                for _ in 0..n {
                    sc.spawn(|| {
                        loop {
                            let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                            if i >= paths.len() {
                                break;
                            }
                            if cancel.load(Relaxed) {
                                break;
                            }
                            let r = probe(&paths[i]);
                            progress.done.fetch_add(1, Relaxed);
                            out.lock().unwrap_or_else(|e| e.into_inner())[i] = Some(r);
                        }
                    });
                }
            });
            return results.into_iter().map(|r| r.unwrap_or_else(|| Err("not probed".into()))).collect();
        }
    }
    paths
        .iter()
        .map(|p| {
            if cancel.load(Relaxed) {
                return Err("cancelled".into());
            }
            let r = probe(p);
            progress.done.fetch_add(1, Relaxed);
            r
        })
        .collect()
}

/// Copy `src` into `dir` as `name` (default: its own name), made unique with -1, -2…; returns the
/// new path. The copy is verified ([`crate::import_move::copy_new`]: a new file, synced and
/// checked against `probe_hash`, the content hash the probe computed — or compared byte for byte
/// with the source when there is none); a bad one is removed and the photo reported as failed.
fn copy_into(dir: &Path, src: &str, name: Option<&str>, probe_hash: Option<&str>) -> Result<String, String> {
    let name = name
        .map(str::to_string)
        .unwrap_or_else(|| Path::new(src).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| "photo".into()));
    let expect = probe_hash.and_then(lightcraft_preview::Hash128::parse);
    crate::import_move::copy_new(Path::new(src), dir, &name, expect).map(|p| p.to_string_lossy().to_string())
}

/// Source facts and ownership only; independent edits/history stay in the owner catalog.
#[derive(Clone, Debug)]
struct LocalReference {
    id: PhotoId,
    path: String,
    width: u32,
    height: u32,
    file_size: u64,
    format: String,
    content_hash: Option<String>,
}

impl LocalReference {
    fn of(photo: &Photo, path: &str) -> Self {
        Self {
            id: photo.id,
            path: path.into(),
            width: photo.width,
            height: photo.height,
            file_size: photo.file_size,
            format: photo.format.clone(),
            content_hash: photo.content_hash.clone(),
        }
    }

    fn matches_probe(&self, info: &ProbeInfo) -> bool {
        self.width == info.width
            && self.height == info.height
            && self.file_size == info.file_size
            && self.format.eq_ignore_ascii_case(&info.format)
            && match (&self.content_hash, &info.content_hash) {
                (Some(old), Some(current)) => old == current,
                // A native probe has a full content hash; absence of the retained hash cannot
                // establish that this is still the browsed file. Custom/virtual hooks keep theirs.
                (None, _) => info.file_stamp.is_none(),
                (Some(_), None) => false,
            }
    }

    fn matches_owner(&self, photo: &Photo) -> bool {
        matches!(&photo.source, Source::File { path } if path == &self.path)
            && photo.width == self.width
            && photo.height == self.height
            && photo.file_size == self.file_size
            && photo.format == self.format
            && photo.content_hash == self.content_hash
    }
}

fn changed_local_source() -> String {
    "Local source no longer matches its retained file facts; Reload the photo, then review the import again. Its edits and original stay unchanged."
        .into()
}

/// What a [`scan`] needs from the session, so it can run on another thread (a folder on a network
/// share can take minutes to read).
pub struct ScanInput {
    subfolders: bool,
    probe: Option<crate::media::FileProbe>,
    skip: Option<PathBuf>,
    /// path → (photo, its summary) for files already in the library.
    by_path: HashMap<PathBuf, (PhotoId, ImportCandidate)>,
    known_paths: Vec<String>,
    by_hash: HashMap<String, PhotoId>,
    local: HashMap<PhotoId, LocalReference>,
    cache: std::sync::Arc<HashMap<String, ProbeInfo>>,
}

/// Progress and cancellation of a running [`scan_with`].
#[derive(Default)]
pub struct ScanProgress {
    /// Files to probe, set once the folders are expanded.
    pub total: std::sync::atomic::AtomicUsize,
    pub done: std::sync::atomic::AtomicUsize,
    pub cancel: std::sync::atomic::AtomicBool,
}

/// The result of [`scan_with`]: the candidates, and the probes to keep for the import that follows.
pub struct ScanOutput {
    /// Whether a directly named source was a folder (checked on the scan worker).
    pub has_folders: bool,
    pub candidates: Vec<ImportCandidate>,
    pub probes: HashMap<String, ProbeInfo>,
}

/// Prefer a real library record when Local browsing has also seen the same bytes. Among
/// library records, a live photo wins over a trashed one; IDs make ties deterministic.
fn prefer_duplicate(photo: &Photo, previous: &Photo) -> bool {
    (photo.local, photo.deleted, photo.id) < (previous.local, previous.deleted, previous.id)
}

// A path with a retained live Local copy is still promotable, including catalogs saved
// after the old Add behavior promoted only its master. Content duplicates keep the separate
// library-first preference above; importing this source preserves every reference's edits.
fn import_path_rank(photo: &Photo) -> (bool, bool, PhotoId) {
    (!(photo.local && !photo.deleted), photo.deleted, photo.id)
}

fn prefer_import_path(photo: &Photo, previous: &Photo) -> bool {
    import_path_rank(photo) < import_path_rank(previous)
}

fn duplicate_hashes(s: &Session, on_deleted: OnDeleted) -> HashMap<String, PhotoId> {
    let mut hashes = HashMap::new();
    for photo in s.catalog.photos() {
        if photo.deleted && on_deleted == OnDeleted::Fresh {
            continue;
        }
        if let Some(hash) = &photo.content_hash {
            let replace = hashes.get(hash).and_then(|id| s.catalog.photo(*id)).is_none_or(|previous| prefer_duplicate(photo, previous));
            if replace {
                hashes.insert(hash.clone(), photo.id);
            }
        }
    }
    hashes
}

// Built on the filesystem worker, never while snapshotting the session. Content hashes
// cannot identify Local files: separate physical copies intentionally keep separate edits.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
enum NativePath {
    #[cfg(unix)]
    File(u64, u64),
    #[cfg(not(unix))]
    Canonical(PathBuf),
}

fn native_path(path: &str) -> Option<NativePath> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        let metadata = std::fs::metadata(path).ok()?;
        metadata.is_file().then(|| NativePath::File(metadata.dev(), metadata.ino()))
    }
    #[cfg(not(unix))]
    {
        Path::new(path).canonicalize().ok().filter(|p| p.is_file()).map(NativePath::Canonical)
    }
}

fn known_paths(s: &Session, on_deleted: OnDeleted) -> Vec<String> {
    let mut photos: Vec<_> = s.catalog.photos().filter(|p| !p.deleted || on_deleted != OnDeleted::Fresh).collect();
    photos.sort_by_key(|p| import_path_rank(p));
    photos
        .into_iter()
        .filter_map(|p| match &p.source {
            Source::File { path } => Some(path.clone()),
            _ => None,
        })
        .collect()
}

fn native_paths(paths: &[String], cancel: &std::sync::atomic::AtomicBool) -> HashMap<NativePath, String> {
    let mut native = HashMap::new();
    for path in paths {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            break;
        }
        if let Some(key) = native_path(path) {
            native.entry(key).or_insert_with(|| path.clone());
        }
    }
    native
}

fn native_match<'a>(path: &str, native: &'a HashMap<NativePath, String>) -> Option<&'a str> {
    let previous = native.get(&native_path(path)?)?;
    #[cfg(unix)]
    {
        use crate::rename::MoveFs;
        if !crate::rename::RealFs.same_file(Path::new(path), Path::new(previous)) {
            return None;
        }
    }
    Some(previous)
}

/// Worker-resolved alternate paths for the current Local view. The stored source and all
/// edits stay unchanged; querying/revealing a photo does not inspect the filesystem.
pub(crate) struct BrowseAliases {
    browse: crate::Browse,
    library: std::sync::Weak<()>,
    paths: HashMap<String, String>,
}

impl Session {
    /// Replace the current folder's worker-resolved aliases without changing stored records.
    pub fn set_browse_aliases(&mut self, paths: impl IntoIterator<Item = (String, PhotoId)>) {
        self.browse_aliases = None;
        self.remember_browse_aliases(paths);
    }

    /// Record only identity matches returned by a Local scan/import, never content duplicates.
    pub fn remember_browse_aliases(&mut self, paths: impl IntoIterator<Item = (String, PhotoId)>) {
        let Some(browse) = self.browse.clone() else { return };
        let current =
            self.browse_aliases.as_ref().is_some_and(|a| a.browse == browse && a.library.as_ptr() == std::sync::Arc::as_ptr(&self.library_identity));
        if !current {
            self.browse_aliases = Some(BrowseAliases { browse, library: std::sync::Arc::downgrade(&self.library_identity), paths: HashMap::new() });
        }
        if let Some(aliases) = &mut self.browse_aliases {
            for (path, id) in paths {
                if let Some(photo) = self.catalog.photo(id)
                    && let Source::File { path: source } = &photo.source
                    && source != &path
                {
                    aliases.paths.insert(source.clone(), path);
                }
            }
        }
        self.visible_key = None;
        self.total = None;
    }

    pub(crate) fn browse_aliases_current(&self) -> bool {
        self.browse_aliases.as_ref().is_some_and(|a| {
            self.browse.as_ref() == Some(&a.browse) && a.library.as_ptr() == std::sync::Arc::as_ptr(&self.library_identity) && !a.paths.is_empty()
        })
    }

    pub(crate) fn query_browse(&self, filter: &lightcraft_catalog::Filter, sort: &lightcraft_catalog::Sort) -> Vec<PhotoId> {
        let aliases = self.browse_aliases.as_ref().filter(|_| self.browse_aliases_current());
        let Some(aliases) = aliases else { return self.catalog.query(filter, sort) };
        let mut photos: Vec<_> = self
            .catalog
            .photos()
            .filter(|photo| {
                if filter.matches(photo, &self.catalog) {
                    return true;
                }
                // A virtual copy has its own ID and edits, but shares this unchanged source.
                let Source::File { path: source } = &photo.source else { return false };
                let Some(path) = aliases.paths.get(source) else { return false };
                let mut view = photo.as_ref().clone();
                view.source = Source::File { path: path.clone() };
                filter.matches(&view, &self.catalog)
            })
            .collect();
        // The same ordering as Catalog::query, including shuffle and deterministic ties.
        photos.sort_by(|a, b| {
            use lightcraft_catalog::{SortKey, mix64};
            let order = match sort.key {
                SortKey::CaptureDate => a.date().cmp(b.date()),
                SortKey::ImportDate => a.imported.cmp(&b.imported),
                SortKey::EditDate => a.edited.cmp(&b.edited),
                SortKey::FileName => a.file_name.to_lowercase().cmp(&b.file_name.to_lowercase()),
                SortKey::Rating => a.rating.cmp(&b.rating),
                SortKey::FileSize => a.file_size.cmp(&b.file_size),
                SortKey::Random => (mix64(mix64(sort.seed) ^ a.id.0)).cmp(&mix64(mix64(sort.seed) ^ b.id.0)),
            }
            .then(a.id.cmp(&b.id));
            if sort.ascending { order } else { order.reverse() }
        });
        photos.into_iter().map(|p| p.id).collect()
    }
}

impl ScanInput {
    pub fn new(s: &mut Session, paths: &[String]) -> (Self, Vec<String>) {
        let skip = s.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.clone());
        let mut by_path = HashMap::new();
        let by_hash = duplicate_hashes(s, OnDeleted::Skip);
        let mut local = HashMap::new();
        for p in s.catalog.photos() {
            if let Source::File { path } = &p.source {
                if p.local && !p.deleted {
                    local.insert(p.id, LocalReference::of(p, path));
                }
                let c = ImportCandidate {
                    path: path.clone(),
                    format: p.format.clone(),
                    kind: p.kind,
                    width: p.width,
                    height: p.height,
                    file_size: p.file_size,
                    captured: p.captured.clone(),
                    // Local-only records can be promoted through this file identity on import.
                    duplicate: (!p.local || p.deleted).then(|| "path".into()),
                    existing: Some(p.id.0),
                    ..Default::default()
                };
                let key = PathBuf::from(path);
                if by_path.get(&key).and_then(|(id, _)| s.catalog.photo(*id)).is_none_or(|previous| prefer_import_path(p, previous)) {
                    by_path.insert(key, (p.id, c));
                }
            }
        }
        let cache = if s.import_probes.is_empty() {
            s.import_probe_cache.clone().unwrap_or_default()
        } else {
            std::sync::Arc::new(std::mem::take(&mut s.import_probes))
        };
        s.import_probe_cache = Some(cache.clone());
        let input = ScanInput {
            subfolders: true,
            probe: s.media.file_probe.clone(),
            skip,
            by_path,
            known_paths: known_paths(s, OnDeleted::Skip),
            by_hash,
            local,
            cache,
        };
        (input, paths.to_vec())
    }

    /// Include nested folders (the default), or just the directly named folders' files.
    pub fn with_subfolders(mut self, subfolders: bool) -> Self {
        self.subfolders = subfolders;
        self
    }
}

/// Find and probe what importing `paths` would add, marking duplicates (by path, or by content
/// against the library and earlier candidates). Nothing is added; the probes are kept for the
/// import that follows.
pub fn scan(s: &mut Session, paths: &[String]) -> Vec<ImportCandidate> {
    scan_with_subfolders(s, paths, true)
}

/// Preview only the directly named folders when `subfolders` is false.
pub fn scan_with_subfolders(s: &mut Session, paths: &[String], subfolders: bool) -> Vec<ImportCandidate> {
    let (input, paths) = ScanInput::new(s, paths);
    let input = input.with_subfolders(subfolders);
    let out = scan_with(input, &paths, &ScanProgress::default());
    s.accept_import_scan(out.probes);
    out.candidates
}

/// [`scan`] without the session, so it can run on a worker thread. Stops early (returning what it
/// has) when `progress.cancel` is set.
pub fn scan_with(input: ScanInput, paths: &[String], progress: &ScanProgress) -> ScanOutput {
    use std::sync::atomic::Ordering::Relaxed;
    if progress.cancel.load(Relaxed) {
        return ScanOutput { has_folders: false, candidates: Vec::new(), probes: HashMap::new() };
    }
    let has_folders = paths.iter().take_while(|_| !progress.cancel.load(Relaxed)).any(|p| Path::new(p).is_dir());
    let files = expand_cancellable(paths, input.skip.as_deref(), input.subfolders, Some(&progress.cancel));
    let native = if files.iter().any(|f| !input.by_path.contains_key(Path::new(f))) {
        native_paths(&input.known_paths, &progress.cancel)
    } else {
        HashMap::new()
    };
    let matches: HashMap<_, _> = files
        .iter()
        .take_while(|_| !progress.cancel.load(Relaxed))
        .filter_map(|f| {
            let key = if input.by_path.contains_key(Path::new(f)) { f.as_str() } else { native_match(f, &native)? };
            Some((f.clone(), PathBuf::from(key)))
        })
        .collect();
    // A selectable Local path is an existing identity, not proof of unchanged bytes.
    // Resolve native facts only here on the filesystem worker. Virtual/custom sources retain
    // their existing in-memory promotion contract.
    let native_local: std::collections::HashSet<String> = matches
        .iter()
        .take_while(|_| !progress.cancel.load(Relaxed))
        .filter(|(path, key)| input.by_path.get(*key).is_some_and(|(id, _)| input.local.contains_key(id)) && native_path(path).is_some())
        .map(|(path, _)| path.clone())
        .collect();
    let todo: Vec<String> = files.iter().filter(|f| !matches.contains_key(*f) || native_local.contains(*f)).cloned().collect();
    progress.total.store(todo.len(), Relaxed);
    // Recheck native source facts on this worker before reusing a preceding review.
    let cached: Vec<Option<ProbeInfo>> = todo
        .iter()
        .map(|f| {
            if progress.cancel.load(Relaxed) {
                return None;
            }
            input.cache.get(f).filter(|info| info.cache_is_current(f)).cloned()
        })
        .collect();
    let missing: Vec<String> = todo.iter().zip(&cached).filter(|(_, c)| c.is_none()).map(|(f, _)| f.clone()).collect();
    progress.done.store(todo.len() - missing.len(), Relaxed);
    let mut fresh = probe_all(input.probe.as_ref(), &missing, progress, &progress.cancel).into_iter();
    let probed: Vec<Result<ProbeInfo, String>> =
        cached.into_iter().map(|c| c.map(Ok).unwrap_or_else(|| fresh.next().unwrap_or_else(|| Err("not probed".into())))).collect();
    crate::memory::release();
    let mut probes: HashMap<String, Result<ProbeInfo, String>> = todo.into_iter().zip(probed).collect();
    let mut seen_hash: HashMap<String, Option<u64>> = input.by_hash.into_iter().map(|(h, id)| (h, Some(id.0))).collect();
    let mut out = Vec::with_capacity(files.len());
    let mut kept = HashMap::new();
    for f in files {
        if progress.cancel.load(Relaxed) {
            break;
        }
        let selection_identity = Path::new(&f).canonicalize().ok().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
        if let Some((_, c)) = matches.get(&f).and_then(|key| input.by_path.get(key)) {
            let mut c = c.clone();
            c.path = f.clone();
            c.selection_identity = selection_identity;
            c.name = Path::new(&f).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| f.clone());
            if native_local.contains(&f) {
                match probes.remove(&f) {
                    Some(Ok(info)) => {
                        if c.existing.and_then(|id| input.local.get(&PhotoId(id))).is_none_or(|local| !local.matches_probe(&info)) {
                            c.error = Some(changed_local_source());
                        }
                        (c.format, c.kind, c.width, c.height, c.file_size, c.captured) =
                            (info.format.clone(), info.kind, info.width, info.height, info.file_size, info.captured.clone());
                        c.preview_only = info.preview_only.clone();
                        kept.insert(f.clone(), info);
                    }
                    Some(Err(error)) => c.error = Some(error),
                    None => c.error = Some("not probed".into()),
                }
            }
            out.push(c);
            continue;
        }
        let name = Path::new(&f).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| f.clone());
        let mut c = ImportCandidate { path: f.clone(), selection_identity, name, ..Default::default() };
        match probes.remove(&f) {
            Some(Ok(info)) => {
                (c.format, c.kind, c.width, c.height, c.file_size, c.captured) =
                    (info.format.clone(), info.kind, info.width, info.height, info.file_size, info.captured.clone());
                c.preview_only = info.preview_only.clone();
                if let Some(h) = &info.content_hash {
                    match seen_hash.get(h) {
                        Some(existing) => {
                            c.duplicate = Some("content".into());
                            c.existing = *existing;
                        }
                        None => {
                            seen_hash.insert(h.clone(), None);
                        }
                    }
                }
                kept.insert(f, info);
            }
            Some(Err(e)) => c.error = Some(e),
            None => c.error = Some("not probed".into()),
        }
        out.push(c);
    }
    ScanOutput { has_folders, candidates: out, probes: kept }
}

/// Import files/folders. See the module docs.
pub fn import(s: &mut Session, paths: &[String], mode: ImportMode) -> crate::Result<ImportReport> {
    import_with(s, paths, &ImportOptions { mode, ..Default::default() })
}

/// Import files/folders with options (preset, keywords). See the module docs.
///
/// The file-system half ([`ImportJob::prepare`]: expanding folders, probing, reading sidecars,
/// copying / placing files) and the catalog half ([`commit_prepared`]) are separate so the app can
/// run the first on a worker thread; here both run in turn.
pub fn import_with(s: &mut Session, paths: &[String], opts: &ImportOptions) -> crate::Result<ImportReport> {
    let mut job = ImportJob::new(s, opts.clone())?;
    let prepared = job.prepare(paths, &std::sync::atomic::AtomicBool::new(false));
    // probes a scan left for files not imported now stay for a later import
    s.import_probes.extend(std::mem::take(&mut job.cache));
    s.import_probe_cache = job.shared_cache.take();
    commit_prepared(s, &job.opts, &job.now, prepared)
}

/// Everything the file-system half of an import needs, detached from the session so it can run
/// on another thread (a network share or a card can take minutes). Built by [`ImportJob::new`];
/// [`ImportJob::prepare`] may be called for several batches in turn (renamed copies keep counting,
/// duplicates are found across batches); each batch is then [`commit_prepared`] on the session.
pub struct ImportJob {
    pub opts: ImportOptions,
    /// The mode in effect (Copy / Move become Add in a library without its own folder).
    mode: ImportMode,
    lib_dir: Option<PathBuf>,
    copy_root: Option<PathBuf>,
    /// path → (photo, a Local browse record?)
    by_path: HashMap<String, (PhotoId, bool)>,
    known_paths: Vec<String>,
    references: Vec<LocalReference>,
    reviewed_paths: std::sync::Arc<std::collections::HashSet<String>>,
    native_references: Option<HashMap<NativePath, Vec<LocalReference>>>,
    promoted_native: HashMap<NativePath, (PhotoId, ProbeInfo)>,
    library_identity: std::sync::Arc<()>,
    native_paths: Option<HashMap<NativePath, String>>,
    pending_local: HashMap<NativePath, String>,
    /// content hash → the photo that has it (`None`: a file earlier in this import)
    by_hash: HashMap<String, Option<PhotoId>>,
    probe: Option<crate::media::FileProbe>,
    file_bytes: Option<crate::merge::ByteReader>,
    /// Probes from a preceding scan (not read again).
    cache: HashMap<String, ProbeInfo>,
    shared_cache: Option<std::sync::Arc<HashMap<String, ProbeInfo>>>,
    naming: crate::sidecar::SidecarNaming,
    /// Exact retained repairs, captured in memory; path identity is resolved on the worker.
    retained_sidecars: Vec<(PathBuf, PathBuf)>,
    move_repair_paths: Option<HashMap<PathBuf, String>>,
    now: String,
    seq: usize,
    /// Files found so far (folders expanded) and handled so far, for progress.
    pub total: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    pub done: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

#[derive(Clone, Debug)]
struct LocalPromotion {
    id: PhotoId,
    references: Vec<LocalReference>,
    reviewed_paths: std::sync::Arc<std::collections::HashSet<String>>,
    library: std::sync::Arc<()>,
    // No filesystem checks on the owner. A new retained repair rejects this prepared Move;
    // the worker's existing exact-path preflight already covered the captured repairs.
    retained_sidecars: Option<Vec<(PathBuf, PathBuf)>>,
    destination_stamp: Option<crate::media::FileStamp>,
}

/// The outcome of [`ImportJob::prepare`] for one batch, in file order.
#[derive(Debug, Default)]
pub struct Prepared {
    pub scanned: usize,
    items: Vec<PreparedItem>,
}

#[derive(Debug)]
enum PreparedItem {
    /// A Local (browsed) record imported for real.
    Promote {
        promotion: LocalPromotion,
        info: Option<ProbeInfo>,
    },
    /// Another spelling of a Local file readied earlier in this import.
    LocalAlias {
        path: String,
        source: String,
    },
    Duplicate {
        path: String,
        existing: Option<PhotoId>,
        reason: &'static str,
        hash: Option<String>,
    },
    Failed(String, String),
    Kept(Kept),
    Ready(Box<ReadyFile>),
}

#[derive(Debug)]
struct ReadyFile {
    path: String,
    stored: String,
    info: ProbeInfo,
    sidecar: Option<crate::sidecar::SidecarData>,
    placed: Option<crate::import_move::Placed>,
    promotion: Option<LocalPromotion>,
}

impl Prepared {
    /// Undo what the batch did on disk that only a commit would make permanent: files placed by a
    /// Move (their sources were never touched). Copies stay, like copies of a failed commit.
    pub fn rollback(&self) {
        for it in &self.items {
            if let PreparedItem::Ready(r) = it
                && let Some(pl) = &r.placed
            {
                crate::import_move::rollback(pl);
            } else if let PreparedItem::Ready(r) = it
                && r.promotion.is_some()
            {
                // Only this promotion's newly created Copy; never its retained source.
                let _ = std::fs::remove_file(&r.stored);
            }
        }
    }

    /// Entries in the batch (after expanding folders).
    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// Recheck Add-only entries against the owner catalog immediately before commit.
    /// Preparation may have run while another import added the same path or content.
    pub fn revalidate_add(&mut self, s: &Session) {
        let mut paths = HashMap::new();
        let mut hashes = HashMap::new();
        for photo in s.catalog.photos() {
            if let Source::File { path } = &photo.source {
                paths.insert(normalize_import_path(path), photo.id);
            }
            if let Some(hash) = &photo.content_hash {
                hashes.insert(hash.clone(), photo.id);
            }
        }
        let mut items = Vec::with_capacity(self.items.len());
        for item in std::mem::take(&mut self.items) {
            let PreparedItem::Ready(ready) = item else {
                items.push(item);
                continue;
            };
            if ready.promotion.is_some() {
                items.push(PreparedItem::Ready(ready));
                continue;
            }
            let ReadyFile { path, stored, info, sidecar, placed, promotion } = *ready;
            let existing_path = paths.get(&normalize_import_path(&stored)).copied().or_else(|| paths.get(&normalize_import_path(&path)).copied());
            let existing_hash = info.content_hash.as_ref().and_then(|hash| hashes.get(hash).copied());
            if let Some(existing) = existing_path.or(existing_hash) {
                if let Some(placed) = &placed {
                    crate::import_move::rollback(placed);
                }
                items.push(PreparedItem::Duplicate {
                    path,
                    existing: Some(existing),
                    reason: if existing_path.is_some() { "path" } else { "content" },
                    hash: info.content_hash,
                });
            } else {
                items.push(PreparedItem::Ready(Box::new(ReadyFile { path, stored, info, sidecar, placed, promotion })));
            }
        }
        self.items = items;
    }
}

fn normalize_import_path(path: &str) -> String {
    let path = path.replace('\\', "/");
    if cfg!(windows) { path.to_lowercase() } else { path }
}

// Used only by worker preparation, never by ImportJob::new/validate or a UI query.
fn repair_path_key(path: &Path) -> PathBuf {
    if let Some(name) = path.file_name() {
        let parent = path.parent().filter(|p| !p.as_os_str().is_empty()).unwrap_or_else(|| Path::new("."));
        parent.canonicalize().unwrap_or_else(|_| parent.to_path_buf()).join(name)
    } else {
        path.canonicalize().unwrap_or_else(|_| path.to_path_buf())
    }
}

fn retained_repair_paths(repairs: &[(PathBuf, PathBuf)], cancel: &std::sync::atomic::AtomicBool) -> Option<HashMap<PathBuf, String>> {
    let mut paths = HashMap::new();
    for (from, to) in repairs {
        if cancel.load(std::sync::atomic::Ordering::Relaxed) {
            return None;
        }
        let error = format!(
            "Move refused: unfinished XMP repair {} → {}. Retry the failed Rename or file Undo/Redo to finish the repair, then retry Move; the original and sidecars stay in place.",
            from.display(),
            to.display()
        );
        for path in [from, to] {
            paths.entry(repair_path_key(path)).or_insert_with(|| error.clone());
        }
    }
    Some(paths)
}

fn move_repair_error<'a>(original: &str, repairs: &'a HashMap<PathBuf, String>) -> Option<&'a String> {
    use crate::sidecar::{SidecarNaming, sidecar_path};
    if repairs.is_empty() {
        return None;
    }
    [SidecarNaming::Stem, SidecarNaming::Full].into_iter().find_map(|naming| {
        let sidecar = sidecar_path(original, naming);
        [sidecar.clone(), sidecar.with_extension("XMP")].into_iter().find_map(|path| repairs.get(&repair_path_key(&path)))
    })
}

fn move_destination_repair<'a>(root: Option<&Path>, repairs: &'a HashMap<PathBuf, String>) -> Option<&'a String> {
    if repairs.is_empty() {
        return None;
    }
    let root = root?;
    let root = root.canonicalize().unwrap_or_else(|_| repair_path_key(root));
    // Templates may depend on not-yet-probed metadata. Refuse this output root rather than
    // risk taking a pending repair's destination name while deriving the transfer plan.
    repairs.iter().find(|(path, _)| path.starts_with(&root)).map(|(_, error)| error)
}

impl ImportJob {
    /// Check mode and destination requirements without taking review probes or mutating state.
    pub fn validate(s: &Session, opts: &ImportOptions) -> crate::Result<()> {
        Self::destination(s, opts).map(|_| ())
    }

    fn destination(s: &Session, opts: &ImportOptions) -> crate::Result<(ImportMode, Option<PathBuf>, Option<PathBuf>)> {
        let mode = opts.mode;
        // Only a library on disk has an `Originals/` folder. The browser build keeps the bytes of
        // every added file in its own storage already, so "copy" there means "add".
        let transfer = matches!(mode, ImportMode::Copy | ImportMode::Move);
        let mode = if transfer && s.library.as_ref().is_some_and(|l| !l.on_disk) { ImportMode::Add } else { mode };
        if mode == ImportMode::Move && opts.local {
            return Err(crate::EngineError::Other("browsing a folder never moves its files".into()));
        }
        let lib_dir = s.library.as_ref().filter(|l| l.on_disk).map(|l| l.dir.clone());
        // where copies go: the chosen folder, else the library's Originals/
        let copy_root = opts
            .destination
            .as_deref()
            .filter(|d| !d.trim().is_empty())
            .map(std::path::PathBuf::from)
            .or_else(|| lib_dir.as_ref().map(|l| l.join("Originals")));
        if matches!(mode, ImportMode::Copy | ImportMode::Move) && copy_root.is_none() {
            return Err(crate::EngineError::Other("copying or moving needs a destination folder or an open library".into()));
        }
        Ok((mode, lib_dir, copy_root))
    }

    /// Snapshot what an import with `opts` needs from the session (no file-system calls). Takes
    /// the probes a preceding scan left ([`Session::import_probes`]).
    pub fn new(s: &mut Session, opts: ImportOptions) -> crate::Result<Self> {
        let (mode, lib_dir, copy_root) = Self::destination(s, &opts)?;
        let mut by_path = HashMap::new();
        let by_hash = duplicate_hashes(s, opts.on_deleted).into_iter().map(|(hash, id)| (hash, Some(id))).collect();
        for p in s.catalog.photos() {
            // a trashed photo is replaced by a fresh import, so it doesn't count as a duplicate
            if p.deleted && opts.on_deleted == OnDeleted::Fresh {
                continue;
            }
            if let Source::File { path } = &p.source
                && by_path.get(path).and_then(|(id, _)| s.catalog.photo(*id)).is_none_or(|previous| prefer_import_path(p, previous))
            {
                by_path.insert(path.clone(), (p.id, p.local && !p.deleted));
            }
        }
        let known_paths = known_paths(s, opts.on_deleted);
        let references: Vec<LocalReference> = s
            .catalog
            .photos()
            .filter_map(|photo| match &photo.source {
                Source::File { path } => Some(LocalReference::of(photo, path)),
                _ => None,
            })
            .collect();
        let reviewed_paths = std::sync::Arc::new(references.iter().map(|r| r.path.clone()).collect());
        Ok(ImportJob {
            seq: opts.rename_start.max(1),
            opts,
            mode,
            lib_dir,
            copy_root,
            by_path,
            known_paths,
            references,
            reviewed_paths,
            native_references: None,
            promoted_native: HashMap::new(),
            library_identity: s.library_identity.clone(),
            native_paths: None,
            pending_local: HashMap::new(),
            by_hash,
            probe: s.media.file_probe.clone(),
            file_bytes: s.media.file_bytes.clone(),
            cache: std::mem::take(&mut s.import_probes),
            shared_cache: s.import_probe_cache.take(),
            naming: s.xmp.naming,
            retained_sidecars: if mode == ImportMode::Move { retained_sidecars(s) } else { Vec::new() },
            move_repair_paths: None,
            now: (s.clock)(),
            total: Default::default(),
            done: Default::default(),
        })
    }

    /// The file-system half of importing `paths`: expand folders, skip known paths, probe, skip
    /// duplicates by content, read sidecars, copy or place (Move) the files — several at once,
    /// with the names and outcomes of one after another. Nothing in the catalog changes. Stops
    /// before the next file once `cancel` is set.
    pub fn prepare(&mut self, paths: &[String], cancel: &std::sync::atomic::AtomicBool) -> Prepared {
        let files = self.expand_with_cancel(paths, cancel);
        self.prepare_files(files, cancel)
    }

    /// Expand `paths` (folders recursively, the library's own folder skipped) into the files an
    /// import would handle; counted in [`ImportJob::total`].
    pub fn expand(&self, paths: &[String]) -> Vec<String> {
        let files = expand(paths, self.lib_dir.as_deref());
        self.total.fetch_add(files.len(), std::sync::atomic::Ordering::Relaxed);
        files
    }

    /// Expand on the filesystem worker, stopping before further listing once its owner cancels.
    pub fn expand_with_cancel(&self, paths: &[String], cancel: &std::sync::atomic::AtomicBool) -> Vec<String> {
        let files = expand_cancellable(paths, self.lib_dir.as_deref(), true, Some(cancel));
        self.total.fetch_add(files.len(), std::sync::atomic::Ordering::Relaxed);
        files
    }

    /// When the import started ([`Session::clock`]), for [`commit_prepared`].
    pub fn now(&self) -> &str {
        &self.now
    }

    /// [`ImportJob::prepare`] for files [`ImportJob::expand`] returned.
    pub fn prepare_files(&mut self, files: Vec<String>, cancel: &std::sync::atomic::AtomicBool) -> Prepared {
        use std::sync::atomic::Ordering::Relaxed;
        let opts = self.opts.clone();
        let mode = self.mode;
        let mut out = Prepared { scanned: files.len(), items: Vec::new() };
        // Retained XMP work must not be orphaned by a transfer. This preflight is wholly on
        // the worker and runs before consuming a probe, reading XMP or placing any file.
        if cancel.load(Relaxed) {
            return out;
        }
        if self.move_repair_paths.is_none() {
            let Some(paths) = retained_repair_paths(&self.retained_sidecars, cancel) else { return out };
            self.move_repair_paths = Some(paths);
        }
        let Some(repairs) = self.move_repair_paths.as_ref() else { return out };
        let destination_repair = move_destination_repair(self.copy_root.as_deref(), repairs).cloned();
        let mut todo = Vec::new();
        if self.native_paths.is_none() && files.iter().any(|f| !self.by_path.contains_key(f)) {
            self.native_paths = Some(native_paths(&self.known_paths, cancel));
        }
        // Build source-reference identity groups once, lazily, on this worker. Separate files
        // with equal bytes never join this map. No edits/history or native facts were captured
        // by ImportJob::new on the UI owner.
        let has_local = !opts.local
            && files.iter().any(|f| {
                let key = if self.by_path.contains_key(f) {
                    Some(f.as_str())
                } else {
                    self.native_paths.as_ref().and_then(|native| native_match(f, native))
                };
                key.and_then(|key| self.by_path.get(key)).is_some_and(|(_, local)| *local)
            });
        if has_local && mode != ImportMode::Add && self.native_references.is_none() {
            let mut groups: HashMap<NativePath, Vec<LocalReference>> = HashMap::new();
            let mut paths: HashMap<&str, Vec<&LocalReference>> = HashMap::new();
            for reference in &self.references {
                paths.entry(&reference.path).or_default().push(reference);
            }
            for (path, references) in paths {
                if cancel.load(Relaxed) {
                    return out;
                }
                if let Some(key) = native_path(path) {
                    groups.entry(key).or_default().extend(references.into_iter().cloned());
                }
            }
            for references in groups.values_mut() {
                references.sort_by_key(|r| r.id);
            }
            self.native_references = Some(groups);
        }
        let mut promotions = HashMap::new();
        let mut queued = std::collections::HashSet::new();
        for f in files {
            if cancel.load(Relaxed) {
                break;
            }
            if mode == ImportMode::Move
                && let Some(error) = destination_repair.as_ref().or_else(|| move_repair_error(&f, repairs))
            {
                out.items.push(PreparedItem::Failed(f, error.clone()));
                self.done.fetch_add(1, Relaxed);
                continue;
            }
            // Alias spellings later in the same job still deduplicate after a Move removed the
            // representative source. The recorded native stamp validates that surviving alias.
            if let Some(key) = native_path(&f)
                && let Some((id, info)) = self.promoted_native.get(&key)
                && info.cache_is_current(&f)
            {
                out.items.push(PreparedItem::Duplicate { path: f, existing: Some(*id), reason: "path", hash: None });
                self.done.fetch_add(1, Relaxed);
                continue;
            }
            let key =
                if self.by_path.contains_key(&f) { Some(f.as_str()) } else { self.native_paths.as_ref().and_then(|native| native_match(&f, native)) };
            let existing = key.and_then(|key| self.by_path.get(key).copied());
            let key = key.map(str::to_owned);
            match existing {
                Some((id, true)) if !opts.local => {
                    let source = key.as_deref().unwrap_or(&f);
                    let references = native_path(&f)
                        .and_then(|key| self.native_references.as_ref().and_then(|groups| groups.get(&key)).cloned())
                        .unwrap_or_else(|| self.references.iter().filter(|r| r.path == source).cloned().collect());
                    if !queued.insert(id) {
                        out.items.push(PreparedItem::Duplicate { path: f, existing: Some(id), reason: "path", hash: None });
                        self.done.fetch_add(1, Relaxed);
                        continue;
                    }
                    queued.extend(references.iter().map(|r| r.id));
                    let promotion = LocalPromotion {
                        id,
                        references,
                        reviewed_paths: self.reviewed_paths.clone(),
                        library: self.library_identity.clone(),
                        retained_sidecars: (mode == ImportMode::Move).then(|| self.retained_sidecars.clone()),
                        destination_stamp: None,
                    };
                    if mode == ImportMode::Add && native_path(&f).is_none() {
                        // Custom virtual sources have no native file facts to revalidate.
                        out.items.push(PreparedItem::Promote { promotion, info: None });
                        self.done.fetch_add(1, Relaxed);
                    } else {
                        promotions.insert(f.clone(), promotion);
                        todo.push(f);
                    }
                }
                Some((id, _)) => {
                    out.items.push(PreparedItem::Duplicate { path: f, existing: Some(id), reason: "path", hash: None });
                    self.done.fetch_add(1, Relaxed);
                }
                None => todo.push(f),
            }
        }

        // Unchanged files reuse their probes. Freshness checks and shared-cache consumption
        // happen on this worker, leaving an in-flight scan's snapshot intact.
        let cached: Vec<Option<Result<ProbeInfo, String>>> = todo
            .iter()
            .map(|f| {
                if cancel.load(Relaxed) {
                    return Some(Err("cancelled".into()));
                }
                let info = self.cache.remove(f).or_else(|| self.shared_cache.as_mut().and_then(|cache| std::sync::Arc::make_mut(cache).remove(f)))?;
                if info.cache_is_current(f) {
                    Some(Ok(info))
                } else if mode == ImportMode::Copy {
                    // Copy has always refused bytes changed since review, rather than copying
                    // a different photo. The consumed probe lets a deliberate retry probe afresh.
                    Some(Err("source changed meanwhile; review or retry the import".into()))
                } else {
                    None
                }
            })
            .collect();
        let missing: Vec<String> = todo.iter().zip(&cached).filter(|(_, c)| c.is_none()).map(|(f, _)| f.clone()).collect();
        let progress = ScanProgress::default();
        let mut fresh = probe_all(self.probe.as_ref(), &missing, &progress, cancel).into_iter();
        let probed: Vec<Result<ProbeInfo, String>> =
            cached.into_iter().map(|c| c.unwrap_or_else(|| fresh.next().unwrap_or_else(|| Err("not probed".into())))).collect();
        crate::memory::release();
        // Files are planned one by one, in order (duplicates, sidecars, names, folders); their
        // copies or moves — the slow part, reading and writing every byte and verifying it — are
        // collected and run on several threads (issue #367), then their outcomes take their places.
        let mut plan = Plan::default();
        for (path, info) in todo.into_iter().zip(probed) {
            if cancel.load(Relaxed) {
                break;
            }
            if opts.local
                && let Some(source) = native_match(&path, &self.pending_local).map(str::to_owned)
            {
                self.done.fetch_add(1, Relaxed);
                plan.slots.push(Slot::Item(PreparedItem::LocalAlias { path, source }));
                continue;
            }
            let mut info = match info {
                Ok(i) => i,
                Err(e) => {
                    log::warn!("import {path}: {e}");
                    self.done.fetch_add(1, Relaxed);
                    plan.slots.push(Slot::Item(PreparedItem::Failed(path, e)));
                    continue;
                }
            };
            let promotion = promotions.remove(&path);
            if let Some(promotion) = &promotion
                && promotion.references.iter().any(|reference| !reference.matches_probe(&info))
            {
                self.done.fetch_add(1, Relaxed);
                plan.slots.push(Slot::Item(PreparedItem::Failed(path, changed_local_source())));
                continue;
            }
            if promotion.is_some()
                && mode == ImportMode::Copy
                && opts.convert_dng
                && info.kind == MediaKind::Raw
                && !info.format.eq_ignore_ascii_case("DNG")
            {
                self.done.fetch_add(1, Relaxed);
                plan.slots.push(Slot::Item(PreparedItem::Failed(
                    path,
                    "Convert the Local photo to DNG before importing it with Copy as DNG; its current file and edits are unchanged.".into(),
                )));
                continue;
            }
            if mode == ImportMode::Add
                && let Some(promotion) = promotion.clone()
            {
                self.done.fetch_add(1, Relaxed);
                plan.slots.push(Slot::Item(PreparedItem::Promote { promotion, info: Some(info) }));
                continue;
            }
            // a copy of a file whose transfer is still pending: a duplicate only if that one succeeds
            if !opts.local
                && let Some(h) = &info.content_hash
                && plan.transfers.iter().any(|t| t.info.content_hash.as_ref() == Some(h))
            {
                self.run_plan(&mut plan, &mut out, cancel);
                if cancel.load(Relaxed) {
                    break;
                }
            }
            if let Some(h) = &info.content_hash
                && !opts.local
                && promotion.is_none()
                && let Some(existing) = self.by_hash.get(h)
            {
                self.done.fetch_add(1, Relaxed);
                plan.slots.push(Slot::Item(PreparedItem::Duplicate { path, existing: *existing, reason: "content", hash: Some(h.clone()) }));
                continue;
            }
            // the XMP sidecar (or a raw's embedded XMP), read before copying: its capture time files
            // and names the copy when the file itself has none
            let raw = info.kind == MediaKind::Raw;
            let packet = crate::sidecar::find_sidecar(&path, self.naming)
                .and_then(|f| std::fs::read_to_string(f).ok())
                .or_else(|| info.xmp.clone().filter(|_| raw));
            let sidecar = packet.and_then(|x| match crate::sidecar::parse_sidecar(&x, raw) {
                Ok(sc) => Some(sc),
                Err(e) => {
                    log::warn!("import {path}: XMP: {e}");
                    None
                }
            });
            if info.captured.is_none() {
                info.captured = sidecar.as_ref().and_then(|sc| sc.captured.clone());
            }
            let stored = match (mode, &self.copy_root) {
                (ImportMode::Copy | ImportMode::Move, Some(root))
                    if !crate::import_move::inside(Path::new(&path), root)
                        && !self.lib_dir.as_ref().is_some_and(|l| crate::import_move::inside(Path::new(&path), l)) =>
                {
                    // the templates see the photo as it will be catalogued (undated: the import time)
                    let own = Path::new(&path).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    let mut q = Photo::new(PhotoId(0), Source::File { path: path.clone() }, &own, &info.format, 0, 0, &self.now);
                    q.captured = info.captured.clone();
                    q.meta = info.meta.clone();
                    let name = opts.rename.as_deref().filter(|t| !t.trim().is_empty()).map(|t| {
                        let n = crate::rename::expand(t, &q, self.seq);
                        self.seq += 1;
                        n
                    });
                    let folders = opts.organize.folders(&q);
                    let dir = folders.iter().fold(root.to_path_buf(), |d, f| d.join(f));
                    // the name this file gets depends on what a pending transfer of a like-named file
                    // into the same folder ends up with: that one goes first
                    let as_named = name.as_deref().unwrap_or(if own.is_empty() { "photo" } else { own.as_str() });
                    let family = (dir.to_string_lossy().to_lowercase(), name_family(as_named));
                    if plan.transfers.iter().any(|t| t.family == family) {
                        self.run_plan(&mut plan, &mut out, cancel);
                        if cancel.load(Relaxed) {
                            break;
                        }
                    }
                    let how = if mode == ImportMode::Move && !crate::import_move::is_symlink(Path::new(&path)) {
                        How::Place { name: name.unwrap_or(own) }
                    } else {
                        if mode == ImportMode::Move {
                            let reason = "a symbolic link: the file it points to was copied, the link and its target stay";
                            plan.slots.push(Slot::Item(PreparedItem::Kept(Kept { path: path.clone(), reason: reason.into() })));
                        }
                        How::Copy { name }
                    };
                    // Copy as DNG: the copied raw becomes a DNG (the copy is ours to replace)
                    let dng = promotion.is_none() && opts.convert_dng && mode == ImportMode::Copy && raw && !info.format.eq_ignore_ascii_case("DNG");
                    plan.slots.push(Slot::Transfer);
                    plan.transfers.push(Transfer { path, info, sidecar, dir, how, dng, family, promotion });
                    continue;
                }
                (ImportMode::Move, _) => {
                    let reason = "already inside the destination or the library: added where it is";
                    plan.slots.push(Slot::Item(PreparedItem::Kept(Kept { path: path.clone(), reason: reason.into() })));
                    path.clone()
                }
                _ => path.clone(),
            };
            if let Some(h) = &info.content_hash {
                self.by_hash.insert(h.clone(), None);
            }
            self.done.fetch_add(1, Relaxed);
            if opts.local
                && let Some(key) = native_path(&path)
            {
                self.pending_local.insert(key, stored.clone());
            }
            if let Some(promotion) = promotion {
                plan.slots.push(Slot::Item(PreparedItem::Promote { promotion, info: Some(info) }));
            } else {
                plan.slots.push(Slot::Item(PreparedItem::Ready(Box::new(ReadyFile { path, stored, info, sidecar, placed: None, promotion: None }))));
            }
        }
        self.run_plan(&mut plan, &mut out, cancel);
        out
    }

    /// Run the transfers `plan` collected (on several threads) and append its items to `out` in
    /// file order. Transfers not started because `cancel` was set leave no item.
    fn run_plan(&mut self, plan: &mut Plan, out: &mut Prepared, cancel: &std::sync::atomic::AtomicBool) {
        let mut done = run_transfers(std::mem::take(&mut plan.transfers), self.file_bytes.as_ref(), cancel, &self.done).into_iter();
        for slot in std::mem::take(&mut plan.slots) {
            let item = match slot {
                Slot::Item(it) => it,
                Slot::Transfer => match done.next().flatten() {
                    Some(it) => it,
                    None => continue,
                },
            };
            let promoted = match &item {
                PreparedItem::Ready(r) => r.promotion.as_ref().map(|p| (p, Some(&r.info))),
                PreparedItem::Promote { promotion, info } => Some((promotion, info.as_ref())),
                _ => None,
            };
            if let Some((promotion, info)) = promoted {
                for reference in &promotion.references {
                    if let Some(entry) = self.by_path.get_mut(&reference.path) {
                        entry.1 = false;
                    }
                    if let (Some(info), Some(key)) = (info, native_path(&reference.path)) {
                        self.promoted_native.insert(key, (promotion.id, info.clone()));
                    }
                }
            }
            if let PreparedItem::Ready(r) = &item
                && let Some(h) = &r.info.content_hash
            {
                self.by_hash.insert(h.clone(), None);
            }
            out.items.push(item);
        }
    }
}

/// A batch's items in file order while its transfers are pending ([`ImportJob::run_plan`]).
#[derive(Default)]
struct Plan {
    slots: Vec<Slot>,
    transfers: Vec<Transfer>,
}

enum Slot {
    Item(PreparedItem),
    /// The outcome of the next of [`Plan::transfers`].
    Transfer,
}

/// A file to copy or move into `dir`, planned in order and run on any thread ([`run_transfer`]).
struct Transfer {
    path: String,
    info: ProbeInfo,
    sidecar: Option<crate::sidecar::SidecarData>,
    dir: PathBuf,
    how: How,
    /// Copy as DNG.
    dng: bool,
    /// The folder (lowercase) and [`name_family`]: transfers of one family can take each other's
    /// names, so they never run at the same time.
    family: (String, String),
    promotion: Option<LocalPromotion>,
}

enum How {
    /// A verified copy, under `name` (default: the file's own).
    Copy { name: Option<String> },
    /// Move: placed (linked or copied) without touching the source.
    Place { name: String },
}

/// The names a file called `name` could end up with, as one key: the transfers number a taken name
/// `IMG-1.JPG`, `IMG-2.JPG`…, and Copy as DNG names its DNG `IMG.dng`, `IMG-2.dng`…, so `IMG.JPG`,
/// `IMG-1.jpg` and `img.dng` are all `img` (case-insensitive: so are some file systems).
fn name_family(name: &str) -> String {
    let mut stem = match name.rsplit_once('.') {
        Some((s, _)) if !s.is_empty() => s,
        _ => name,
    };
    while let Some((head, tail)) = stem.rsplit_once('-')
        && !head.is_empty()
        && !tail.is_empty()
        && tail.bytes().all(|b| b.is_ascii_digit())
    {
        stem = head;
    }
    stem.to_lowercase()
}

/// Threads for `jobs` file reads (probes, copies) at once.
#[cfg(not(target_arch = "wasm32"))]
fn workers(jobs: usize) -> usize {
    std::thread::available_parallelism().map(|n| n.get()).unwrap_or(4).clamp(1, 8).min(jobs.max(1))
}

/// Files per batch for a caller that prepares an import in batches ([`ImportJob::prepare_files`]):
/// enough to keep every probe and copy thread busy (a batch waits for its slowest file), few
/// enough that each batch soon joins the catalog.
pub fn batch_size() -> usize {
    #[cfg(not(target_arch = "wasm32"))]
    {
        workers(usize::MAX).saturating_mul(2).clamp(8, 16)
    }
    #[cfg(target_arch = "wasm32")]
    {
        8
    }
}

/// Run `transfers` (several at once where threads exist); their outcomes in order, `None` for one
/// not started because `cancel` was set. `done` counts each one finished.
fn run_transfers(
    transfers: Vec<Transfer>,
    file_bytes: Option<&crate::merge::ByteReader>,
    cancel: &std::sync::atomic::AtomicBool,
    done: &std::sync::atomic::AtomicUsize,
) -> Vec<Option<PreparedItem>> {
    use std::sync::atomic::Ordering::Relaxed;
    let one = |t: Transfer| {
        if cancel.load(Relaxed) {
            return None;
        }
        let path = t.path.clone();
        // last-resort guard: a panic is this file's failure, not the import's
        let item = crate::guard::catch("import", || run_transfer(t, file_bytes)).unwrap_or_else(|e| PreparedItem::Failed(path, e));
        done.fetch_add(1, Relaxed);
        Some(item)
    };
    #[cfg(not(target_arch = "wasm32"))]
    {
        let n = workers(transfers.len());
        if n > 1 {
            let mut results: Vec<Option<PreparedItem>> = std::iter::repeat_with(|| None).take(transfers.len()).collect();
            let queue = std::sync::Mutex::new(transfers.into_iter().enumerate());
            let out = std::sync::Mutex::new(&mut results);
            // (the tests' injected faults hold for the workers too)
            #[cfg(test)]
            let fault = crate::import_move::injected_fault();
            let work = &|| {
                #[cfg(test)]
                crate::import_move::inject(fault);
                loop {
                    let next = queue.lock().unwrap_or_else(std::sync::PoisonError::into_inner).next();
                    let Some((i, t)) = next else { break };
                    let r = one(t);
                    if let Some(slot) = out.lock().unwrap_or_else(std::sync::PoisonError::into_inner).get_mut(i) {
                        *slot = r;
                    }
                }
            };
            std::thread::scope(|sc| {
                let handles: Vec<_> =
                    (0..n).filter_map(|i| std::thread::Builder::new().name(format!("lc-import-copy-{i}")).spawn_scoped(sc, work).ok()).collect();
                if handles.is_empty() {
                    // no thread to be had: copy here, one after another
                    work();
                }
                for h in handles {
                    if h.join().is_err() {
                        log::error!("import: a copy worker stopped unexpectedly");
                    }
                }
            });
            return results;
        }
    }
    transfers.into_iter().map(one).collect()
}

/// Copy or move one planned file (and Copy as DNG); its item.
fn run_transfer(t: Transfer, file_bytes: Option<&crate::merge::ByteReader>) -> PreparedItem {
    let Transfer { path, mut info, sidecar, dir, how, dng, mut promotion, .. } = t;
    if promotion.is_some() && !info.cache_is_current(&path) {
        return PreparedItem::Failed(path, changed_local_source());
    }
    let moving = matches!(&how, How::Place { .. });
    let mut placed = None;
    let mut stored = match how {
        How::Place { name } => match crate::import_move::place(Path::new(&path), &dir, &name) {
            Ok(pl) => {
                let dst = pl.dst.to_string_lossy().to_string();
                placed = Some(pl);
                dst
            }
            Err(e) => return PreparedItem::Failed(path, e),
        },
        How::Copy { name } => match copy_into(&dir, &path, name.as_deref(), info.content_hash.as_deref()) {
            Ok(p) => p,
            Err(e) => return PreparedItem::Failed(path, e),
        },
    };
    if promotion.is_some() && (!info.cache_is_current(&path) || moving && verify_promoted_bytes(&stored, &info).is_err()) {
        if let Some(placed) = &placed {
            crate::import_move::rollback(placed);
        } else {
            let _ = std::fs::remove_file(&stored);
        }
        return PreparedItem::Failed(path, changed_local_source());
    }
    if moving && let Some(promotion) = &mut promotion {
        match std::fs::metadata(&stored) {
            Ok(metadata) => promotion.destination_stamp = Some(crate::media::FileStamp::from_metadata(&metadata)),
            Err(error) => {
                if let Some(placed) = &placed {
                    crate::import_move::rollback(placed);
                }
                return PreparedItem::Failed(path, format!("could not validate the placed Local destination ({error}); the original stays"));
            }
        }
    }
    if dng {
        // decoding the raw twice and encoding it takes several times the file's size in memory
        let weight = usize::try_from(info.file_size).unwrap_or(usize::MAX).saturating_mul(6);
        let _permit = crate::memory::work_gate().acquire(weight);
        match crate::cmd::convert::write_dng_with(file_bytes, &stored, String::new()) {
            Ok(dng) => {
                let _ = std::fs::remove_file(&stored);
                stored = dng;
                info.format = "DNG".into();
            }
            Err(e) => log::warn!("import {path}: copy as DNG: {e}"),
        }
    }
    PreparedItem::Ready(Box::new(ReadyFile { path, stored, info, sidecar, placed, promotion }))
}

/// Move's byte comparison must also agree with the facts validated for this Local record.
/// Stream the destination on the filesystem worker rather than allocate another original.
fn verify_promoted_bytes(path: &str, info: &ProbeInfo) -> std::result::Result<(), String> {
    use std::io::Read;
    let Some(expected) = info.content_hash.as_deref().and_then(lightcraft_preview::Hash128::parse) else {
        return if info.file_stamp.is_some() { Err(changed_local_source()) } else { Ok(()) };
    };
    let mut file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hash = lightcraft_preview::Hasher128::new();
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let read = file.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        hash.update(buffer.get(..read).ok_or_else(changed_local_source)?);
    }
    if hash.finish() == expected { Ok(()) } else { Err(changed_local_source()) }
}

fn retained_sidecars(s: &Session) -> Vec<(PathBuf, PathBuf)> {
    s.pending_sidecar_repairs
        .iter()
        .chain(s.undo.iter().chain(&s.redo).flat_map(|entry| &entry.retry_sidecars))
        .map(|repair| (repair.from.clone(), repair.to.clone()))
        .collect()
}

/// Owner-side source fence. An ordinary rating/develop edit is allowed; changed references,
/// bytes/headers, removed records, a replacement library or new retained Move work are not.
fn promotion_references(
    s: &Session,
    promotion: &LocalPromotion,
    info: Option<&ProbeInfo>,
    by_source: &HashMap<String, Vec<LocalReference>>,
    unreviewed_source_hashes: &std::collections::HashSet<String>,
) -> std::result::Result<Vec<LocalReference>, String> {
    if !std::sync::Arc::ptr_eq(&promotion.library, &s.library_identity)
        || !s.catalog.photo(promotion.id).is_some_and(|p| p.local && !p.deleted)
        || promotion.references.is_empty()
        || promotion.references.iter().any(|r| s.catalog.photo(r.id).is_none_or(|p| !r.matches_owner(p)))
        || promotion.retained_sidecars.as_ref().is_some_and(|captured| *captured != retained_sidecars(s))
        || promotion.retained_sidecars.is_some()
            && info.and_then(|info| info.content_hash.as_ref()).is_some_and(|hash| unreviewed_source_hashes.contains(hash))
    {
        return Err("Local source ownership or retained XMP repair work changed; review the import again. The original stays.".into());
    }
    // Follow late virtual copies using one catalog/source lookup per batch. No native I/O,
    // no copying back stale edits, and no full catalog walk per promoted source.
    let paths: std::collections::HashSet<_> = promotion.references.iter().map(|r| r.path.as_str()).collect();
    let references: Vec<_> = paths.into_iter().filter_map(|path| by_source.get(path)).flatten().cloned().collect();
    if info.is_some_and(|info| references.iter().any(|r| !r.matches_probe(info))) {
        return Err(changed_local_source());
    }
    Ok(references)
}

fn append_promotion_ops(s: &Session, references: &[LocalReference], stored: Option<&str>, ops: &mut Vec<Op>, report: &mut ImportReport) {
    for reference in references {
        if let Some(stored) = stored
            && stored != reference.path
        {
            let file_name = Path::new(stored).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| stored.into());
            // Import Undo never moves files back onto a card. Move inverses below retain this
            // destination too, so retained records cannot point at a deleted old source.
            ops.push(Op::Relink { id: reference.id, file_name, source: Source::File { path: stored.into() }, format: None });
        }
        if s.catalog.photo(reference.id).is_some_and(|p| p.local && !p.deleted) {
            ops.push(Op::SetLocal { id: reference.id, local: false });
            if !report.imported.contains(&reference.id.0) {
                report.imported.push(reference.id.0);
            }
        }
    }
}

fn keep_move_destinations(op: &mut Op, destinations: &HashMap<PhotoId, String>) {
    let mut pending = vec![op];
    while let Some(op) = pending.pop() {
        match op {
            Op::Relink { id, file_name, source, .. } => {
                if let Some(path) = destinations.get(id) {
                    *file_name = Path::new(path).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| path.clone());
                    *source = Source::File { path: path.clone() };
                }
            }
            Op::Batch { ops } => pending.extend(ops.iter_mut()),
            _ => {}
        }
    }
}

/// The catalog half of an import: add the photos [`ImportJob::prepare`] readied (one undoable op;
/// a Move's sources are removed once it is saved). On a failed commit a Move's placed files are
/// taken back.
pub fn commit_prepared(s: &mut Session, opts: &ImportOptions, now: &str, prepared: Prepared) -> crate::Result<ImportReport> {
    let now = now.to_string();
    let mut report = ImportReport { scanned: prepared.scanned, ..Default::default() };
    let mut ops = Vec::new();
    // Move: placed at the destination, sources untouched until the records are committed
    let mut placed: Vec<crate::import_move::Placed> = Vec::new();
    let mut move_guards = HashMap::new();
    let mut move_destinations = HashMap::new();
    let mut promotion_copies = Vec::new();
    let promotion_paths: std::collections::HashSet<&str> = prepared
        .items
        .iter()
        .filter_map(|item| match item {
            PreparedItem::Promote { promotion, .. } => Some(promotion),
            PreparedItem::Ready(ready) => ready.promotion.as_ref(),
            _ => None,
        })
        .flat_map(|promotion| promotion.references.iter().map(|r| r.path.as_str()))
        .collect();
    // All promotions in a prepared batch come from the same worker snapshot. New source
    // spellings with the moved content require worker identity checks before deleting it.
    // Compare in memory once per batch; ordinary edits and captured-path virtual copies stay valid.
    let reviewed_paths = prepared.items.iter().find_map(|item| match item {
        PreparedItem::Promote { promotion, .. } => Some(&promotion.reviewed_paths),
        PreparedItem::Ready(ready) => ready.promotion.as_ref().map(|p| &p.reviewed_paths),
        _ => None,
    });
    let mut unreviewed_source_hashes = std::collections::HashSet::new();
    let mut by_source: HashMap<String, Vec<LocalReference>> = HashMap::new();
    if !promotion_paths.is_empty() {
        for photo in s.catalog.photos() {
            if let Source::File { path } = &photo.source {
                if promotion_paths.contains(path.as_str()) {
                    by_source.entry(path.clone()).or_default().push(LocalReference::of(photo, path));
                }
                if reviewed_paths.is_some_and(|known| !known.contains(path))
                    && let Some(hash) = photo.content_hash.as_deref()
                {
                    unreviewed_source_hashes.insert(hash.split(':').next().unwrap_or(hash).to_owned());
                }
            }
        }
    }
    drop(promotion_paths);
    // photos added by this batch, by content (a duplicate of a file earlier in the import names it)
    let mut new_hash: HashMap<String, PhotoId> = HashMap::new();
    let mut new_paths: HashMap<String, PhotoId> = HashMap::new();
    // Fresh: trashed photos a new import replaces (by content or by file), each removed once
    let (mut trash_hash, mut trash_path): (HashMap<String, PhotoId>, HashMap<String, PhotoId>) = Default::default();
    if opts.on_deleted == OnDeleted::Fresh {
        for p in s.catalog.photos().filter(|p| p.deleted) {
            if let Some(h) = &p.content_hash {
                trash_hash.insert(h.clone(), p.id);
            }
            if let Source::File { path } = &p.source {
                trash_path.insert(path.clone(), p.id);
            }
        }
    }
    let mut purge: Vec<PhotoId> = Vec::new();
    let mut purged: std::collections::HashSet<PhotoId> = Default::default();
    for it in prepared.items {
        match it {
            PreparedItem::Promote { promotion, info } => {
                match promotion_references(s, &promotion, info.as_ref(), &by_source, &unreviewed_source_hashes) {
                    Ok(references) => append_promotion_ops(s, &references, None, &mut ops, &mut report),
                    Err(error) => report.failed.push((promotion.references.first().map(|r| r.path.clone()).unwrap_or_default(), error)),
                }
            }
            PreparedItem::LocalAlias { path, source } => {
                let existing = new_paths
                    .get(&source)
                    .copied()
                    .or_else(|| s.catalog.photos().find_map(|p| matches!(&p.source, Source::File { path } if path == &source).then_some(p.id)));
                if let Some(id) = existing {
                    report.duplicates.push(Duplicate { path, existing: Some(id.0), reason: "path", existing_deleted: false });
                } else {
                    report.failed.push((path, "the earlier Local file was not committed; browse again".into()));
                }
            }
            PreparedItem::Duplicate { path, existing, reason, hash } => {
                let existing = existing.or_else(|| {
                    let h = hash?;
                    new_hash.get(&h).copied().or_else(|| s.catalog.photos().find(|p| p.content_hash.as_deref() == Some(h.as_str())).map(|p| p.id))
                });
                let trashed = existing.filter(|id| s.catalog.photo(*id).is_some_and(|p| p.deleted));
                match trashed {
                    Some(id) if opts.on_deleted == OnDeleted::Restore => {
                        if !report.restored.contains(&id.0) {
                            ops.push(Op::SetDeleted { id, deleted: false });
                            report.restored.push(id.0);
                        }
                    }
                    _ => report.duplicates.push(Duplicate { path, existing: existing.map(|i| i.0), reason, existing_deleted: trashed.is_some() }),
                }
            }
            PreparedItem::Failed(path, e) => report.failed.push((path, e)),
            PreparedItem::Kept(k) => report.kept.push(k),
            PreparedItem::Ready(r) => {
                let ReadyFile { path, stored, info, sidecar, placed: pl, promotion } = *r;
                if let Some(promotion) = promotion {
                    let references = match promotion_references(s, &promotion, Some(&info), &by_source, &unreviewed_source_hashes) {
                        Ok(references) => references,
                        Err(error) => {
                            if let Some(placed) = &pl {
                                crate::import_move::rollback(placed);
                            } else {
                                let _ = std::fs::remove_file(&stored);
                            }
                            report.failed.push((path, error));
                            continue;
                        }
                    };
                    append_promotion_ops(s, &references, Some(&stored), &mut ops, &mut report);
                    if let Some(pl) = &pl {
                        move_guards.insert(pl.src.clone(), (info.clone(), promotion.destination_stamp.clone()));
                        move_destinations.extend(references.iter().map(|r| (r.id, stored.clone())));
                    } else {
                        promotion_copies.push(stored.clone());
                    }
                    placed.extend(pl);
                    continue;
                }
                placed.extend(pl);
                let id = s.catalog.alloc_photo_id();
                new_paths.insert(stored.clone(), id);
                if let Some(h) = &info.content_hash {
                    new_hash.insert(h.clone(), id);
                }
                for old in info.content_hash.as_ref().and_then(|h| trash_hash.get(h)).into_iter().chain(trash_path.get(&stored)) {
                    if purged.insert(*old) {
                        purge.push(*old);
                    }
                }
                let sidecar = sidecar.map(|sc| sc.resolve_label(&s.catalog)).filter(|sc| *sc != crate::sidecar::SidecarData::default());
                // a copy is catalogued under its new name
                let name = Path::new(&stored).file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_else(|| path.clone());
                let mut p = Photo::new(id, Source::File { path: stored }, &name, &info.format, info.width, info.height, &now);
                p.kind = info.kind;
                p.file_size = info.file_size;
                p.captured = info.captured;
                p.meta = info.meta;
                p.as_shot_wb = info.as_shot_wb;
                p.content_hash = info.content_hash;
                p.embedded_lens = info.embedded_lens;
                p.preview_only = info.preview_only.clone();
                apply_import_defaults(s, &mut p);
                if let Some(sc) = &sidecar {
                    crate::sidecar::merge_into(&mut p, sc, &now);
                    report.sidecars += 1;
                }
                if let Some(mp) = opts.metadata_preset.as_ref().and_then(|n| s.metadata_presets.iter().find(|m| m.name.eq_ignore_ascii_case(n))) {
                    crate::cmd::metadata::apply_to(&mut p.meta, &mp.fields);
                }
                for k in &opts.keywords {
                    let k = lightcraft_catalog::keywords::clean(k);
                    if !k.is_empty() && !p.meta.keywords.iter().any(|x| x.eq_ignore_ascii_case(&k)) {
                        p.meta.keywords.push(k);
                    }
                }
                if let Some(preset) = &opts.preset {
                    let d = std::sync::Arc::new(preset.apply(&p.develop, 1.0));
                    let label = format!("Preset: {}", preset.name);
                    p.develop = d.clone();
                    p.edited = Some(now.clone());
                    p.history.push(lightcraft_catalog::HistoryStep { label, settings: d });
                }
                report.imported.push(id.0);
                p.local = opts.local;
                if opts.local {
                    // the state as browsed: a later change keeps the record from being forgotten
                    p.set_local_baseline();
                }
                ops.push(Op::AddPhoto { photo: Box::new(p) });
            }
        }
    }
    // (built against the catalog as it is: one op list for all of them, ahead of the additions)
    if !purge.is_empty() {
        ops.insert(0, s.catalog.delete_photos_permanently_ops(&purge));
    }
    let log0 = s.pending_log.len();
    let (verb, n) = if report.imported.is_empty() && !report.restored.is_empty() {
        ("Restore", report.restored.len())
    } else {
        ("Add", report.imported.len().max(1))
    };
    if !ops.is_empty()
        && let Err(e) = s.commit(&format!("{verb} {n} Photo{}", if n == 1 { "" } else { "s" }), Op::Batch { ops })
    {
        // nothing was catalogued: take the moves back (their sources were never touched)
        placed.iter().for_each(crate::import_move::rollback);
        for path in promotion_copies {
            let _ = std::fs::remove_file(path);
        }
        return Err(e);
    }
    if !move_destinations.is_empty()
        && let Some(entry) = s.undo.last_mut()
    {
        keep_move_destinations(&mut entry.op, &move_destinations);
    }
    if !placed.is_empty() {
        finish_moves(s, placed, &move_guards, log0, &mut report);
    }
    Ok(report)
}

/// Move, after the commit: save the library, then remove each source whose destination is
/// still intact. Nothing is removed when the library can't be saved.
fn finish_moves(
    s: &mut Session,
    placed: Vec<crate::import_move::Placed>,
    guards: &HashMap<PathBuf, (ProbeInfo, Option<crate::media::FileStamp>)>,
    log0: usize,
    report: &mut ImportReport,
) {
    // (saving here clears the pending ops, so the sidecars `execute` would auto-write are written here)
    let auto: Vec<Op> = if s.xmp.auto_write { s.pending_log.get(log0..).map(<[Op]>::to_vec).unwrap_or_default() } else { Vec::new() };
    let before = s.pending_log.len();
    if let Err(e) = s.persist() {
        for p in placed {
            let reason = format!("the library could not be saved ({e}): the original stays; the photo uses the copy at {}", p.dst.display());
            report.kept.push(Kept { path: p.src.to_string_lossy().to_string(), reason });
        }
        return;
    }
    for p in placed {
        let from = p.src.to_string_lossy().to_string();
        // The worker carried its validated stamp into the persisted move. Do not unlink an
        // atomic replacement that arrived while the owner was saving the catalog.
        if guards.get(&p.src).is_some_and(|(info, destination)| {
            !info.cache_is_current(&from)
                || destination.as_ref().is_none_or(|stamp| {
                    std::fs::metadata(&p.dst).map_or(true, |metadata| *stamp != crate::media::FileStamp::from_metadata(&metadata))
                })
        }) {
            report.kept.push(Kept {
                path: from,
                reason: "the original or destination changed after preparation: the original stays; check the library destination before retrying"
                    .into(),
            });
            continue;
        }
        match crate::import_move::finish(&p) {
            Ok(kept) => {
                let sidecars = p.sidecars.iter().map(|sc| sc.dst.to_string_lossy().to_string()).collect();
                report.moved.push(Moved { from, to: p.dst.to_string_lossy().to_string(), sidecars });
                report.kept.extend(kept.into_iter().map(|(path, reason)| Kept { path, reason }));
            }
            Err(reason) => {
                log::warn!("import: move {from}: {reason}");
                report.kept.push(Kept { path: from, reason });
            }
        }
    }
    if s.pending_log.len() < before && s.interaction.is_none() && !auto.is_empty() {
        s.auto_write_sidecars(&auto);
    }
}

/// The current local time as ISO 8601 (`YYYY-MM-DDTHH:MM:SS`, UTC on targets without a clock
/// offset); for [`Session::clock`] on native hosts.
pub fn system_clock() -> String {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let secs = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs() as i64).unwrap_or(0);
        civil(secs)
    }
    #[cfg(target_arch = "wasm32")]
    {
        "2026-01-01T00:00:00".to_string()
    }
}

/// Unix seconds → `YYYY-MM-DDTHH:MM:SS` (UTC), proleptic Gregorian.
pub fn civil(secs: i64) -> String {
    lightcraft_catalog::dates::civil(secs)
}

impl Session {
    /// A thumbnail for an import candidate (not in the catalog): a raw's embedded preview, else a
    /// small render of the file. `slot` makes the job's photo id unique (`u64::MAX - slot`).
    pub fn candidate_thumb_job(&mut self, c: &ImportCandidate, edge: usize, slot: u64) -> Option<crate::media::QuickJob> {
        if c.error.is_some() {
            return None;
        }
        let id = PhotoId(u64::MAX - slot);
        let mut p = Photo::new(id, Source::File { path: c.path.clone() }, &c.name, &c.format, c.width.max(1), c.height.max(1), "");
        p.kind = c.kind;
        if let Some(info) = self.cached_import_probe(&c.path) {
            p.as_shot_wb = info.as_shot_wb;
            p.embedded_lens = info.embedded_lens;
            p.preview_only = info.preview_only.clone();
        }
        p.develop = std::sync::Arc::new(p.import_defaults());
        let edge = edge.clamp(64, crate::media::SourceLevel::Thumb.max_edge());
        let level = crate::media::SourceLevel::Thumb;
        let source = self.media.origin_ref(&p.source, level.max_edge());
        let key = lightcraft_preview::Hasher128::new().str(&c.path).u64(c.file_size).u64(edge as u64).finish().0 as u64;
        let small = crate::media::RenderJob {
            request_id: 0,
            cache_generation: self.media.rendered.generation(),
            source_key: None,
            photo: id,
            level,
            source,
            origin: p.source.clone(),
            info: crate::media::source_info(&p),
            settings: p.develop.clone(),
            request: lightcraft_pipeline::RenderRequest::fit(edge, edge),
            key,
            cache: None,
            stages: None,
            view_cache: None,
        };
        let embedded = match (&self.media.preview_loader, c.kind) {
            (Some(l), MediaKind::Raw) => Some((c.path.clone(), l.clone(), edge)),
            _ => None,
        };
        Some(crate::media::QuickJob { request_id: 0, source_key: None, photo: id, key, cached: Vec::new(), embedded, small: Some(Box::new(small)) })
    }

    /// Use the system clock for import/edit times (native hosts).
    pub fn with_system_clock(mut self) -> Self {
        self.clock = Box::new(system_clock);
        self
    }
}

#[cfg(test)]
mod prepared_tests {
    use super::*;

    #[test]
    fn revalidate_add_only_converts_matching_ready_item() {
        let mut s = Session::new();
        let existing_path = std::env::temp_dir().join("lightcraft-revalidate-existing.jpg");
        let other_path = std::env::temp_dir().join("lightcraft-revalidate-other.jpg");
        let existing = Photo::new(PhotoId(1), Source::File { path: existing_path.to_string_lossy().into() }, "existing.jpg", "JPG", 1, 1, "now");
        s.commit("existing", Op::AddPhoto { photo: Box::new(existing) }).unwrap();
        let ready = |path: &Path| ReadyFile {
            path: path.to_string_lossy().into(),
            stored: path.to_string_lossy().into(),
            info: ProbeInfo { format: "JPG".into(), ..ProbeInfo::default() },
            sidecar: None,
            placed: None,
            promotion: None,
        };
        let mut prepared = Prepared {
            scanned: 2,
            items: vec![PreparedItem::Ready(Box::new(ready(&existing_path))), PreparedItem::Ready(Box::new(ready(&other_path)))],
        };
        prepared.revalidate_add(&s);
        assert!(matches!(prepared.items.first(), Some(PreparedItem::Duplicate { reason: "path", .. })));
        assert!(matches!(prepared.items.get(1), Some(PreparedItem::Ready(_))));
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests_run1_fixes;

#[cfg(all(test, target_os = "linux"))]
mod tests_run2_fixes;
