//! Pure draft/review inputs for explicit native Robot setup. No asset I/O.
use crate::workspace::RobotAssets;
use studio_protocol::{spatial::Configuration, spatial_input::SpatialClockPolicy};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RobotDraft {
    pub assets: RobotAssets,
    pub description: Option<crate::workspace::RobotTopicSource>,
    pub tf: Configuration,
    pub clock_verified: bool,
}
impl Default for RobotDraft {
    fn default() -> Self {
        Self {
            assets: RobotAssets::default(),
            description: None,
            tf: Configuration {
                fixed_frame: String::new(),
                clock_policy: SpatialClockPolicy::Unknown,
                dynamic_tf_topic: "/tf".into(),
                dynamic_tf_best_effort: false,
                static_tf_topic: "/tf_static".into(),
            },
            clock_verified: false,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RobotReview {
    pub name: String,
    pub links: usize,
    pub visuals: usize,
    pub collisions: usize,
    pub triangles: usize,
}
impl RobotReview {
    pub fn from_loaded(loaded: &super::LoadedRobot) -> Self {
        let parts = loaded.model.links.iter().flat_map(|link| &link.visuals);
        Self {
            name: loaded.model.name.clone(),
            links: loaded.model.links.len(),
            visuals: parts
                .clone()
                .filter(|v| v.role == super::GeometryRole::Visual)
                .count(),
            collisions: parts
                .filter(|v| v.role == super::GeometryRole::Collision)
                .count(),
            triangles: loaded.prepared.triangle_count,
        }
    }
}
pub struct RobotSetupState<'a> {
    pub busy: bool,
    pub review: Option<&'a RobotReview>,
    pub assets_match: bool,
    pub error: Option<&'a str>,
    pub target_active: bool,
    pub is_replacement: bool,
    pub connected: bool,
    pub tf_topics: &'a [String],
    pub description_topics: &'a [String],
    pub operation: Option<&'a str>,
    pub source_summary: Option<&'a str>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RobotSetupAction {
    Load,
    CancelLoad,
    Apply,
    Cancel,
}
