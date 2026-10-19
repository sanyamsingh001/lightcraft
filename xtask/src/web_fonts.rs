//! Generated optional web font assets; font bytes remain outside source control.
use sha2::{Digest, Sha256};
use std::{io::Read, path::Path};
#[path = "../../crates/engine/src/font_asset.rs"]
mod asset;
pub use asset::CHINESE_UI_FONT;

pub const FILE_LIMIT: u64 = 25 * 1024 * 1024;
const DESCRIPTOR: &str = "fonts/chinese-ui.json";

fn bounded_read(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .and_then(|f| f.take(limit.saturating_add(1)).read_to_end(&mut bytes))
        .map_err(|e| format!("{}: {e}", path.display()))?;
    if bytes.len() as u64 > limit {
        return Err(format!("{} exceeds its byte limit", path.display()));
    }
    Ok(bytes)
}

/// Removes only this feature's generated files, never other font/packaging output.
fn clear(out: &Path) -> Result<(), String> {
    let a = CHINESE_UI_FONT;
    for name in [a.url, a.licence_url, a.notice_url, DESCRIPTOR] {
        for suffix in ["", ".gz", ".br"] {
            let path = out.join(format!("{name}{suffix}"));
            if let Err(e) = std::fs::remove_file(&path)
                && e.kind() != std::io::ErrorKind::NotFound
            {
                return Err(format!("{}: {e}", path.display()));
            }
        }
    }
    Ok(())
}

pub fn stage(out: &Path, input: Option<&Path>, required: bool) -> Result<bool, String> {
    let a = CHINESE_UI_FONT;
    let Some(input) = input else {
        clear(out)?;
        return if required { Err("CRAFT_FONTS_REQUIRED: missing CRAFT_FONTS_DIR for the web Chinese UI font".into()) } else { Ok(false) };
    };
    let manifest = bounded_read(&input.join("fonts/manifest.txt"), 256 * 1024)?;
    let manifest = std::str::from_utf8(&manifest).map_err(|_| "font manifest is not UTF-8")?;
    let entry = manifest.lines().find(|line| {
        let mut fields = line.split(" | ").map(str::trim);
        fields.next() == Some(a.family) && fields.next() == Some("Regular")
    });
    let Some(entry) = entry else {
        clear(out)?;
        return if required { Err("CRAFT_FONTS_REQUIRED: the pinned Chinese UI font is absent".into()) } else { Ok(false) };
    };
    let fields: Vec<_> = entry.split(" | ").map(str::trim).collect();
    if !matches!(fields.as_slice(), [_, _, file, _, "OFL-1.1", licence, hash, ..] if *file == a.input && *licence == a.licence_input && *hash == a.sha256)
    {
        return Err("the Chinese UI font manifest entry does not match the pinned asset".into());
    }
    let bytes = bounded_read(&input.join(a.input), a.bytes as u64)?;
    if bytes.len() != a.bytes || format!("{:x}", Sha256::digest(&bytes)) != a.sha256 {
        return Err("the Chinese UI font bytes do not match the pinned length/SHA256".into());
    }
    let licence = bounded_read(&input.join(a.licence_input), 128 * 1024)?;
    let licence_text = std::str::from_utf8(&licence).map_err(|_| "font licence is not UTF-8")?;
    if !licence_text.contains("SIL OPEN FONT LICENSE Version 1.1") {
        return Err("the font OFL licence is missing".into());
    }
    let notice = format!(
        "{} Regular\n{}\nThe Noto Project Authors (Adobe, Google)\n{}\nUnmodified font, SIL Open Font License 1.1; see OFL-NotoSansCJKSC.txt.\nSHA256: {}\n",
        a.family, a.copyright, a.source, a.sha256
    );
    let descriptor = serde_json::to_vec_pretty(&serde_json::json!({
        "family": a.family, "style": "Regular", "url": a.url, "bytes": a.bytes, "sha256": a.sha256,
        "licence": a.licence_url, "notice": a.notice_url, "source": a.source,
    }))
    .map_err(|e| e.to_string())?;
    std::fs::create_dir_all(out.join("fonts")).map_err(|e| e.to_string())?;
    for (name, data) in
        [(a.url, bytes.as_slice()), (a.licence_url, licence.as_slice()), (a.notice_url, notice.as_bytes()), (DESCRIPTOR, descriptor.as_slice())]
    {
        if data.len() as u64 > FILE_LIMIT {
            return Err(format!("{name} exceeds the 25 MiB per-file budget"));
        }
        std::fs::write(out.join(name), data).map_err(|e| format!("{name}: {e}"))?;
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn staging_is_exact_and_removes_stale_assets_when_input_is_unset() -> Result<(), Box<dyn std::error::Error>> {
        let Some(input) = std::env::var_os("CRAFT_FONTS_DIR") else { return Ok(()) };
        let out = crate::root().join("target/web-font-stage-test");
        assert!(stage(&out, Some(Path::new(&input)), true)?);
        assert_eq!(std::fs::read(out.join(CHINESE_UI_FONT.url))?, std::fs::read(Path::new(&input).join(CHINESE_UI_FONT.input))?);
        assert!(std::fs::read_to_string(out.join(CHINESE_UI_FONT.notice_url))?.contains(CHINESE_UI_FONT.copyright));
        assert!(!stage(&out, None, false)?);
        assert!(!out.join(CHINESE_UI_FONT.url).exists());
        assert!(stage(&out, None, true).is_err());
        Ok(())
    }
}
