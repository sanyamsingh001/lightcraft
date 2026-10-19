//! One owned UI face and request lifecycle. Disabled for native hosts by default.
use crate::i18n::Locale;
use std::sync::Arc;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum FontStatus {
    #[default]
    Disabled,
    Idle,
    Requested,
    Loading,
    Ready,
    Failed(String),
}

#[derive(Default)]
pub struct RuntimeUiFont {
    status: FontStatus,
    data: Option<Arc<egui::FontData>>,
    revision: u64,
}

impl RuntimeUiFont {
    pub fn enable(&mut self) {
        if self.status == FontStatus::Disabled {
            self.status = FontStatus::Idle;
        }
    }
    pub fn status(&self) -> &FontStatus {
        &self.status
    }
    pub fn data(&self) -> Option<&Arc<egui::FontData>> {
        self.data.as_ref()
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    /// Automatic startup/picker requests do not retry failures. A language click is explicit.
    pub fn request(&mut self, retry: bool) -> bool {
        if self.status == FontStatus::Idle || (retry && matches!(self.status, FontStatus::Failed(_))) {
            self.status = FontStatus::Requested;
            return true;
        }
        false
    }
    pub fn take_request(&mut self) -> bool {
        if self.status != FontStatus::Requested {
            return false;
        }
        self.status = FontStatus::Loading;
        true
    }
    pub fn finish(&mut self, result: Result<egui::FontData, String>) {
        if self.status != FontStatus::Loading {
            return;
        }
        match result {
            Ok(data) => {
                self.data = Some(Arc::new(data));
                self.revision = self.revision.wrapping_add(1);
                self.status = FontStatus::Ready;
            }
            Err(error) => self.status = FontStatus::Failed(error),
        }
    }
    /// Never change the persisted preference while the requested face is unavailable.
    pub fn display_language(&self, preferred: Locale) -> Locale {
        if self.status != FontStatus::Disabled && self.data.is_none() && chinese(preferred) { Locale::En } else { preferred }
    }
    pub fn language_name(&self, language: Locale) -> &'static str {
        if self.status != FontStatus::Disabled && self.data.is_none() {
            match language {
                Locale::ZhHans => return "Simplified Chinese",
                Locale::ZhHant => return "Traditional Chinese",
                _ => {}
            }
        }
        language.name()
    }
}
pub fn chinese(language: Locale) -> bool {
    matches!(language, Locale::ZhHans | Locale::ZhHant)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pending_and_failed_keep_preference_and_need_an_explicit_retry() {
        let mut font = RuntimeUiFont::default();
        assert_eq!(font.display_language(Locale::ZhHans), Locale::ZhHans);
        font.enable();
        font.request(false);
        assert!(font.take_request());
        font.request(true);
        assert!(!font.take_request());
        assert_eq!(font.display_language(Locale::ZhHant), Locale::En);
        font.finish(Err("offline".into()));
        font.request(false);
        assert!(!font.take_request());
        font.request(true);
        assert!(font.take_request());
        font.finish(Ok(egui::FontData::from_owned(vec![])));
        assert_eq!(font.display_language(Locale::ZhHant), Locale::ZhHant);
        assert_eq!(font.display_language(Locale::Ja), Locale::Ja);
        font.request(true);
        assert!(!font.take_request());
        assert_eq!(font.revision(), 1);
    }
}
