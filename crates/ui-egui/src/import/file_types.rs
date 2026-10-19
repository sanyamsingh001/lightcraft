//! File-type controls use the scan's candidates and the engine's supported extensions only.
use super::{ImportDialog, ImportTypes, field};
use crate::LightcraftApp;
use crate::widgets::register;
use serde_json::{Value, json};

/// `import.fileTypes {types: {group: all|raw} | {formats: [extensions]}}`.
/// Validate before touching the draft; changing types does not start or replace an import.
pub fn choose_file_types(app: &mut LightcraftApp, params: &Value) -> Result<Value, String> {
    if params.get("types").is_none() {
        return Err("types is required".into());
    }
    let types = ImportTypes::from_params(params)?;
    let Some(crate::state::Dialog::Import { opts }) = app.ui.dialog.as_mut() else { return Err("no import review open".into()) };
    opts.set_types(types);
    Ok(
        json!({"types": opts.types, "found": (0..opts.candidates.len()).filter(|i| opts.visible(*i)).count(), "selected": opts.selected_paths().len()}),
    )
}

pub(super) fn body(ui: &mut egui::Ui, dialog: &mut ImportDialog) {
    field(ui, "File types", |ui| {
        if crate::widgets::text_button(ui, "importTypesAll", crate::i18n::tr("All"), dialog.types.is_all()).clicked() {
            dialog.set_types(ImportTypes::default());
        }
        if crate::widgets::text_button(ui, "importTypesRaw", crate::i18n::tr("RAW only"), dialog.types.is_raw()).clicked() {
            dialog.set_types(ImportTypes::raw());
        }
        let formats = ImportTypes::supported_formats();
        let label = if dialog.types.is_all() || dialog.types.is_raw() { "Choose types…" } else { "Selected types" };
        let combo = egui::ComboBox::from_id_salt("import-file-types").selected_text(crate::i18n::tr(label)).show_ui(ui, |ui| {
            for &extension in &formats {
                let mut included = dialog.types.includes_extension(extension);
                let label = match extension {
                    "jpg" => "JPG / JPEG".into(),
                    "tif" => "TIF / TIFF".into(),
                    extension => extension.to_ascii_uppercase(),
                };
                let response = ui.checkbox(&mut included, label);
                register(ui.ctx(), format!("check:importType-{extension}"), response.rect);
                if response.changed() {
                    let mut chosen: Vec<String> =
                        formats.iter().filter(|e| **e != extension && dialog.types.includes_extension(e)).map(|e| (*e).to_string()).collect();
                    if included {
                        chosen.push(extension.to_string());
                    }
                    if let Ok(types) = ImportTypes::specific(chosen) {
                        dialog.set_types(types);
                    }
                }
            }
        });
        register(ui.ctx(), "combo:importFileTypes", combo.response.rect);
    });
}
