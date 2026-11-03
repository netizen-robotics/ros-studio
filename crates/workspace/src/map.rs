//! Pure saved Map intent. Runtime data and authorization never enter this model.
use crate::workspace::{Error, Qos};
use serde::{Deserialize, Serialize};
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Palette {
    #[default]
    Map,
    Costmap,
    Raw,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Appearance {
    pub palette: Palette,
    pub opacity_per_mille: u16,
    pub draw_behind: bool,
}
impl Default for Appearance {
    fn default() -> Self {
        Self {
            palette: Palette::Map,
            opacity_per_mille: 1000,
            draw_behind: false,
        }
    }
}
impl Appearance {
    pub fn validate(&self) -> Result<(), Error> {
        if self.opacity_per_mille > 1000 {
            return Err(Error::Invalid(
                "Map opacity must be in 0..1000 per mille".into(),
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateSource {
    pub topic: String,
    pub message_type: String,
    pub qos: Qos,
}
impl UpdateSource {
    pub fn validate(&self) -> Result<(), Error> {
        if !(studio_protocol::map_grid::UpdateSource {
            topic: self.topic.clone(),
            best_effort: false,
            transient_local: false,
        })
        .valid()
            || self.message_type != studio_protocol::map_grid::UPDATE_MESSAGE_TYPE
        {
            return Err(Error::Invalid(
                "Map updates require an absolute topic and map_msgs/msg/OccupancyGridUpdate".into(),
            ));
        }
        Ok(())
    }
}
