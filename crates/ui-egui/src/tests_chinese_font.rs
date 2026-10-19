//! Existing owned-face screenshot regression, adapted to upstream pre-startup font delivery.
use crate::{
    LightcraftApp, Services,
    headless::Headless,
    i18n::{self, Locale},
};
use std::{sync::Arc, time::Duration};
const T: Duration = Duration::from_secs(5);
const KEY: &str = "web Noto Sans CJK SC";
fn app(bytes: Vec<u8>) -> Headless {
    Headless::new(LightcraftApp::new(lightcraft_engine::Session::new(), Services::default()).with_chinese_font(bytes), [960.0, 800.0], 1.0)
}
fn font_data() -> Result<Option<Vec<u8>>, Box<dyn std::error::Error>> {
    let Some(dir) = std::env::var_os("CRAFT_FONTS_DIR") else { return Ok(None) };
    let bytes = std::fs::read(std::path::PathBuf::from(dir).join("fonts/noto-sans-cjk-sc/NotoSansCJKsc-Regular.otf"))?;
    Ok(Some(bytes))
}
#[test]
fn loaded_owned_face_survives_locale_switches_and_shadow_screenshots() -> Result<(), Box<dyn std::error::Error>> {
    let Some(data) = font_data()? else { return Ok(()) };
    let mut h = app(data);
    for locale in [Locale::ZhHant, Locale::Ja, Locale::En, Locale::ZhHans, Locale::ZhHant] {
        h.app.ui.language = locale;
        assert!(h.settle(T));
        let owned = h.app.chinese_font.as_ref().ok_or("font not retained")?;
        assert!(h.view.ctx.fonts_mut(|f| f.definitions().font_data.get(KEY).is_some_and(|v| Arc::ptr_eq(owned, v))));
        let ctx = h.view.ctx.clone();
        h.app.headless_screenshot(&ctx, false);
        let shadow = h.app.shadow.as_ref().ok_or("shadow not created")?;
        assert!(shadow.ctx.fonts_mut(|f| f.definitions().font_data.get(KEY).is_some_and(|v| Arc::ptr_eq(h.app.chinese_font.as_ref().unwrap(), v))));
        assert!(shadow.ctx.fonts_mut(|f| f.has_glyph(&egui::FontId::proportional(13.0), '简')));
    }
    i18n::set_language(Locale::En);
    Ok(())
}
