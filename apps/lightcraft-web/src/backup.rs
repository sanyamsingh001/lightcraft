//! Library backups for the browser build: a plain zip (entries stored, not compressed — photos
//! don't shrink) of the library files and every stored original, built one entry at a time so
//! only one file is in wasm memory at once, and read back the same way for a restore.
//!
//! Layout of a backup:
//! - `library/catalog.snap`, `library/catalog.log`, `library/presets.json`, … (see
//!   [`crate::files::LIBRARY_FILES`]);
//! - `originals/<content hash>/<file name>`: each imported photo's original bytes, under its own
//!   name so an unzipped backup is browsable;
//! - `README.txt`; `MISSING-ORIGINALS.txt` when originals are missing or unreadable.
//!
//! Restoring never deletes anything: the library files go to a new library folder in browser
//! storage, originals are checked against their content hashes before being added or reused,
//! corrupt stored originals are repaired from verified backup bytes, and only then does a small
//! pointer file ([`ACTIVE_LIBRARY`]) switch to the restored library. The prior library stays in storage.
//!
//! No zip64: a backup holds at most [`MAX_BYTES`] and 65 535 entries.

/// Storage key of the file naming the library folder in use (absent: `library`).
pub const ACTIVE_LIBRARY: &str = "active-library";

/// The default library folder.
pub const DEFAULT_LIBRARY_DIR: &str = "library";

/// Largest backup (offsets are 32-bit without zip64).
pub const MAX_BYTES: u64 = 0xFFFF_FFFF - (64 << 20);

const LOCAL_SIG: u32 = 0x0403_4b50;
const CENTRAL_SIG: u32 = 0x0201_4b50;
const END_SIG: u32 = 0x0605_4b50;
/// General-purpose flag bit 11: names are UTF-8.
const UTF8: u16 = 0x0800;
/// 1980-01-01 00:00 in MS-DOS format (date, time).
const DOS_DATE: u16 = (1 << 5) | 1;

/// The readme inside every backup.
pub const README: &str = "LightCraft library backup (browser version)\n\n\
library/      the catalog (catalog.snap + catalog.log), presets, view state and preferences\n\
originals/    available original photos, as originals/<content hash>/<file name>\n\n\
Restore it in LightCraft for the web with File > Restore Library from Backup...\n\
The photos in originals/ are the untouched files you imported; edits live in the catalog.\n\
If MISSING-ORIGINALS.txt is present, this is a PARTIAL backup: those photos cannot be restored.\n";

/// An original omitted from a downloaded backup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MissingOriginal {
    pub hash: String,
    pub name: String,
    pub reason: String,
}

/// Completion of a download; a useful partial backup must not promise full photo recovery.
#[derive(Clone, Debug, Default)]
pub struct BackupReport {
    pub originals: usize,
    pub bytes: f64,
    pub missing: Vec<MissingOriginal>,
}

impl BackupReport {
    pub fn missing_text(&self) -> String {
        let mut text = "PARTIAL LightCraft library backup\n\nCatalog and edits are included. These originals are unavailable:\n".to_string();
        for original in &self.missing {
            text.push_str(&format!("{} ({}): {}\n", original.name, original.hash, original.reason));
        }
        text
    }

    pub fn notice(&self) -> String {
        if self.missing.is_empty() {
            format!(
                "Library backed up: {} original{} ({:.1} MB). Keep the file somewhere safe.",
                self.originals,
                if self.originals == 1 { "" } else { "s" },
                self.bytes / 1e6
            )
        } else {
            format!(
                "PARTIAL library backup downloaded ({} originals, {:.1} MB). {} Keep the ZIP and your original photos; it cannot restore the missing photos.",
                self.originals,
                self.bytes / 1e6,
                self.missing_text()
            )
        }
    }
}

/// Archive all stored originals and reconcile the catalog's expected hashes with actual
/// entries. Missing, unreadable or misaddressed originals leave a useful partial backup with durable diagnostics.
pub async fn archive_originals<R, F, A>(
    names: &std::collections::HashMap<String, String>,
    stored: Vec<String>,
    mut read: R,
    mut add: A,
) -> Result<BackupReport, String>
where
    R: FnMut(String) -> F,
    F: std::future::Future<Output = Result<Option<Vec<u8>>, String>>,
    A: FnMut(&str, &[u8]) -> Result<(), String>,
{
    let mut report = BackupReport::default();
    let hashes: std::collections::BTreeSet<String> = stored.into_iter().filter(|hash| valid_hash(hash)).chain(names.keys().cloned()).collect();
    for hash in hashes {
        let name = names.get(&hash).map_or("original", String::as_str);
        let data = if valid_hash(&hash) { read(crate::store::storage_key(&hash)).await } else { Err("invalid original storage hash".into()) };
        let entry = original_entry(&hash, name);
        // A fresh ZIP CRC would authenticate the bytes we read, even if storage returned
        // another original. Validate the addressed identity before promising recovery.
        let data = data.and_then(|data| {
            if let Some(bytes) = &data {
                verify_original(&entry, bytes)?;
            }
            Ok(data)
        });
        match data {
            Ok(Some(data)) => {
                add(&entry, &data)?;
                report.originals += 1;
            }
            other => {
                let reason = match other {
                    Err(e) => e,
                    _ => "the original is missing from browser storage".to_string(),
                };
                report.missing.push(MissingOriginal { hash, name: name.to_string(), reason });
            }
        }
    }
    if !report.missing.is_empty() {
        add("MISSING-ORIGINALS.txt", report.missing_text().as_bytes())?;
    }
    Ok(report)
}

/// Is `name` a safe library folder name (`library` or `library-<letters, digits, ->`)?
pub fn valid_library_dir(name: &str) -> bool {
    name == DEFAULT_LIBRARY_DIR
        || name.strip_prefix("library-").is_some_and(|s| !s.is_empty() && s.len() <= 64 && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-'))
}

/// Is `h` a plausible content hash (used as a storage key segment)?
pub fn valid_hash(h: &str) -> bool {
    !h.is_empty() && h.len() <= 128 && h.chars().all(|c| c.is_ascii_alphanumeric())
}

/// Zip entry name for an original.
pub fn original_entry(hash: &str, file_name: &str) -> String {
    let name: String = file_name.chars().map(|c| if c == '/' || c == '\\' || c.is_control() { '_' } else { c }).collect();
    let name = if name.is_empty() || name == "." || name == ".." { "original".to_string() } else { name };
    format!("originals/{hash}/{name}")
}

/// Where a backup entry goes when restoring into library folder `dir`: `Some(storage key)`, or
/// `None` to skip it (README, thumbnails, anything unknown or unsafe).
pub fn restore_key(entry: &str, dir: &str) -> Option<String> {
    if let Some(name) = entry.strip_prefix("library/") {
        return crate::files::LIBRARY_FILES.contains(&name).then(|| format!("{dir}/{name}"));
    }
    let rest = entry.strip_prefix("originals/")?;
    let (hash, _) = rest.split_once('/').unwrap_or((rest, ""));
    valid_hash(hash).then(|| format!("originals/{hash}"))
}

fn u16le(v: &mut Vec<u8>, x: u16) {
    v.extend_from_slice(&x.to_le_bytes());
}
fn u32le(v: &mut Vec<u8>, x: u32) {
    v.extend_from_slice(&x.to_le_bytes());
}

struct Central {
    name: String,
    crc: u32,
    size: u32,
    offset: u32,
}

/// Writes a stored (uncompressed) zip piece by piece: [`ZipWriter::entry`] returns the local
/// header to emit before the entry's bytes, [`ZipWriter::finish`] the central directory.
#[derive(Default)]
pub struct ZipWriter {
    entries: Vec<Central>,
    offset: u64,
}

impl ZipWriter {
    /// The local header for `name` holding `data` (emit it, then `data`).
    pub fn entry(&mut self, name: &str, data: &[u8]) -> Result<Vec<u8>, String> {
        let size = u32::try_from(data.len()).map_err(|_| format!("{name}: larger than 4 GB"))?;
        let name_len = u16::try_from(name.len()).map_err(|_| format!("{name}: name too long"))?;
        if self.entries.len() >= usize::from(u16::MAX) {
            return Err("too many files for one backup".into());
        }
        let header_len = 30 + u64::from(name_len);
        if self.offset + header_len + u64::from(size) > MAX_BYTES {
            return Err("the library is too large for one backup file (4 GB)".into());
        }
        let offset = u32::try_from(self.offset).map_err(|_| "backup too large".to_string())?;
        let crc = crc32fast::hash(data);
        let mut h = Vec::with_capacity(header_len as usize);
        u32le(&mut h, LOCAL_SIG);
        u16le(&mut h, 20); // version needed
        u16le(&mut h, UTF8);
        u16le(&mut h, 0); // stored
        u16le(&mut h, 0); // time
        u16le(&mut h, DOS_DATE);
        u32le(&mut h, crc);
        u32le(&mut h, size);
        u32le(&mut h, size);
        u16le(&mut h, name_len);
        u16le(&mut h, 0); // extra
        h.extend_from_slice(name.as_bytes());
        self.offset += header_len + u64::from(size);
        self.entries.push(Central { name: name.to_string(), crc, size, offset });
        Ok(h)
    }

    /// The central directory and end record (emit last).
    pub fn finish(self) -> Result<Vec<u8>, String> {
        let mut cd = Vec::new();
        for e in &self.entries {
            u32le(&mut cd, CENTRAL_SIG);
            u16le(&mut cd, 20); // made by
            u16le(&mut cd, 20); // needed
            u16le(&mut cd, UTF8);
            u16le(&mut cd, 0);
            u16le(&mut cd, 0);
            u16le(&mut cd, DOS_DATE);
            u32le(&mut cd, e.crc);
            u32le(&mut cd, e.size);
            u32le(&mut cd, e.size);
            u16le(&mut cd, e.name.len() as u16);
            u16le(&mut cd, 0); // extra
            u16le(&mut cd, 0); // comment
            u16le(&mut cd, 0); // disk
            u16le(&mut cd, 0); // internal attrs
            u32le(&mut cd, 0); // external attrs
            u32le(&mut cd, e.offset);
            cd.extend_from_slice(e.name.as_bytes());
        }
        let cd_offset = u32::try_from(self.offset).map_err(|_| "backup too large".to_string())?;
        let cd_size = u32::try_from(cd.len()).map_err(|_| "backup too large".to_string())?;
        let n = self.entries.len() as u16;
        let mut out = cd;
        u32le(&mut out, END_SIG);
        u16le(&mut out, 0);
        u16le(&mut out, 0);
        u16le(&mut out, n);
        u16le(&mut out, n);
        u32le(&mut out, cd_size);
        u32le(&mut out, cd_offset);
        u16le(&mut out, 0); // comment
        Ok(out)
    }
}

/// One entry of a zip's central directory.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZipEntry {
    pub name: String,
    pub crc: u32,
    pub size: u64,
    /// Offset of the entry's local header.
    pub header: u64,
    /// 0 = stored (the only method a restore reads).
    pub method: u16,
}

fn rd16(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}
fn rd32(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// How many bytes from the end of a zip to read to find its end record.
pub fn tail_len(file_len: u64) -> u64 {
    file_len.min(22 + 65_535)
}

/// The central directory's (offset, size) from the last [`tail_len`] bytes of a zip.
pub fn find_central(tail: &[u8]) -> Result<(u64, u64), String> {
    let at = (0..tail.len().saturating_sub(21)).rev().find(|&i| rd32(tail, i) == Some(END_SIG)).ok_or("not a zip file (no end record)")?;
    let size = rd32(tail, at + 12).ok_or("truncated zip")?;
    let offset = rd32(tail, at + 16).ok_or("truncated zip")?;
    if size == u32::MAX || offset == u32::MAX {
        return Err("zip64 backups are not supported".into());
    }
    Ok((u64::from(offset), u64::from(size)))
}

/// Parse a central directory.
pub fn parse_central(cd: &[u8]) -> Result<Vec<ZipEntry>, String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i + 46 <= cd.len() {
        if rd32(cd, i) != Some(CENTRAL_SIG) {
            return Err("damaged zip directory".into());
        }
        let bad = || "damaged zip directory".to_string();
        let method = rd16(cd, i + 10).ok_or_else(bad)?;
        let crc = rd32(cd, i + 16).ok_or_else(bad)?;
        let size = rd32(cd, i + 24).ok_or_else(bad)?;
        let n = usize::from(rd16(cd, i + 28).ok_or_else(bad)?);
        let extra = usize::from(rd16(cd, i + 30).ok_or_else(bad)?);
        let comment = usize::from(rd16(cd, i + 32).ok_or_else(bad)?);
        let header = rd32(cd, i + 42).ok_or_else(bad)?;
        let name = cd.get(i + 46..i + 46 + n).ok_or_else(bad)?;
        out.push(ZipEntry { name: String::from_utf8_lossy(name).into_owned(), crc, size: u64::from(size), header: u64::from(header), method });
        i += 46 + n + extra + comment;
    }
    Ok(out)
}

/// Offset of an entry's data from its local header's first 30 bytes.
pub fn data_offset(entry: &ZipEntry, local: &[u8]) -> Result<u64, String> {
    if rd32(local, 0) != Some(LOCAL_SIG) {
        return Err(format!("{}: damaged entry", entry.name));
    }
    let n = rd16(local, 26).ok_or("truncated entry")?;
    let extra = rd16(local, 28).ok_or("truncated entry")?;
    Ok(entry.header + 30 + u64::from(n) + u64::from(extra))
}

/// Check an entry's bytes, including the content identity of an original.
pub fn verify(entry: &ZipEntry, data: &[u8]) -> Result<(), String> {
    if entry.method != 0 {
        return Err(format!("{}: compressed entries are not supported (not a LightCraft backup?)", entry.name));
    }
    if data.len() as u64 != entry.size || crc32fast::hash(data) != entry.crc {
        return Err(format!("{}: damaged (checksum mismatch)", entry.name));
    }
    verify_original(&entry.name, data)
}

/// An original's name is its content identity, independent of the ZIP checksum. Check this
/// even when the destination already exists; old or damaged stored bytes must not be reused.
pub fn verify_original(entry_name: &str, data: &[u8]) -> Result<(), String> {
    if let Some(rest) = entry_name.strip_prefix("originals/") {
        let hash = rest.split_once('/').map_or(rest, |(hash, _)| hash);
        if crate::store::content_hash(data) != hash {
            return Err(format!("{entry_name}: original content hash mismatch"));
        }
    }
    Ok(())
}

/// Validate an archived original before deciding whether its stored copy can be reused.
/// Verified archive bytes repair corrupt copies left by older releases under the same hash.
pub fn original_needs_write(entry: &ZipEntry, archived: &[u8], stored: Option<&[u8]>) -> Result<bool, String> {
    verify(entry, archived)?;
    Ok(stored.is_none_or(|bytes| verify_original(&entry.name, bytes).is_err()))
}

/// A name for the restored library's folder.
pub fn restored_dir_name(now_ms: f64) -> String {
    format!("library-restored-{}", now_ms.max(0.0) as u64)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn build(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut w = ZipWriter::default();
        let mut out = Vec::new();
        for (n, d) in files {
            out.extend(w.entry(n, d).unwrap());
            out.extend_from_slice(d);
        }
        out.extend(w.finish().unwrap());
        out
    }

    fn ready<F: std::future::Future>(future: F) -> Result<F::Output, String> {
        let mut future = std::pin::pin!(future);
        match future.as_mut().poll(&mut std::task::Context::from_waker(std::task::Waker::noop())) {
            std::task::Poll::Ready(result) => Ok(result),
            std::task::Poll::Pending => Err("the test reader must settle synchronously".into()),
        }
    }

    fn original_backup(
        names: &std::collections::HashMap<String, String>,
        stored: Vec<String>,
        disk: &std::collections::HashMap<String, Vec<u8>>,
        refusal: Option<&str>,
    ) -> Result<(BackupReport, Vec<(String, Vec<u8>)>), String> {
        let mut zip = ZipWriter::default();
        let mut bytes = Vec::new();
        // The library remains useful even when one original cannot be read.
        bytes.extend(zip.entry("library/catalog.log", b"")?);
        let report = ready(archive_originals(
            names,
            stored,
            |key| std::future::ready(if refusal == Some(key.as_str()) { Err("fixture read refusal".into()) } else { Ok(disk.get(&key).cloned()) }),
            |name, data| {
                bytes.extend(zip.entry(name, data)?);
                bytes.extend_from_slice(data);
                Ok(())
            },
        ))??;
        bytes.extend(zip.finish()?);
        let tail_start = bytes.len().saturating_sub(tail_len(bytes.len() as u64) as usize);
        let tail = bytes.get(tail_start..).ok_or("missing zip tail")?;
        let (offset, size) = find_central(tail)?;
        let central = bytes.get(offset as usize..(offset + size) as usize).ok_or("missing central directory")?;
        let mut contents = Vec::new();
        for entry in parse_central(central)? {
            let local = bytes.get(entry.header as usize..entry.header as usize + 30).ok_or("missing local header")?;
            let start = data_offset(&entry, local)? as usize;
            let data = bytes.get(start..start + entry.size as usize).ok_or("missing entry bytes")?;
            verify(&entry, data)?;
            contents.push((entry.name, data.to_vec()));
        }
        Ok((report, contents))
    }

    #[test]
    fn missing_and_unreadable_originals_make_valid_useful_partial_archives() {
        let a = crate::store::valid_png_fixture(12, 8).unwrap();
        let b = crate::store::valid_png_fixture(13, 9).unwrap();
        let ha = crate::store::content_hash(&a);
        let hb = crate::store::content_hash(&b);
        let names = std::collections::HashMap::from([(ha.clone(), "available.png".into()), (hb.clone(), "missing.png".into())]);
        for listed in [false, true] {
            for unreadable in [false, true] {
                let mut disk = std::collections::HashMap::from([(crate::store::storage_key(&ha), a.clone())]);
                if unreadable {
                    disk.insert(crate::store::storage_key(&hb), b.clone());
                }
                let mut stored = vec![ha.clone()];
                if listed {
                    stored.push(hb.clone());
                }
                let key = crate::store::storage_key(&hb);
                let (report, entries) = original_backup(&names, stored, &disk, unreadable.then_some(key.as_str())).unwrap();
                assert_eq!(report.originals, 1);
                assert_eq!(report.missing.len(), 1);
                assert_eq!(report.missing[0].hash, hb);
                assert_eq!(report.missing[0].name, "missing.png");
                assert!(report.notice().starts_with("PARTIAL"));
                assert!(!report.notice().contains("Library backed up"));
                assert!(report.notice().contains(&hb) && report.notice().contains("missing.png"));
                assert!(entries.iter().any(|(name, data)| name == "library/catalog.log" && data.is_empty()));
                assert!(entries.iter().any(|(name, data)| name == &original_entry(&ha, "available.png") && data == &a));
                assert!(!entries.iter().any(|(name, _)| name.starts_with(&format!("originals/{hb}"))));
                let diagnostic = entries.iter().find(|(name, _)| name == "MISSING-ORIGINALS.txt").unwrap();
                assert_eq!(diagnostic.1, report.missing_text().as_bytes());
                if unreadable {
                    assert!(report.missing[0].reason.contains("fixture read refusal"));
                }
            }
        }
    }

    #[test]
    fn readable_hash_mismatch_is_missing_and_the_partial_archive_restores() {
        let good = vec![0, 1, 2, 3, 255];
        let expected = vec![4, 5, 6, 7, 255];
        let corrupt = vec![4, 5, 9, 7, 255];
        let good_hash = crate::store::content_hash(&good);
        let bad_hash = crate::store::content_hash(&expected);
        let names = std::collections::HashMap::from([(good_hash.clone(), "healthy.png".into()), (bad_hash.clone(), "misaddressed.png".into())]);
        let disk =
            std::collections::HashMap::from([(crate::store::storage_key(&good_hash), good.clone()), (crate::store::storage_key(&bad_hash), corrupt)]);
        // original_backup parses every entry through the actual restore CRC/hash validator.
        for listed in [false, true] {
            let stored = if listed { vec![good_hash.clone(), bad_hash.clone()] } else { Vec::new() };
            let (report, entries) = original_backup(&names, stored, &disk, None).unwrap();
            assert_eq!(report.originals, 1);
            assert_eq!(report.missing.len(), 1);
            assert_eq!(report.missing[0].hash, bad_hash);
            assert_eq!(report.missing[0].name, "misaddressed.png");
            assert!(report.missing[0].reason.contains("content hash mismatch"));
            assert!(report.notice().starts_with("PARTIAL"));
            assert!(!report.notice().contains("Library backed up"));
            assert!(entries.iter().any(|(name, data)| name == &original_entry(&good_hash, "healthy.png") && data == &good));
            assert!(!entries.iter().any(|(name, _)| name.starts_with(&format!("originals/{bad_hash}/"))));
            let diagnostics = entries.iter().find(|(name, _)| name == "MISSING-ORIGINALS.txt").unwrap();
            assert_eq!(diagnostics.1, report.missing_text().as_bytes());
            assert!(entries.iter().any(|(name, data)| name == "library/catalog.log" && data.is_empty()));
        }
    }

    #[test]
    fn complete_backups_keep_all_originals_and_do_not_report_partial() {
        let a = crate::store::valid_png_fixture(12, 8).unwrap();
        let b = crate::store::valid_png_fixture(13, 9).unwrap();
        let ha = crate::store::content_hash(&a);
        let hb = crate::store::content_hash(&b);
        let names = std::collections::HashMap::from([(ha.clone(), "a.png".into()), (hb.clone(), "b.png".into())]);
        let disk = std::collections::HashMap::from([(crate::store::storage_key(&ha), a.clone()), (crate::store::storage_key(&hb), b.clone())]);
        // Omitted/stale listing entries cannot hide a catalog original; reads are deduplicated.
        let (report, entries) = original_backup(&names, vec![ha.clone(), ha.clone(), "not/a/hash".into()], &disk, None).unwrap();
        assert_eq!(report.originals, 2);
        assert!(report.missing.is_empty());
        assert!(report.notice().starts_with("Library backed up: 2 originals"));
        assert!(!entries.iter().any(|(name, _)| name == "MISSING-ORIGINALS.txt"));
        assert!(entries.iter().any(|(name, data)| name == &original_entry(&hb, "b.png") && data == &b));
    }

    #[test]
    fn written_backups_read_back() {
        let original = [0, 1, 2, 3, 255];
        let name = original_entry(&crate::store::content_hash(&original), "IMG 1.jpg");
        let files: [(&str, &[u8]); 3] = [("README.txt", README.as_bytes()), ("library/catalog.log", b"{\"seq\":1}\n"), (&name, &original)];
        let zip = build(&files);
        let tail = &zip[zip.len() - tail_len(zip.len() as u64) as usize..];
        let (off, size) = find_central(tail).unwrap();
        let entries = parse_central(&zip[off as usize..(off + size) as usize]).unwrap();
        assert_eq!(entries.len(), 3);
        for (e, (name, data)) in entries.iter().zip(files) {
            assert_eq!(e.name, name);
            let start = data_offset(e, &zip[e.header as usize..e.header as usize + 30]).unwrap() as usize;
            let got = &zip[start..start + e.size as usize];
            assert_eq!(got, data);
            verify(e, got).unwrap();
        }
        // damage is caught
        let e = &entries[2];
        assert!(verify(e, &[0, 1, 2, 3, 254]).is_err());
        assert!(find_central(b"not a zip at all, just some text").is_err());
    }

    #[test]
    fn valid_zip_crc_cannot_replace_original_content_identity() {
        let original = [0, 1, 2, 3, 255];
        let replacement = [0, 1, 9, 3, 255]; // same size, different original bytes
        let hash = crate::store::content_hash(&original);
        // Both the legacy hash-only layout and the browsable named layout are supported.
        for name in [format!("originals/{hash}"), original_entry(&hash, "photo.png")] {
            let entry = ZipEntry { name: name.clone(), crc: crc32fast::hash(&replacement), size: replacement.len() as u64, header: 0, method: 0 };
            assert!(verify(&entry, &replacement).unwrap_err().contains("content hash mismatch"));
            assert!(verify_original(&name, &replacement).is_err()); // existing storage too
        }
    }

    #[test]
    fn verified_originals_reuse_matching_storage_and_repair_corrupt_storage() {
        let original = [0, 1, 2, 3, 255];
        let corrupt = [0, 1, 9, 3, 255];
        let hash = crate::store::content_hash(&original);
        let entry =
            ZipEntry { name: original_entry(&hash, "photo.png"), crc: crc32fast::hash(&original), size: original.len() as u64, header: 0, method: 0 };
        assert!(!original_needs_write(&entry, &original, Some(&original)).unwrap());
        assert!(original_needs_write(&entry, &original, None).unwrap());
        assert!(original_needs_write(&entry, &original, Some(&corrupt)).unwrap());
        let mut stored = corrupt;
        if original_needs_write(&entry, &original, Some(&stored)).unwrap() {
            stored = original;
        }
        assert!(!original_needs_write(&entry, &original, Some(&stored)).unwrap());
    }

    #[test]
    fn invalid_archive_original_cannot_authorize_reuse_or_repair() {
        let original = [0, 1, 2, 3, 255];
        let invalid = [0, 1, 9, 3, 255];
        let hash = crate::store::content_hash(&original);
        let entry =
            ZipEntry { name: original_entry(&hash, "photo.png"), crc: crc32fast::hash(&invalid), size: invalid.len() as u64, header: 0, method: 0 };
        for stored in [None, Some(original.as_slice()), Some(invalid.as_slice())] {
            assert!(original_needs_write(&entry, &invalid, stored).unwrap_err().contains("content hash mismatch"));
        }
        let damaged = ZipEntry { crc: entry.crc ^ 1, ..entry };
        assert!(original_needs_write(&damaged, &invalid, Some(&invalid)).unwrap_err().contains("checksum mismatch"));
    }

    #[test]
    fn matching_originals_and_library_files_pass_restore_validation() {
        let original = [0, 1, 2, 3, 255];
        let hash = crate::store::content_hash(&original);
        for name in [format!("originals/{hash}"), original_entry(&hash, "photo.png"), "library/catalog.log".into()] {
            let entry = ZipEntry { name, crc: crc32fast::hash(&original), size: original.len() as u64, header: 0, method: 0 };
            verify(&entry, &original).unwrap();
        }
    }

    #[test]
    fn restore_keys_are_safe() {
        assert_eq!(restore_key("library/catalog.snap", "library-restored-1").as_deref(), Some("library-restored-1/catalog.snap"));
        assert_eq!(restore_key("library/../../x", "library-r"), None);
        assert_eq!(restore_key("library/evil.bin", "library-r"), None);
        assert_eq!(restore_key("originals/abc123/IMG.jpg", "d").as_deref(), Some("originals/abc123"));
        assert_eq!(restore_key("originals/../library/catalog.log", "d"), None);
        assert_eq!(restore_key("README.txt", "d"), None);
        assert_eq!(restore_key("thumbs/x.jpg", "d"), None);
        assert_eq!(original_entry("abc", "a/b\\c.jpg"), "originals/abc/a_b_c.jpg");
        assert_eq!(original_entry("abc", ".."), "originals/abc/original");
        assert!(valid_library_dir("library") && valid_library_dir("library-restored-17"));
        assert!(!valid_library_dir("library-../x") && !valid_library_dir("originals") && !valid_library_dir("library-"));
    }

    #[test]
    fn size_limits_are_errors_not_panics() {
        let mut w = ZipWriter { offset: MAX_BYTES - 10, ..Default::default() };
        assert!(w.entry("x", &[0; 64]).is_err());
    }
}
