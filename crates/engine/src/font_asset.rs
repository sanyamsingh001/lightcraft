//! Existing pinned input for bounded verification of the separately delivered Chinese UI font.
#[derive(Clone, Copy, Debug)]
pub struct FontAsset {
    pub input: &'static str,
    pub bytes: usize,
    pub sha256: &'static str,
}
pub const CHINESE_UI_FONT: FontAsset = FontAsset {
    input: "fonts/noto-sans-cjk-sc/NotoSansCJKsc-Regular.otf",
    bytes: 16_437_364,
    sha256: "2c76254f6fc379fddfce0a7e84fb5385bb135d3e399294f6eeb6680d0365b74b",
};
