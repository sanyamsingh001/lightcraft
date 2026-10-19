//! Existing bounded integrity checks for the upstream separately staged Chinese UI face.
use super::{CHINESE_FONT_FILE, CHINESE_FONT_LICENSE_FILE, chinese_font_in_manifest};
use sha2::{Digest, Sha256};
use std::{io::Read, path::Path};
pub const FILE_LIMIT: u64 = 25 * 1024 * 1024;
const INPUT: &str = "fonts/noto-sans-cjk-sc/NotoSansCJKsc-Regular.otf";
const LICENCE_INPUT: &str = "fonts/noto-sans-cjk-sc/OFL.txt";
const FONT_BYTES: usize = 16_437_364;
const FONT_SHA256: &str = "2c76254f6fc379fddfce0a7e84fb5385bb135d3e399294f6eeb6680d0365b74b";

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

pub fn stage(out: &Path, input: Option<&Path>, required: bool) -> Result<bool, String> {
    // Preserve upstream cleanup and its font-free/required build contract.
    for name in [CHINESE_FONT_FILE, CHINESE_FONT_LICENSE_FILE, "lightcraft_zh_hans.otf.gz", "lightcraft_zh_hans.otf.br"] {
        let path = out.join(name);
        match std::fs::remove_file(&path) {
            Ok(()) => (),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
            Err(e) => return Err(format!("{}: {e}", path.display())),
        }
    }
    let Some(input) = input else {
        return if required { Err("CRAFT_FONTS_REQUIRED is set but CRAFT_FONTS_DIR is missing".into()) } else { Ok(false) };
    };
    let manifest_path = input.join("fonts/manifest.txt");
    let manifest = bounded_read(&manifest_path, 256 * 1024)?;
    let manifest = std::str::from_utf8(&manifest).map_err(|_| "font manifest is not UTF-8")?;
    let (font, licence) =
        chinese_font_in_manifest(manifest).ok_or_else(|| format!("{}: Noto Sans CJK SC Regular is missing", manifest_path.display()))?;
    let pinned = manifest
        .lines()
        .find_map(|line| {
            let fields: Vec<_> = line.split('|').map(str::trim).collect();
            let selected =
                matches!(fields.as_slice(), ["Noto Sans CJK SC", "Regular", file, _, _, licence, ..] if !file.is_empty() && !licence.is_empty());
            selected.then(|| {
                matches!(fields.as_slice(), [_, _, file, _, "OFL-1.1", licence, hash, ..]
            if *file == INPUT && *licence == LICENCE_INPUT && *hash == FONT_SHA256)
            })
        })
        .unwrap_or(false);
    if font != INPUT || licence != LICENCE_INPUT || !pinned {
        return Err("the Chinese UI font manifest entry does not match the pinned asset".into());
    }
    let bytes = bounded_read(&input.join(INPUT), FONT_BYTES as u64)?;
    if bytes.len() != FONT_BYTES || format!("{:x}", Sha256::digest(&bytes)) != FONT_SHA256 {
        return Err("the Chinese UI font bytes do not match the pinned length/SHA256".into());
    }
    let licence = bounded_read(&input.join(LICENCE_INPUT), 128 * 1024)?;
    let licence_text = std::str::from_utf8(&licence).map_err(|_| "font licence is not UTF-8")?;
    if !licence_text.contains("SIL OPEN FONT LICENSE Version 1.1") {
        return Err("the font OFL licence is missing".into());
    }
    for (name, data) in [(CHINESE_FONT_FILE, bytes.as_slice()), (CHINESE_FONT_LICENSE_FILE, licence.as_slice())] {
        if data.len() as u64 > FILE_LIMIT {
            return Err(format!("{name} exceeds the 25 MiB per-file budget"));
        }
        std::fs::write(out.join(name), data).map_err(|e| format!("{name}: {e}"))?;
    }
    println!("web Chinese font: {} → {}", input.join(INPUT).display(), out.join(CHINESE_FONT_FILE).display());
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn staging_is_exact_and_removes_stale_assets_when_input_is_unset() -> Result<(), Box<dyn std::error::Error>> {
        let Some(input) = std::env::var_os("CRAFT_FONTS_DIR") else { return Ok(()) };
        let out = crate::root().join("target/web-font-stage-test");
        std::fs::create_dir_all(&out)?;
        assert!(stage(&out, Some(Path::new(&input)), true)?);
        assert_eq!(std::fs::read(out.join(CHINESE_FONT_FILE))?, std::fs::read(Path::new(&input).join(INPUT))?);
        assert!(std::fs::read_to_string(out.join(CHINESE_FONT_LICENSE_FILE))?.contains("SIL OPEN FONT LICENSE Version 1.1"));
        assert!(!stage(&out, None, false)?);
        assert!(!out.join(CHINESE_FONT_FILE).exists());
        assert!(stage(&out, None, true).is_err());
        Ok(())
    }
}
