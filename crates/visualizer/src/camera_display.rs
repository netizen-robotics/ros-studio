//! Camera application composition. Paired readers and conversion stay in the
//! shared actor/client libraries; this root owns explicit intent and visibility.
use super::*;
use std::sync::Arc;
use studio_client_core::camera_stream::{CameraStream, CameraUpdate};
use studio_protocol::camera_snapshot::{transport::Permission, Owner, Snapshot, Status};
use studio_workspace::camera::{Appearance, ImagePlacement};

#[derive(Default)]
pub struct CameraDisplay {
    pub stream: Option<CameraStream>,
    pub snapshot: Option<Arc<Snapshot>>,
    pub permission: Option<Permission>,
    pub image: Option<Arc<images::TypedImage>>,
    pub projection: Option<spatial::calibrated_camera::RectifiedProjection>,
    pub appearance: Appearance,
}
impl CameraDisplay {
    pub fn stop(&mut self) {
        if let Some(stream) = self.stream.take() {
            stream.stop();
        }
        self.clear();
    }
    fn clear(&mut self) {
        self.snapshot = None;
        self.permission = None;
        self.image = None;
        self.projection = None;
    }
    pub fn is_current(&self) -> bool {
        self.snapshot
            .as_ref()
            .zip(self.permission.as_ref())
            .is_some_and(|(s, p)| {
                s.status == Status::Receiving
                    && p.authorizes(s)
                    && self.image.is_some()
                    && self.projection.is_some()
            })
    }
}
pub(super) fn inferred_info(topic: &str) -> String {
    let prefix = topic.rsplit_once('/').map_or("", |(prefix, _)| prefix);
    format!("{prefix}/camera_info")
}
fn status_label(status: Status) -> &'static str {
    match status {
        Status::WaitingInfo => "Waiting for CameraInfo",
        Status::WaitingImage => "Waiting for a fresh rectified Image",
        Status::Uncalibrated => "CameraInfo is uncalibrated",
        Status::Incompatible => "Image and CameraInfo frames or dimensions disagree",
        Status::Unsupported => "Image encoding or calibration rectification is unsupported",
        Status::InvalidInfo => "Invalid CameraInfo; waiting for valid calibration",
        Status::InvalidImage => "Invalid Image; waiting for a valid image",
        Status::WaitingTf => "Waiting for Image acquisition-time TF",
        Status::TfTimeout => "Image transform timed out; waiting for a fresh image",
        Status::Receiving => "Receiving aligned rectified Image and scene",
    }
}
impl ViewState {
    pub(super) fn begin_camera(&mut self) -> Result<backend_pipe::CameraTarget, String> {
        let Output::CameraSnapshot(info) = &self.configuration.output else {
            return Err("Missing CameraInfo source".into());
        };
        let owner = Owner {
            generation: self.generation,
            subscription: self.subscription.ok_or("Missing Camera subscription")?,
            topic: self.configuration.topic.clone(),
            info: (**info).clone(),
            fixed_frame: self
                .configuration
                .spatial
                .as_ref()
                .ok_or("Missing Camera scene")?
                .fixed_frame
                .clone(),
        };
        let stream = CameraStream::new(owner.clone())?;
        self.camera.stop();
        self.camera.stream = Some(stream.clone());
        self.spatial_status = "Waiting for CameraInfo and a fresh rectified Image".into();
        Ok(backend_pipe::CameraTarget { owner, stream })
    }
    fn poll_camera(&mut self) -> Result<(), String> {
        if let Some(result) = self.camera.stream.as_ref().and_then(CameraStream::take) {
            match result? {
                CameraUpdate::Snapshot {
                    snapshot,
                    permission,
                    image,
                    projection,
                } => {
                    self.spatial_status = status_label(snapshot.status).into();
                    if let Some(diagnostic) = &snapshot.diagnostic {
                        self.spatial_status.push_str(" · ");
                        self.spatial_status.push_str(diagnostic);
                    }
                    self.received.typed_samples = snapshot.counters.images;
                    self.status = if snapshot.status == Status::Receiving
                        && permission.authorizes(&snapshot)
                    {
                        ViewStatus::Receiving
                    } else {
                        ViewStatus::Waiting
                    };
                    self.camera.snapshot = Some(snapshot);
                    self.camera.permission = Some(permission);
                    self.camera.image = image;
                    self.camera.projection = projection;
                    self.error = None;
                    self.deadline = None;
                }
                CameraUpdate::Invalidated(_) => {
                    self.camera.clear();
                    self.status = ViewStatus::Waiting;
                    self.spatial_status =
                        "Camera alignment changed; waiting for a fresh image".into();
                }
            }
        }
        if let Some(stream) = &self.camera.stream {
            self.queue_drops = stream.coalesced();
        }
        if self
            .camera
            .permission
            .as_ref()
            .is_some_and(|p| !p.is_valid())
        {
            self.camera.clear();
            if self.status == ViewStatus::Receiving {
                self.status = ViewStatus::Waiting;
            }
        }
        Ok(())
    }
}
impl App {
    pub(super) fn poll_cameras(&mut self) {
        let mut errors = Vec::new();
        for view in &mut self.views {
            if matches!(view.kind, ViewKind::Camera)
                && (view.status.active() || view.status == ViewStatus::Starting)
            {
                if let Err(error) = view.poll_camera() {
                    errors.push((view.id, error));
                }
            }
        }
        for (id, error) in errors {
            self.stop_view(id);
            if let Some(view) = self.views.iter_mut().find(|v| v.id == id) {
                view.error = Some(error.clone());
            }
            self.diagnostic(error);
        }
        if !self
            .views
            .iter()
            .any(|v| v.enabled && v.camera.is_current())
        {
            self.camera_canvas.clear();
            if let Some(renderer) = &mut self.camera_renderer {
                renderer.release_frame();
            }
        }
    }
    pub(super) fn camera_source_draft_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let Output::CameraSnapshot(info) = &mut self.draft_output else {
            return true;
        };
        ui.separator();
        studio_ui_foundation::theme::strong(ui, "Rectified camera sources");
        ui.small("Choose an already rectified Image. Raw-image undistortion is unavailable.");
        let label = ui.label("CameraInfo topic");
        let discovered = egui::ComboBox::from_id_salt("discovered-camera-info")
            .selected_text("Choose discovered CameraInfo…")
            .show_ui(ui, |ui| {
                for (topic, types) in &self.discovery.topics {
                    if types.iter().any(|ty| ty == "sensor_msgs/msg/CameraInfo") {
                        ui.selectable_value(&mut info.topic, topic.clone(), topic);
                    }
                }
                ui.small("You can also enter a topic below.");
            });
        crate::ui::describe_choice(
            &discovered.response,
            "Discovered CameraInfo",
            "Choose a discovered topic",
        );
        ui.add(studio_ui_foundation::theme::input(ui, &mut info.topic))
            .labelled_by(label.id);
        ui.collapsing("CameraInfo QoS", |ui| {
            ui.checkbox(&mut info.best_effort, "CameraInfo best effort");
            ui.checkbox(&mut info.transient_local, "CameraInfo transient local");
        });
        let valid = info.validate().is_ok()
            && info.topic != self.discovery.topic
            && info.topic != self.scene_settings.configuration.dynamic_tf_topic
            && info.topic != self.scene_settings.configuration.static_tf_topic;
        if !valid {
            ui.colored_label(
                ui.visuals().error_fg_color,
                "Choose a valid CameraInfo topic distinct from Image and TF.",
            );
        }
        valid
    }
    pub(super) fn camera_inspector_ui(&mut self, ui: &mut egui::Ui, index: usize, frozen: bool) {
        let view = &mut self.views[index];
        ui.separator();
        studio_ui_foundation::theme::strong(ui, "Rectified Camera");
        ui.label(&view.spatial_status);
        if let Output::CameraSnapshot(info) = &view.configuration.output {
            ui.monospace(&info.topic);
        }
        ui.add_enabled_ui(!frozen, |ui| {
            let a = &mut view.camera.appearance;
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut a.placement, ImagePlacement::Background, "Background");
                ui.selectable_value(&mut a.placement, ImagePlacement::Overlay, "Overlay");
                ui.selectable_value(&mut a.placement, ImagePlacement::Both, "Both");
            });
            if a.placement != ImagePlacement::Background {
                ui.add(studio_ui_foundation::theme::slider(
                    egui::Slider::new(&mut a.overlay_alpha_milli, 0..=1000)
                        .text("Overlay opacity")
                        .custom_formatter(|value, _| format!("{:.1}%", value / 10.0)),
                ));
            }
            ui.add(studio_ui_foundation::theme::slider(
                egui::Slider::new(&mut a.zoom_milli, 100..=10000)
                    .logarithmic(true)
                    .text("Zoom")
                    .custom_formatter(|value, _| format!("{:.2}×", value / 1000.0)),
            ));
        });
        if let Some(snapshot) = &view.camera.snapshot {
            let c = snapshot.counters;
            ui.small(format!(
                "{} Images · {} calibrations · {} pending replaced · {} TF timeouts",
                c.images, c.calibrations, c.pending_dropped, c.lookup_timeouts
            ));
            if let Some(frame) = &snapshot.frame {
                ui.monospace(format!(
                    "{}×{} · {}",
                    frame.image.metadata.width,
                    frame.image.metadata.height,
                    frame.image.metadata.frame_id
                ));
                ui.small(
                    if frame.image.metadata.header_stamp.seconds == 0
                        && frame.image.metadata.header_stamp.nanoseconds == 0
                    {
                        "Zero stamp: explicitly latest TF"
                    } else {
                        "Image acquisition-time TF"
                    },
                );
            }
        }
    }
}
