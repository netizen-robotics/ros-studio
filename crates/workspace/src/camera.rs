//! Calibrated-camera presentation intent, independent of live source authority.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImagePlacement {
    #[default]
    Background,
    Overlay,
    Both,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Appearance {
    pub placement: ImagePlacement,
    pub overlay_alpha_milli: u16,
    pub zoom_milli: u16,
}
impl Default for Appearance {
    fn default() -> Self {
        Self {
            placement: ImagePlacement::Background,
            overlay_alpha_milli: 500,
            zoom_milli: 1000,
        }
    }
}
impl Appearance {
    pub fn validate(self) -> Result<(), String> {
        if self.overlay_alpha_milli > 1000 || !(100..=10_000).contains(&self.zoom_milli) {
            return Err("Camera needs 0–100% overlay opacity and 0.1–10 zoom".into());
        }
        Ok(())
    }
}
