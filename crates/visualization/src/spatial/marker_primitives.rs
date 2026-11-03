//! Bounded instanced Marker geometry GPU component. Shared-scene composition owns
//! draw ordering and lifecycle; this module performs no ROS, assets or UI work.
use super::marker_clock::{self, MarkerClock};
use super::{
    marker_geometry::{Instance, Primitive},
    CameraPose, Point3,
};
use eframe::egui_wgpu::{self, wgpu};
use std::time::Duration;
use std::{
    ops::{Deref, Range},
    sync::{Arc, Weak},
};
mod atlas_gpu;
mod diffuse_gpu;
mod flat;
mod glyph;
mod mesh;
mod prepared;
mod projection;
mod storage;
mod strip;
mod visibility;
pub(crate) use storage::StoragePool;
pub use visibility::{KeyIdentity, MarkerLayer, Permit, UNKEYED};
pub(crate) fn validate_projection(camera: CameraPose, size: [u32; 2]) -> Result<(), String> {
    projection::Projection::new(camera, size).map(|_| ())
}
pub const MAX_NON_TEXT_INSTANCES: usize =
    2 * (studio_protocol::marker::MAX_RETAINED_POINTS + 2 * studio_protocol::marker::MAX_MARKERS);
pub const MAX_INSTANCES: usize =
    MAX_NON_TEXT_INSTANCES + super::marker_text::MAX_GLYPHS + super::marker_mesh::MAX_TRIANGLES;
pub const INSTANCE_BYTES: usize = 128;
pub const MAX_INSTANCE_BYTES: usize = MAX_INSTANCES * INSTANCE_BYTES;
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Kind {
    Cube,
    Sphere,
    Cylinder,
    Cone,
    Point,
    Line,
    Triangle,
    Ribbon,
    Glyph,
    MeshTriangle,
    TexturedTriangle(u8),
}
impl Kind {
    pub fn mesh(self) -> Self {
        match self {
            Self::Sphere => Self::Cube,
            Self::MeshTriangle => Self::Triangle,
            Self::Line | Self::Ribbon | Self::Glyph => Self::Point,
            _ => self,
        }
    }
}
impl Kind {
    fn is_mesh(self) -> bool {
        matches!(self, Self::MeshTriangle | Self::TexturedTriangle(_))
    }
    fn unit_index(self) -> usize {
        match self {
            Self::Cube => 0,
            Self::Sphere => 1,
            Self::Cylinder => 2,
            Self::Cone => 3,
            Self::Point => 4,
            Self::Line => 5,
            Self::Triangle | Self::MeshTriangle | Self::TexturedTriangle(_) => 6,
            Self::Ribbon => 7,
            Self::Glyph => 8,
        }
    }
    fn tag(self) -> u32 {
        match self {
            Self::Sphere => 1,
            Self::Line => 2,
            Self::Triangle | Self::MeshTriangle => 3,
            Self::TexturedTriangle(slot) => 6 | ((slot as u32) << 8),
            Self::Ribbon => 4,
            Self::Glyph => 5,
            _ => 0,
        }
    }
}
fn alphas(p: &ClipInstance, kind: Kind) -> [f32; 3] {
    match kind {
        Kind::Line | Kind::Ribbon => [p.rgba[3], p.eye[3], p.eye[3]],
        Kind::Triangle | Kind::MeshTriangle => [p.rgba[3], p.eye[3], p.ray[3]],
        _ => [p.rgba[3]; 3],
    }
}
fn packet_valid(p: &ClipInstance, kind: Kind, clock: bool) -> bool {
    p.columns
        .iter()
        .flatten()
        .chain(p.rgba.iter())
        .chain(p.eye.iter())
        .chain(p.ray.iter())
        .all(|v| v.is_finite())
        && p.rgba.iter().all(|v| (0.0..=1.0).contains(v))
        && (!matches!(
            kind,
            Kind::Line | Kind::Ribbon | Kind::Triangle | Kind::MeshTriangle
        ) || p.eye.iter().all(|v| (0.0..=1.0).contains(v)))
        && (!matches!(kind, Kind::Triangle | Kind::MeshTriangle)
            || p.ray.iter().all(|v| (0.0..=1.0).contains(v)))
        && (kind != Kind::Glyph
            || (p.eye.iter().all(|v| (0.0..=1.0).contains(v))
                && p.eye[0] < p.eye[2]
                && p.eye[1] < p.eye[3]))
        && (!matches!(kind, Kind::TexturedTriangle(_))
            || (p.columns[3].iter().all(|v| (0.0..=1.0).contains(v))
                && p.eye
                    .iter()
                    .chain(&p.ray[..2])
                    .all(|v| v.abs() <= 1_000_000.)))
        && p.shape[0] == kind.tag()
        && p.shape[3] <= 1
        && (p.shape[3] == 0 || clock)
}
impl From<Primitive> for Kind {
    fn from(p: Primitive) -> Self {
        match p {
            Primitive::Cube => Self::Cube,
            Primitive::Sphere => Self::Sphere,
            Primitive::Cylinder => Self::Cylinder,
        }
    }
}
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct ClipInstance {
    pub columns: [[f32; 4]; 4],
    pub rgba: [f32; 4],
    pub eye: [f32; 4],
    pub ray: [f32; 4],
    pub shape: [u32; 4],
}
#[derive(Clone, Debug, Default)]
pub struct Frame {
    pub(crate) instances: Vec<ClipInstance>,
    pub(crate) kinds: Vec<Kind>,
    projection: Option<ProjectionKey>,
    clock: Option<MarkerClock>,
    glyph_count: usize,
    mesh_count: usize,
    textures: Vec<Arc<crate::robot::RobotTexture>>,
    atlas: Option<Arc<super::marker_text::Atlas>>,
    keys: Vec<KeyIdentity>,
    key_slots: Vec<u16>,
    current_key: Option<u16>,
    require_keys: bool,
    recycler: Option<std::sync::mpsc::SyncSender<storage::Buffers>>,
}
#[derive(Clone, Debug, PartialEq)]
struct ProjectionKey {
    camera: [u32; 12],
    span: Option<u32>,
    calibrated: Option<super::camera3d::CalibratedView>,
    size: [u32; 2],
}
impl ProjectionKey {
    fn new(camera: CameraPose, size: [u32; 2]) -> Self {
        Self {
            camera: std::array::from_fn(|i| {
                [camera.eye, camera.right, camera.up, camera.forward][i / 3][i % 3].to_bits()
            }),
            span: camera.orthographic_span.map(f32::to_bits),
            calibrated: camera.calibrated,
            size: super::gpu::bounded_dimensions(size),
        }
    }
}
/// Immutable, worker-validated projection. Heavy storage validation belongs
/// before sealing; renderer identity checks are constant-time.
#[derive(Clone, Debug)]
pub struct ReadyFrame(Arc<Sealed>);
#[derive(Debug)]
struct Sealed {
    frame: Frame,
    opaque: Vec<(Kind, Range<u32>)>,
    opaque_keys: Vec<u16>,
    used_keys: [u32; 16],
    has_unkeyed: bool,
    transparent: Vec<TransparentInstance>,
    deadlines: Vec<u64>,
    has_forever: bool,
}
#[derive(Clone, Copy, Debug)]
pub struct TransparentInstance {
    pub key: u16,
    pub mesh: Kind,
    pub index: u32,
    pub depth: f32,
}
impl Deref for ReadyFrame {
    type Target = Frame;
    fn deref(&self) -> &Frame {
        &self.0.frame
    }
}
impl ReadyFrame {
    pub fn all_expired(&self) -> bool {
        !self.0.has_forever && self.0.deadlines.last().is_some_and(|d| self.now_ns() >= *d)
    }
    pub fn now_ns(&self) -> u64 {
        self.clock.as_ref().map_or(0, MarkerClock::now_ns)
    }
    pub fn next_expiry_after(&self, rendered_ns: u64) -> Option<Duration> {
        self.next_expiry_after_at(rendered_ns, self.now_ns())
    }
    fn next_expiry_after_at(&self, rendered_ns: u64, now: u64) -> Option<Duration> {
        self.0
            .deadlines
            .get(self.0.deadlines.partition_point(|d| *d <= rendered_ns))
            .map(|d| Duration::from_nanos(d.saturating_sub(now)))
    }

    pub fn opaque_runs(&self) -> &[(Kind, Range<u32>)] {
        &self.0.opaque
    }
    pub fn transparent_instances(&self) -> &[TransparentInstance] {
        &self.0.transparent
    }

    pub fn validate_projection(&self, camera: CameraPose, size: [u32; 2]) -> Result<(), String> {
        if !self.is_empty() && self.projection.as_ref() != Some(&ProjectionKey::new(camera, size)) {
            return Err("Marker projection is stale for this camera/viewport".into());
        }
        Ok(())
    }
}
impl Frame {
    pub fn with_clock(clock: MarkerClock) -> Self {
        Self {
            clock: Some(clock),
            ..Self::default()
        }
    }

    pub fn len(&self) -> usize {
        self.instances.len()
    }
    pub fn is_empty(&self) -> bool {
        self.instances.is_empty()
    }
    pub fn push(
        &mut self,
        kind: Kind,
        instance: &Instance,
        camera: CameraPose,
        size: [u32; 2],
    ) -> Result<(), String> {
        self.push_timed(kind, instance, camera, size, None)
    }
    pub fn push_timed(
        &mut self,
        kind: Kind,
        instance: &Instance,
        camera: CameraPose,
        size: [u32; 2],
        deadline_ns: Option<u64>,
    ) -> Result<(), String> {
        if matches!(
            kind,
            Kind::Point
                | Kind::Line
                | Kind::Ribbon
                | Kind::Triangle
                | Kind::Glyph
                | Kind::MeshTriangle
                | Kind::TexturedTriangle(_)
        ) {
            return Err("Use typed Marker geometry projection".into());
        }
        let packed = project(kind, instance, camera, size)?;
        self.commit(kind, packed, camera, size, deadline_ns)
    }
    fn commit(
        &mut self,
        kind: Kind,
        mut packed: ClipInstance,
        camera: CameraPose,
        size: [u32; 2],
        deadline_ns: Option<u64>,
    ) -> Result<(), String> {
        if self.require_keys && self.current_key.is_none() {
            return Err("Owned Marker frame requires a key scope".into());
        }
        if deadline_ns.is_some() && self.clock.is_none() {
            return Err("Timed Marker requires an explicit monotonic clock".into());
        }
        if size.contains(&0) {
            return Err("Invalid Marker viewport".into());
        }
        let size = super::gpu::bounded_dimensions(size);
        if self.len() >= MAX_INSTANCES
            || (kind == Kind::Glyph && self.glyph_count >= super::marker_text::MAX_GLYPHS)
            || (kind.is_mesh() && self.mesh_count >= super::marker_mesh::MAX_TRIANGLES)
            || (kind != Kind::Glyph
                && !kind.is_mesh()
                && self.len() - self.glyph_count - self.mesh_count >= MAX_NON_TEXT_INSTANCES)
        {
            return Err("Marker GPU instance limit".into());
        }
        let key = ProjectionKey::new(camera, size);
        if self.projection.as_ref().is_some_and(|old| old != &key) {
            return Err("Marker frame mixes projection identities".into());
        }
        if let Some(deadline) = deadline_ns {
            let w = marker_clock::words(deadline);
            packed.shape = [packed.shape[0], w[0], w[1], 1];
        }
        if !packet_valid(&packed, kind, self.clock.is_some()) {
            return Err("Invalid Marker instance packet".into());
        }
        self.instances.push(packed);
        self.kinds.push(kind);
        self.key_slots.push(self.current_key.unwrap_or(UNKEYED));
        self.glyph_count += usize::from(kind == Kind::Glyph);
        self.mesh_count += usize::from(kind.is_mesh());
        self.projection = Some(key);
        Ok(())
    }
    fn validate_storage(&self) -> Result<(), String> {
        if self.len() > MAX_INSTANCES
            || self.current_key.is_some()
            || self.keys.len() > visibility::MAX_KEYS
            || self.key_slots.len() != self.len()
            || self.key_slots.iter().any(|slot| {
                (*slot == UNKEYED && self.require_keys)
                    || (*slot != UNKEYED && *slot as usize >= self.keys.len())
            })
            || self.glyph_count > super::marker_text::MAX_GLYPHS
            || self.glyph_count > self.len()
            || self.mesh_count > super::marker_mesh::MAX_TRIANGLES
            || self.mesh_count > self.len().saturating_sub(self.glyph_count)
            || self.kinds.iter().filter(|k| k.is_mesh()).count() != self.mesh_count
            || self.textures.len() > super::marker_mesh::MAX_TEXTURES
            || self.textures.iter().map(|t| t.rgba.len()).sum::<usize>()
                > super::marker_mesh::MAX_TEXTURE_BYTES
            || self
                .textures
                .iter()
                .any(|t| super::robot_geometry::validate_texture(t).is_err())
            || self.kinds.iter().any(
                |k| matches!(k,Kind::TexturedTriangle(slot) if *slot as usize>=self.textures.len()),
            )
            || self
                .len()
                .saturating_sub(self.glyph_count + self.mesh_count)
                > MAX_NON_TEXT_INSTANCES
            || self.kinds.iter().filter(|k| **k == Kind::Glyph).count() != self.glyph_count
            || (self.glyph_count > 0 && self.atlas.is_none())
            || self.instances.len() != self.kinds.len()
            || (!self.is_empty() && self.projection.is_none())
            || self
                .instances
                .iter()
                .zip(&self.kinds)
                .any(|(p, kind)| !packet_valid(p, *kind, self.clock.is_some()))
        {
            return Err("Invalid primitive frame".into());
        }
        Ok(())
    }
    pub fn validate_projection(&self, camera: CameraPose, size: [u32; 2]) -> Result<(), String> {
        self.validate_storage()?;
        if !self.is_empty() && self.projection.as_ref() != Some(&ProjectionKey::new(camera, size)) {
            return Err("Marker projection is stale for this camera/viewport".into());
        }
        Ok(())
    }
    fn class(&self, i: usize) -> u8 {
        let values = alphas(&self.instances[i], self.kinds[i]);
        if let Kind::TexturedTriangle(slot) = self.kinds[i] {
            let alpha = values[0] + (1. - values[0]) * self.instances[i].columns[3][3];
            return if alpha == 0. {
                2
            } else if alpha < 1. || self.textures[slot as usize].has_transparency {
                1
            } else {
                0
            };
        }
        if self.kinds[i] == Kind::Glyph {
            return if values[0] > 0. { 1 } else { 2 };
        }
        if values.iter().all(|a| *a == 1.) {
            0
        } else if values.iter().any(|a| *a > 0.) {
            1
        } else {
            2
        }
    }
    pub fn is_transparent(&self, i: usize) -> Option<bool> {
        (i < self.len()).then(|| self.class(i) != 0)
    }
    pub fn seal(mut self) -> Result<ReadyFrame, String> {
        self.validate_storage()?;
        // Sort small indices, then permute the two payload arrays in place.
        // No second complete instance buffer is allocated for ordering.
        {
            let mut order = (0..self.len()).collect::<Vec<_>>();
            let class = |i: usize| self.class(i);
            order.sort_unstable_by(|a, b| {
                class(*a)
                    .cmp(&class(*b))
                    .then_with(|| {
                        if class(*a) == 1 {
                            self.depth(*b).unwrap().total_cmp(&self.depth(*a).unwrap())
                        } else {
                            std::cmp::Ordering::Equal
                        }
                    })
                    .then(self.kinds[*a].mesh().cmp(&self.kinds[*b].mesh()))
                    .then_with(|| self.key_slots[*a].cmp(&self.key_slots[*b]))
                    .then(a.cmp(b))
            });
            let mut destination = vec![0; self.len()];
            for (new, old) in order.into_iter().enumerate() {
                destination[old] = new;
            }
            for i in 0..self.len() {
                while destination[i] != i {
                    let to = destination[i];
                    self.instances.swap(i, to);
                    self.kinds.swap(i, to);
                    self.key_slots.swap(i, to);
                    destination.swap(i, to);
                }
            }
        }
        let mut opaque: Vec<(Kind, Range<u32>)> = Vec::new();
        let mut opaque_keys = Vec::new();
        let mut used_keys = [0u32; 16];
        let mut has_unkeyed = false;
        let mut transparent = Vec::new();
        for i in 0..self.len() {
            let class = self.class(i);
            let mesh = self.kinds[i].mesh();
            let key = self.key_slots[i];
            if class != 2 {
                if key == UNKEYED {
                    has_unkeyed = true;
                } else {
                    used_keys[key as usize / 32] |= 1 << (key % 32);
                }
            }
            if class == 0 {
                if let Some((_, range)) = opaque.last_mut().filter(|(old, range)| {
                    *old == mesh && range.end == i as u32 && opaque_keys.last() == Some(&key)
                }) {
                    range.end += 1;
                } else {
                    opaque.push((mesh, i as u32..i as u32 + 1));
                    opaque_keys.push(key);
                }
            } else if class == 1 {
                transparent.push(TransparentInstance {
                    key,
                    mesh,
                    index: i as u32,
                    depth: self.depth(i).unwrap(),
                });
            }
        }
        let mut deadlines = self
            .instances
            .iter()
            .filter_map(|p| marker_clock::deadline(p.shape))
            .collect::<Vec<_>>();
        deadlines.sort_unstable();
        deadlines.dedup();
        let has_forever = self.instances.iter().any(|p| p.shape[3] == 0);
        Ok(ReadyFrame(Arc::new(Sealed {
            frame: self,
            opaque,
            opaque_keys,
            used_keys,
            has_unkeyed,
            transparent,
            has_forever,
            deadlines,
        })))
    }
    pub fn kind(&self, index: usize) -> Option<Kind> {
        self.kinds.get(index).copied()
    }
    pub fn depth(&self, index: usize) -> Option<f32> {
        let p = self.instances.get(index)?;
        let depth = |c: [f32; 4]| {
            let d = c[2] / c[3];
            if c[3] > 0. && d.is_finite() {
                d
            } else {
                0.
            }
        };
        Some(if self.kinds[index] == Kind::Ribbon {
            p.columns.iter().map(|c| depth(*c)).sum::<f32>() / 4.
        } else if matches!(
            self.kinds[index],
            Kind::Triangle | Kind::MeshTriangle | Kind::TexturedTriangle(_)
        ) {
            p.columns[..3].iter().map(|c| depth(*c)).sum::<f32>() / 3.
        } else {
            depth(p.columns[3])
        })
    }
    pub fn image_dimensions(&self) -> impl Iterator<Item = [u32; 2]> + '_ {
        self.textures.iter().map(|t| [t.width, t.height])
    }
    pub fn atlas_dimensions(&self) -> Option<[u32; 2]> {
        self.atlas.as_ref().map(|a| a.dimensions())
    }
    pub fn rgba(&self, index: usize) -> Option<[f32; 4]> {
        self.instances.get(index).map(|p| p.rgba)
    }
}
fn project(
    kind: Kind,
    instance: &Instance,
    camera: CameraPose,
    size: [u32; 2],
) -> Result<ClipInstance, String> {
    if instance
        .scale
        .iter()
        .any(|v| !v.is_finite() || *v <= 0. || *v > 1_000_000.)
        || instance
            .rgba
            .iter()
            .any(|v| !v.is_finite() || !(0.0..=1.0).contains(v))
    {
        return Err("Invalid Marker primitive".into());
    }
    let projection = projection::Projection::new(camera, size)?;
    let origin = instance.pose.apply(Point3([0.; 3])).0;
    for mask in 0..8 {
        let p = instance.pose.apply(Point3(std::array::from_fn(|i| {
            if mask & (1 << i) == 0 {
                -instance.scale[i] * 0.5
            } else {
                instance.scale[i] * 0.5
            }
        })));
        if !super::map_geometry::bounded_point(p) {
            return Err("Marker instance bounds".into());
        }
    }

    let axes = std::array::from_fn::<_, 3, _>(|i| {
        let mut a = [0.; 3];
        a[i] = 1.;
        let end = instance.pose.apply(Point3(a)).0;
        std::array::from_fn::<_, 3, _>(|j| end[j] - origin[j])
    });
    let delta = std::array::from_fn::<_, 3, _>(|i| origin[i] - camera.origin()[i]);
    let dot = |a: [f64; 3], b: [f32; 3]| a.iter().zip(b).map(|(a, b)| a * b as f64).sum::<f64>();
    let mut columns = [[0.; 4]; 4];
    for i in 0..3 {
        columns[i] = projection.delta(axes[i].map(|v| v * instance.scale[i]), 0.)?;
    }
    columns[3] = projection.delta(delta, 1.)?;
    let eye = std::array::from_fn::<_, 3, _>(|i| {
        -delta.iter().zip(axes[i]).map(|(a, b)| a * b).sum::<f64>() / instance.scale[i]
    });
    let ray = std::array::from_fn::<_, 3, _>(|i| dot(axes[i], camera.forward) / instance.scale[i]);
    let eye = [
        eye[0] as f32,
        eye[1] as f32,
        eye[2] as f32,
        if camera.orthographic_span.is_some() {
            0.
        } else {
            1.
        },
    ];
    let ray = [ray[0] as f32, ray[1] as f32, ray[2] as f32, 0.];
    let packed = ClipInstance {
        columns,
        rgba: instance.rgba,
        eye,
        ray,
        shape: [if kind == Kind::Sphere { 1 } else { 0 }, 0, 0, 0],
    };
    if packed
        .columns
        .iter()
        .flatten()
        .chain(packed.rgba.iter())
        .chain(packed.eye.iter())
        .chain(packed.ray.iter())
        .any(|v| !v.is_finite())
    {
        return Err("Marker projection exceeds finite GPU precision".into());
    }
    Ok(packed)
}
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct Vertex {
    position: [f32; 3],
    shade: f32,
}
fn meshes() -> (Vec<Vertex>, [Range<u32>; 9]) {
    let mut out = Vec::new();
    let mut triangle = |a: [f32; 3], b: [f32; 3], c: [f32; 3]| {
        let u = std::array::from_fn::<_, 3, _>(|i| b[i] - a[i]);
        let v = std::array::from_fn::<_, 3, _>(|i| c[i] - a[i]);
        let n = [
            u[1] * v[2] - u[2] * v[1],
            u[2] * v[0] - u[0] * v[2],
            u[0] * v[1] - u[1] * v[0],
        ];
        let norm = n.iter().map(|v| v * v).sum::<f32>().sqrt();
        let shade = if norm > 0. {
            0.55 + 0.45 * (n[0] * 0.3 + n[1] * 0.4 + n[2] * 0.866).abs() / norm
        } else {
            1.
        };
        for position in [a, b, c] {
            out.push(Vertex { position, shade });
        }
    };
    let p = super::primitive_mesh::CUBE_CORNERS.map(|p| p.map(|v| v as f32));
    for [a, b, c, d] in super::primitive_mesh::CUBE_FACES {
        triangle(p[a], p[b], p[c]);
        triangle(p[a], p[c], p[d]);
    }
    let cube = 0..36;
    let start = 36;
    for i in 0..16 {
        let ring = |i: usize, z: f32| {
            let a = std::f32::consts::TAU * i as f32 / 16.;
            [0.5 * a.cos(), 0.5 * a.sin(), z]
        };
        let (a, b, c, d) = (
            ring(i, -0.5),
            ring(i + 1, -0.5),
            ring(i + 1, 0.5),
            ring(i, 0.5),
        );
        triangle(a, b, c);
        triangle(a, c, d);
        triangle([0., 0., -0.5], b, a);
        triangle([0., 0., 0.5], d, c);
    }
    let cylinder = start..start + 16 * 12;
    let start = cylinder.end;
    for i in 0..16 {
        let ring = |i: usize| {
            let a = std::f32::consts::TAU * i as f32 / 16.;
            [0.5 * a.cos(), 0.5 * a.sin(), -0.5]
        };
        let (a, b) = (ring(i), ring(i + 1));
        triangle(a, b, [0., 0., 0.5]);
        triangle([0., 0., -0.5], b, a);
    }
    let cone = start..start + 16 * 6;
    let start = cone.end;
    let quad = [
        [-0.5, -0.5, 0.],
        [0.5, -0.5, 0.],
        [0.5, 0.5, 0.],
        [-0.5, 0.5, 0.],
    ];
    for i in [0, 1, 2, 0, 2, 3] {
        out.push(Vertex {
            position: quad[i],
            shade: 1.,
        });
    }
    let point = start..start + 6;
    let start = point.end;
    for position in [[1., 0., 0.], [0., 1., 0.], [0., 0., 1.]] {
        out.push(Vertex {
            position,
            shade: 1.,
        });
    }
    let triangle = start..start + 3;
    (
        out,
        [
            cube.clone(),
            cube,
            cylinder,
            cone,
            point.clone(),
            point.clone(),
            triangle,
            point.clone(),
            point,
        ],
    )
}
/// Reusable resources for a coordinator that owns the render pass and ordering.
/// Upload/clear never waits for GPU completion on a UI frame.
pub struct Renderer {
    pub opaque: wgpu::RenderPipeline,
    pub transparent: wgpu::RenderPipeline,
    glyph_textures: atlas_gpu::Textures,
    diffuse_textures: diffuse_gpu::Textures,
    mesh: wgpu::Buffer,
    clock_buffer: wgpu::Buffer,
    clock_bind: wgpu::BindGroup,
    ranges: [Range<u32>; 9],
    instances: Option<wgpu::Buffer>,
    capacity: usize,
    len: usize,
    uploaded: Option<Weak<Sealed>>,
    rendered_ns: u64,
    #[cfg(test)]
    pub(crate) uploads: usize,
    #[cfg(test)]
    pub(crate) clock_writes: usize,
}
impl Renderer {
    pub fn new(state: &egui_wgpu::RenderState) -> Self {
        let device = &state.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Instanced Marker primitives"),
            source: wgpu::ShaderSource::Wgsl(
                include_str!("marker_primitives/primitive.wgsl").into(),
            ),
        });
        let clock_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Marker monotonic clock layout"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: std::num::NonZeroU64::new(16),
                },
                count: None,
            }],
        });
        let glyph_textures = atlas_gpu::Textures::new(state);
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Marker timed primitive layout"),
            bind_group_layouts: &[&clock_layout, &glyph_textures.layout],
            push_constant_ranges: &[],
        });
        let make = |transparent: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor{label:Some("Marker primitive depth pipeline"),layout:Some(&layout),vertex:wgpu::VertexState{module:&shader,entry_point:Some("vs_main"),compilation_options:Default::default(),buffers:&[
            wgpu::VertexBufferLayout{array_stride:16,step_mode:wgpu::VertexStepMode::Vertex,attributes:&wgpu::vertex_attr_array![0=>Float32x3,1=>Float32]},
            wgpu::VertexBufferLayout{array_stride:128,step_mode:wgpu::VertexStepMode::Instance,attributes:&wgpu::vertex_attr_array![2=>Float32x4,3=>Float32x4,4=>Float32x4,5=>Float32x4,6=>Float32x4,7=>Float32x4,8=>Float32x4,9=>Uint32x4]},
        ]},fragment:Some(wgpu::FragmentState{module:&shader,entry_point:Some("fs_main"),compilation_options:Default::default(),targets:&[Some(wgpu::ColorTargetState{format:wgpu::TextureFormat::Rgba8UnormSrgb,blend:transparent.then_some(wgpu::BlendState::ALPHA_BLENDING),write_mask:wgpu::ColorWrites::ALL})]}),primitive:wgpu::PrimitiveState{cull_mode:None,..Default::default()},depth_stencil:Some(wgpu::DepthStencilState{format:wgpu::TextureFormat::Depth32Float,depth_write_enabled:!transparent,depth_compare:wgpu::CompareFunction::LessEqual,stencil:Default::default(),bias:Default::default()}),multisample:Default::default(),multiview:None,cache:None})
        };
        let opaque = make(false);
        let transparent = make(true);
        let (vertices, ranges) = meshes();
        use wgpu::util::DeviceExt;
        let mesh = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Shared Marker unit meshes"),
            contents: bytemuck::cast_slice(&vertices),
            usage: wgpu::BufferUsages::VERTEX,
        });
        let clock_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("Marker monotonic clock"),
            contents: bytemuck::cast_slice(&[0u32; 4]),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let clock_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("Marker monotonic clock binding"),
            layout: &clock_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: clock_buffer.as_entire_binding(),
            }],
        });
        Self {
            opaque,
            transparent,
            glyph_textures,
            diffuse_textures: diffuse_gpu::Textures::default(),
            mesh,
            clock_buffer,
            clock_bind,
            ranges,
            instances: None,
            capacity: 0,
            len: 0,
            uploaded: None,
            rendered_ns: 0,
            #[cfg(test)]
            uploads: 0,
            #[cfg(test)]
            clock_writes: 0,
        }
    }
    pub fn clear(&mut self) {
        self.glyph_textures.clear();
        self.diffuse_textures.clear();
        self.instances = None;
        self.capacity = 0;
        self.len = 0;
        self.uploaded = None;
    }
    #[cfg(test)]
    pub(crate) fn atlas_uploads(&self) -> usize {
        self.glyph_textures.uploads
    }
    #[cfg(test)]
    pub(crate) fn mesh_texture_uploads(&self) -> usize {
        self.diffuse_textures.uploads
    }
    pub fn allocated_mesh_texture_bytes(&self) -> usize {
        self.diffuse_textures.bytes()
    }
    pub fn allocated_atlas_bytes(&self) -> usize {
        self.glyph_textures.bytes()
    }
    pub fn allocated_instance_bytes(&self) -> usize {
        self.capacity * INSTANCE_BYTES
    }
    pub fn upload(&mut self, state: &egui_wgpu::RenderState, frame: &Frame) -> Result<(), String> {
        frame.validate_storage()?;
        self.write(state, frame)?;
        self.update_clock(state, frame.clock.as_ref().map_or(0, MarkerClock::now_ns));
        self.uploaded = None;
        Ok(())
    }
    pub fn upload_ready(
        &mut self,
        state: &egui_wgpu::RenderState,
        frame: &ReadyFrame,
    ) -> Result<(), String> {
        if self
            .uploaded
            .as_ref()
            .and_then(Weak::upgrade)
            .is_some_and(|old| Arc::ptr_eq(&old, &frame.0))
        {
            self.update_clock(state, frame.now_ns());
            return Ok(());
        }
        self.write(state, frame)?;
        self.update_clock(state, frame.now_ns());
        if !frame.is_empty() {
            self.uploaded = Some(Arc::downgrade(&frame.0));
        }
        Ok(())
    }
    fn write(&mut self, state: &egui_wgpu::RenderState, frame: &Frame) -> Result<(), String> {
        if frame.len() * INSTANCE_BYTES > state.device.limits().max_buffer_size as usize {
            return Err("Marker instance buffer exceeds device limit".into());
        }
        if frame.is_empty() {
            self.clear();
            return Ok(());
        }
        if frame.image_dimensions().any(|s| {
            s.into_iter()
                .any(|n| n > state.device.limits().max_texture_dimension_2d)
        }) {
            return Err("Marker mesh image exceeds GPU dimensions".into());
        }
        self.glyph_textures.sync(state, frame.atlas.as_ref())?;
        self.diffuse_textures
            .sync(state, &self.glyph_textures.layout, &frame.textures);
        if frame.len() > self.capacity {
            self.capacity = frame.len();
            self.instances = Some(state.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Bounded Marker instances"),
                size: (self.capacity * INSTANCE_BYTES) as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        state.queue.write_buffer(
            self.instances.as_ref().unwrap(),
            0,
            bytemuck::cast_slice(&frame.instances),
        );
        self.len = frame.len();
        #[cfg(test)]
        {
            self.uploads += 1;
        }
        Ok(())
    }
    pub fn rendered_clock_for(&self, frame: &ReadyFrame) -> Option<u64> {
        self.uploaded
            .as_ref()
            .and_then(Weak::upgrade)
            .filter(|old| Arc::ptr_eq(old, &frame.0))
            .map(|_| self.rendered_ns)
    }
    /// Low-level pass owners must use the frame's monotonic time domain. The
    /// production coordinator calls this through upload_ready on every frame.
    pub fn update_clock(&mut self, state: &egui_wgpu::RenderState, now_ns: u64) {
        self.rendered_ns = now_ns;
        #[cfg(test)]
        {
            self.clock_writes += 1;
        }
        state.queue.write_buffer(
            &self.clock_buffer,
            0,
            bytemuck::cast_slice(&marker_clock::words(now_ns)),
        );
    }
    pub fn draw(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        kind: Kind,
        instances: Range<u32>,
        transparent: bool,
    ) {
        assert!(
            instances.start <= instances.end && instances.end as usize <= self.len,
            "Marker draw instance range"
        );
        if instances.is_empty() {
            return;
        }
        pass.set_pipeline(if transparent {
            &self.transparent
        } else {
            &self.opaque
        });
        pass.set_bind_group(0, &self.clock_bind, &[]);
        pass.set_bind_group(
            1,
            if let Kind::TexturedTriangle(slot) = kind {
                self.diffuse_textures.bind(slot)
            } else {
                self.glyph_textures.bind()
            },
            &[],
        );
        pass.set_vertex_buffer(0, self.mesh.slice(..));
        pass.set_vertex_buffer(1, self.instances.as_ref().unwrap().slice(..));
        pass.draw(self.ranges[kind.unit_index()].clone(), instances);
    }
}
#[cfg(test)]
mod tests;
