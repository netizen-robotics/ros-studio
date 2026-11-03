//! Independent scene/clip-space checks. These do not qualify GPU presentation.
use studio_desktop::spatial::{
    Camera, CameraPose, Clock, Point3, RigidTransform, Scan, Scene, SceneError, TfOutcome,
};

fn scan(ranges: &[f32], stamp_ns: i64) -> Scan<'_> {
    Scan {
        frame: "laser",
        stamp_ns,
        clock: Clock::System,
        complete: true,
        angle_min: 0.0,
        angle_increment: std::f64::consts::FRAC_PI_2,
        range_min: 0.1,
        range_max: 10.0,
        ranges,
    }
}

fn transformed(stamp_ns: i64, transform: RigidTransform) -> TfOutcome<'static> {
    TfOutcome::Ready {
        source: "laser",
        target: "map",
        stamp_ns,
        transform,
    }
}

#[test]
fn tilted_scan_preserves_height_and_tf_failure_clears_only_its_layer() {
    let mut scene = Scene::default();
    scene.set_fixed_frame("map").unwrap();
    scene.add_layer(1).unwrap();
    scene.add_layer(2).unwrap();
    scene.start().unwrap();
    let first = scene.layer_token(1).unwrap();
    let peer = scene.layer_token(2).unwrap();
    let half = std::f64::consts::FRAC_PI_4;
    let tilt = RigidTransform::new([0.0, 0.0, 1.5], [0.0, half.sin(), 0.0, half.cos()]).unwrap();
    scene
        .update(first, scan(&[1.0, 1.0, 1.0], 1), transformed(1, tilt))
        .unwrap();
    scene
        .update(
            peer,
            scan(&[1.0], 1),
            transformed(1, RigidTransform::identity()),
        )
        .unwrap();
    let points = scene.layers()[0].points();
    assert!((points[0].0[2] - 0.5).abs() < 1e-6);
    assert!((points[2].0[2] - 2.5).abs() < 1e-6);
    // The tilted scan and an off-plane point form true noncoplanar geometry.
    let a = points[0].0;
    let b = points[1].0;
    let c = points[2].0;
    let p = scene.layers()[1].points()[0].0;
    let sub = |x: [f64; 3], y: [f64; 3]| [x[0] - y[0], x[1] - y[1], x[2] - y[2]];
    let u = sub(b, a);
    let v = sub(c, a);
    let w = sub(p, a);
    let volume = (u[1] * v[2] - u[2] * v[1]) * w[0]
        + (u[2] * v[0] - u[0] * v[2]) * w[1]
        + (u[0] * v[1] - u[1] * v[0]) * w[2];
    assert!(volume.abs() > 0.5);
    assert_eq!(
        scene.update(first, scan(&[1.0], 2), TfOutcome::Missing),
        Err(SceneError::MissingTf)
    );
    assert!(scene.layers()[0].points().is_empty());
    assert_eq!(scene.layers()[1].points().len(), 1);
    scene.stop();
    scene.start().unwrap();
    assert_eq!(
        scene.update(first, scan(&[1.0], 3), transformed(3, tilt)),
        Err(SceneError::StaleGeneration)
    );
    assert!(scene.layers().iter().all(|layer| layer.points().is_empty()));
}

fn point_on_view(pose: &CameraPose, depth: f32, side: f32) -> Point3 {
    Point3(std::array::from_fn(|i| {
        f64::from(pose.eye[i] + pose.forward[i] * depth + pose.right[i] * side)
    }))
}

#[test]
fn perspective_has_depth_and_rejects_behind_eye_and_clipping_planes() {
    let pose = CameraPose::orbit(0.7, 0.4, 10.0, [0.0; 3]);
    let near = pose.clip(point_on_view(&pose, 2.0, 0.2), 1.5);
    let far = pose.clip(point_on_view(&pose, 4.0, 0.2), 1.5);
    assert!((near[3] - 2.0).abs() < 1e-4);
    assert!((far[3] - 4.0).abs() < 1e-4);
    assert!((near[0] / near[3]).abs() > (far[0] / far[3]).abs() * 1.9);
    assert!(near[2] / near[3] < far[2] / far[3]);
    assert!(pose.visible(point_on_view(&pose, 2.0, 0.0), 1.5));
    assert!(!pose.visible(point_on_view(&pose, -1.0, 0.0), 1.5));
    assert!(!pose.visible(point_on_view(&pose, 0.01, 0.0), 1.5));
    assert!(!pose.visible(point_on_view(&pose, 20_000.0, 0.0), 1.5));
}

#[test]
fn orbit_pan_zoom_and_reset_change_and_restore_camera_state() {
    let mut camera = Camera::default();
    let initial = camera.pose();
    camera.orbit_delta([80.0, 40.0]);
    assert_ne!(camera.pose().eye, initial.eye);
    let before_pan = camera.target;
    camera.pan_delta([20.0, -15.0]);
    assert_ne!(camera.target, before_pan);
    let before_zoom = camera.distance;
    camera.zoom(1.25);
    assert!(camera.distance < before_zoom);
    assert!((camera.distance - before_zoom / 1.25).abs() < 1e-5);
    let changed = camera.pose();
    assert!(changed
        .eye
        .iter()
        .chain(changed.forward.iter())
        .all(|x| x.is_finite()));
    camera.reset();
    assert_eq!(camera.pose().eye, initial.eye);
    assert_eq!(camera.pose().forward, initial.forward);
    assert_eq!(camera.target, Camera::default().target);
    assert_eq!(camera.distance, Camera::default().distance);
}

/// Synthetic egui events establish control routing only, not native/GPU behavior.
#[test]
fn primary_drag_orbits_and_secondary_drag_pans_in_camera_basis() {
    use eframe::egui;
    let context = egui::Context::default();
    context.enable_accesskit();
    let mut camera = Camera::default();
    let mut scene = Scene::default();
    scene.set_fixed_frame("map").unwrap();
    scene.add_layer(1).unwrap();
    scene.start().unwrap();
    scene
        .update(
            scene.layer_token(1).unwrap(),
            scan(&[1.0, 2.0], 1),
            transformed(1, RigidTransform::identity()),
        )
        .unwrap();
    let geometry = scene.layers()[0].points().to_vec();
    let draw = |camera: &mut Camera, events| {
        context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 800.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| camera.show(ui, &scene));
            },
        )
    };
    draw(&mut camera, vec![]);
    let output = draw(&mut camera, vec![]);
    let tree = output.platform_output.accesskit_update.unwrap();
    let bounds = tree
        .nodes
        .iter()
        .find(|(_, node)| {
            node.label()
                .is_some_and(|s| s.starts_with("Perspective 3D spatial scene."))
        })
        .unwrap()
        .1
        .bounds()
        .unwrap();
    let start = egui::pos2(
        ((bounds.x0 + bounds.x1) / 2.0) as f32,
        ((bounds.y0 + bounds.y1) / 2.0) as f32,
    );
    let drag = |camera: &mut Camera, button, from: egui::Pos2, delta: egui::Vec2| {
        draw(camera, vec![egui::Event::PointerMoved(from)]);
        draw(
            camera,
            vec![egui::Event::PointerButton {
                pos: from,
                button,
                pressed: true,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        draw(camera, vec![egui::Event::PointerMoved(from + delta)]);
        draw(
            camera,
            vec![egui::Event::PointerButton {
                pos: from + delta,
                button,
                pressed: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
    };
    let initial = (
        camera.azimuth,
        camera.elevation,
        camera.target,
        camera.distance,
    );
    drag(
        &mut camera,
        egui::PointerButton::Primary,
        start,
        egui::vec2(40.0, 20.0),
    );
    assert!((camera.azimuth - initial.0 - 0.4).abs() < 1e-5);
    assert!((camera.elevation - initial.1 + 0.2).abs() < 1e-5);
    assert_eq!(camera.target, initial.2);
    assert_eq!(camera.distance, initial.3);
    let before_pan = camera.clone();
    let basis = before_pan.pose();
    let delta = egui::vec2(30.0, -15.0);
    drag(&mut camera, egui::PointerButton::Secondary, start, delta);
    assert_eq!(camera.azimuth, before_pan.azimuth);
    assert_eq!(camera.elevation, before_pan.elevation);
    for i in 0..3 {
        let expected = before_pan.target[i]
            + f64::from(
                (-basis.right[i] * delta.x + basis.up[i] * delta.y) * before_pan.distance * 0.002,
            );
        assert!((camera.target[i] - expected).abs() < 1e-5);
    }
    drag(
        &mut camera,
        egui::PointerButton::Secondary,
        start + delta,
        -delta,
    );
    for i in 0..3 {
        assert!((camera.target[i] - before_pan.target[i]).abs() < 1e-5);
    }
    assert_eq!(scene.layers()[0].points(), geometry);
    assert!(scene.is_active());
}

#[test]
fn keyboard_space_activates_focused_zoom_and_reset_buttons() {
    use eframe::egui;
    let context = egui::Context::default();
    context.enable_accesskit();
    let mut camera = Camera::default();
    let scene = Scene::default();
    let draw = |camera: &mut Camera, events| {
        context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1000.0, 800.0),
                )),
                events,
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| camera.show(ui, &scene));
            },
        )
    };
    for (label, expected) in [
        ("Zoom in", Camera::default().distance / 1.25),
        ("Reset camera", Camera::default().distance),
    ] {
        let output = draw(&mut camera, vec![]);
        let tree = output.platform_output.accesskit_update.unwrap();
        let target = tree
            .nodes
            .iter()
            .find(|(_, node)| node.label() == Some(label))
            .unwrap()
            .0;
        draw(
            &mut camera,
            vec![egui::Event::AccessKitActionRequest(
                egui::accesskit::ActionRequest {
                    action: egui::accesskit::Action::Focus,
                    target,
                    data: None,
                },
            )],
        );
        for pressed in [true, false] {
            draw(
                &mut camera,
                vec![egui::Event::Key {
                    key: egui::Key::Space,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
        }
        assert!(
            (camera.distance - expected).abs() < 1e-5,
            "{label} must respond to keyboard activation"
        );
    }
}
