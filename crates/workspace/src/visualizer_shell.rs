//! Configuration-only shell state introduced by workspace18. No live identities.
use crate::workspace::{Error, ViewConfig, ViewKind};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layout {
    pub topics_open: bool,
    pub global_options_open: bool,
    pub companions_open: bool,
    pub companion_percent: u8,
    pub selected_companion: Option<String>,
    pub expanded_displays: Vec<String>,
}
impl Default for Layout {
    fn default() -> Self {
        Self {
            topics_open: false,
            global_options_open: false,
            companions_open: false,
            companion_percent: 30,
            selected_companion: None,
            expanded_displays: Vec::new(),
        }
    }
}
impl Layout {
    pub fn validate(&self, views: &[ViewConfig]) -> Result<(), Error> {
        let mut seen = HashSet::new();
        if !(15..=40).contains(&self.companion_percent)
            || self.expanded_displays.len() > crate::workspace::MAX_VISUALIZER_VIEWS
        {
            return Err(Error::Invalid(
                "Companion height must be 15–40 percent; at most 32 expanded displays".into(),
            ));
        }
        for id in &self.expanded_displays {
            if !seen.insert(id) || !views.iter().any(|v| &v.id == id) {
                return Err(Error::Invalid(
                    "Expanded properties must name distinct configured displays".into(),
                ));
            }
        }
        if self.selected_companion.as_ref().is_some_and(|id| {
            !views.iter().any(|v| {
                &v.id == id && matches!(v.kind, ViewKind::Image { .. } | ViewKind::Camera { .. })
            })
        }) {
            return Err(Error::Invalid(
                "Selected companion must name a configured Image or Camera".into(),
            ));
        }
        Ok(())
    }
}
