//! Small presentation snapshots share all immutable triangles, UVs and images.
//! No asset IO, model mutation, TF ownership or subscription side effects.
use super::{GeometryRole, RobotError, RobotErrorKind};
use crate::spatial::robot_geometry::PreparedRobot;
pub use studio_workspace::workspace::{LinkAppearance, RobotAppearance};

pub fn present_robot(
    source: &PreparedRobot,
    appearance: &RobotAppearance,
) -> Result<PreparedRobot, RobotError> {
    appearance
        .validate()
        .map_err(|e| RobotError::new(RobotErrorKind::InvalidModel, e.to_string()))?;
    if appearance
        .links
        .iter()
        .any(|link| !source.link_names.contains(&link.name))
    {
        return Err(RobotError::new(
            RobotErrorKind::InvalidModel,
            "Robot appearance refers to a link absent from the reviewed model",
        ));
    }
    let mut presented = source.clone();
    for visual in &mut presented.visuals {
        let name = source.link_names.get(visual.link_index).ok_or_else(|| {
            RobotError::new(
                RobotErrorKind::InvalidModel,
                "Robot appearance has an invalid prepared link index",
            )
        })?;
        let link = appearance.links.iter().find(|link| &link.name == name);
        visual.visible = match visual.role {
            GeometryRole::Visual => appearance.visual,
            GeometryRole::Collision => appearance.collision,
        } && link.is_none_or(|link| link.visible);
        visual.rgba[3] *= f32::from(appearance.opacity_permille) / 1000.
            * link.map_or(1., |link| f32::from(link.opacity_permille) / 1000.);
    }
    Ok(presented)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::robot::{load_model, LoadOptions};
    use crate::spatial::robot_geometry::prepare_robot;
    use std::{
        path::Path,
        sync::{atomic::AtomicBool, Arc},
    };
    #[test]
    fn appearance_multiplies_alpha_without_mutating_materials_or_copying_resources() {
        let path =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/robot_visualizer/rich.urdf");
        let cancel = AtomicBool::new(false);
        let model = load_model(&path, &LoadOptions::default(), &cancel).unwrap();
        let source = prepare_robot(&model, &cancel).unwrap();
        let appearance = RobotAppearance {
            opacity_permille: 500,
            links: vec![LinkAppearance {
                name: "base".into(),
                visible: true,
                opacity_permille: 400,
            }],
            ..Default::default()
        };
        let shown = present_robot(&source, &appearance).unwrap();
        for (a, b) in source.visuals.iter().zip(&shown.visuals) {
            assert!((b.rgba[3] - a.rgba[3] * 0.2).abs() < 1e-6);
            assert_eq!(a.rgba[..3], b.rgba[..3]);
            assert!(Arc::ptr_eq(&a.triangles, &b.triangles));
            if let (Some(a), Some(b)) = (&a.texture, &b.texture) {
                assert!(Arc::ptr_eq(a, b));
            }
            if let (Some(a), Some(b)) = (&a.uvs, &b.uvs) {
                assert!(Arc::ptr_eq(a, b));
            }
        }
        assert_eq!(shown.triangle_count, source.triangle_count);
        let reset = present_robot(&source, &RobotAppearance::default()).unwrap();
        for (a, b) in source.visuals.iter().zip(&reset.visuals) {
            assert_eq!(a.rgba, b.rgba);
        }
        let mut stale = appearance.clone();
        stale.links[0].name = "other".into();
        assert!(present_robot(&source, &stale).is_err());
        stale = appearance;
        stale.links[0].opacity_permille = 1001;
        assert!(present_robot(&source, &stale).is_err());
    }
    #[test]
    fn role_and_link_visibility_are_independent_and_hidden_parts_keep_the_budget() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/robot_visualizer/collision.urdf");
        let cancel = AtomicBool::new(false);
        let source = prepare_robot(
            &load_model(&path, &LoadOptions::default(), &cancel).unwrap(),
            &cancel,
        )
        .unwrap();
        let appearance = RobotAppearance {
            visual: false,
            collision: true,
            links: vec![LinkAppearance {
                name: "tip".into(),
                visible: false,
                opacity_permille: 1000,
            }],
            ..Default::default()
        };
        let shown = present_robot(&source, &appearance).unwrap();
        assert_eq!(
            shown.visuals.iter().map(|v| v.visible).collect::<Vec<_>>(),
            [false, true, false]
        );
        assert_eq!(shown.triangle_count, source.triangle_count);
        assert!(!shown.visuals[2].triangles.is_empty());
    }
    #[test]
    fn camera_fit_uses_presented_roles_and_never_hidden_or_missing_link_geometry() {
        use crate::spatial::{
            camera_fit::visible_bounds,
            combined::{CombinedSceneFrame, RobotGeometryFrame},
            RigidTransform,
        };
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/robot_visualizer/collision.urdf");
        let cancel = AtomicBool::new(false);
        let source = prepare_robot(
            &load_model(&path, &LoadOptions::default(), &cancel).unwrap(),
            &cancel,
        )
        .unwrap();
        let poses = [
            Some(RigidTransform::identity()),
            Some(RigidTransform::new([100., 0., 0.], [0., 0., 0., 1.]).unwrap()),
        ];
        let bounds = |prepared: &PreparedRobot, poses: &[Option<RigidTransform>]| {
            visible_bounds(
                &CombinedSceneFrame {
                    fixed_frame: "map",
                    robot: Some(RobotGeometryFrame { prepared, poses }),
                    layers: &[],
                    map: None,
                    pose: None,
                    path: None,
                    tf: None,
                },
                None,
                Some(1),
            )
        };
        let visual = bounds(&source, &poses).unwrap();
        assert_eq!(visual.min, [-0.5, -0.5, 0.]);
        assert_eq!(visual.max, [0.5, 0.5, 1.]);
        let collision = present_robot(
            &source,
            &RobotAppearance {
                visual: false,
                collision: true,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(bounds(&collision, &poses).unwrap().max, [101., 1., 1.2]);
        assert_eq!(
            bounds(&collision, &[poses[0], None]).unwrap().max,
            [0.6, 0.6, 1.2]
        );
        assert!(bounds(&collision, &[None, None]).is_none());
    }
}
