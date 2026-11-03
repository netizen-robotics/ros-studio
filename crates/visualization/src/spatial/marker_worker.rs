//! Two bounded background stages. No ROS, GPU or active UI-frame asset work.
mod bounds;
mod executor;
mod preparation;
#[cfg(test)]
pub(crate) mod tests;
use super::{
    marker_clock::MarkerClock,
    marker_geometry::Body,
    marker_mesh,
    marker_primitives::{Frame, KeyIdentity, ReadyFrame},
    marker_text, CameraPose,
};
use executor::LatestWorker;
pub use executor::{Completion, Replacements, Wake};
use std::{
    collections::BTreeMap,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, Instant},
};
use studio_protocol::marker_snapshot;

/// Created only from explicitly reviewed local roots. A changed policy or an
/// explicit Reload must use a new epoch; never derive roots from a Marker URI.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AssetPolicy {
    epoch: u64,
    root: Option<PathBuf>,
    packages: BTreeMap<String, PathBuf>,
}
impl AssetPolicy {
    pub fn new(
        epoch: u64,
        root: Option<PathBuf>,
        packages: BTreeMap<String, PathBuf>,
    ) -> Result<Self, String> {
        if epoch == 0
            || packages.len() > crate::robot::MAX_PACKAGE_ROOTS
            || packages
                .keys()
                .any(|name| name.len() > crate::robot::MAX_NAME_BYTES)
            || root
                .iter()
                .chain(packages.values())
                .any(|p| p.as_os_str().len() > crate::robot::MAX_PATH_BYTES)
        {
            return Err("Invalid Marker asset policy epoch/root budget".into());
        }
        Ok(Self {
            epoch,
            root,
            packages,
        })
    }
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
}
#[derive(Clone, Debug)]
pub struct OwnerInput<C = ()> {
    pub owner: u64,
    pub source_epoch: u64,
    /// Captured with the exact client visibility observation for this input.
    pub revision: u64,
    pub origin: Instant,
    pub state: Arc<marker_snapshot::Complete>,
    /// Opaque typed context returned unchanged, e.g. the client's captured
    /// visibility observation. This avoids a frontend dependency or an
    /// unbounded UI map of coalesced request identifiers.
    pub context: C,
}
#[derive(Clone, Debug)]
pub struct PrepareRequest<C = ()> {
    pub scene_epoch: u64,
    pub owners: Vec<OwnerInput<C>>,
    pub assets: Option<Arc<AssetPolicy>>,
}
impl<C> PrepareRequest<C> {
    fn validate(&self) -> Result<(), String> {
        if self.scene_epoch == 0 || self.owners.len() > marker_snapshot::MAX_OWNERS {
            return Err("Marker worker scene/owner limit".into());
        }
        for (index, owner) in self.owners.iter().enumerate() {
            if owner.owner == 0
                || owner.source_epoch == 0
                || owner.origin > Instant::now()
                || owner.revision < owner.state.header().revision
                || self.owners[..index].iter().any(|p| {
                    p.owner == owner.owner
                        || (p.state.header().owner.generation
                            == owner.state.header().owner.generation
                            && p.state.header().owner.subscription
                                == owner.state.header().owner.subscription)
                })
            {
                return Err("Invalid or duplicate Marker worker owner".into());
            }
            owner.state.header().owner.validate()?;
        }
        Ok(())
    }
}
#[derive(Clone, Debug)]
pub struct OwnerCapture<C = ()> {
    pub owner: u64,
    pub source_epoch: u64,
    pub revision: u64,
    pub wire_owner: marker_snapshot::Owner,
    pub context: C,
}
#[derive(Clone, Debug)]
pub struct Diagnostic {
    pub owner: u64,
    pub key: studio_protocol::marker::Key,
    pub message: String,
}
#[derive(Clone, Debug)]
enum Drawing {
    Geometry(Body),
    Text(marker_text::Layout),
    Mesh(marker_mesh::Prepared),
}
#[derive(Clone, Debug)]
struct Item {
    identity: KeyIdentity,
    deadline: Option<u64>,
    drawing: Drawing,
    bounds: Option<super::camera_fit::Bounds>,
}
#[derive(Clone, Debug)]
pub struct PreparedScene<C = ()> {
    items: Vec<Item>,
    clock: MarkerClock,
    pub scene_epoch: u64,
    pub owners: Vec<OwnerCapture<C>>,
    pub diagnostics: Vec<Diagnostic>,
    pub preparation_time: Duration,
}
impl<C> PreparedScene<C> {
    pub fn bounds(
        &self,
        mut include: impl FnMut(&KeyIdentity) -> bool,
    ) -> Option<super::camera_fit::Bounds> {
        let mut result: Option<super::camera_fit::Bounds> = None;
        for item in &self.items {
            if !include(&item.identity) || item.deadline.is_some_and(|d| self.clock.now_ns() >= d) {
                continue;
            }
            if let Some(b) = item.bounds {
                if let Some(all) = &mut result {
                    for i in 0..3 {
                        all.min[i] = all.min[i].min(b.min[i]);
                        all.max[i] = all.max[i].max(b.max[i]);
                    }
                } else {
                    result = Some(b);
                }
            }
        }
        result
    }
    pub fn key_count(&self) -> usize {
        self.items.len()
    }
}
#[derive(Clone, Debug)]
pub struct ProjectionRequest<C = ()> {
    pub scene: Arc<PreparedScene<C>>,
    pub camera: CameraPose,
    pub size: [u32; 2],
}
#[derive(Clone, Debug)]
pub struct ProjectedScene<C = ()> {
    pub scene: Arc<PreparedScene<C>>,
    pub camera: CameraPose,
    pub size: [u32; 2],
    pub frame: ReadyFrame,
    pub projection_time: Duration,
}
fn cancelled(cancel: &AtomicBool) -> Result<(), String> {
    if cancel.load(Ordering::Acquire) {
        Err("Marker worker cancelled".into())
    } else {
        Ok(())
    }
}
fn project_into<C>(
    request: ProjectionRequest<C>,
    cancel: &AtomicBool,
    mut frame: Frame,
) -> Result<ProjectedScene<C>, String> {
    let began = Instant::now();
    cancelled(cancel)?;
    // Validate even an empty/fully expired scene's requested presentation.
    super::marker_primitives::validate_projection(request.camera, request.size)?;
    for item in &request.scene.items {
        cancelled(cancel)?;
        if item
            .deadline
            .is_some_and(|d| request.scene.clock.now_ns() >= d)
        {
            continue;
        }
        frame.append_key(item.identity.clone(), |frame| match &item.drawing {
            Drawing::Geometry(body) => {
                frame.push_geometry(body, request.camera, request.size, item.deadline, || {
                    cancel.load(Ordering::Acquire)
                })
            }
            Drawing::Text(layout) => {
                frame.push_text(layout, request.camera, request.size, item.deadline, || {
                    cancel.load(Ordering::Acquire)
                })
            }
            Drawing::Mesh(mesh) => {
                frame.push_mesh(mesh, request.camera, request.size, item.deadline, || {
                    cancel.load(Ordering::Acquire)
                })
            }
        })?;
    }
    cancelled(cancel)?;
    let frame = frame.seal()?;
    cancelled(cancel)?;
    Ok(ProjectedScene {
        scene: request.scene,
        camera: request.camera,
        size: request.size,
        frame,
        projection_time: began.elapsed(),
    })
}
#[cfg(test)]
fn project<C>(
    request: ProjectionRequest<C>,
    cancel: &AtomicBool,
) -> Result<ProjectedScene<C>, String> {
    let frame = Frame::owned(request.scene.clock.clone());
    project_into(request, cancel, frame)
}
fn record_capacity<C>(scene: &PreparedScene<C>) -> usize {
    scene
        .items
        .iter()
        .map(|item| match &item.drawing {
            Drawing::Geometry(Body::Instances { instances, .. }) => instances.len(),
            Drawing::Geometry(Body::Points { points, .. }) => points.len(),
            Drawing::Geometry(Body::Lines { segments, .. }) => segments.len(),
            Drawing::Geometry(Body::Triangles { triangles }) => triangles.len(),
            Drawing::Geometry(Body::Arrow { .. }) => 2,
            Drawing::Geometry(Body::Text { .. } | Body::Mesh { .. }) => 0,
            Drawing::Text(layout) => layout.glyphs.len(),
            Drawing::Mesh(mesh) => mesh.triangle_count(),
        })
        .sum()
}
pub struct Workers<C: Send + Sync + 'static = ()> {
    prepare: LatestWorker<PrepareRequest<C>, Arc<PreparedScene<C>>>,
    project: LatestWorker<ProjectionRequest<C>, ProjectedScene<C>>,
    camera_project: LatestWorker<ProjectionRequest<C>, ProjectedScene<C>>,
}
impl<C: Send + Sync + 'static> Workers<C> {
    pub fn new(wake: Wake) -> Result<Self, String> {
        let clock = MarkerClock::new();
        let mut preparation = preparation::Preparation::new(clock);
        let mut storage = super::marker_primitives::StoragePool::new();
        let mut camera_storage = super::marker_primitives::StoragePool::new();
        Ok(Self {
            prepare: LatestWorker::new("marker-prepare", wake.clone(), move |request, cancel| {
                preparation.run(request, cancel)
            })?,
            project: LatestWorker::new(
                "marker-project",
                wake.clone(),
                move |request: ProjectionRequest<C>, cancel| {
                    cancelled(cancel)?;
                    super::marker_primitives::validate_projection(request.camera, request.size)?;
                    let frame = storage
                        .frame(request.scene.clock.clone(), record_capacity(&request.scene))?;
                    project_into(request, cancel, frame)
                },
            )?,
            camera_project: LatestWorker::new(
                "camera-marker-project",
                wake,
                move |request: ProjectionRequest<C>, cancel| {
                    cancelled(cancel)?;
                    super::marker_primitives::validate_projection(request.camera, request.size)?;
                    let frame = camera_storage
                        .frame(request.scene.clock.clone(), record_capacity(&request.scene))?;
                    project_into(request, cancel, frame)
                },
            )?,
        })
    }
    pub fn prepare(&mut self, request: PrepareRequest<C>) -> Result<u64, String> {
        request.validate()?;
        self.prepare.submit(request)
    }
    pub fn project(&mut self, request: ProjectionRequest<C>) -> Result<u64, String> {
        self.project.submit(request)
    }
    pub fn poll_prepared(&mut self) -> Option<Completion<Arc<PreparedScene<C>>>> {
        self.prepare.poll()
    }
    pub fn poll_projected(&mut self) -> Option<Completion<ProjectedScene<C>>> {
        self.project.poll()
    }
    pub fn project_camera(&mut self, request: ProjectionRequest<C>) -> Result<u64, String> {
        self.camera_project.submit(request)
    }
    pub fn poll_camera_projected(&mut self) -> Option<Completion<ProjectedScene<C>>> {
        self.camera_project.poll()
    }
    pub fn camera_replacements(&self) -> Replacements {
        self.camera_project.replacements()
    }
    pub fn replacements(&self) -> [Replacements; 2] {
        [self.prepare.replacements(), self.project.replacements()]
    }
    pub fn cancel(&mut self) {
        self.prepare.cancel();
        self.project.cancel();
        self.camera_project.cancel();
    }
    pub fn request_shutdown(&mut self) {
        self.prepare.request_shutdown();
        self.project.request_shutdown();
        self.camera_project.request_shutdown();
    }
    /// Native exit/off-frame owner only: filesystem calls may delay joining.
    pub fn finish_shutdown(&mut self) -> Result<(), String> {
        self.request_shutdown();
        let a = self.prepare.finish_shutdown();
        let b = self.project.finish_shutdown();
        let c = self.camera_project.finish_shutdown();
        a.and(b).and(c)
    }
}
