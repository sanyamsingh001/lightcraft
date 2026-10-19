//! Validated extension choices shared by import commands, scan workers and the review UI.
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::EXTENSIONS;

const RAW: &[&str] = &["dng", "cr2", "cr3", "nef", "nrw", "arw", "raf", "orf", "rw2", "rwl", "raw", "pef"];

/// `types: {group: all|raw}` or `types: {formats: [supported extensions]}`.
/// Fields stay private so every specific choice is validated and normalized before use.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(try_from = "Value", into = "Value")]
pub struct ImportTypes {
    raw: bool,
    formats: Option<Vec<String>>,
}

impl ImportTypes {
    /// Missing `types` retains the existing All default. A supplied value must be structured.
    pub fn from_params(params: &Value) -> Result<Self, String> {
        params.get("types").map(Self::parse).transpose().map(|types| types.unwrap_or_default())
    }

    fn parse(value: &Value) -> Result<Self, String> {
        let Some(object) = value.as_object().filter(|object| object.len() == 1) else {
            return Err("types must contain either group (all|raw) or formats (supported extensions)".into());
        };
        if let Some(group) = object.get("group") {
            return match group.as_str() {
                Some("all") => Ok(Self::default()),
                Some("raw") => Ok(Self::raw()),
                _ => Err("types.group must be all or raw".into()),
            };
        }
        let Some(formats) = object.get("formats").and_then(Value::as_array).filter(|formats| formats.len() <= EXTENSIONS.len()) else {
            return Err("types.formats must be an array of supported extension strings".into());
        };
        let formats = formats
            .iter()
            .map(|format| {
                format
                    .as_str()
                    .filter(|format| EXTENSIONS.iter().any(|extension| extension.eq_ignore_ascii_case(format)))
                    .map(str::to_string)
                    .ok_or_else(|| "types.formats must contain supported extension strings".to_string())
            })
            .collect::<Result<Vec<_>, _>>()?;
        Self::specific(formats)
    }

    pub fn raw() -> Self {
        Self { raw: true, formats: None }
    }

    pub fn specific(formats: Vec<String>) -> Result<Self, String> {
        if formats.len() > EXTENSIONS.len() {
            return Err("too many import file types".into());
        }
        let mut normalized = Vec::with_capacity(formats.len());
        for format in formats {
            if !EXTENSIONS.iter().any(|extension| extension.eq_ignore_ascii_case(&format)) {
                return Err("unsupported import file type".into());
            }
            let format = format.to_ascii_lowercase();
            normalized.push(normalize(&format).to_string());
        }
        normalized.sort();
        normalized.dedup();
        Ok(Self { raw: false, formats: Some(normalized) })
    }

    pub fn is_all(&self) -> bool {
        !self.raw && self.formats.is_none()
    }

    pub fn is_raw(&self) -> bool {
        self.raw
    }

    /// Canonical choices for the UI, including supported types absent from the current scan.
    pub fn supported_formats() -> Vec<&'static str> {
        let mut formats: Vec<_> = EXTENSIONS.iter().copied().map(normalize).collect();
        formats.sort();
        formats.dedup();
        formats
    }

    pub fn includes_extension(&self, extension: &str) -> bool {
        let extension = extension.to_ascii_lowercase();
        let extension = normalize(&extension);
        if self.raw { RAW.contains(&extension) } else { self.formats.as_ref().is_none_or(|formats| formats.iter().any(|format| format == extension)) }
    }

    /// Extension matching only: no filesystem work or probing, including on a UI frame.
    pub fn includes_path(&self, path: &str) -> bool {
        self.is_all() || std::path::Path::new(path).extension().and_then(|extension| extension.to_str()).is_some_and(|e| self.includes_extension(e))
    }
}

impl TryFrom<Value> for ImportTypes {
    type Error = String;

    fn try_from(value: Value) -> Result<Self, Self::Error> {
        Self::parse(&value)
    }
}

impl From<ImportTypes> for Value {
    fn from(types: ImportTypes) -> Self {
        if types.raw {
            json!({"group": "raw"})
        } else if let Some(formats) = types.formats {
            json!({"formats": formats})
        } else {
            json!({"group": "all"})
        }
    }
}

fn normalize(extension: &str) -> &str {
    match extension {
        "jpeg" => "jpg",
        "tiff" => "tif",
        other => other,
    }
}
