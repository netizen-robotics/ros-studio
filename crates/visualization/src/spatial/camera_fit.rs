//! Fit/focus calculations operate on visible fixed-frame geometry, on explicit
//! user actions only. The world grid and axes never enlarge bounds.
use super::{combined::CombinedSceneFrame, Camera, CameraMode, Point3};
use studio_workspace::presentation::CameraState;

#[derive(Clone, Copy, Debug)]
pub struct Bounds {
    pub min: [f64; 3],
    pub max: [f64; 3],
}
impl Bounds {
    pub fn corners(self) -> impl Iterator<Item = Point3> {
        (0..8).map(move |mask| {
            Point3(std::array::from_fn(|i| {
                if mask & (1 << i) == 0 {
                    self.min[i]
                } else {
                    self.max[i]
                }
            }))
        })
    }
}
fn add(bounds: &mut Option<Bounds>, p: Point3) {
    if p.0.iter().any(|v| !v.is_finite() || v.abs() > 1_000_000.) {
        return;
    }
    match bounds {
        Some(b) => {
            for i in 0..3 {
                b.min[i] = b.min[i].min(p.0[i]);
                b.max[i] = b.max[i].max(p.0[i]);
            }
        }
        None => *bounds = Some(Bounds { min: p.0, max: p.0 }),
    }
}
pub fn visible_bounds(
    frame: &CombinedSceneFrame<'_>,
    selected: Option<u64>,
    robot_id: Option<u64>,
) -> Option<Bounds> {
    visible_bounds_with_map(frame, selected, robot_id, None)
}
/// A texture map contributes only its four validated corners, never raw cells.
pub fn visible_bounds_with_map(
    frame: &CombinedSceneFrame<'_>,
    selected: Option<u64>,
    robot_id: Option<u64>,
    map: Option<super::map_texture::TextureMapFrame<'_>>,
) -> Option<Bounds> {
    visible_bounds_with_maps(frame, selected, robot_id, map.as_slice())
}
pub fn visible_bounds_with_maps(
    frame: &CombinedSceneFrame<'_>,
    selected: Option<u64>,
    robot_id: Option<u64>,
    maps: &[super::map_texture::TextureMapFrame<'_>],
) -> Option<Bounds> {
    let mut bounds = None;
    for layer in maps.iter().filter(|layer| {
        layer.appearance.opacity > 0.
            && selected.is_none_or(|id| id == layer.id)
            && layer.validate_scene(frame).is_ok()
    }) {
        for &corner in layer.map.corners() {
            add(&mut bounds, corner);
        }
    }
    for layer in frame
        .layers
        .iter()
        .filter(|l| selected.is_none_or(|id| id == l.id) && l.error.is_none())
    {
        if layer.appearance.is_some_and(|s| s.opacity == 0) {
            continue;
        }
        for p in layer.values() {
            if layer.appearance.is_some_and(|s| {
                s.color == super::sensor_appearance::ColorMode::Rgb
                    && p.rgba.is_some_and(|c| c[3] == 0)
            }) {
                continue;
            }
            add(&mut bounds, p.position);
        }
    }
    if let Some(tf) = frame.tf.filter(|tf| selected.is_none_or(|id| id == tf.id)) {
        if let Some(b) = tf.view.bounds(selected.is_some()) {
            for p in b.corners() {
                add(&mut bounds, p);
            }
        }
    }
    if selected.is_none() || selected == robot_id {
        if let Some(robot) = frame.robot {
            for visual in robot
                .prepared
                .visuals
                .iter()
                .filter(|v| v.visible && v.rgba[3] > 0.)
            {
                let Some(pose) = robot.poses.get(visual.link_index).copied().flatten() else {
                    continue;
                };
                for triangle in visual.triangles.iter() {
                    for p in triangle.0 {
                        add(&mut bounds, pose.apply(Point3(p)));
                    }
                }
            }
        }
    }
    if let Some(layer) = frame
        .map
        .filter(|l| selected.is_none_or(|id| id == l.id) && l.error.is_none())
    {
        if let Some(map) = layer.map {
            for &p in map.corners.iter() {
                add(&mut bounds, p);
            }
        }
    }
    if let Some(layer) = frame
        .pose
        .filter(|l| selected.is_none_or(|id| id == l.id) && l.error.is_none())
    {
        if let Some(pose) = layer.pose {
            if let Ok(segments) = super::overlay_geometry::pose_segments(pose) {
                for (a, b, _) in segments {
                    add(&mut bounds, a);
                    add(&mut bounds, b);
                }
            }
        }
    }
    if let Some(layer) = frame.path.filter(|l| {
        selected.is_none_or(|id| id == l.id) && l.error.is_none() && l.points.len() >= 2
    }) {
        for &p in layer.points {
            add(&mut bounds, p);
        }
    }
    bounds
}
impl Camera {
    pub fn snapshot(&self) -> CameraState {
        let mut camera = self.clone();
        camera.zoom(1.);
        CameraState {
            mode: camera.mode,
            azimuth_microrad: (camera.azimuth * 1_000_000.).round() as i32,
            elevation_microrad: (camera.elevation * 1_000_000.).round() as i32,
            target_mm: camera.target.map(|v| (v * 1000.).round() as i64),
            distance_mm: (camera.distance * 1000.).round() as u32,
            vertical_span_mm: (camera.vertical_span * 1000.).round() as u32,
        }
    }
    pub fn restore(&mut self, state: &CameraState) -> Result<(), String> {
        state.validate().map_err(|e| e.to_string())?;
        self.mode = state.mode;
        self.azimuth = state.azimuth_microrad as f32 / 1_000_000.;
        self.elevation = state.elevation_microrad as f32 / 1_000_000.;
        self.target = state.target_mm.map(|v| v as f64 / 1000.);
        self.distance = state.distance_mm as f32 / 1000.;
        self.vertical_span = state.vertical_span_mm as f32 / 1000.;
        Ok(())
    }
    /// Fit a fixed-frame bounding box to the actual viewport aspect. Error leaves
    /// the previous camera untouched; no clamped "fit" can silently omit bounds.
    pub fn fit(&mut self, bounds: Option<Bounds>, aspect: f32) -> Result<(), String> {
        let bounds = bounds.ok_or("No visible geometry is available to fit or focus")?;
        if !aspect.is_finite()
            || aspect <= 0.
            || bounds
                .corners()
                .any(|p| p.0.iter().any(|v| !v.is_finite() || v.abs() > 1_000_000.))
            || (0..3).any(|i| bounds.min[i] > bounds.max[i])
        {
            return Err("Visible bounds or viewport aspect are invalid".into());
        }
        let mut candidate = self.clone();
        candidate.zoom(1.);
        candidate.target = std::array::from_fn(|i| (bounds.min[i] + bounds.max[i]) * 0.5);
        match candidate.mode {
            CameraMode::TopDown => {
                let span = ((bounds.max[1] - bounds.min[1])
                    .max((bounds.max[0] - bounds.min[0]) / f64::from(aspect))
                    * 1.12)
                    .max(0.2);
                if span > 2000. || bounds.max[2] - bounds.min[2] > 9000. {
                    return Err("Geometry exceeds the top-down camera span/depth limits".into());
                }
                candidate.vertical_span = span as f32;
            }
            CameraMode::Orbit => {
                let pose = candidate.pose();
                let dot = |a: [f32; 3], b: [f32; 3]| {
                    a.into_iter().zip(b).map(|(x, y)| x * y).sum::<f32>()
                };
                let focal = 1. / std::f32::consts::FRAC_PI_6.tan();
                let mut distance = 0.2f32;
                for p in bounds.corners() {
                    let delta = std::array::from_fn(|i| (p.0[i] - candidate.target[i]) as f32);
                    let z = dot(delta, pose.forward);
                    distance = distance
                        .max(dot(delta, pose.right).abs() * focal / aspect - z)
                        .max(dot(delta, pose.up).abs() * focal - z)
                        .max(0.1 - z);
                }
                if distance * 1.12 > 2000. {
                    return Err("Geometry exceeds the orbit camera distance limit".into());
                }
                candidate.distance = distance * 1.12;
            }
        }
        if bounds
            .corners()
            .any(|p| !candidate.pose().visible(p, aspect))
        {
            return Err("Geometry cannot fit within the supported camera clipping range".into());
        }
        *self = candidate;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spatial::{
        combined::{PointLayerFrame, PointStyle},
        RigidTransform,
    };
    #[test]
    fn fit_uses_viewport_aspect_and_contains_all_bbox_corners_in_both_modes() {
        let bounds = Bounds {
            min: [-8., -3., -2.],
            max: [6., 7., 5.],
        };
        for mode in [CameraMode::Orbit, CameraMode::TopDown] {
            for aspect in [0.25, 1., 4.] {
                let mut camera = Camera {
                    mode,
                    ..Default::default()
                };
                camera.fit(Some(bounds), aspect).unwrap();
                assert!(bounds.corners().all(|p| camera.pose().visible(p, aspect)));
                assert_eq!(camera.target, [-1., 2., 1.5]);
            }
        }
    }
    #[test]
    fn failed_fit_is_nonmutating_and_saved_camera_precision_is_stable_at_world_limits() {
        let mut camera = Camera::default();
        let before = camera.snapshot();
        for bounds in [
            None,
            Some(Bounds {
                min: [-1_000_000.; 3],
                max: [1_000_000.; 3],
            }),
            Some(Bounds {
                min: [f64::NAN; 3],
                max: [0.; 3],
            }),
        ] {
            assert!(camera.fit(bounds, 1.).is_err());
            assert_eq!(camera.snapshot(), before);
        }
        for target_mm in [[999_999_999, -999_999_999, 123456789], [0, 1, -1]] {
            let state = CameraState {
                target_mm,
                ..Default::default()
            };
            camera.restore(&state).unwrap();
            assert_eq!(camera.snapshot(), state);
        }
    }
    #[test]
    fn fit_and_focus_ignore_empty_or_missing_layers_and_include_pose_triad() {
        let ready = [Point3([1., 2., 3.]), Point3([4., 5., 6.])];
        let invalid = [Point3([f64::NAN, 0., 0.])];
        let layers = [
            PointLayerFrame {
                samples: None,
                appearance: None,
                id: 1,
                points: &ready,
                style: PointStyle::default(),
                status: "Ready",
                error: None,
            },
            PointLayerFrame {
                samples: None,
                appearance: None,
                id: 2,
                points: &invalid,
                style: PointStyle::default(),
                status: "Missing transform",
                error: Some("Missing transform"),
            },
        ];
        let frame = CombinedSceneFrame {
            tf: None,
            fixed_frame: "map",
            robot: None,
            layers: &layers,
            map: None,
            pose: Some(crate::spatial::combined::PoseLayerFrame {
                id: 3,
                pose: Some(RigidTransform::identity()),
                status: "Ready",
                error: None,
            }),
            path: None,
        };
        let focus = visible_bounds(&frame, Some(1), None).unwrap();
        assert_eq!(focus.min, [1., 2., 3.]);
        assert_eq!(focus.max, [4., 5., 6.]);
        assert!(visible_bounds(&frame, Some(2), None).is_none());
        let pose = visible_bounds(&frame, Some(3), None).unwrap();
        assert_eq!(pose.min, [0.; 3]);
        assert_eq!(pose.max, [0.35; 3]);
        assert!(visible_bounds(&frame, Some(4), None).is_none());
    }
}

#[cfg(test)]
mod texture_map_tests {
    use super::*;
    use crate::spatial::{
        map_appearance::MapAppearance,
        map_texture::{fixture, TextureMap, TextureMapFrame},
    };
    use std::sync::Arc;
    #[test]
    fn all_maps_fit_and_only_selected_map_focuses() {
        let a = Arc::new(TextureMap::prepare(fixture(3, 2, vec![0; 6])).unwrap());
        let mut complete = fixture(4, 4, vec![100; 16]);
        Arc::get_mut(&mut complete).unwrap().header.transform[0] = 10.;
        let b = Arc::new(TextureMap::prepare(complete).unwrap());
        let maps = [
            TextureMapFrame {
                id: 1,
                map: &a,
                appearance: Default::default(),
            },
            TextureMapFrame {
                id: 2,
                map: &b,
                appearance: Default::default(),
            },
        ];
        let frame = CombinedSceneFrame {
            fixed_frame: "world",
            robot: None,
            tf: None,
            layers: &[],
            map: None,
            pose: None,
            path: None,
        };
        assert_eq!(
            visible_bounds_with_maps(&frame, None, None, &maps)
                .unwrap()
                .max,
            [14., 4., 0.]
        );
        let focus = visible_bounds_with_maps(&frame, Some(2), None, &maps).unwrap();
        assert_eq!(focus.min, [10., 0., 0.]);
        assert_eq!(focus.max, [14., 4., 0.]);
        assert!(visible_bounds_with_maps(&frame, Some(3), None, &maps).is_none());
        let mut hidden = maps;
        hidden[1].appearance.opacity = 0.;
        assert_eq!(
            visible_bounds_with_maps(&frame, None, None, &hidden)
                .unwrap()
                .max,
            [3., 2., 0.]
        );
    }
    #[test]
    fn texture_map_fit_and_focus_use_four_world_corners_without_cell_geometry() {
        let map = Arc::new(TextureMap::prepare(fixture(3, 2, vec![0; 6])).unwrap());
        let layer = TextureMapFrame {
            id: 5,
            map: &map,
            appearance: Default::default(),
        };
        let frame = CombinedSceneFrame {
            fixed_frame: "world",
            robot: None,
            tf: None,
            layers: &[],
            map: None,
            pose: None,
            path: None,
        };
        for selected in [None, Some(5)] {
            let bounds = visible_bounds_with_map(&frame, selected, None, Some(layer)).unwrap();
            assert_eq!(bounds.min, [0., 0., 0.]);
            assert_eq!(bounds.max, [3., 2., 0.]);
            let mut camera = Camera {
                mode: CameraMode::TopDown,
                ..Default::default()
            };
            camera.fit(Some(bounds), 1.).unwrap();
            assert_eq!(camera.target, [1.5, 1., 0.]);
            assert!((camera.vertical_span - 3.36).abs() < 1e-5);
        }
        assert!(visible_bounds_with_map(&frame, Some(8), None, Some(layer)).is_none());
        assert!(visible_bounds_with_map(
            &frame,
            None,
            None,
            Some(TextureMapFrame {
                appearance: MapAppearance {
                    opacity: 0.,
                    ..Default::default()
                },
                ..layer
            })
        )
        .is_none());
        let wrong = CombinedSceneFrame {
            fixed_frame: "other",
            ..frame
        };
        assert!(visible_bounds_with_map(&wrong, None, None, Some(layer)).is_none());
        assert!(visible_bounds_with_map(
            &frame,
            None,
            None,
            Some(TextureMapFrame { id: 0, ..layer })
        )
        .is_none());
    }
}
