//! Pure, bounded Visualizer presentation. No runtime authorization or handles.
use crate::workspace::{Error, Workspace};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraMode {
    #[default]
    Orbit,
    TopDown,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CameraState {
    pub mode: CameraMode,
    pub azimuth_microrad: i32,
    pub elevation_microrad: i32,
    pub target_mm: [i64; 3],
    pub distance_mm: u32,
    pub vertical_span_mm: u32,
}
impl Default for CameraState {
    fn default() -> Self {
        Self {
            mode: CameraMode::Orbit,
            azimuth_microrad: 500_000,
            elevation_microrad: 800_000,
            target_mm: [0; 3],
            distance_mm: 12_000,
            vertical_span_mm: 12_000,
        }
    }
}
impl CameraState {
    pub fn validate(&self) -> Result<(), Error> {
        if !(0..=6_283_185).contains(&self.azimuth_microrad)
            || !(-1_500_000..=1_500_000).contains(&self.elevation_microrad)
            || self
                .target_mm
                .iter()
                .any(|v| !(-1_000_000_000..=1_000_000_000).contains(v))
            || !(200..=2_000_000).contains(&self.distance_mm)
            || !(200..=2_000_000).contains(&self.vertical_span_mm)
        {
            return Err(Error::Invalid(
                "Camera settings exceed the finite angle/target/distance/span limits".into(),
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SavedView {
    pub name: String,
    pub camera: CameraState,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneSettings {
    pub fixed_frame: String,
    pub dynamic_tf_topic: String,
    pub static_tf_topic: String,
    pub dynamic_tf_best_effort: bool,
}
impl SceneSettings {
    pub fn validate(&self) -> Result<(), Error> {
        // Reuse wire structure validation without serializing its runtime clock policy.
        use studio_protocol::{
            spatial_input::SpatialClockPolicy,
            subscriptions::{Configuration, Output},
        };
        let c = Configuration {
            map_updates: None,
            topic: "/scene_validation".into(),
            message_type: "sensor_msgs/msg/LaserScan".into(),
            best_effort: false,
            transient_local: false,
            output: Output::SpatialScan,
            robot: None,
            spatial: Some(studio_protocol::spatial::Configuration {
                fixed_frame: self.fixed_frame.clone(),
                dynamic_tf_topic: self.dynamic_tf_topic.clone(),
                static_tf_topic: self.static_tf_topic.clone(),
                dynamic_tf_best_effort: self.dynamic_tf_best_effort,
                clock_policy: SpatialClockPolicy::VerifiedSystem,
            }),
        };
        if !c.valid() {
            return Err(Error::Invalid(
                "Scene presentation requires a valid frame and distinct TF topics".into(),
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VisualizerPresentation {
    /// Kept outside the frozen presentation grammar, in workspace18's shell field.
    #[serde(skip)]
    pub shell: crate::visualizer_shell::Layout,
    pub scene: Option<SceneSettings>,
    pub camera: CameraState,
    pub saved_views: Vec<SavedView>,
    pub robots: Vec<RobotPresentation>,
    pub sensors: Vec<SensorPresentation>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SensorPresentation {
    pub view: String,
    pub appearance: crate::sensor::SensorAppearance,
}
/// Frozen v12 grammar. Sensor settings must never leak into older versions.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LegacyV12Presentation {
    pub scene: Option<SceneSettings>,
    pub camera: CameraState,
    pub saved_views: Vec<SavedView>,
    pub robots: Vec<RobotPresentation>,
}
impl From<LegacyV12Presentation> for VisualizerPresentation {
    fn from(old: LegacyV12Presentation) -> Self {
        Self {
            shell: Default::default(),
            scene: old.scene,
            camera: old.camera,
            saved_views: old.saved_views,
            robots: old.robots,
            sensors: Vec::new(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RobotPresentation {
    pub view: String,
    pub appearance: crate::workspace::RobotAppearance,
}
/// Frozen presentation grammar used by Visualizer v9–v11 imports.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct LegacyVisualizerPresentation {
    pub scene: Option<SceneSettings>,
    pub camera: CameraState,
    pub saved_views: Vec<SavedView>,
}
impl From<LegacyVisualizerPresentation> for VisualizerPresentation {
    fn from(old: LegacyVisualizerPresentation) -> Self {
        Self {
            shell: Default::default(),
            scene: old.scene,
            camera: old.camera,
            saved_views: old.saved_views,
            robots: Vec::new(),
            sensors: Vec::new(),
        }
    }
}
impl VisualizerPresentation {
    pub fn validate(&self) -> Result<(), Error> {
        let mut sensors = HashSet::new();
        if self.sensors.len() > crate::workspace::MAX_VISUALIZER_VIEWS {
            return Err(Error::Invalid(
                "At most 32 sensor appearance configurations are supported".into(),
            ));
        }
        for sensor in &self.sensors {
            if sensor.view.is_empty()
                || sensor.view.len() > 64
                || sensor.view.chars().any(char::is_control)
                || !sensors.insert(&sensor.view)
            {
                return Err(Error::Invalid(
                    "Sensor appearance view identifiers must be unique and bounded".into(),
                ));
            }
            sensor
                .appearance
                .validate()
                .map_err(|message| Error::Invalid(message.into()))?;
        }
        let mut robots = HashSet::new();
        if self.robots.len() > crate::workspace::MAX_VISUALIZER_VIEWS {
            return Err(Error::Invalid(
                "At most 32 Robot appearance configurations are supported".into(),
            ));
        }
        for robot in &self.robots {
            if robot.view.is_empty()
                || robot.view.len() > 64
                || robot.view.chars().any(char::is_control)
                || !robots.insert(&robot.view)
            {
                return Err(Error::Invalid(
                    "Robot appearance view identifiers must be unique and bounded".into(),
                ));
            }
            robot.appearance.validate()?;
        }
        self.camera.validate()?;
        if let Some(scene) = &self.scene {
            scene.validate()?;
        }
        if self.saved_views.len() > 8 {
            return Err(Error::Invalid(
                "At most eight camera views can be saved".into(),
            ));
        }
        let mut names = HashSet::new();
        for view in &self.saved_views {
            if view.name.trim().is_empty()
                || view.name.len() > 64
                || view.name.chars().any(char::is_control)
                || !names.insert(&view.name)
            {
                return Err(Error::Invalid("Saved view names must be unique and contain 1–64 UTF-8 bytes without control characters".into()));
            }
            view.camera.validate()?;
        }
        Ok(())
    }
    pub fn legacy(workspace: &Workspace) -> Self {
        use crate::workspace::ViewKind;
        let scene = workspace.views.iter().find_map(|v| match &v.kind {
            ViewKind::TfGraph {
                fixed_frame,
                transforms: tf,
                ..
            } => Some(SceneSettings {
                fixed_frame: fixed_frame.clone(),
                dynamic_tf_topic: tf.dynamic_tf.topic.clone(),
                static_tf_topic: tf.static_tf.topic.clone(),
                dynamic_tf_best_effort: tf.dynamic_tf.qos.reliability
                    == crate::workspace::Reliability::BestEffort,
            }),
            ViewKind::Spatial {
                fixed_frame,
                transforms: Some(tf),
                ..
            }
            | ViewKind::PointCloud {
                fixed_frame,
                transforms: tf,
                ..
            }
            | ViewKind::Camera {
                fixed_frame,
                transforms: tf,
                ..
            }
            | ViewKind::Uncertainty {
                fixed_frame,
                transforms: tf,
                ..
            }
            | ViewKind::Markers {
                fixed_frame,
                transforms: tf,
                ..
            }
            | ViewKind::Map {
                fixed_frame,
                transforms: tf,
                ..
            }
            | ViewKind::OccupancyGrid {
                fixed_frame,
                transforms: tf,
                ..
            }
            | ViewKind::Pose {
                fixed_frame,
                transforms: tf,
                ..
            }
            | ViewKind::Path {
                fixed_frame,
                transforms: tf,
                ..
            }
            | ViewKind::RobotTopic {
                fixed_frame,
                transforms: tf,
                ..
            }
            | ViewKind::Robot {
                fixed_frame,
                transforms: tf,
                ..
            } => Some(SceneSettings {
                fixed_frame: fixed_frame.clone(),
                dynamic_tf_topic: tf.dynamic_tf.topic.clone(),
                static_tf_topic: tf.static_tf.topic.clone(),
                dynamic_tf_best_effort: tf.dynamic_tf.qos.reliability
                    == crate::workspace::Reliability::BestEffort,
            }),
            _ => None,
        });
        Self {
            scene,
            ..Default::default()
        }
    }
}
