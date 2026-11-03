//! Map delivery is owned by one display. Reception/geometry stays on the reader.
use super::*;
use std::sync::Arc;
use studio_protocol::{
    map_grid::Metadata,
    map_snapshot::{ClearReason, Owner, SourceStatus},
};
#[derive(Default)]
pub struct MapDisplay {
    pub stream: Option<backend_pipe::MapStream>,
    pub frame: Option<Arc<spatial::map_texture::TextureMap>>,
    pub appearance: studio_workspace::map::Appearance,
    pub metadata: Option<Metadata>,
    pub source_status: Option<SourceStatus>,
    pub clear_reason: Option<ClearReason>,
    pub revision: u64,
}
impl MapDisplay {
    pub fn stop(&mut self) {
        if let Some(stream) = self.stream.take() {
            stream.stop();
        }
        self.frame = None;
        self.metadata = None;
        self.source_status = None;
        self.clear_reason = None;
        self.revision = 0;
    }
}
impl ViewState {
    pub fn begin_map(&mut self) -> Result<backend_pipe::MapTarget, String> {
        self.map.stop();
        let owner = Owner {
            generation: self.generation,
            subscription: self.subscription.ok_or("Map owner is not allocated")?,
            topic: self.configuration.topic.clone(),
            fixed_frame: self
                .configuration
                .spatial
                .as_ref()
                .ok_or("Map scene is not configured")?
                .fixed_frame
                .clone(),
        };
        let stream =
            backend_pipe::MapStream::new(owner.clone(), self.configuration.map_updates.is_some())?;
        self.map.stream = Some(stream.clone());
        self.spatial_status = "Waiting for a complete map and acquisition-time transform…".into();
        Ok(backend_pipe::MapTarget { owner, stream })
    }
    pub fn poll_map(&mut self) -> Result<bool, String> {
        use backend_pipe::PreparedMapUpdate;
        let Some(result) = self
            .map
            .stream
            .as_ref()
            .and_then(|stream| stream.take_prepared())
        else {
            return Ok(false);
        };
        match result? {
            PreparedMapUpdate::Complete(map) => {
                self.map.revision = map.complete().header.revision;
                self.map.metadata = Some(map.complete().header.metadata.clone());
                self.map.source_status = Some(map.complete().header.source_status.clone());
                self.map.clear_reason = None;
                self.received.typed_samples =
                    map.complete().header.source_counters.messages_received;
                self.spatial_status = "Receiving · acquisition-time transform ready".into();
                self.spatial_details = format!(
                    "{} × {} cells · {:.4} m/cell · {} → {}",
                    map.dimensions()[0],
                    map.dimensions()[1],
                    map.complete().header.metadata.resolution,
                    map.complete().header.metadata.frame_id,
                    map.complete().header.owner.fixed_frame
                );
                self.map.frame = Some(map);
                self.status = ViewStatus::Receiving;
                self.error = None;
            }
            PreparedMapUpdate::Cleared(clear) => {
                self.map.frame = None;
                self.map.revision = clear.revision;
                self.map.metadata = clear.metadata.clone();
                self.map.source_status = Some(clear.source_status.clone());
                self.map.clear_reason = Some(clear.reason);
                self.spatial_status = match clear.reason {
                    ClearReason::WaitingFull => "Waiting for a full map before applying updates",
                    ClearReason::InvalidInput => {
                        "Invalid map/update · waiting for a fresh full map"
                    }
                    ClearReason::QueueOverflow => {
                        "Map continuity lost · waiting for a fresh full map"
                    }
                    ClearReason::SourceFailure => "Source failed · waiting for a fresh full map",
                    ClearReason::MissingTransform => {
                        "Waiting for the map's acquisition-time transform"
                    }
                    ClearReason::InvalidTransform => "Invalid transform · map hidden",
                    ClearReason::Extrapolated => "Transform time unavailable · map hidden",
                    ClearReason::LookupTimeout => "Transform lookup timed out · map hidden",
                    ClearReason::Clear => "Map cleared · waiting for a full map",
                }
                .into();
                self.spatial_details.clear();
                self.status = ViewStatus::Waiting;
            }
        }
        self.deadline = None;
        Ok(true)
    }
}
impl App {
    pub(super) fn prune_scene_resources(&mut self) {
        let Some(renderer) = &mut self.spatial_renderer else {
            return;
        };
        if !self
            .views
            .iter()
            .any(|v| v.enabled && v.configuration.owns_tf())
        {
            renderer.clear_texture_map();
            renderer.clear_marker_primitives();
        } else if !self.views.iter().any(|v| {
            v.enabled && v.configuration.output == Output::MapSnapshot && v.map.frame.is_some()
        }) {
            renderer.clear_texture_map();
        }
    }
}

impl App {
    pub(super) fn map_updates_available(&self, configuration: &Configuration) -> bool {
        configuration.map_updates.as_ref().is_none_or(|source| {
            self.discovery.topics.iter().any(|(topic, types)| {
                topic == &source.topic
                    && types
                        .iter()
                        .any(|ty| ty == studio_protocol::map_grid::UPDATE_MESSAGE_TYPE)
            })
        })
    }
    pub(super) fn map_update_draft_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let mut enabled = self.draft_map_updates.is_some();
        ui.separator();
        if ui.checkbox(&mut enabled, "Receive map updates").changed() {
            self.draft_map_updates = enabled.then(|| studio_protocol::map_grid::UpdateSource {
                topic: String::new(),
                best_effort: false,
                transient_local: false,
            });
        }
        let Some(source) = &mut self.draft_map_updates else {
            ui.small(
                "Full maps only. Enable updates for a publisher that provides OccupancyGridUpdate.",
            );
            return true;
        };
        let caption = ui.label("Update topic");
        let response = egui::ComboBox::from_id_salt("map-update-topic")
            .selected_text(if source.topic.is_empty() {
                "Choose an update topic"
            } else {
                &source.topic
            })
            .width((ui.available_width() - 12.).max(1.))
            .truncate()
            .show_ui(ui, |ui| {
                for (topic, types) in &self.discovery.topics {
                    if types
                        .iter()
                        .any(|ty| ty == studio_protocol::map_grid::UPDATE_MESSAGE_TYPE)
                        && ui
                            .selectable_value(&mut source.topic, topic.clone(), topic)
                            .clicked()
                    {
                        ui.close_menu();
                    }
                }
            })
            .response
            .labelled_by(caption.id);
        crate::ui::describe_choice(&response, "Update topic", &source.topic);
        response.on_hover_text(&source.topic);
        ui.small(studio_protocol::map_grid::UPDATE_MESSAGE_TYPE);
        ui.collapsing("Update source QoS", |ui| {
            ui.checkbox(&mut source.best_effort, "Best effort updates");
            ui.checkbox(&mut source.transient_local, "Transient local updates");
            ui.small("Requested QoS; publisher compatibility is not discovered.");
        });
        ui.small("Updates have no shared base revision. Continuity may remain unknown; detected loss hides the map until a fresh full map arrives.");
        let valid = source.valid()
            && source.topic != self.discovery.topic
            && source.topic != self.scene_settings.configuration.dynamic_tf_topic
            && source.topic != self.scene_settings.configuration.static_tf_topic
            && self.discovery.topics.iter().any(|(topic, types)| {
                topic == &source.topic
                    && types
                        .iter()
                        .any(|ty| ty == studio_protocol::map_grid::UPDATE_MESSAGE_TYPE)
            });
        if !valid {
            ui.colored_label(
                ui.visuals().error_fg_color,
                "Choose a distinct, available update topic and exact type before applying.",
            );
        }
        valid
    }
}
pub(super) fn diagnostics(ui: &mut egui::Ui, view: &ViewState) {
    use studio_protocol::map_grid::Continuity;
    egui::CollapsingHeader::new("Map details").default_open(false).show(ui,|ui| {
        if let Some(metadata)=&view.map.metadata {
            ui.small(format!("{} × {} cells · {:.4} m/cell",metadata.width,metadata.height,metadata.resolution));
            ui.add(egui::Label::new(format!("Source frame: {}",metadata.frame_id)).wrap());
            ui.small(format!("Acquisition: {} ns",metadata.stamp_ns));
        }
        if let Some(source)=&view.configuration.map_updates {ui.add(egui::Label::new(format!("Updates: {}",source.topic)).wrap());}
        if let Some(status)=&view.map.source_status {
            ui.small(match status.continuity {Continuity::Disabled=>"Full maps only",Continuity::Unknown=>"Update continuity: unknown",Continuity::SequenceAvailable=>"Update sequence metadata available; base revision is not verified"});
            if let Some(floor)=status.recovery_floor_ns {ui.small(format!("Recovery requires a full map at or after {floor} ns."));}
            let c=status.ingress_counters;
            ui.small(format!("Sequence faults: {} · Publisher changes: {} · Reported lost: {} · Old recovery maps rejected: {}",c.sequence_faults,c.publisher_changes,c.reported_lost,c.rejected_recovery_full));
        }
        if let Some(frame)=&view.map.frame {let c=frame.complete().header.assembly_counters;ui.small(format!("Full maps: {} · Applied patches: {}",c.full_maps,c.patches));}
    });
}
