use super::{
    gpu_readback::{center_pixel, native_renderer},
    *,
};
use crate::spatial::{combined::*, map_geometry::prepare_map, uncertainty::*, RigidTransform};
use std::sync::Arc;
use studio_protocol::{
    pose_snapshot::{
        self as pose,
        transport::{Configuration, Receiver},
    },
    subscriptions::{Generation, RequestId, SubscriptionId},
    tf_graph::LatestPose,
    uncertainty::Covariance,
};

#[test]
#[ignore = "requires native Metal/wgpu; serialize depth, revocation and buffer acceptance"]
fn uncertainty_obeys_peer_depth_and_immediate_clear_revokes_cached_geometry() {
    let renderer = native_renderer();
    let mut renderer = Renderer3d::with_sensor_history(renderer.state.clone());
    let map = prepare_map(
        1,
        1,
        1.,
        RigidTransform::new([-0.5, -0.5, 0.], [0., 0., 0., 1.]).unwrap(),
        RigidTransform::identity(),
        &[0],
    )
    .unwrap();
    let frame = CombinedSceneFrame {
        fixed_frame: "world",
        robot: None,
        layers: &[],
        tf: None,
        map: Some(MapLayerFrame {
            id: 1,
            local_generation: 1,
            sequence: 1,
            map: Some(&map),
            status: "Receiving",
            error: None,
        }),
        pose: None,
        path: None,
    };
    let camera = CameraPose::top_down(2., [0.1, 0.1, 0.]);
    let visuals = egui::Visuals::dark();
    renderer
        .render_combined([128, 128], &frame, camera, &visuals)
        .unwrap();
    let baseline = center_pixel(&renderer);
    for (z, occluded) in [(-1., true), (1., false)] {
        let owner = pose::Owner {
            generation: Generation(1),
            subscription: SubscriptionId(2),
            kind: pose::Kind::PoseWithCovariance,
            topic: "/pose".into(),
            fixed_frame: "world".into(),
        };
        let mut receiver =
            Receiver::new(owner.clone(), Configuration::for_kind(owner.kind)).unwrap();
        receiver
            .apply(pose::Snapshot {
                contract_version: 1,
                owner,
                revision: 1,
                continuity_epoch: 1,
                source: Some(pose::Source {
                    frame_id: "world".into(),
                    child_frame_id: None,
                }),
                history: None,
                latest: Some(pose::Entry {
                    sequence: 1,
                    acquisition_stamp_ns: 1,
                    components: [-0.075, 0.1, z, 0., 0., 0., 1.],
                    covariance: Covariance::new([0.; 36]).unwrap(),
                    transform: LatestPose::Ready {
                        resolved_stamp_ns: 1,
                        components: [0., 0., 0., 0., 0., 0., 1.],
                    },
                }),
                trail: vec![],
                last_reset: None,
                known_history_gap: false,
                counters: Default::default(),
            })
            .unwrap();
        let prepared = Prepared::new(Arc::clone(receiver.snapshot().unwrap())).unwrap();
        let geometry = Geometry::new(&prepared, Default::default()).unwrap();
        let permission = receiver.permission().unwrap();
        let layer = Layer {
            id: 2,
            prepared: &prepared,
            geometry: &geometry,
            permission: &permission,
            width: 4,
        };
        renderer
            .render_combined_with_uncertainty(
                [128, 128],
                &frame,
                camera,
                &visuals,
                None,
                None,
                &[layer],
            )
            .unwrap();
        assert_eq!(center_pixel(&renderer) == baseline, occluded, "z={z}");
        receiver.begin_clear(RequestId(1)).unwrap();
        assert!(layer.bounds().is_none());
        renderer
            .render_combined_with_uncertainty(
                [128, 128],
                &frame,
                camera,
                &visuals,
                None,
                None,
                &[layer],
            )
            .unwrap();
        assert_eq!(
            center_pixel(&renderer),
            baseline,
            "Clear must remove even retained geometry"
        );
    }
    assert_eq!(
        MAX_HISTORY_VERTICES * std::mem::size_of::<RenderVertex>(),
        49_203_648
    );
}
