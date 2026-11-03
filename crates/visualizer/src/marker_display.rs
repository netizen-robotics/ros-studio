//! App-owned Marker reception, worker scheduling and current presentation policy.
use super::*;
use spatial::{
    marker_primitives::{KeyIdentity, Permit},
    marker_worker as worker,
};
use std::{collections::BTreeMap, path::PathBuf, sync::Arc};
use studio_protocol::{
    marker::{self, Key},
    marker_snapshot::{Complete, Owner},
};
#[path = "marker_assets.rs"]
mod assets_ui;
type Context = Arc<backend_pipe::MarkerVisibility>;
type Prepared = worker::PreparedScene<Context>;
type Projected = worker::ProjectedScene<Context>;
#[derive(Default)]
pub struct MarkerDisplay {
    pub stream: Option<backend_pipe::MarkerStream>,
    pub state: Option<Arc<Complete>>,
    pub visibility: Option<Context>,
    pub revision: u64,
    pub source_epoch: u64,
    pub clear_request: Option<RequestId>,
    pub search: String,
    pub selected: Option<Key>,
}
impl MarkerDisplay {
    pub fn stop(&mut self) {
        if let Some(stream) = self.stream.take() {
            stream.stop();
        }
        self.state = None;
        self.visibility = None;
        self.revision = 0;
        self.clear_request = None;
    }
}
impl ViewState {
    pub fn begin_marker(&mut self) -> Result<backend_pipe::MarkerTarget, String> {
        self.marker.stop();
        self.marker.source_epoch = self
            .marker
            .source_epoch
            .checked_add(1)
            .ok_or("Marker source identities exhausted")?;
        let owner = Owner {
            generation: self.generation,
            subscription: self.subscription.ok_or("Marker owner is not allocated")?,
            topic: self.configuration.topic.clone(),
            message_type: self.configuration.message_type.clone(),
            fixed_frame: self
                .configuration
                .spatial
                .as_ref()
                .ok_or("Marker scene is not configured")?
                .fixed_frame
                .clone(),
        };
        let stream = backend_pipe::MarkerStream::new(owner.clone())?;
        self.marker.stream = Some(stream.clone());
        self.spatial_status = "Waiting for Markers and transforms".into();
        Ok(backend_pipe::MarkerTarget { owner, stream })
    }
    fn refresh_marker_status(&mut self) {
        let Some(state) = &self.marker.state else {
            return;
        };
        let now = self
            .marker
            .stream
            .as_ref()
            .map_or(0, |stream| stream.monotonic_ns());
        let ready = state.visible_entries(now).count();
        let count = state.entries().len();
        self.spatial_status = format!(
            "{count} {} · {ready} with a ready transform",
            if count == 1 { "marker" } else { "markers" }
        );
        self.status = if ready > 0 {
            ViewStatus::Receiving
        } else {
            ViewStatus::Waiting
        };
    }
    fn poll_marker(&mut self) -> Result<bool, String> {
        let Some(result) = self.marker.stream.as_ref().and_then(|s| s.take()) else {
            return Ok(false);
        };
        match result? {
            backend_pipe::MarkerUpdate::Complete {
                revision,
                state,
                visibility,
            } => {
                self.received.typed_samples = state.counters.batches;
                self.marker.state = Some(state);
                self.marker.visibility = Some(visibility);
                self.marker.revision = revision;
                self.refresh_marker_status();
                self.error = None;
            }
            backend_pipe::MarkerUpdate::Cleared(clear) => {
                self.marker.state = None;
                self.marker.visibility = None;
                self.marker.revision = clear.revision;
                self.status = ViewStatus::Waiting;
                self.spatial_status = format!("Markers hidden · {:?}", clear.reason);
            }
        }
        self.deadline = None;
        Ok(true)
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(super) struct AssetDraft {
    pub(super) root: String,
    pub(super) packages: Vec<crate::components::workspace::PackageRoot>,
}
impl AssetDraft {
    fn add_package(&mut self) -> bool {
        if self.packages.len() >= crate::components::robot::MAX_PACKAGE_ROOTS {
            return false;
        }
        self.packages.push(Default::default());
        true
    }
    fn policy(&self, epoch: u64) -> Result<worker::AssetPolicy, String> {
        use crate::components::robot::{MAX_NAME_BYTES, MAX_PACKAGE_ROOTS, MAX_PATH_BYTES};
        if self.packages.len() > MAX_PACKAGE_ROOTS {
            return Err("Use at most 16 package mappings.".into());
        }
        let mut packages = BTreeMap::new();
        for (index, mapping) in self.packages.iter().enumerate() {
            let number = index + 1;
            let (name, path) = (mapping.package.trim(), mapping.path.trim());
            if name.is_empty()
                || name.len() > MAX_NAME_BYTES
                || !name
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
            {
                return Err(format!(
                    "Package {number} name: use letters, digits, _ or -."
                ));
            }
            if path.is_empty() || !PathBuf::from(path).is_absolute() {
                return Err(format!("Package {number} needs an absolute folder path."));
            }
            if path.len() > MAX_PATH_BYTES {
                return Err(format!("Package {number} folder exceeds the path limit."));
            }
            if packages
                .insert(name.to_owned(), PathBuf::from(path))
                .is_some()
            {
                return Err(format!(
                    "Package {number} repeats an existing package name."
                ));
            }
        }
        let root_text = self.root.trim();
        let root = (!root_text.is_empty()).then(|| PathBuf::from(root_text));
        if root_text.len() > MAX_PATH_BYTES || root.as_ref().is_some_and(|p| !p.is_absolute()) {
            return Err("Asset folder must be an absolute local path within the limit.".into());
        }
        if root.is_none() && packages.is_empty() {
            return Err("Enter an asset folder or add a package mapping.".into());
        }
        worker::AssetPolicy::new(epoch, root, packages)
    }
}
#[derive(Default)]
pub struct MarkerRuntime {
    workers: Option<worker::Workers<Context>>,
    prepared: Option<Arc<Prepared>>,
    pub projected: Option<Projected>,
    pub camera_projected: Option<Projected>,
    camera_last_project: Option<(Arc<Prepared>, spatial::CameraPose, [u32; 2])>,
    last_inputs: Option<InputKey>,
    last_project: Option<(Arc<Prepared>, spatial::CameraPose, [u32; 2])>,
    scene: Option<studio_protocol::spatial::Configuration>,
    pub scene_epoch: u64,
    assets: Option<Arc<worker::AssetPolicy>>,
    asset_epoch: u64,
    draft: AssetDraft,
    approved: Option<AssetDraft>,
    asset_editor_open: bool,
    asset_error: Option<String>,
    asset_notice: Option<String>,
    pub error: Option<String>,
}
#[derive(PartialEq, Eq)]
struct InputKey {
    scene: u64,
    assets: u64,
    owners: Vec<(u64, u64, u64)>,
}
impl MarkerRuntime {
    pub(super) fn current_asset_error(&self) -> Option<&str> {
        self.asset_error.as_deref()
    }
    pub(super) fn reveal_assets(&mut self) {
        self.asset_editor_open = true;
    }
    pub fn projection(&self, camera: bool) -> Option<&Projected> {
        if camera {
            self.camera_projected.as_ref()
        } else {
            self.projected.as_ref()
        }
    }

    pub(super) fn review(&mut self, draft: AssetDraft) -> Result<(), String> {
        let epoch = self
            .asset_epoch
            .checked_add(1)
            .ok_or("Marker asset identities exhausted")?;
        let policy = draft.policy(epoch)?;
        self.asset_epoch = epoch;
        self.assets = Some(Arc::new(policy));
        self.approved = Some(draft);
        self.error = None;
        self.asset_error = None;
        self.asset_notice = Some("Mesh roots approved for this session.".into());
        Ok(())
    }
    fn asset_draft_changed(&self) -> bool {
        self.draft != self.approved.clone().unwrap_or_default()
    }
    fn discard_asset_draft(&mut self) {
        self.draft = self.approved.clone().unwrap_or_default();
        self.asset_error = None;
        self.asset_notice = Some("Draft discarded. Approved access is unchanged.".into());
    }
    fn reload_assets(&mut self) -> Result<(), String> {
        let approved = self.approved.clone().ok_or("No mesh roots are approved.")?;
        self.review(approved)?;
        self.asset_notice =
            Some("Approved meshes reload requested. Drafts were not applied.".into());
        Ok(())
    }
    pub fn revoke_assets(&mut self) {
        self.assets = None;
        self.approved = None;
        self.asset_error = None;
        self.asset_notice = Some("Mesh access revoked. Drafts are retained.".into());
    }
    pub fn request_shutdown(&mut self) {
        if let Some(w) = &mut self.workers {
            w.request_shutdown();
        }
    }
    pub fn finish_shutdown(&mut self) -> Result<(), String> {
        self.workers
            .as_mut()
            .map_or(Ok(()), |w| w.finish_shutdown())
    }
}
fn eligible(view: &ViewState) -> bool {
    view.enabled && view.configuration.output == Output::MarkerSnapshot && view.status.active()
}
struct CurrentOwner<'a> {
    view: &'a ViewState,
    capture: &'a worker::OwnerCapture<Context>,
    visibility: Option<Context>,
    now: u64,
}
impl CurrentOwner<'_> {
    fn allows(
        &self,
        key: &KeyIdentity,
        assets: Option<&worker::AssetPolicy>,
        scene_epoch: u64,
    ) -> bool {
        eligible(self.view)
            && key.scene_epoch == scene_epoch
            && key.source_epoch == self.view.marker.source_epoch
            && self.capture.source_epoch == self.view.marker.source_epoch
            && self.capture.wire_owner.generation == self.view.generation
            && Some(self.capture.wire_owner.subscription) == self.view.subscription
            && key
                .asset_epoch
                .is_none_or(|epoch| assets.is_some_and(|p| p.epoch() == epoch))
            && self.visibility.as_ref().is_some_and(|current| {
                current.allows(&self.capture.context, &key.key, key.version, self.now)
            })
    }
}
impl App {
    fn current_marker_owners<'a>(&'a self, output: &'a Projected) -> Vec<CurrentOwner<'a>> {
        output
            .scene
            .owners
            .iter()
            .filter_map(|capture| {
                let view = self.views.iter().find(|v| v.id.0 == capture.owner)?;
                let stream = view.marker.stream.as_ref()?;
                Some(CurrentOwner {
                    view,
                    capture,
                    visibility: stream.visibility(),
                    now: stream.monotonic_ns(),
                })
            })
            .collect()
    }
    pub(super) fn marker_bounds(
        &self,
        selected: Option<u64>,
    ) -> Option<spatial::camera_fit::Bounds> {
        let output = self.markers.projected.as_ref()?;
        let owners = self.current_marker_owners(output);
        output.scene.bounds(|key| {
            selected.is_none_or(|id| key.owner == id)
                && owners
                    .iter()
                    .find(|owner| owner.view.id.0 == key.owner)
                    .is_some_and(|owner| {
                        owner.allows(
                            key,
                            self.markers.assets.as_deref(),
                            self.markers.scene_epoch,
                        )
                    })
        })
    }
    #[cfg(test)]
    pub(super) fn marker_permit(&self) -> Option<Permit> {
        self.marker_permit_for(false)
    }
    pub(super) fn marker_permit_for(&self, camera: bool) -> Option<Permit> {
        let output = self.markers.projection(camera)?;
        let owners = self.current_marker_owners(output);
        Some(output.frame.permit(|key| {
            owners
                .iter()
                .find(|owner| owner.view.id.0 == key.owner)
                .is_some_and(|owner| {
                    owner.allows(
                        key,
                        self.markers.assets.as_deref(),
                        self.markers.scene_epoch,
                    )
                })
        }))
    }
    pub(super) fn poll_markers(&mut self, ctx: &egui::Context) {
        let mut failed = vec![];
        for view in &mut self.views {
            if !eligible(view) {
                continue;
            }
            if let Err(error) = view.poll_marker() {
                failed.push((view.id, error));
            }
            view.refresh_marker_status();
            if let Some(state) = &view.marker.state {
                let now = view.marker.stream.as_ref().map_or(0, |s| s.monotonic_ns());
                if let Some(deadline) = state
                    .entries()
                    .iter()
                    .filter_map(|e| e.expires_at_monotonic_ns)
                    .filter(|d| *d > now)
                    .min()
                {
                    ctx.request_repaint_after(Duration::from_nanos(deadline - now));
                }
            }
        }
        for (id, error) in failed {
            self.stop_view(id);
            if let Some(v) = self.views.iter_mut().find(|v| v.id == id) {
                v.error = Some(error);
            }
        }
        if self.markers.scene.as_ref() != Some(&self.scene_settings.configuration) {
            self.markers.scene_epoch = match self.markers.scene_epoch.checked_add(1) {
                Some(v) => v,
                None => {
                    self.markers.error = Some("Marker scene identities exhausted".into());
                    return;
                }
            };
            self.markers.scene = Some(self.scene_settings.configuration.clone());
            if let Some(w) = &mut self.markers.workers {
                w.cancel();
            }
            self.markers.prepared = None;
            self.markers.projected = None;
            self.markers.camera_projected = None;
            self.markers.camera_last_project = None;
            self.markers.last_project = None;
            self.markers.last_inputs = None;
        }
        let owners: Vec<_> = self
            .views
            .iter()
            .filter(|v| eligible(v))
            .filter_map(|v| {
                Some(worker::OwnerInput {
                    owner: v.id.0,
                    source_epoch: v.marker.source_epoch,
                    revision: v.marker.revision,
                    origin: v.marker.stream.as_ref()?.monotonic_origin(),
                    state: v.marker.state.clone()?,
                    context: v.marker.visibility.clone()?,
                })
            })
            .collect();
        if owners.is_empty() {
            if self.markers.last_inputs.take().is_some() {
                if let Some(w) = &mut self.markers.workers {
                    w.cancel();
                }
            }
            self.markers.prepared = None;
            self.markers.projected = None;
            self.markers.camera_projected = None;
            self.markers.camera_last_project = None;
            self.markers.last_project = None;
            if let Some(renderer) = &mut self.spatial_renderer {
                renderer.clear_marker_primitives();
            }
            return;
        }
        if self.markers.workers.is_none() {
            let repaint = ctx.clone();
            match worker::Workers::new(Arc::new(move || repaint.request_repaint())) {
                Ok(w) => self.markers.workers = Some(w),
                Err(e) => {
                    self.markers.error = Some(e);
                    return;
                }
            }
        }
        let input_key = InputKey {
            scene: self.markers.scene_epoch,
            assets: self.markers.assets.as_ref().map_or(0, |p| p.epoch()),
            owners: owners
                .iter()
                .map(|o| (o.owner, o.source_epoch, o.revision))
                .collect(),
        };
        let workers = self.markers.workers.as_mut().unwrap();
        if self.markers.last_inputs.as_ref() != Some(&input_key) {
            match workers.prepare(worker::PrepareRequest {
                scene_epoch: self.markers.scene_epoch,
                owners,
                assets: self.markers.assets.clone(),
            }) {
                Ok(_) => self.markers.last_inputs = Some(input_key),
                Err(e) => self.markers.error = Some(e),
            }
        }
        if let Some(completion) = workers.poll_prepared() {
            match completion.result {
                Ok(scene) if scene.scene_epoch == self.markers.scene_epoch => {
                    self.markers.prepared = Some(scene);
                    self.markers.error = None;
                }
                Ok(_) => (),
                Err(e) => self.markers.error = Some(e),
            }
        }
        if let Some(completion) = workers.poll_projected() {
            match completion.result {
                Ok(output) if output.scene.scene_epoch == self.markers.scene_epoch => {
                    self.markers.projected = Some(output);
                }
                Ok(_) => (),
                Err(e) => self.markers.error = Some(e),
            }
        }
        if let Some(completion) = workers.poll_camera_projected() {
            match completion.result {
                Ok(output) if output.scene.scene_epoch == self.markers.scene_epoch => {
                    self.markers.camera_projected = Some(output);
                }
                Ok(_) => (),
                Err(e) => self.markers.error = Some(e),
            }
        }
        self.submit_marker_projection();
    }
    pub(super) fn submit_marker_projection(&mut self) {
        let Some(scene) = self.markers.prepared.clone() else {
            return;
        };
        for camera_view in [false, true] {
            let request = if camera_view {
                if self.companion_open
                    && self.companion_selected.is_some_and(|id| {
                        self.views
                            .iter()
                            .any(|v| v.id == id && v.enabled && matches!(v.kind, ViewKind::Camera))
                    })
                {
                    self.camera_canvas.marker_request
                } else {
                    None
                }
            } else {
                self.shared_scene.marker_request
            };
            let Some((camera, size)) = request else {
                continue;
            };
            let last = if camera_view {
                &mut self.markers.camera_last_project
            } else {
                &mut self.markers.last_project
            };
            if last
                .as_ref()
                .is_some_and(|(old, c, s)| Arc::ptr_eq(old, &scene) && *c == camera && *s == size)
            {
                continue;
            }
            if let Some(w) = &mut self.markers.workers {
                let request = worker::ProjectionRequest {
                    scene: scene.clone(),
                    camera,
                    size,
                };
                let result = if camera_view {
                    w.project_camera(request)
                } else {
                    w.project(request)
                };
                match result {
                    Ok(_) => *last = Some((scene.clone(), camera, size)),
                    Err(e) => self.markers.error = Some(e),
                }
            }
        }
    }
    pub(super) fn clear_marker(&mut self, id: ViewId) {
        let Some(index) = self.views.iter().position(|v| v.id == id && eligible(v)) else {
            return;
        };
        let Some(session) = &self.session else { return };
        let Some(subscription) = self.views[index].subscription else {
            return;
        };
        let Some(epoch) = self.views[index].marker.source_epoch.checked_add(1) else {
            self.views[index].error =
                Some("Marker source identities exhausted; restart the app".into());
            return;
        };
        let result = session
            .shared
            .lock()
            .unwrap()
            .clear_markers_explicit(subscription);
        match result {
            Ok(command) => {
                let Command::ClearMarkers { request, .. } = &command else {
                    unreachable!()
                };
                let view = &mut self.views[index];
                view.marker.source_epoch = epoch;
                view.marker.clear_request = Some(*request);
                view.marker.state = None;
                view.marker.visibility = None;
                view.status = ViewStatus::Waiting;
                view.spatial_status = "Clearing Markers".into();
                if let Err(e) = session.send(command) {
                    self.fail(e);
                }
            }
            Err(e) => self.views[index].error = Some(e),
        }
    }
    pub(super) fn marker_control_event(&mut self, event: &Event) -> bool {
        let (generation, subscription, request, message) = match event {
            Event::MarkersCleared {
                generation,
                subscription,
                request,
            } => (*generation, *subscription, *request, None),
            Event::RequestError {
                generation,
                subscription,
                request,
                message,
                ..
            } => (*generation, *subscription, *request, Some(message)),
            _ => return false,
        };
        let Some(view) = self.views.iter_mut().find(|v| {
            v.generation == generation
                && v.subscription == Some(subscription)
                && v.marker.clear_request == Some(request)
        }) else {
            return false;
        };
        view.marker.clear_request = None;
        view.error = message.cloned();
        view.spatial_status = if message.is_some() {
            "Clear failed · retry Clear or Stop"
        } else {
            "Markers cleared · waiting for new data"
        }
        .into();
        true
    }
    pub(super) fn marker_inspector_ui(&mut self, ui: &mut egui::Ui, index: usize, frozen: bool) {
        let view = &mut self.views[index];
        let id = view.id;
        let active = eligible(view);
        let clear = ui
            .add_enabled(
                !frozen && active && view.marker.clear_request.is_none(),
                egui::Button::new("Clear Markers"),
            )
            .clicked();
        ui.label(&view.spatial_status);
        let search = ui.label("Find namespace or id");
        ui.add(studio_ui_foundation::theme::input(ui, &mut view.marker.search).char_limit(256))
            .labelled_by(search.id);
        if let Some(state) = &view.marker.state {
            let now = view.marker.stream.as_ref().map_or(0, |s| s.monotonic_ns());
            let query = view.marker.search.to_lowercase();
            egui::ScrollArea::vertical()
                .id_salt((id.0, "marker-keys"))
                .max_height(240.)
                .show(ui, |ui| {
                    for entry in state.entries() {
                        let label = format!(
                            "{} / {}",
                            if entry.marker.key.namespace.is_empty() {
                                "(default)"
                            } else {
                                &entry.marker.key.namespace
                            },
                            entry.marker.key.id
                        );
                        if !query.is_empty() && !label.to_lowercase().contains(&query) {
                            continue;
                        }
                        let status = if entry.expires_at_monotonic_ns.is_some_and(|d| now >= d) {
                            "Expired"
                        } else if entry.visible_at(now) {
                            "TF ready"
                        } else {
                            "Waiting for TF"
                        };
                        let row = ui
                            .scope(|ui| {
                                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                                ui.selectable_label(
                                    view.marker.selected.as_ref() == Some(&entry.marker.key),
                                    format!(
                                        "{} · {status} · {}",
                                        entry.marker.key.id,
                                        if entry.marker.key.namespace.is_empty() {
                                            "(default)"
                                        } else {
                                            &entry.marker.key.namespace
                                        }
                                    ),
                                )
                                .on_hover_text(&label)
                            })
                            .inner;
                        if row.clicked() {
                            view.marker.selected = Some(entry.marker.key.clone());
                        }
                    }
                    crate::ui::reveal_focused_control(ui);
                });
            if let Some(entry) = state
                .entries()
                .iter()
                .find(|e| Some(&e.marker.key) == view.marker.selected.as_ref())
            {
                ui.label(format!("Marker id: {}", entry.marker.key.id));
                ui.label("Namespace");
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(if entry.marker.key.namespace.is_empty() {
                            "(default)"
                        } else {
                            &entry.marker.key.namespace
                        })
                        .monospace(),
                    )
                    .wrap(),
                );
                ui.monospace(format!("Frame: {}", entry.marker.frame_id));
                let shape = match &entry.marker.geometry {
                    marker::Geometry::Arrow { .. } => "Arrow",
                    marker::Geometry::Cube => "Cube",
                    marker::Geometry::Sphere => "Sphere",
                    marker::Geometry::Cylinder => "Cylinder",
                    marker::Geometry::LineStrip { .. } => "Line strip",
                    marker::Geometry::LineList { .. } => "Line list",
                    marker::Geometry::CubeList { .. } => "Cube list",
                    marker::Geometry::SphereList { .. } => "Sphere list",
                    marker::Geometry::Points { .. } => "Points",
                    marker::Geometry::Text { .. } => "Text",
                    marker::Geometry::Mesh { .. } => "Mesh",
                    marker::Geometry::Triangles { .. } => "Triangles",
                    marker::Geometry::ArrowStrip { .. } => "Arrow strip (unavailable)",
                };
                ui.small(format!("Shape: {shape}"));
                if !entry.marker.geometry.points().is_empty() {
                    ui.small(format!(
                        "Source points: {}",
                        entry.marker.geometry.points().len()
                    ));
                }
                ui.monospace(format!(
                    "Position in source frame [m]: {:.3}, {:.3}, {:.3}",
                    entry.marker.pose[0], entry.marker.pose[1], entry.marker.pose[2]
                ));
                match entry.expires_at_monotonic_ns {
                    None => {
                        ui.small("Lifetime: persistent");
                    }
                    Some(deadline) if deadline > now => {
                        ui.small(format!(
                            "Lifetime: {:.2} s remaining",
                            (deadline - now) as f64 / 1e9
                        ));
                        ui.ctx().request_repaint_after(Duration::from_nanos(
                            (deadline - now).min(100_000_000),
                        ));
                    }
                    Some(_) => {
                        ui.small("Lifetime: expired");
                    }
                }
                ui.small(if entry.marker.frame_locked {
                    "Latest available TF, frame locked"
                } else if entry.marker.stamp_ns == 0 {
                    "Latest available TF"
                } else {
                    "Acquisition-time TF"
                });
                use studio_protocol::{latest_pose::LatestPose, marker_snapshot::Transform};
                match &entry.transform {
                    Transform::Resolved {
                        pose:
                            LatestPose::Ready {
                                resolved_stamp_ns,
                                components,
                            },
                    } => {
                        ui.small("Transform ready");
                        ui.collapsing("Transform values", |ui| {
                            ui.monospace(format!(
                                "Translation [m]: {:.3}, {:.3}, {:.3}",
                                components[0], components[1], components[2]
                            ));
                            ui.monospace(format!(
                                "Quaternion [x y z w]: {:.4}, {:.4}, {:.4}, {:.4}",
                                components[3], components[4], components[5], components[6]
                            ));
                            ui.small(format!(
                                "Transform timestamp: {resolved_stamp_ns} ns · Source revision: {}",
                                entry.version
                            ));
                        });
                    }
                    Transform::Pending => {
                        ui.small("Waiting for transform");
                    }
                    Transform::Resolved { pose } => {
                        ui.small(match pose {
                            LatestPose::Missing => "Transform is missing",
                            LatestPose::Stale => "Transform is stale",
                            LatestPose::Extrapolated => "Transform time is unavailable",
                            LatestPose::LookupTimeout => "Transform lookup timed out",
                            LatestPose::Ready { .. } => unreachable!(),
                        });
                    }
                }
                if let marker::Geometry::Mesh { resource, .. } = &entry.marker.geometry {
                    ui.add(egui::Label::new(resource).wrap());
                }
            }
            for diagnostic in &state.diagnostics {
                ui.small(format!(
                    "{} / {}: {:?}",
                    diagnostic.key.namespace, diagnostic.key.id, diagnostic.reason
                ));
            }
            if state.known_history_gap {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "Source history is incomplete. Re-enable for a fresh activation.",
                );
            }
        } else {
            ui.small(if active {
                "Waiting for new Marker messages. Clear keeps this subscription running."
            } else {
                "Enable this display to receive namespace/id entries."
            });
        }
        if let Some(output) = &self.markers.projected {
            for diagnostic in output.scene.diagnostics.iter().filter(|d| d.owner == id.0) {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    format!(
                        "{} / {}: {}",
                        diagnostic.key.namespace, diagnostic.key.id, diagnostic.message
                    ),
                );
            }
        }
        if let Some(error) = &self.markers.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        ui.collapsing("Presentation diagnostics",|ui| {
            if let Some(workers)=&self.markers.workers {
                let [prepare,project]=workers.replacements();
                let camera = workers.camera_replacements();
                ui.small(format!("Camera projection coalesced: {} pending requests, {} results", camera.requests, camera.results));
                ui.small(format!("Preparation coalesced: {} pending requests, {} results",prepare.requests,prepare.results));
                ui.small(format!("Projection coalesced: {} pending requests, {} results",project.requests,project.results));
            }
            if let Some(output)=&self.markers.projected {
                ui.small(format!("Last preparation: {:.1} ms · Projection: {:.1} ms",output.scene.preparation_time.as_secs_f64()*1000.,output.projection_time.as_secs_f64()*1000.));
            } else if active {ui.small("Waiting for prepared, transformed geometry.");}
            ui.small("Presentation coalescing keeps the latest pending update; it is separate from ROS transport loss.");
        });
        if clear {
            self.clear_marker(id);
        }
        self.marker_assets_ui(ui, frozen);
    }
}

#[cfg(all(test, unix))]
#[path = "marker_performance_tests.rs"]
mod performance;
