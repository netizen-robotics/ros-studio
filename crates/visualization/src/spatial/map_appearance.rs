//! Presentation-only map settings; they never change native source identity.
use super::combined::CombinedSceneError;
use serde::{Deserialize, Serialize};
pub use studio_workspace::map::Palette;
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MapAppearance {
    pub palette: Palette,
    pub opacity: f32,
    pub draw_behind: bool,
}
impl Default for MapAppearance {
    fn default() -> Self {
        Self {
            palette: Palette::Map,
            opacity: 1.,
            draw_behind: false,
        }
    }
}
impl MapAppearance {
    pub fn validate(&self) -> Result<(), CombinedSceneError> {
        if !self.opacity.is_finite() || !(0.0..=1.0).contains(&self.opacity) {
            return Err(CombinedSceneError::new(
                "Map opacity must be finite in 0..1",
            ));
        }
        Ok(())
    }
}

impl From<studio_workspace::map::Appearance> for MapAppearance {
    fn from(value: studio_workspace::map::Appearance) -> Self {
        Self {
            palette: value.palette,
            opacity: value.opacity_per_mille as f32 / 1000.,
            draw_behind: value.draw_behind,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn appearance_rejects_invalid_opacity_and_unknown_intent() {
        for opacity in [f32::NAN, f32::INFINITY, -0.1, 1.1] {
            assert!(MapAppearance {
                opacity,
                ..Default::default()
            }
            .validate()
            .is_err());
        }
        for opacity in [0., 0.5, 1.] {
            assert!(MapAppearance {
                opacity,
                ..Default::default()
            }
            .validate()
            .is_ok());
        }
        let value = serde_json::to_value(MapAppearance::default()).unwrap();
        assert_eq!(
            serde_json::from_value::<MapAppearance>(value.clone()).unwrap(),
            MapAppearance::default()
        );
        let mut bad = value;
        bad["verified"] = true.into();
        assert!(serde_json::from_value::<MapAppearance>(bad).is_err());
    }
}
