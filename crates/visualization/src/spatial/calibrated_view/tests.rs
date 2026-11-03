use super::*;
use studio_protocol::{
    camera_info::Calibration,
    camera_snapshot::{
        transport::{Accepted, Receiver},
        Counters, Frame, InfoSource, Owner, CONTRACT_VERSION,
    },
    image::{HeaderStamp, ImageMetadata, ImageSample},
    subscriptions::{Generation, SubscriptionId},
};

pub(crate) fn fixture() -> (Receiver, Accepted, Arc<TypedImage>, RectifiedProjection) {
    let owner = Owner {
        generation: Generation(1),
        subscription: SubscriptionId(1),
        topic: "/camera/image_rect".into(),
        info: InfoSource {
            topic: "/camera/camera_info".into(),
            best_effort: false,
            transient_local: false,
        },
        fixed_frame: "world".into(),
    };
    let calibration: Calibration = serde_json::from_value(serde_json::json!({
        "stamp_ns":1,"frame_id":"optical","width":2,"height":2,
        "distortion_model":"plumb_bob","d":[],"k":[1.,0.,1.,0.,1.,1.,0.,0.,1.],
        "r":[1.,0.,0.,0.,1.,0.,0.,0.,1.],"p":[1.,0.,1.,0.,0.,1.,1.,0.,0.,0.,1.,0.],
        "binning_x":0,"binning_y":0,"roi":{"x_offset":0,"y_offset":0,"width":0,"height":0,"do_rectify":false}
    })).unwrap();
    let image = ImageSample::from_raw(
        "/camera/image_rect",
        ImageMetadata {
            header_stamp: HeaderStamp {
                seconds: 1,
                nanoseconds: 0,
            },
            frame_id: "optical".into(),
            width: 2,
            height: 2,
            encoding: "rgba8".into(),
            is_bigendian: false,
            step: 8,
        },
        &[
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ],
    )
    .unwrap();
    let decoded = Arc::new(studio_images::decode_compact_image(&image).unwrap());
    let effective = calibration.effective().unwrap();
    let projection = RectifiedProjection::new(effective.size, effective.p, 0.05, 10000.).unwrap();
    let snapshot = Snapshot {
        contract_version: CONTRACT_VERSION,
        owner: owner.clone(),
        revision: 1,
        continuity_epoch: 1,
        status: Status::Receiving,
        diagnostic: None,
        counters: Counters {
            images: 1,
            calibrations: 1,
            ..Default::default()
        },
        frame: Some(Box::new(Frame {
            image_sequence: 1,
            calibration_revision: 1,
            image,
            calibration,
            transform: LatestPose::Ready {
                resolved_stamp_ns: 1_000_000_000,
                components: [0., 0., 0., 0., 0., 0., 1.],
            },
        })),
    };
    let mut receiver = Receiver::new(owner).unwrap();
    let accepted = receiver.apply(snapshot).unwrap().unwrap();
    (receiver, accepted, decoded, projection)
}
#[test]
fn retired_camera_authority_clears_cached_pixels_and_marker_request() {
    let (mut receiver, accepted, image, projection) = fixture();
    let scene = CombinedSceneFrame {
        fixed_frame: "world",
        robot: None,
        layers: &[],
        tf: None,
        map: None,
        pose: None,
        path: None,
    };
    let frame = CalibratedFrame {
        snapshot: &accepted.snapshot,
        permission: &accepted.permission,
        image: &image,
        projection,
        scene: &scene,
        overlays: Default::default(),
    };
    let context = egui::Context::default();
    let mut view = CalibratedSceneView::default();
    assert!(view.camera(&frame).is_ok());
    studio_images::update_texture(&context, &mut view.texture, &image.frame);
    view.displayed = Arc::downgrade(&image);
    view.marker_request = Some((view.camera(&frame).unwrap(), [64, 64]));
    receiver.stop();
    let _ = context.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            assert_eq!(
                view.show(ui, &frame, None, 100.).unwrap(),
                CameraPresentation::Superseded
            );
        });
    });
    assert!(view.texture.is_none());
    assert!(view.displayed.upgrade().is_none());
    assert!(view.marker_request.is_none());
}
#[test]
fn camera_rejects_other_scene_and_invalid_presentation_without_changing_orbit() {
    let (_receiver, accepted, image, projection) = fixture();
    let mut scene = CombinedSceneFrame {
        fixed_frame: "other",
        robot: None,
        layers: &[],
        tf: None,
        map: None,
        pose: None,
        path: None,
    };
    let mut view = CalibratedSceneView::default();
    let camera = |scene: &CombinedSceneFrame<'_>, view: &CalibratedSceneView| {
        view.camera(&CalibratedFrame {
            snapshot: &accepted.snapshot,
            permission: &accepted.permission,
            image: &image,
            projection,
            scene,
            overlays: Default::default(),
        })
    };
    assert!(camera(&scene, &view).is_err());
    scene.fixed_frame = "world";
    assert!(camera(&scene, &view).is_ok());
    view.appearance.zoom_milli = 0;
    assert!(camera(&scene, &view).is_err());
    view.appearance = Appearance::default();
    view.appearance.overlay_alpha_milli = 1001;
    assert!(camera(&scene, &view).is_err());
}
