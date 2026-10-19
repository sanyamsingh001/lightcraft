//! Metadata for the unmodified optional Chinese UI font, never embedded in WASM.
#[derive(Clone, Copy, Debug)]
pub struct FontAsset {
    pub family: &'static str,
    pub input: &'static str,
    pub licence_input: &'static str,
    pub url: &'static str,
    pub licence_url: &'static str,
    pub notice_url: &'static str,
    pub bytes: usize,
    pub sha256: &'static str,
    pub source: &'static str,
    pub copyright: &'static str,
}

pub const CHINESE_UI_FONT: FontAsset = FontAsset {
    family: "Noto Sans CJK SC",
    input: "fonts/noto-sans-cjk-sc/NotoSansCJKsc-Regular.otf",
    licence_input: "fonts/noto-sans-cjk-sc/OFL.txt",
    url: "fonts/NotoSansCJKsc-Regular.otf",
    licence_url: "fonts/OFL-NotoSansCJKSC.txt",
    notice_url: "fonts/NOTICE-NotoSansCJKSC.txt",
    bytes: 16_437_364,
    sha256: "2c76254f6fc379fddfce0a7e84fb5385bb135d3e399294f6eeb6680d0365b74b",
    source: "https://github.com/notofonts/noto-cjk/tree/f8d157532fbfaeda587e826d4cd5b21a49186f7c/Sans/OTF/SimplifiedChinese",
    copyright: "© 2014-2021 Adobe (http://www.adobe.com/).",
};
