//! Immutable fixed-frame geometry for one shared viewport. The caller owns
//! source decoding, TF, activation identities and per-layer replacement/clearing.
use super::{
    robot_geometry::{robot_vertices, PreparedRobot},
    CameraPose, Point3, RenderVertex, RigidTransform,
};
use crate::robot::{RobotError, MAX_COORDINATE, MAX_LINKS, MAX_ROBOT_TRIANGLES, MAX_VISUALS};
use std::{collections::HashSet, fmt};
pub const MAX_SENSOR_LAYERS: usize = 4;
pub const MAX_LAYER_POINTS: usize = 8192;
pub const MAX_TOTAL_POINTS: usize = MAX_SENSOR_LAYERS * MAX_LAYER_POINTS;
pub const MAX_HISTORY_LAYER_POINTS: usize = super::sensor_history::MAX_HISTORY_POINTS;
pub const MAX_HISTORY_TOTAL_POINTS: usize = MAX_SENSOR_LAYERS * MAX_HISTORY_LAYER_POINTS;
pub const MAX_POINT_SIZE: f32 = 8.0;
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CombinedSceneError {
    pub message: String,
}
impl CombinedSceneError {
    pub fn new(message: impl Into<String>) -> Self {
        let mut message = message.into();
        let mut end = message.len().min(512);
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
        Self { message }
    }
}
impl fmt::Display for CombinedSceneError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.message)
    }
}
impl std::error::Error for CombinedSceneError {}
impl From<RobotError> for CombinedSceneError {
    fn from(error: RobotError) -> Self {
        Self::new(error.message)
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointStyle {
    pub rgba: [f32; 4],
    pub size_px: f32,
}
impl Default for PointStyle {
    fn default() -> Self {
        Self::for_id(1)
    }
}
impl PointStyle {
    /// Stable palette by logical owner, never selection or arrival order.
    pub fn for_id(id: u64) -> Self {
        let colors = [
            [0.02, 0.55, 0.72, 1.0],
            [0.85, 0.28, 0.03, 1.0],
            [0.48, 0.16, 0.82, 1.0],
            [0.06, 0.58, 0.18, 1.0],
        ];
        Self {
            rgba: colors[(id.wrapping_sub(1) % colors.len() as u64) as usize],
            size_px: 3.0,
        }
    }
    pub fn validate(&self) -> Result<(), CombinedSceneError> {
        if self.rgba[3] != 1.0
            || !self
                .rgba
                .iter()
                .all(|v| v.is_finite() && (0.0..=1.0).contains(v))
            || !self.size_px.is_finite()
            || !(1.0..=MAX_POINT_SIZE).contains(&self.size_px)
        {
            return Err(CombinedSceneError::new(
                "Point style requires opaque finite RGBA in [0,1] and size 1..8 pixels",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Copy)]
pub struct RobotGeometryFrame<'a> {
    pub prepared: &'a PreparedRobot,
    pub poses: &'a [Option<RigidTransform>],
}
#[derive(Clone, Copy)]
pub struct PointLayerFrame<'a> {
    /// Explicit opt-in. Legacy points must be empty when samples are supplied.
    pub samples: Option<&'a [super::sensor_history::SensorPoint]>,
    pub appearance: Option<&'a super::sensor_appearance::SensorAppearance>,
    pub id: u64,
    pub points: &'a [Point3],
    pub style: PointStyle,
    pub status: &'a str,
    pub error: Option<&'a str>,
}
impl PointLayerFrame<'_> {
    pub fn point_count(&self) -> usize {
        self.samples.map_or(self.points.len(), |s| s.len())
    }
    pub fn values(&self) -> impl Iterator<Item = super::sensor_history::SensorPoint> + '_ {
        (0..self.point_count()).map(|i| {
            self.samples.map_or_else(
                || super::sensor_history::SensorPoint {
                    position: self.points[i],
                    ..Default::default()
                },
                |s| s[i],
            )
        })
    }
}
#[derive(Clone, Copy)]
pub struct MapLayerFrame<'a> {
    pub id: u64,
    pub local_generation: u64,
    pub sequence: u64,
    pub map: Option<&'a super::map_geometry::PreparedMap>,
    pub status: &'a str,
    pub error: Option<&'a str>,
}
#[derive(Clone, Copy)]
pub struct PoseLayerFrame<'a> {
    pub id: u64,
    pub pose: Option<RigidTransform>,
    pub status: &'a str,
    pub error: Option<&'a str>,
}
#[derive(Clone, Copy)]
pub struct PathLayerFrame<'a> {
    pub id: u64,
    pub points: &'a [Point3],
    pub style: PointStyle,
    pub status: &'a str,
    pub error: Option<&'a str>,
}
#[derive(Clone, Copy)]
pub struct TfLayerFrame<'a> {
    pub id: u64,
    pub view: &'a crate::tf_graph::TfView,
}
#[derive(Clone, Copy)]
pub struct CombinedSceneFrame<'a> {
    pub tf: Option<TfLayerFrame<'a>>,
    pub fixed_frame: &'a str,
    pub robot: Option<RobotGeometryFrame<'a>>,
    pub layers: &'a [PointLayerFrame<'a>],
    pub map: Option<MapLayerFrame<'a>>,
    pub pose: Option<PoseLayerFrame<'a>>,
    pub path: Option<PathLayerFrame<'a>>,
}
impl CombinedSceneFrame<'_> {
    /// Structural/point validation is independent of the GPU. Robot projection
    /// additionally validates its prepared triangles and supplied world poses.
    pub fn validate(&self) -> Result<(), CombinedSceneError> {
        if !studio_protocol::spatial::valid_frame(self.fixed_frame) {
            return Err(CombinedSceneError::new(
                "Combined scene requires one valid fixed frame",
            ));
        }
        if self.layers.len() > MAX_SENSOR_LAYERS {
            return Err(CombinedSceneError::new(
                "Combined scene supports at most four sensor layers",
            ));
        }
        let mut ids = HashSet::with_capacity(self.layers.len());
        let mut total = 0usize;
        for layer in self.layers {
            if layer.id == 0 || !ids.insert(layer.id) {
                return Err(CombinedSceneError::new(
                    "Scene sensor layer identities must be nonzero and unique",
                ));
            }
            layer.style.validate()?;
            if layer.status.len() > 512 || layer.error.is_some_and(|e| e.len() > 512) {
                return Err(CombinedSceneError::new(
                    "Scene layer diagnostic exceeds its 512-byte bound",
                ));
            }
            if layer.error.is_some() && layer.point_count() > 0 {
                return Err(CombinedSceneError::new(
                    "A sensor layer with an error must clear its geometry before presentation",
                ));
            }
            total = total
                .checked_add(layer.point_count())
                .filter(|n| *n <= MAX_HISTORY_TOTAL_POINTS)
                .ok_or_else(|| CombinedSceneError::new("Combined sensor point budget exceeded"))?;
            let limit = match (layer.samples, layer.appearance) {
                (None, None) => MAX_LAYER_POINTS,
                (Some(_), Some(settings)) if layer.points.is_empty() => {
                    settings
                        .validate()
                        .map_err(|e| CombinedSceneError::new(e.to_string()))?;
                    MAX_HISTORY_LAYER_POINTS
                }
                _ => {
                    return Err(CombinedSceneError::new(
                        "Attributed layers require explicit appearance and no legacy points",
                    ))
                }
            };
            if layer.point_count() > limit
                || !layer.values().all(|p| {
                    p.position
                        .0
                        .iter()
                        .all(|v| v.is_finite() && v.abs() <= MAX_COORDINATE)
                        && p.intensity.is_none_or(|v| v.is_finite())
                })
            {
                return Err(CombinedSceneError::new(
                    "Sensor layer exceeds its legacy/history budget or contains invalid fixed-frame samples",
                ));
            }
        }
        let mut overlay_identity = |id: u64,
                                    status: &str,
                                    error: Option<&str>,
                                    has_geometry: bool|
         -> Result<(), CombinedSceneError> {
            if id == 0
                || !ids.insert(id)
                || status.len() > 512
                || error.is_some_and(|e| e.len() > 512)
                || (error.is_some() && has_geometry)
            {
                return Err(CombinedSceneError::new(
                    "Overlay identity, diagnostics or uncleared error geometry is invalid",
                ));
            }
            Ok(())
        };
        if let Some(layer) = self.map {
            overlay_identity(layer.id, layer.status, layer.error, layer.map.is_some())?;
            if layer.map.is_some() && (layer.local_generation == 0 || layer.sequence == 0) {
                return Err(CombinedSceneError::new(
                    "Map geometry requires local generation and sequence identity",
                ));
            }
            if let Some(map) = layer.map {
                map.validate()?;
            }
        }
        if let Some(layer) = self.pose {
            overlay_identity(layer.id, layer.status, layer.error, layer.pose.is_some())?;
            if let Some(pose) = layer.pose {
                super::overlay_geometry::pose_segments(pose)?;
            }
        }
        if let Some(layer) = self.path {
            overlay_identity(
                layer.id,
                layer.status,
                layer.error,
                !layer.points.is_empty(),
            )?;
            layer.style.validate()?;
            if layer.points.len() > 1024
                || !layer
                    .points
                    .iter()
                    .copied()
                    .all(super::map_geometry::bounded_point)
            {
                return Err(CombinedSceneError::new(
                    "Path exceeds pose or fixed-frame world budget",
                ));
            }
        }
        if let Some(tf) = self.tf {
            overlay_identity(tf.id, "", None, tf.view.ready_count() > 0)?;
            tf.view
                .validate_frame(self.fixed_frame)
                .map_err(CombinedSceneError::new)?;
        }
        if let Some(robot) = self.robot {
            let p = robot.prepared;
            let count = p
                .visuals
                .iter()
                .try_fold(0usize, |n, v| n.checked_add(v.triangles.len()));
            if p.link_names.is_empty()
                || p.link_names.len() > MAX_LINKS
                || robot.poses.len() != p.link_names.len()
                || p.visuals.len() > MAX_VISUALS
                || p.triangle_count > MAX_ROBOT_TRIANGLES
                || count != Some(p.triangle_count)
                || p.visuals.iter().any(|v| v.link_index >= p.link_names.len())
            {
                return Err(CombinedSceneError::new(
                    "Combined Robot geometry/pose identities or budgets are inconsistent",
                ));
            }
        }
        Ok(())
    }
}
/// Robot triangles plus per-layer point billboards. No grid, TF or retained data.
/// Missing/cleared layers contribute zero vertices without touching their peers.
pub fn combined_vertices(
    frame: &CombinedSceneFrame<'_>,
    camera: CameraPose,
    size: [u32; 2],
) -> Result<Vec<RenderVertex>, CombinedSceneError> {
    let mut out = Vec::new();
    frame.validate()?;
    if let Some(map) = frame.map.and_then(|m| m.map) {
        out = super::map_geometry::map_vertices(map, camera, size)?;
    }
    out.extend(content_vertices(frame, camera, size)?);
    Ok(out)
}
/// Contents after the Map: opaque Robot/sensors, then Path and oriented Pose.
pub(super) fn content_vertices(
    frame: &CombinedSceneFrame<'_>,
    camera: CameraPose,
    size: [u32; 2],
) -> Result<Vec<RenderVertex>, CombinedSceneError> {
    frame.validate()?;
    if size.contains(&0)
        || !camera
            .eye
            .iter()
            .chain(camera.right.iter())
            .chain(camera.up.iter())
            .chain(camera.forward.iter())
            .all(|v| v.is_finite())
    {
        return Err(CombinedSceneError::new(
            "Combined viewport/camera must be finite and nonempty",
        ));
    }
    let points = frame
        .layers
        .iter()
        .map(PointLayerFrame::point_count)
        .sum::<usize>();
    let robot_count = frame.robot.map_or(0, |r| r.prepared.triangle_count * 3);
    let mut out = Vec::with_capacity(robot_count + points * 6);
    if let Some(robot) = frame.robot {
        out.extend(robot_vertices(robot.prepared, robot.poses, camera, size)?);
    }
    let aspect = size[0] as f32 / size[1] as f32;
    for layer in frame.layers {
        let range = layer
            .appearance
            .and_then(|s| super::sensor_appearance::range(s, layer.values()));
        for sample in layer.values() {
            let point = sample.position;
            let color = layer.appearance.map_or(layer.style.rgba, |s| {
                super::sensor_appearance::color(s, sample, range)
            });
            if color[3] == 0. {
                continue;
            }
            let p = camera.clip(point, aspect);
            if !p.iter().all(|v| v.is_finite()) {
                return Err(CombinedSceneError::new(
                    "Combined point projection is nonfinite",
                ));
            }
            if p[3] <= 0.0 || p[2] < 0.0 || p[2] > p[3] {
                continue;
            }
            let half = layer.appearance.map_or(
                [
                    layer.style.size_px / size[0] as f32 * p[3],
                    layer.style.size_px / size[1] as f32 * p[3],
                ],
                |s| super::sensor_appearance::half_extents(s, camera, point, size, p),
            );
            if half.iter().any(|v| !v.is_finite()) {
                return Err(CombinedSceneError::new(
                    "Sensor square projection is nonfinite",
                ));
            }
            let corner = |x: f32, y: f32| [p[0] + x * half[0], p[1] + y * half[1], p[2], p[3]];
            let corners = [
                corner(-1.0, -1.0),
                corner(1.0, -1.0),
                corner(1.0, 1.0),
                corner(-1.0, 1.0),
            ];
            for index in [0, 1, 2, 0, 2, 3] {
                out.push(RenderVertex {
                    clip: corners[index],
                    color,
                });
            }
        }
    }
    if let Some(path) = frame.path {
        out.extend(super::overlay_geometry::path_vertices(
            path.points,
            path.style,
            camera,
            size,
        )?);
    }
    if let Some(tf) = frame.tf {
        out.extend(tf.view.vertices(camera, size)?);
    }
    if let Some(pose) = frame.pose.and_then(|p| p.pose) {
        out.extend(super::overlay_geometry::pose_vertices(pose, camera, size)?);
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    fn layer(id: u64, points: &[Point3]) -> PointLayerFrame<'_> {
        PointLayerFrame {
            samples: None,
            appearance: None,
            id,
            points,
            style: PointStyle::for_id(id),
            status: "Receiving",
            error: None,
        }
    }
    #[test]
    fn clearing_one_layer_preserves_peer_geometry_and_style() {
        let points = [Point3([0.0, 0.0, 0.5])];
        let mut layers = [layer(1, &points), layer(2, &points)];
        let camera = CameraPose::orbit(0.5, 0.8, 12.0, [0.0; 3]);
        let frame = CombinedSceneFrame {
            tf: None,
            map: None,
            pose: None,
            path: None,
            fixed_frame: "map",
            robot: None,
            layers: &layers,
        };
        let both = combined_vertices(&frame, camera, [640, 480]).unwrap();
        assert_eq!(both.len(), 12);
        layers[0].points = &[];
        layers[0].error = Some("Waiting for acquisition-time transform");
        let frame = CombinedSceneFrame {
            tf: None,
            map: None,
            pose: None,
            path: None,
            fixed_frame: "map",
            robot: None,
            layers: &layers,
        };
        let remaining = combined_vertices(&frame, camera, [640, 480]).unwrap();
        assert_eq!(remaining.len(), 6);
        assert!(remaining.iter().all(|v| v.color == layers[1].style.rgba));
        assert_eq!(remaining[0].clip, both[6].clip);
    }
    #[test]
    fn identities_styles_stale_geometry_and_point_bounds_fail_without_truncation() {
        let points = [Point3([0.0; 3])];
        let mut layers = [layer(1, &points), layer(1, &points)];
        assert!(CombinedSceneFrame {
            tf: None,
            map: None,
            pose: None,
            path: None,
            fixed_frame: "map",
            robot: None,
            layers: &layers
        }
        .validate()
        .is_err());
        layers[1].id = 2;
        layers[1].style.rgba[3] = 0.5;
        assert!(CombinedSceneFrame {
            tf: None,
            map: None,
            pose: None,
            path: None,
            fixed_frame: "map",
            robot: None,
            layers: &layers
        }
        .validate()
        .is_err());
        layers[1].style = PointStyle::default();
        layers[1].error = Some("Missing TF");
        assert!(CombinedSceneFrame {
            tf: None,
            map: None,
            pose: None,
            path: None,
            fixed_frame: "map",
            robot: None,
            layers: &layers
        }
        .validate()
        .is_err());
        let oversized = vec![Point3([0.0; 3]); MAX_LAYER_POINTS + 1];
        let layers = [layer(1, &oversized)];
        assert!(CombinedSceneFrame {
            tf: None,
            map: None,
            pose: None,
            path: None,
            fixed_frame: "map",
            robot: None,
            layers: &layers
        }
        .validate()
        .is_err());
        let invalid = [Point3([f64::NAN, 0.0, 0.0])];
        let layers = [layer(1, &invalid)];
        assert!(CombinedSceneFrame {
            tf: None,
            map: None,
            pose: None,
            path: None,
            fixed_frame: "map",
            robot: None,
            layers: &layers
        }
        .validate()
        .is_err());
    }
    #[test]
    fn maximum_sensor_geometry_fits_reviewed_vertex_budget() {
        let points = vec![Point3([0.0, 0.0, 0.5]); MAX_LAYER_POINTS];
        let layers =
            std::array::from_fn::<_, MAX_SENSOR_LAYERS, _>(|i| layer(i as u64 + 1, &points));
        let frame = CombinedSceneFrame {
            tf: None,
            map: None,
            pose: None,
            path: None,
            fixed_frame: "map",
            robot: None,
            layers: &layers,
        };
        assert_eq!(
            combined_vertices(
                &frame,
                CameraPose::orbit(0.5, 0.8, 12.0, [0.0; 3]),
                [2048, 2048]
            )
            .unwrap()
            .len(),
            MAX_TOTAL_POINTS * 6
        );
        assert_eq!(
            (MAX_TOTAL_POINTS + 45) * 6 + MAX_ROBOT_TRIANGLES * 3,
            346878
        );
        assert!(CombinedSceneError::new("🤖".repeat(200)).message.len() <= 512);
    }
}
