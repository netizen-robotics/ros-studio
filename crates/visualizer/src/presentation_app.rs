//! Convert presentation only. Restoring a camera never grants clock permission
//! or starts reception, including in empty/Image-only application documents.
use super::*;
use studio_workspace::presentation::{SceneSettings, VisualizerPresentation};

impl App {
    pub(super) fn presentation_snapshot(&self) -> VisualizerPresentation {
        let scene = &self.scene_settings.configuration;
        VisualizerPresentation {
            shell: studio_workspace::visualizer_shell::Layout {
                topics_open: self.topics_tab,
                global_options_open: self.global_options_open,
                companions_open: self.companion_open,
                companion_percent: self.companion_percent,
                selected_companion: self
                    .companion_selected
                    .and_then(|id| {
                        self.views.iter().find(|v| {
                            v.id == id && matches!(v.kind, ViewKind::Image | ViewKind::Camera)
                        })
                    })
                    .map(|v| v.workspace_id.clone()),
                expanded_displays: self
                    .expanded_displays
                    .iter()
                    .filter(|id| self.views.iter().any(|v| &v.workspace_id == *id))
                    .cloned()
                    .collect(),
            },
            scene: studio_protocol::spatial::valid_frame(&scene.fixed_frame).then(|| {
                SceneSettings {
                    fixed_frame: scene.fixed_frame.clone(),
                    dynamic_tf_topic: scene.dynamic_tf_topic.clone(),
                    static_tf_topic: scene.static_tf_topic.clone(),
                    dynamic_tf_best_effort: scene.dynamic_tf_best_effort,
                }
            }),
            camera: self.shared_scene.camera.snapshot(),
            saved_views: self.shared_scene.saved_views.clone(),
            sensors: self
                .views
                .iter()
                .filter(|v| {
                    matches!(v.kind, ViewKind::Spatial | ViewKind::PointCloud)
                        && v.sensor_appearance_explicit
                })
                .map(|v| studio_workspace::presentation::SensorPresentation {
                    view: v.workspace_id.clone(),
                    appearance: v.sensor_appearance,
                })
                .collect(),
            robots: self
                .views
                .iter()
                .filter(|view| {
                    matches!(view.kind, ViewKind::Robot)
                        && view.robot_appearance != Default::default()
                })
                .map(|view| studio_workspace::presentation::RobotPresentation {
                    view: view.workspace_id.clone(),
                    appearance: view.robot_appearance.clone(),
                })
                .collect(),
        }
    }
    pub(super) fn restore_presentation(
        &mut self,
        presentation: Option<&VisualizerPresentation>,
    ) -> Result<(), String> {
        let defaults = VisualizerPresentation::default();
        let presentation = presentation.unwrap_or(&defaults);
        presentation.validate().map_err(|e| e.to_string())?;
        if let Some(scene) = &presentation.scene {
            self.scene_settings.configuration = studio_protocol::spatial::Configuration {
                fixed_frame: scene.fixed_frame.clone(),
                dynamic_tf_topic: scene.dynamic_tf_topic.clone(),
                static_tf_topic: scene.static_tf_topic.clone(),
                dynamic_tf_best_effort: scene.dynamic_tf_best_effort,
                clock_policy: studio_protocol::spatial_input::SpatialClockPolicy::Unknown,
            };
        }
        self.scene_settings.draft = self.scene_settings.configuration.clone();
        self.scene_settings.verified = true;
        self.scene_settings.draft_verified = true;
        if self.connection.phase == Phase::Connected {
            self.use_system_scene_defaults();
        }
        self.shared_scene.camera.restore(&presentation.camera)?;
        self.shared_scene.saved_views = presentation.saved_views.clone();
        self.topics_tab = presentation.shell.topics_open;
        self.global_options_open = presentation.shell.global_options_open;
        self.companion_open = presentation.shell.companions_open;
        self.companion_percent = presentation.shell.companion_percent;
        self.companion_layout_revision = self.companion_layout_revision.wrapping_add(1);
        self.companion_was_open = self.companion_open;
        self.expanded_displays = presentation
            .shell
            .expanded_displays
            .iter()
            .cloned()
            .collect();
        self.companion_selected = presentation
            .shell
            .selected_companion
            .as_ref()
            .and_then(|id| self.views.iter().find(|v| &v.workspace_id == id))
            .map(|v| v.id);
        Ok(())
    }
}
