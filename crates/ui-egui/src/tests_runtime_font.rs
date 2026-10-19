//! Actual shared-app regressions. Browser transport/optimized bundle gates run separately.
use crate::{
    LightcraftApp, Services,
    headless::Headless,
    i18n::{self, Locale},
    runtime_font::FontStatus,
};
use serde_json::json;
use std::{sync::Arc, time::Duration};
const T: Duration = Duration::from_secs(5);
const KEY: &str = "craft-fonts Noto Sans CJK SC Regular (web)";
fn app() -> Headless {
    Headless::new(LightcraftApp::new(lightcraft_engine::Session::new(), Services::default()), [960.0, 800.0], 1.0)
}
fn font_data() -> Result<Option<egui::FontData>, Box<dyn std::error::Error>> {
    let Some(dir) = std::env::var_os("CRAFT_FONTS_DIR") else { return Ok(None) };
    let bytes = std::fs::read(std::path::PathBuf::from(dir).join(lightcraft_engine::fonts::CHINESE_UI_FONT.input))?;
    Ok(Some(egui::FontData::from_owned(bytes)))
}
#[test]
fn native_does_not_request_a_web_font_or_change_the_selected_language() {
    let mut h = app();
    h.app.ui.language = Locale::ZhHant;
    assert!(h.settle(T));
    assert_eq!(h.app.runtime_ui_font.status(), &FontStatus::Disabled);
    assert_eq!(h.app.display_language(), Locale::ZhHant);
    assert!(!h.app.runtime_ui_font.take_request());
    assert!(!h.app.runtime_ui_font.request(true));
    i18n::set_language(Locale::En);
}
#[test]
fn startup_and_menu_selection_preserve_saved_chinese_during_failure() {
    let mut h = app();
    h.app.runtime_ui_font.enable();
    h.app.ui.language = Locale::ZhHans;
    h.step();
    assert_eq!(h.app.display_language(), Locale::En);
    assert_eq!(serde_json::to_value(&h.app.ui).unwrap()["language"], "zh-hans");
    assert!(h.app.runtime_ui_font.take_request());
    h.app.runtime_ui_font.finish(Err("fixture offline".into()));
    h.step();
    h.step();
    assert!(!h.app.runtime_ui_font.take_request());
    h.app.run("app.language.traditionalChinese", json!({})).unwrap();
    assert_eq!(h.app.ui.language, Locale::ZhHant);
    assert_eq!(h.app.display_language(), Locale::En);
    assert!(h.app.runtime_ui_font.take_request());
    h.app.run("app.language.english", json!({})).unwrap();
    h.app.runtime_ui_font.finish(Err("fixture offline".into()));
    assert_eq!(h.app.display_language(), Locale::En);
}
#[test]
fn menu_exposure_and_settings_click_request_before_selection_and_retry_current_choice() {
    let mut h = app();
    h.app.runtime_ui_font.enable();
    assert!(h.settle(T));
    assert!(!h.app.runtime_ui_font.take_request());
    assert_eq!(h.request("ui.clickWidget", json!({"id": "menu:Edit"}), T)["ok"], true);
    h.step();
    assert!(h.app.runtime_ui_font.take_request());
    assert_eq!(h.app.ui.language, Locale::En);
    assert_eq!(h.request("ui.key", json!({"key": "Escape"}), T)["ok"], true);
    h.app.runtime_ui_font.finish(Err("fixture offline".into()));
    h.app.run("app.language.simplifiedChinese", json!({})).unwrap();
    assert!(h.app.runtime_ui_font.take_request());
    h.app.runtime_ui_font.finish(Err("fixture offline".into()));
    assert_eq!(h.request("ui.menu.invoke", json!({"id": "app.settings"}), T)["ok"], true);
    assert!(h.settle(T));
    assert!(!h.app.runtime_ui_font.take_request());
    assert_eq!(h.request("ui.clickWidget", json!({"id": "button:settingsLanguage-1"}), T)["ok"], true);
    h.step();
    assert_eq!(h.app.ui.language, Locale::ZhHans);
    assert!(h.app.runtime_ui_font.take_request());
}
#[test]
fn loaded_owned_face_survives_locale_switches_and_shadow_screenshots() -> Result<(), Box<dyn std::error::Error>> {
    let Some(data) = font_data()? else { return Ok(()) };
    let mut h = app();
    h.app.runtime_ui_font.enable();
    h.app.select_language(Locale::ZhHans);
    assert!(h.app.runtime_ui_font.take_request());
    h.app.select_language(Locale::ZhHant);
    h.app.runtime_ui_font.finish(Ok(data));
    for locale in [Locale::ZhHant, Locale::Ja, Locale::En, Locale::ZhHans, Locale::ZhHant] {
        h.app.select_language(locale);
        assert!(h.settle(T));
        let owned = h.app.runtime_ui_font.data().ok_or("font not retained")?;
        assert!(h.view.ctx.fonts_mut(|f| f.definitions().font_data.get(KEY).is_some_and(|v| Arc::ptr_eq(owned, v))));
        let ctx = h.view.ctx.clone();
        h.app.headless_screenshot(&ctx, false);
        let shadow = h.app.shadow.as_ref().ok_or("shadow not created")?;
        assert!(shadow.ctx.fonts_mut(|f| f.definitions().font_data.get(KEY).is_some_and(|v| Arc::ptr_eq(h.app.runtime_ui_font.data().unwrap(), v))));
        assert!(shadow.ctx.fonts_mut(|f| f.has_glyph(&egui::FontId::proportional(13.0), '简')));
    }
    i18n::set_language(Locale::En);
    Ok(())
}
#[test]
fn wasm_face_order_keeps_latin_and_japanese_first_and_shares_the_arc() -> Result<(), Box<dyn std::error::Error>> {
    let Some(data) = font_data()? else { return Ok(()) };
    let data = Arc::new(data);
    // The exact WASM embedded filter, exercised on the native shared definition API.
    let craft = Box::leak(
        lightcraft_engine::CRAFT_FONTS
            .iter()
            .filter(|f| f.family == "BIZ UDPGothic" && f.style == "Regular")
            .map(|f| lightcraft_engine::CraftFont { family: f.family, style: f.style, scripts: f.scripts, bytes: f.bytes })
            .collect::<Vec<_>>()
            .into_boxed_slice(),
    );
    for locale in [Locale::ZhHans, Locale::ZhHant, Locale::Ja, Locale::En] {
        i18n::set_language(locale);
        let definitions = crate::theme::font_definitions_with_runtime(craft, Some(&data));
        assert!(Arc::ptr_eq(&definitions.font_data[KEY], &data));
        for family in [egui::FontFamily::Proportional, egui::FontFamily::Name(crate::theme::FONT_SEMIBOLD.into()), egui::FontFamily::Monospace] {
            let names = &definitions.families[&family];
            assert!(!names[0].starts_with("craft-fonts"));
            let first_cjk = names.iter().find(|n| n.starts_with("craft-fonts")).ok_or("no fallback")?;
            if matches!(locale, Locale::ZhHans | Locale::ZhHant) {
                assert_eq!(first_cjk, KEY);
            } else {
                assert!(first_cjk.contains("BIZ UDPGothic"));
            }
        }
    }
    i18n::set_language(Locale::En);
    Ok(())
}
