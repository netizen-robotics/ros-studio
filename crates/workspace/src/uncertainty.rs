//! Saved local presentation intent. Source covariance and clock authority stay upstream.
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Appearance {
    pub position: bool,
    pub orientation: bool,
    pub history_covariance: bool,
    pub sigma_milli: u16,
    pub pose_length_mm: u16,
    pub line_width_px: u8,
}
impl Default for Appearance {
    fn default() -> Self {
        Self {
            position: true,
            orientation: true,
            history_covariance: false,
            sigma_milli: 1000,
            pose_length_mm: 350,
            line_width_px: 2,
        }
    }
}
impl Appearance {
    pub fn validate(self) -> Result<(), String> {
        if !(100..=6000).contains(&self.sigma_milli)
            || !(10..=5000).contains(&self.pose_length_mm)
            || !(1..=8).contains(&self.line_width_px)
        {
            return Err(
                "Uncertainty needs0.1–6 standard deviations,10–5000mm pose size and1–8px lines"
                    .into(),
            );
        }
        Ok(())
    }
}
