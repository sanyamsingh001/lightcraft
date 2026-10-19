//! Decoded-body bound and pinned integrity checks, shared with native unit tests.
use lightcraft_engine::fonts::FontAsset;
use sha2::{Digest, Sha256};

pub struct FontBody {
    asset: FontAsset,
    bytes: Vec<u8>,
    hash: Sha256,
}
impl FontBody {
    pub fn new(asset: FontAsset) -> Self {
        Self { asset, bytes: Vec::new(), hash: Sha256::new() }
    }
    pub fn remaining(&self) -> usize {
        self.asset.bytes.saturating_sub(self.bytes.len())
    }
    #[cfg(test)]
    pub fn push(&mut self, chunk: &[u8]) -> Result<(), String> {
        self.append(chunk.len(), |bytes| bytes.copy_from_slice(chunk))
    }
    /// Reserve a bounded region and copy directly into it; avoids a second full-chunk buffer.
    pub fn append(&mut self, length: usize, copy: impl FnOnce(&mut [u8])) -> Result<(), String> {
        if length > self.remaining() {
            return Err("the font response is larger than expected".into());
        }
        let start = self.bytes.len();
        let needed = start.saturating_add(length);
        if needed > self.bytes.capacity() {
            let capacity = self.bytes.capacity().max(32_768).saturating_mul(2).max(needed).min(self.asset.bytes);
            self.bytes.try_reserve_exact(capacity.saturating_sub(start)).map_err(|_| "not enough memory to load the Chinese font")?;
        }
        self.bytes.resize(needed, 0);
        let region = self.bytes.get_mut(start..).ok_or("invalid font buffer length")?;
        copy(region);
        self.hash.update(region);
        Ok(())
    }
    pub fn finish(self) -> Result<Vec<u8>, String> {
        if self.bytes.len() != self.asset.bytes {
            return Err("the font response is incomplete".into());
        }
        if format!("{:x}", self.hash.finalize()) != self.asset.sha256 {
            return Err("the font response failed its integrity check".into());
        }
        ab_glyph::FontRef::try_from_slice(&self.bytes).map_err(|_| "the downloaded font is invalid")?;
        Ok(self.bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tiny(bytes: usize, sha256: &'static str) -> FontAsset {
        FontAsset { bytes, sha256, ..lightcraft_engine::fonts::CHINESE_UI_FONT }
    }
    #[test]
    fn refuses_oversized_and_truncated_decoded_bodies() {
        let mut body = FontBody::new(tiny(3, ""));
        body.push(b"ab").unwrap();
        assert!(body.push(b"cd").is_err());
        assert_eq!(body.remaining(), 1);
        assert!(body.finish().unwrap_err().contains("incomplete"));
    }
    #[test]
    fn checks_hash_before_font_and_rejects_even_hash_matched_invalid_fonts() {
        let mut bad_hash = FontBody::new(tiny(3, "wrong"));
        bad_hash.push(b"abc").unwrap();
        assert!(bad_hash.finish().unwrap_err().contains("integrity"));
        let mut invalid = FontBody::new(tiny(3, "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"));
        invalid.push(b"abc").unwrap();
        assert!(invalid.finish().unwrap_err().contains("invalid"));
    }
    #[test]
    fn pinned_input_round_trips_without_transformation() -> Result<(), Box<dyn std::error::Error>> {
        let Some(dir) = std::env::var_os("CRAFT_FONTS_DIR") else { return Ok(()) };
        let asset = lightcraft_engine::fonts::CHINESE_UI_FONT;
        let bytes = std::fs::read(std::path::PathBuf::from(dir).join(asset.input))?;
        let mut body = FontBody::new(asset);
        for chunk in bytes.chunks(65_536) {
            body.push(chunk)?;
        }
        assert_eq!(body.finish()?, bytes);
        Ok(())
    }
}
