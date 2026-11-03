//! Complete fixed-frame pose/error models; covariance decomposition stays off UI.
use super::{
    combined::CombinedSceneError, map_geometry::bounded_point, overlay_geometry::ribbon,
    CameraPose, Point3, RenderVertex, RigidTransform,
};
use std::sync::Arc;
use studio_protocol::{
    latest_pose::LatestPose,
    pose_snapshot::{Entry, Snapshot, MAX_HISTORY},
    uncertainty::{Marginal, PrincipalAxes},
};
pub use studio_workspace::uncertainty::Appearance;
pub const MAX_SEGMENTS: usize = 2 * (MAX_HISTORY * 63 + 219);
pub const MAX_VERTICES: usize = MAX_SEGMENTS * 6;
const AXES: [[f32; 4]; 3] = [
    [0.95, 0.25, 0.2, 1.],
    [0.25, 0.8, 0.3, 1.],
    [0.25, 0.55, 1., 1.],
];
#[derive(Clone, Debug)]
pub struct PreparedEntry {
    pub sequence: u64,
    pub pose: RigidTransform,
    covariance_frame: RigidTransform,
    position: PrincipalAxes,
    angular: [[f64; 3]; 3],
}
#[cfg(test)]
mod tests;
#[derive(Clone, Debug)]
pub struct Prepared {
    pub snapshot: Arc<Snapshot>,
    pub head: Option<PreparedEntry>,
    pub trail: Vec<PreparedEntry>,
}
fn prepare(entry: &Entry) -> Result<Option<PreparedEntry>, String> {
    let LatestPose::Ready { components, .. } = &entry.transform else {
        return Ok(None);
    };
    let tf = RigidTransform::new(
        [components[0], components[1], components[2]],
        [components[3], components[4], components[5], components[6]],
    )
    .ok_or("Invalid covariance capture transform")?;
    let source = RigidTransform::new(
        [
            entry.components[0],
            entry.components[1],
            entry.components[2],
        ],
        [
            entry.components[3],
            entry.components[4],
            entry.components[5],
            entry.components[6],
        ],
    )
    .ok_or("Invalid covariance source pose")?;
    let pose = tf.compose(source);
    let origin = pose.apply(Point3([0.; 3]));
    if !bounded_point(origin) {
        return Ok(None);
    }
    let covariance_frame = RigidTransform::new(
        origin.0,
        [components[3], components[4], components[5], components[6]],
    )
    .unwrap();
    Ok(Some(PreparedEntry {
        sequence: entry.sequence,
        pose,
        covariance_frame,
        position: entry.covariance.principal_axes(Marginal::Position)?,
        angular: entry.covariance.marginal(Marginal::FixedAxisRotation),
    }))
}
impl Prepared {
    pub fn new(snapshot: Arc<Snapshot>) -> Result<Self, String> {
        snapshot.validate()?;
        let head = snapshot.latest.as_ref().map(prepare).transpose()?.flatten();
        let trail = snapshot
            .trail
            .iter()
            .map(prepare)
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .flatten()
            .collect();
        Ok(Self {
            snapshot,
            head,
            trail,
        })
    }
}
#[derive(Clone, Copy, Debug)]
pub struct Segment {
    pub a: Point3,
    pub b: Point3,
    pub color: [f32; 4],
}
#[derive(Clone, Debug, Default)]
pub struct Geometry {
    pub segments: Vec<Segment>,
    pub hidden_shapes: usize,
    pub capped_angles: usize,
    source: std::sync::Weak<Snapshot>,
}
#[derive(Clone, Copy)]
pub struct Layer<'a> {
    pub id: u64,
    pub prepared: &'a Prepared,
    pub geometry: &'a Geometry,
    pub permission: &'a studio_protocol::pose_snapshot::transport::Permission,
    pub width: u8,
}
impl Layer<'_> {
    pub fn is_current(&self) -> bool {
        self.id != 0
            && self.permission.authorizes(&self.prepared.snapshot)
            && self
                .geometry
                .source
                .upgrade()
                .is_some_and(|s| Arc::ptr_eq(&s, &self.prepared.snapshot))
    }
    pub fn bounds(&self) -> Option<super::camera_fit::Bounds> {
        self.is_current().then(|| self.geometry.bounds()).flatten()
    }
}
impl Geometry {
    fn line(&mut self, a: Point3, b: Point3, color: [f32; 4]) {
        if a != b {
            self.segments.push(Segment { a, b, color });
        }
    }
    fn ring(
        &mut self,
        center: RigidTransform,
        u: [f64; 3],
        v: [f64; 3],
        samples: usize,
        color: [f32; 4],
    ) {
        let points: Vec<_> = (0..samples)
            .map(|i| {
                let angle = std::f64::consts::TAU * i as f64 / samples as f64;
                center.apply(Point3(std::array::from_fn(|j| {
                    u[j] * angle.cos() + v[j] * angle.sin()
                })))
            })
            .collect();
        let origin = center.apply(Point3([0.; 3]));
        if points.iter().any(|p| !bounded_point(*p)) || !bounded_point(origin) {
            self.hidden_shapes += 1;
            return;
        }
        for i in 0..samples {
            self.line(points[i], points[(i + 1) % samples], color);
        }
    }
    fn glyph(&mut self, e: &PreparedEntry, appearance: Appearance, history: bool) {
        let length = f64::from(appearance.pose_length_mm) / 1000.;
        let origin = e.pose.apply(Point3([0.; 3]));
        for i in 0..3 {
            let mut axis = [0.; 3];
            axis[i] = length;
            let end = e.pose.apply(Point3(axis));
            if bounded_point(end) {
                let mut color = AXES[i];
                if history {
                    for c in &mut color[..3] {
                        *c = *c * 0.55 + 0.2;
                    }
                }
                self.line(origin, end, color);
            } else {
                self.hidden_shapes += 1;
            }
        }
        if history && !appearance.history_covariance {
            return;
        }
        let samples = if history { 8 } else { 32 };
        let sigma = f64::from(appearance.sigma_milli) / 1000.;
        if appearance.position {
            for (a, b) in [(0, 1), (0, 2), (1, 2)] {
                let u =
                    e.position.directions[a].map(|v| v * e.position.variances[a].sqrt() * sigma);
                let v =
                    e.position.directions[b].map(|v| v * e.position.variances[b].sqrt() * sigma);
                self.ring(e.covariance_frame, u, v, samples, [0.96, 0.73, 0.16, 1.]);
            }
        }
        if appearance.orientation {
            // Rotation error about k moves direction i by e_k cross e_i.
            for axis in 0..3 {
                let a = (axis + 1) % 3;
                let b = (axis + 2) % 3;
                let xx = e.angular[b][b];
                let yy = e.angular[a][a];
                let xy = -e.angular[b][a];
                let mid = (xx + yy) * 0.5;
                let delta = ((xx - yy) * 0.5).hypot(xy);
                let angle = 0.5 * (2. * xy).atan2(xx - yy);
                let radii = [
                    (mid + delta).max(0.).sqrt() * sigma,
                    (mid - delta).max(0.).sqrt() * sigma,
                ];
                let limit = 89_f64.to_radians();
                self.capped_angles += radii.iter().filter(|v| **v > limit).count();
                let r = radii.map(|r| length * r.min(limit).tan());
                let mut u = [0.; 3];
                let mut v = [0.; 3];
                u[a] = r[0] * angle.cos();
                u[b] = r[0] * angle.sin();
                v[a] = -r[1] * angle.sin();
                v[b] = r[1] * angle.cos();
                let mut offset = [0.; 3];
                offset[axis] = length;
                let center = e
                    .covariance_frame
                    .compose(RigidTransform::new(offset, [0., 0., 0., 1.]).unwrap());
                let start = self.segments.len();
                self.ring(center, u, v, samples, AXES[axis]);
                if self.segments.len() > start {
                    for i in 0..4 {
                        let theta = std::f64::consts::FRAC_PI_2 * i as f64;
                        let tip = center.apply(Point3(std::array::from_fn(|j| {
                            u[j] * theta.cos() + v[j] * theta.sin()
                        })));
                        if bounded_point(tip) {
                            self.line(origin, tip, AXES[axis]);
                        }
                    }
                }
            }
        }
    }
    pub fn new(prepared: &Prepared, appearance: Appearance) -> Result<Self, String> {
        appearance.validate()?;
        let mut result = Self {
            source: Arc::downgrade(&prepared.snapshot),
            ..Self::default()
        };
        for entry in &prepared.trail {
            if prepared
                .head
                .as_ref()
                .is_none_or(|head| head.sequence != entry.sequence)
            {
                result.glyph(entry, appearance, true);
            }
        }
        if let Some(head) = &prepared.head {
            result.glyph(head, appearance, false);
        }
        if result.segments.len() > MAX_SEGMENTS / 2 {
            return Err("Uncertainty segment budget exceeded".into());
        }
        Ok(result)
    }
    pub fn vertices(
        &self,
        camera: CameraPose,
        size: [u32; 2],
        width: u8,
    ) -> Result<Vec<RenderVertex>, CombinedSceneError> {
        if self.segments.len() > MAX_SEGMENTS / 2 || !(1..=8).contains(&width) || size.contains(&0)
        {
            return Err(CombinedSceneError::new(
                "Uncertainty segment or projection budget exceeded",
            ));
        }
        let mut result = Vec::with_capacity(self.segments.len() * 6);
        for s in &self.segments {
            ribbon(
                &mut result,
                camera,
                size,
                s.a,
                s.b,
                s.color,
                f32::from(width),
            )?;
        }
        Ok(result)
    }
    pub fn bounds(&self) -> Option<super::camera_fit::Bounds> {
        let mut points = self.segments.iter().flat_map(|s| [s.a, s.b]);
        let first = points.next()?;
        let mut bounds = super::camera_fit::Bounds {
            min: first.0,
            max: first.0,
        };
        for point in points {
            for i in 0..3 {
                bounds.min[i] = bounds.min[i].min(point.0[i]);
                bounds.max[i] = bounds.max[i].max(point.0[i]);
            }
        }
        Some(bounds)
    }
}
