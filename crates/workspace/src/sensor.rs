//! Shared, finite sensor presentation intent. No readers, samples or clock grants.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ColorMode {
    #[default]
    Flat,
    Height,
    Rgb,
    Intensity,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SizeMode {
    #[default]
    Pixels,
    World,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SensorAppearance {
    pub history_ms: u16,
    pub size_mode: SizeMode,
    pub pixels: u8,
    pub diameter_mm: u16,
    pub opacity: u8,
    pub color: ColorMode,
    pub flat_rgb: [u8; 3],
    pub automatic_range: bool,
    pub height_mm: [i64; 2],
    pub intensity_milli: [i64; 2],
}
impl Default for SensorAppearance {
    fn default() -> Self {
        Self::for_id(1)
    }
}
impl SensorAppearance {
    pub fn for_id(id: u64) -> Self {
        let colors = [
            [39, 196, 221],
            [237, 144, 48],
            [184, 111, 234],
            [69, 199, 118],
        ];
        Self {
            history_ms: 0,
            size_mode: SizeMode::Pixels,
            pixels: 3,
            diameter_mm: 20,
            opacity: 255,
            color: ColorMode::Flat,
            flat_rgb: colors[(id.wrapping_sub(1) % 4) as usize],
            automatic_range: true,
            height_mm: [0, 2000],
            intensity_milli: [0, 1000],
        }
    }
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.history_ms > 10_000 {
            return Err("History must be Latest or at most 10 seconds");
        }
        if !(1..=32).contains(&self.pixels) || !(1..=1000).contains(&self.diameter_mm) {
            return Err("Point size must be 1–32 pixels or 1–1000 millimetres");
        }
        if self.height_mm[0] >= self.height_mm[1]
            || self.intensity_milli[0] >= self.intensity_milli[1]
        {
            return Err("Maximum must be greater than minimum");
        }
        if self
            .height_mm
            .iter()
            .any(|v| !(-1_000_000_000..=1_000_000_000).contains(v))
            || self
                .intensity_milli
                .iter()
                .any(|v| !(-1_000_000_000_000_000..=1_000_000_000_000_000).contains(v))
        {
            return Err("Color range exceeds the supported finite limits");
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_settings_reject_overflow_unknown_fields_and_invalid_inactive_ranges() {
        let settings = SensorAppearance::default();
        settings.validate().unwrap();
        for field in ["history_ms", "pixels", "diameter_mm", "opacity"] {
            let mut wire = serde_json::to_value(settings).unwrap();
            wire[field] = serde_json::json!(65536);
            assert!(serde_json::from_value::<SensorAppearance>(wire).is_err());
        }
        let mut wire = serde_json::to_value(settings).unwrap();
        wire["receive"] = true.into();
        assert!(serde_json::from_value::<SensorAppearance>(wire).is_err());
        let mut bad = settings;
        bad.height_mm = [i64::MIN, 1];
        assert!(bad.validate().is_err());
        bad = settings;
        bad.intensity_milli = [1, 1];
        assert!(bad.validate().is_err());
        bad = settings;
        bad.history_ms = 10_001;
        assert!(bad.validate().is_err());
    }
}
