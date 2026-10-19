//! Conditional keyword filter transitions for library-wide metadata history.

use crate::Session;

/// One automatically followed keyword filter, in the direction of its history step.
/// Only this field is tracked; other filters and library sources remain the user's choices.
#[derive(Clone, Debug)]
pub(crate) struct KeywordFilterTransition {
    pub from: Option<String>,
    pub to: Option<String>,
}

impl KeywordFilterTransition {
    pub(crate) fn reversed(&self) -> Self {
        Self { from: self.to.clone(), to: self.from.clone() }
    }
}

impl Session {
    pub(crate) fn apply_history_keyword_filter(&mut self, transition: Option<&KeywordFilterTransition>) {
        if let Some(t) = transition
            && self.filter.keyword == t.from
        {
            self.filter.keyword.clone_from(&t.to);
        }
    }
}
