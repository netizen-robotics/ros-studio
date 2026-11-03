use super::{
    gpu_readback::{center_pixel, native_renderer},
    *,
};
use crate::spatial::{
    marker_geometry::Instance,
    marker_primitives::{
        Frame as PrimitiveFrame, Kind, Renderer as PrimitiveRenderer, MAX_INSTANCES,
        MAX_INSTANCE_BYTES, MAX_NON_TEXT_INSTANCES,
    },
    RigidTransform,
};
fn add(
    frame: &mut PrimitiveFrame,
    kind: Kind,
    position: [f64; 3],
    scale: [f64; 3],
    rgba: [f32; 4],
    camera: CameraPose,
) {
    frame
        .push(
            kind,
            &Instance {
                pose: RigidTransform::new(position, [0., 0., 0., 1.]).unwrap(),
                scale,
                rgba,
            },
            camera,
            [128, 128],
        )
        .unwrap();
}
fn draw(renderer: &Renderer3d, gpu: &PrimitiveRenderer, frame: &PrimitiveFrame) {
    let target = renderer.frame.as_ref().unwrap();
    let mut encoder = renderer
        .state
        .device
        .create_command_encoder(&Default::default());
    {
        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Marker primitive component acceptance"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &target.color_view,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                view: &target.depth_view,
                depth_ops: Some(wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                }),
                stencil_ops: None,
            }),
            timestamp_writes: None,
            occlusion_query_set: None,
        });
        for index in 0..frame.len() {
            gpu.draw(
                &mut pass,
                frame.kind(index).unwrap(),
                index as u32..index as u32 + 1,
                frame.is_transparent(index).unwrap(),
            );
        }
    }
    renderer.state.queue.submit([encoder.finish()]);
}
fn clear(renderer: &mut Renderer3d) {
    renderer.render_vertices_clear([128, 128], &[], [0., 0., 0., 1.]);
}
#[test]
#[ignore = "requires native GPU; serialize primitive silhouettes, depth, alpha and lifecycle"]
fn marker_primitive_sphere_surface_depth_alpha_and_resource_lifecycle() {
    let mut renderer = native_renderer();
    let mut gpu = PrimitiveRenderer::new(&renderer.state);
    let camera = CameraPose::top_down(4., [0.; 3]);
    for kind in [Kind::Cube, Kind::Sphere, Kind::Cylinder, Kind::Cone] {
        let mut frame = PrimitiveFrame::default();
        add(&mut frame, kind, [0.; 3], [2.; 3], [1., 0., 0., 1.], camera);
        clear(&mut renderer);
        gpu.upload(&renderer.state, &frame).unwrap();
        draw(&renderer, &gpu, &frame);
        let p = center_pixel(&renderer);
        assert!(p[0] > 150 && p[1] == 0 && p[2] == 0, "{kind:?}: {p:?}");
    }
    // Nonuniform sphere dimensions define an ellipsoid, including silhouette
    // discard inside the box but outside its analytic surface.
    for (target, visible) in [([1.5, 0., 0.], true), ([1.7, 0.4, 0.], false)] {
        let camera = CameraPose::top_down(4., target);
        let mut frame = PrimitiveFrame::default();
        add(
            &mut frame,
            Kind::Sphere,
            [0.; 3],
            [4., 1., 2.],
            [1., 0., 0., 1.],
            camera,
        );
        clear(&mut renderer);
        gpu.upload(&renderer.state, &frame).unwrap();
        draw(&renderer, &gpu, &frame);
        assert_eq!(
            center_pixel(&renderer)[0] > 150,
            visible,
            "Ellipsoid silhouette at {target:?}"
        );
    }
    let outside = CameraPose::orbit(0.5, 0.3, 4., [0.; 3]);
    let mut frame = PrimitiveFrame::default();
    add(
        &mut frame,
        Kind::Sphere,
        [0.; 3],
        [2.; 3],
        [1., 0., 0., 1.],
        outside,
    );
    clear(&mut renderer);
    gpu.upload(&renderer.state, &frame).unwrap();
    draw(&renderer, &gpu, &frame);
    assert!(center_pixel(&renderer)[0] > 150, "Perspective sphere");
    // The sphere bounding box reaches z=1, but its actual surface at x=.8 is
    // z=.6. An opaque cube top at z=.75 must win here even if sphere draws last.
    let camera = CameraPose::top_down(4., [0.8, 0., 0.]);
    let mut frame = PrimitiveFrame::default();
    add(
        &mut frame,
        Kind::Cube,
        [0.8, 0., 0.25],
        [0.5, 0.5, 1.],
        [0., 1., 0., 1.],
        camera,
    );
    add(
        &mut frame,
        Kind::Sphere,
        [0.; 3],
        [2.; 3],
        [1., 0., 0., 1.],
        camera,
    );
    clear(&mut renderer);
    gpu.upload(&renderer.state, &frame).unwrap();
    draw(&renderer, &gpu, &frame);
    let p = center_pixel(&renderer);
    assert!(p[1] > 150 && p[0] == 0, "Surface depth: {p:?}");
    // A center ray receives one analytic sphere surface, without blending both
    // bounding-box faces. sRGB conversion maps linear half alpha near188.
    let mut frame = PrimitiveFrame::default();
    add(
        &mut frame,
        Kind::Sphere,
        [0.; 3],
        [2.; 3],
        [1., 0., 0., 0.5],
        CameraPose::top_down(4., [0.; 3]),
    );
    clear(&mut renderer);
    gpu.upload(&renderer.state, &frame).unwrap();
    draw(&renderer, &gpu, &frame);
    let p = center_pixel(&renderer);
    assert!(
        (160..=190).contains(&p[0]) && p[1] == 0,
        "Sphere alpha: {p:?}"
    );
    let inside = CameraPose::orbit(0., 0., 0.2, [0.; 3]);
    let mut frame = PrimitiveFrame::default();
    add(
        &mut frame,
        Kind::Sphere,
        [0.; 3],
        [2.; 3],
        [1., 0., 0., 1.],
        inside,
    );
    clear(&mut renderer);
    gpu.upload(&renderer.state, &frame).unwrap();
    draw(&renderer, &gpu, &frame);
    assert!(center_pixel(&renderer)[0] > 150, "Camera inside sphere");
    let mut maximum = PrimitiveFrame::default();
    for _ in 0..MAX_NON_TEXT_INSTANCES {
        add(
            &mut maximum,
            Kind::Sphere,
            [0.; 3],
            [1.; 3],
            [1.; 4],
            camera,
        );
    }
    gpu.upload(&renderer.state, &maximum).unwrap();
    assert_eq!(gpu.allocated_instance_bytes(), MAX_NON_TEXT_INSTANCES * 128);
    let mut bad = maximum.clone();
    bad.instances[0].rgba[0] = f32::NAN;
    assert!(gpu.upload(&renderer.state, &bad).is_err());
    assert_eq!(gpu.allocated_instance_bytes(), MAX_NON_TEXT_INSTANCES * 128);
    gpu.upload(&renderer.state, &PrimitiveFrame::default())
        .unwrap();
    assert_eq!(gpu.allocated_instance_bytes(), 0);
    clear(&mut renderer);
    assert_eq!(center_pixel(&renderer), [0, 0, 0, 255]);
    renderer.release_frame();
    assert!(renderer.frame.is_none());
}

#[test]
#[ignore = "explicit serialized native Metal worker/frame readback"]
fn marker_background_worker_frame_renders_and_scope_denial_retires_it() {
    use crate::spatial::{
        marker_primitives::MarkerLayer,
        marker_worker::{self, PrepareRequest, ProjectionRequest, Workers},
    };
    use std::sync::{mpsc, Arc};
    let (tx, rx) = mpsc::channel();
    let mut workers = Workers::new(Arc::new(move || {
        let _ = tx.send(());
    }))
    .unwrap();
    workers
        .prepare(PrepareRequest {
            scene_epoch: 4,
            owners: vec![marker_worker::tests::input(
                vec![studio_protocol::marker::Geometry::Cube],
                1,
            )],
            assets: None,
        })
        .unwrap();
    rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    let scene = workers.poll_prepared().unwrap().result.unwrap();
    let camera = CameraPose::top_down(4., [0.; 3]);
    workers
        .project(ProjectionRequest {
            scene,
            camera,
            size: [128, 128],
        })
        .unwrap();
    rx.recv_timeout(std::time::Duration::from_secs(10)).unwrap();
    let projected = workers.poll_projected().unwrap().result.unwrap();
    let permit = projected
        .frame
        .permit(|key| key.owner == 1 && key.source_epoch == 1 && key.scene_epoch == 4);
    let mut renderer = native_renderer();
    let visuals = egui::Visuals::dark();
    renderer
        .render_combined_with_marker_layer(
            [128, 128],
            &empty_scene(),
            camera,
            &visuals,
            None,
            Some(MarkerLayer::new(&projected.frame, &permit).unwrap()),
        )
        .unwrap();
    let pixel = center_pixel(&renderer);
    // The top face's existing 0.9397 linear shade encodes to sRGB 248.
    assert!(
        pixel[..3].iter().all(|v| v.abs_diff(248) <= 1),
        "worker cube pixel: {pixel:?}"
    );
    assert_eq!(pixel[3], 255);
    let before = center_pixel(&renderer);
    assert!(renderer
        .render_combined_with_marker_layer(
            [128, 128],
            &empty_scene(),
            CameraPose::top_down(6., [0.; 3]),
            &visuals,
            None,
            Some(MarkerLayer::new(&projected.frame, &permit).unwrap())
        )
        .is_err());
    assert_eq!(center_pixel(&renderer), before);
    workers.finish_shutdown().unwrap();
    let denied = projected.frame.permit(|key| key.source_epoch == 2);
    renderer
        .render_combined_with_marker_layer(
            [128, 128],
            &empty_scene(),
            CameraPose::top_down(6., [0.; 3]),
            &visuals,
            None,
            Some(MarkerLayer::new(&projected.frame, &denied).unwrap()),
        )
        .unwrap();
    assert!(renderer.marker_primitives.is_none());
}
fn empty_scene() -> crate::spatial::combined::CombinedSceneFrame<'static> {
    crate::spatial::combined::CombinedSceneFrame {
        tf: None,
        fixed_frame: "world",
        robot: None,
        layers: &[],
        map: None,
        pose: None,
        path: None,
    }
}
#[test]
#[ignore = "serialized native Metal/egui camera and resize scheduling"]
fn marker_scene_view_holds_completed_projection_letterboxes_and_releases_to_map() {
    use crate::spatial::{
        map_texture::{fixture, TextureMap, TextureMapFrame},
        marker_clock::MarkerClock,
        marker_primitives::{KeyIdentity, MarkerLayer},
        scene_view::{MarkerPresentation, SceneOverlays, SharedSceneView},
    };
    use std::sync::Arc;
    let camera = CameraPose::top_down(4., [0.25, 0.25, 0.]);
    let mut frame = PrimitiveFrame::owned(MarkerClock::new());
    frame
        .append_key(
            KeyIdentity {
                owner: 1,
                source_epoch: 1,
                scene_epoch: 1,
                asset_epoch: None,
                key: studio_protocol::marker::Key {
                    namespace: "test".into(),
                    id: 1,
                },
                version: 1,
            },
            |frame| {
                frame.push_timed(
                    Kind::Cube,
                    &Instance {
                        pose: RigidTransform::new([0.25, 0.25, 1.], [0., 0., 0., 1.]).unwrap(),
                        scale: [1.; 3],
                        rgba: [1., 0., 0., 1.],
                    },
                    camera,
                    [128, 128],
                    None,
                )
            },
        )
        .unwrap();
    let frame = frame.seal().unwrap();
    let permit = frame.permit(|_| true);
    let map = Arc::new(TextureMap::prepare(fixture(2, 2, vec![0; 4])).unwrap());
    let mut renderer = native_renderer();
    let ctx = egui::Context::default();
    let mut view = SharedSceneView::visualizer();
    for size in [[800., 600.], [600., 800.]] {
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(size[0], size[1]),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    view.show_with_overlays(
                        ui,
                        &empty_scene(),
                        "test",
                        None,
                        Some(&mut renderer),
                        SceneOverlays {
                            uncertainty: &[],
                            maps: &[TextureMapFrame {
                                id: 2,
                                map: &map,
                                appearance: Default::default(),
                            }],
                            markers: Some(MarkerPresentation {
                                camera,
                                size: [128, 128],
                                layer: MarkerLayer::new(&frame, &permit).unwrap(),
                                bounds: None,
                                selected_bounds: None,
                            }),
                        },
                    )
                    .unwrap()
                });
            },
        );
        assert_eq!(
            [
                renderer.color_texture().unwrap().width(),
                renderer.color_texture().unwrap().height()
            ],
            [128, 128]
        );
        assert_ne!(view.marker_request.unwrap().0, camera);
        let pixel = center_pixel(&renderer);
        assert!(
            pixel[0] > 240 && pixel[1] < 10 && pixel[2] < 10,
            "Held common camera lost depth alignment: {pixel:?}"
        );
        let image = output
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::epaint::Shape::Mesh(mesh)
                    if matches!(mesh.texture_id, egui::TextureId::User(_)) =>
                {
                    Some(mesh)
                }
                _ => None,
            })
            .unwrap();
        let min = image
            .vertices
            .iter()
            .fold(egui::pos2(f32::INFINITY, f32::INFINITY), |p, v| {
                egui::pos2(p.x.min(v.pos.x), p.y.min(v.pos.y))
            });
        let max = image
            .vertices
            .iter()
            .fold(egui::pos2(f32::NEG_INFINITY, f32::NEG_INFINITY), |p, v| {
                egui::pos2(p.x.max(v.pos.x), p.y.max(v.pos.y))
            });
        assert!(
            ((max.x - min.x) - (max.y - min.y)).abs() < 0.01,
            "Completed square viewport must be letterboxed, not stretched"
        );
        assert_eq!(renderer.marker_primitives.as_ref().unwrap().uploads, 1);
    }
    let denied = frame.permit(|_| false);
    let _ = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(600., 800.),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                view.show_with_overlays(
                    ui,
                    &empty_scene(),
                    "test",
                    None,
                    Some(&mut renderer),
                    SceneOverlays {
                        uncertainty: &[],
                        maps: &[TextureMapFrame {
                            id: 2,
                            map: &map,
                            appearance: Default::default(),
                        }],
                        markers: Some(MarkerPresentation {
                            camera,
                            size: [128, 128],
                            layer: MarkerLayer::new(&frame, &denied).unwrap(),
                            bounds: None,
                            selected_bounds: None,
                        }),
                    },
                )
                .unwrap()
            });
        },
    );
    assert!(renderer.marker_primitives.is_none());
    assert!(!renderer.map_textures.is_empty());
    assert_eq!(
        [
            renderer.color_texture().unwrap().width(),
            renderer.color_texture().unwrap().height()
        ],
        view.marker_request.unwrap().1
    );
}
fn ready_sphere(
    camera: CameraPose,
    z: f64,
    alpha: f32,
) -> crate::spatial::marker_primitives::ReadyFrame {
    let mut frame = PrimitiveFrame::default();
    add(
        &mut frame,
        Kind::Sphere,
        [0.25, 0.25, z],
        [1.; 3],
        [1., 0., 0., alpha],
        camera,
    );
    frame.seal().unwrap()
}
#[test]
#[ignore = "requires native GPU; serialize production Marker mixed-scene ordering and cleanup"]
fn marker_scene_coordinator_depth_transparency_projection_and_cache_lifecycle() {
    use crate::{
        robot::{GeometryRole, RobotTexture, Triangle},
        spatial::{
            combined::RobotGeometryFrame,
            map_appearance::MapAppearance,
            map_texture::{fixture, TextureMap, TextureMapFrame},
            robot_geometry::{PreparedRobot, PreparedVisual},
        },
    };
    use std::sync::Arc;
    let mut renderer = native_renderer();
    let camera = CameraPose::top_down(4., [0.25, 0.25, 0.]);
    let visuals = egui::Visuals::dark();
    let triangle: Arc<[Triangle]> =
        vec![Triangle([[-2., -2., 0.], [2., -2., 0.], [0., 2., 0.]])].into();
    let mut robot = PreparedRobot {
        link_names: vec!["base".into()],
        visuals: vec![PreparedVisual {
            role: GeometryRole::Visual,
            visible: true,
            texture: None,
            uvs: None,
            link_index: 0,
            triangles: triangle,
            rgba: [0., 0., 1., 1.],
        }],
        triangle_count: 1,
    };
    let poses = [Some(RigidTransform::identity())];
    for (z, red) in [(1., true), (-1., false)] {
        let ready = ready_sphere(camera, z, 1.);
        let mut scene = empty_scene();
        scene.robot = Some(RobotGeometryFrame {
            prepared: &robot,
            poses: &poses,
        });
        renderer
            .render_combined_with_marker_primitives(
                [128, 128],
                &scene,
                camera,
                &visuals,
                None,
                Some(&ready),
            )
            .unwrap();
        let p = center_pixel(&renderer);
        assert_eq!(p[0] > 150, red, "Opaque depth {z}: {p:?}");
        assert_eq!(p[2] > 150, !red, "Opaque Robot peer {z}: {p:?}");
    }
    robot.visuals[0].rgba[3] = 0.5;
    for textured in [false, true] {
        if textured {
            robot.visuals[0].texture = Some(Arc::new(RobotTexture {
                width: 1,
                height: 1,
                rgba: vec![0, 0, 255, 255],
                has_transparency: false,
            }));
            robot.visuals[0].uvs = Some(vec![[[0.5, 0.5]; 3]].into());
            robot.visuals[0].rgba = [1., 1., 1., 0.5];
        }
        for (z, red_front) in [(1., true), (-1., false)] {
            let ready = ready_sphere(camera, z, 0.5);
            let mut scene = empty_scene();
            scene.robot = Some(RobotGeometryFrame {
                prepared: &robot,
                poses: &poses,
            });
            renderer
                .render_combined_with_marker_primitives(
                    [128, 128],
                    &scene,
                    camera,
                    &visuals,
                    None,
                    Some(&ready),
                )
                .unwrap();
            let p = center_pixel(&renderer);
            assert_eq!(
                p[0] > p[2] + 15,
                red_front,
                "Mixed transparency textured={textured} z={z}: {p:?}"
            );
        }
    }
    let map = Arc::new(TextureMap::prepare(fixture(2, 2, vec![0; 4])).unwrap());
    let mut greens = vec![];
    for z in [1., -1.] {
        let ready = ready_sphere(camera, z, 0.5);
        renderer
            .render_combined_with_marker_primitives(
                [128, 128],
                &empty_scene(),
                camera,
                &visuals,
                Some(TextureMapFrame {
                    id: 99,
                    map: &map,
                    appearance: MapAppearance {
                        opacity: 0.5,
                        ..Default::default()
                    },
                }),
                Some(&ready),
            )
            .unwrap();
        greens.push(center_pixel(&renderer)[1]);
    }
    assert!(
        greens[1] > greens[0] + 20,
        "Map/Marker transparency: {greens:?}"
    );
    for draw_behind in [false, true] {
        let ready = ready_sphere(camera, -1., 1.);
        renderer
            .render_combined_with_marker_primitives(
                [128, 128],
                &empty_scene(),
                camera,
                &visuals,
                Some(TextureMapFrame {
                    id: 99,
                    map: &map,
                    appearance: MapAppearance {
                        draw_behind,
                        ..Default::default()
                    },
                }),
                Some(&ready),
            )
            .unwrap();
        let p = center_pixel(&renderer);
        assert_eq!(
            p[1] == 0,
            draw_behind,
            "Map draw-behind {draw_behind}: {p:?}"
        );
    }
    let scoped = ready_sphere(camera, 1., 1.);
    renderer
        .render_combined_with_marker_primitives(
            [128, 128],
            &empty_scene(),
            camera,
            &visuals,
            Some(TextureMapFrame {
                id: 99,
                map: &map,
                appearance: MapAppearance::default(),
            }),
            Some(&scoped),
        )
        .unwrap();
    let map_uploads = renderer.map_textures.first().unwrap().uploads;
    renderer.clear_marker_primitives();
    assert!(renderer.marker_primitives.is_none());
    assert!(!renderer.map_textures.is_empty());
    renderer
        .render_combined_with_marker_primitives(
            [128, 128],
            &empty_scene(),
            camera,
            &visuals,
            Some(TextureMapFrame {
                id: 99,
                map: &map,
                appearance: MapAppearance::default(),
            }),
            None,
        )
        .unwrap();
    assert_eq!(renderer.map_textures.first().unwrap().uploads, map_uploads);
    assert_eq!(center_pixel(&renderer), [255, 255, 255, 255]);
    let ready = ready_sphere(camera, 1., 1.);
    renderer
        .render_combined_with_marker_primitives(
            [128, 128],
            &empty_scene(),
            camera,
            &visuals,
            None,
            Some(&ready),
        )
        .unwrap();
    let before = center_pixel(&renderer);
    let id = renderer.frame.as_ref().unwrap().id;
    let uploads = renderer.marker_primitives.as_ref().unwrap().uploads;
    assert!(uploads > 0);
    renderer
        .render_combined_with_marker_primitives(
            [128, 128],
            &empty_scene(),
            camera,
            &visuals,
            None,
            Some(&ready.clone()),
        )
        .unwrap();
    assert_eq!(
        renderer.marker_primitives.as_ref().unwrap().uploads,
        uploads
    );
    let other = CameraPose::top_down(5., [0.25, 0.25, 0.]);
    assert!(renderer
        .render_combined_with_marker_primitives(
            [128, 128],
            &empty_scene(),
            other,
            &visuals,
            None,
            Some(&ready)
        )
        .is_err());
    assert_eq!(renderer.frame.as_ref().unwrap().id, id);
    assert_eq!(center_pixel(&renderer), before);
    assert!(renderer
        .render_combined_with_marker_primitives(
            [256, 128],
            &empty_scene(),
            camera,
            &visuals,
            None,
            Some(&ready)
        )
        .is_err());
    assert_eq!(renderer.frame_size(), Some([128, 128]));
    renderer
        .render_combined_with_marker_primitives(
            [128, 128],
            &empty_scene(),
            camera,
            &visuals,
            None,
            None,
        )
        .unwrap();
    assert!(renderer.marker_primitives.is_none());
    assert_ne!(center_pixel(&renderer), before);
    // A valid newly projected resize succeeds after the stale one was rejected.
    let mut resized = PrimitiveFrame::default();
    resized
        .push(
            Kind::Sphere,
            &Instance {
                pose: RigidTransform::new([0.25, 0.25, 1.], [0., 0., 0., 1.]).unwrap(),
                scale: [1.; 3],
                rgba: [1., 0., 0., 1.],
            },
            camera,
            [256, 128],
        )
        .unwrap();
    let resized = resized.seal().unwrap();
    renderer
        .render_combined_with_marker_primitives(
            [256, 128],
            &empty_scene(),
            camera,
            &visuals,
            None,
            Some(&resized),
        )
        .unwrap();
    assert_eq!(renderer.frame_size(), Some([256, 128]));
    assert!(center_pixel(&renderer)[0] > 150);
    let empty = PrimitiveFrame::default().seal().unwrap();
    renderer
        .render_combined_with_marker_primitives(
            [256, 128],
            &empty_scene(),
            camera,
            &visuals,
            None,
            Some(&empty),
        )
        .unwrap();
    assert!(renderer.marker_primitives.is_none());
    let ready = ready_sphere(camera, 1., 0.);
    renderer
        .render_combined_with_marker_primitives(
            [128, 128],
            &empty_scene(),
            camera,
            &visuals,
            None,
            Some(&ready),
        )
        .unwrap();
    assert_ne!(
        center_pixel(&renderer),
        before,
        "Zero alpha must not retain old color"
    );
    renderer.release_frame();
    assert!(renderer.marker_primitives.is_none());
    assert!(renderer.frame.is_none());
}

#[test]
#[ignore = "requires native GPU; serialize exact integer Marker expiry and cached-peer depth"]
fn marker_cached_integer_deadlines_expire_without_geometry_upload_or_peer_loss() {
    use crate::spatial::marker_clock::MarkerClock;
    let mut renderer = native_renderer();
    let mut gpu = PrimitiveRenderer::new(&renderer.state);
    let camera = CameraPose::top_down(4., [0.25, 0.25, 0.]);
    for deadline in [(1u64 << 32) + 1, (1u64 << 48) + u32::MAX as u64, u64::MAX] {
        let mut frame = PrimitiveFrame::with_clock(MarkerClock::new());
        frame
            .push_timed(
                Kind::Sphere,
                &Instance {
                    pose: RigidTransform::new([0.25, 0.25, 1.], [0., 0., 0., 1.]).unwrap(),
                    scale: [1.; 3],
                    rgba: [1., 0., 0., 1.],
                },
                camera,
                [128, 128],
                Some(deadline),
            )
            .unwrap();
        add(
            &mut frame,
            Kind::Cube,
            [0.25, 0.25, 0.],
            [1.; 3],
            [0., 1., 0., 1.],
            camera,
        );
        let frame = frame.seal().unwrap();
        gpu.upload_ready(&renderer.state, &frame).unwrap();
        let uploads = gpu.uploads;
        let mut observations = vec![
            (deadline - 2, true),
            (deadline - 1, true),
            (deadline, false),
        ];
        if let Some(after) = deadline.checked_add(1) {
            observations.push((after, false));
        }
        for (now, visible) in observations {
            gpu.update_clock(&renderer.state, now);
            clear(&mut renderer);
            draw(&renderer, &gpu, &frame);
            let p = center_pixel(&renderer);
            assert_eq!(p[0] > 150, visible, "deadline={deadline} now={now}: {p:?}");
            assert_eq!(p[1] > 150, !visible, "Permanent peer: {p:?}");
            assert_eq!(gpu.uploads, uploads);
        }
    }
    // The production path advances its clock even when the vertex upload is
    // reused. A deadline at zero is unambiguously expired, without test sleeps.
    let mut frame = PrimitiveFrame::with_clock(MarkerClock::new());
    frame
        .push_timed(
            Kind::Sphere,
            &Instance {
                pose: RigidTransform::new([0.25, 0.25, 1.], [0., 0., 0., 1.]).unwrap(),
                scale: [1.; 3],
                rgba: [1., 0., 0., 1.],
            },
            camera,
            [128, 128],
            Some(0),
        )
        .unwrap();
    add(
        &mut frame,
        Kind::Cube,
        [0.25, 0.25, 0.],
        [1.; 3],
        [0., 1., 0., 1.],
        camera,
    );
    let frame = frame.seal().unwrap();
    let visuals = egui::Visuals::dark();
    for _ in 0..2 {
        renderer
            .render_combined_with_marker_primitives(
                [128, 128],
                &empty_scene(),
                camera,
                &visuals,
                None,
                Some(&frame),
            )
            .unwrap();
        let p = center_pixel(&renderer);
        assert!(p[1] > 150 && p[0] == 0, "Production idle expiry: {p:?}");
    }
    assert_eq!(renderer.marker_primitives.as_ref().unwrap().uploads, 1);
    assert_eq!(renderer.marker_primitives.as_ref().unwrap().clock_writes, 2);
    let mut expired = PrimitiveFrame::with_clock(MarkerClock::new());
    expired
        .push_timed(
            Kind::Sphere,
            &Instance {
                pose: RigidTransform::identity(),
                scale: [1.; 3],
                rgba: [1.; 4],
            },
            camera,
            [128, 128],
            Some(0),
        )
        .unwrap();
    let expired = expired.seal().unwrap();
    assert!(expired.all_expired());
    renderer
        .render_combined_with_marker_primitives(
            [128, 128],
            &empty_scene(),
            camera,
            &visuals,
            None,
            Some(&expired),
        )
        .unwrap();
    assert!(renderer.marker_primitives.is_none());
}

#[test]
#[ignore = "requires native GPU; serialize Marker world point size and line gradients"]
fn marker_point_world_size_and_line_endpoint_color_width() {
    use crate::spatial::marker_geometry::ColoredPoint;
    let mut renderer = native_renderer();
    let visuals = egui::Visuals::dark();
    for (target, visible) in [([0.75, 0., 0.], true), ([0., 0.75, 0.], false)] {
        let camera = CameraPose::top_down(4., target);
        let mut f = PrimitiveFrame::default();
        f.push_point(
            ColoredPoint {
                position: Point3([0., 0., 1.]),
                rgba: [1., 0., 1., 1.],
            },
            [2., 1.],
            camera,
            [128, 128],
            None,
        )
        .unwrap();
        let f = f.seal().unwrap();
        renderer
            .render_combined_with_marker_primitives(
                [128, 128],
                &empty_scene(),
                camera,
                &visuals,
                None,
                Some(&f),
            )
            .unwrap();
        let p = center_pixel(&renderer);
        assert_eq!(
            p[0] > 200 && p[2] > 200,
            visible,
            "World point dimensions at {target:?}: {p:?}"
        );
    }
    for (x, red) in [(-0.5, true), (0.5, false)] {
        let camera = CameraPose::top_down(4., [x, 0., 0.]);
        let mut f = PrimitiveFrame::default();
        f.push_line(
            ColoredPoint {
                position: Point3([-1., 0., 1.]),
                rgba: [1., 0., 0., 1.],
            },
            ColoredPoint {
                position: Point3([1., 0., 1.]),
                rgba: [0., 0., 1., 1.],
            },
            0.5,
            camera,
            [128, 128],
            None,
        )
        .unwrap();
        let f = f.seal().unwrap();
        renderer
            .render_combined_with_marker_primitives(
                [128, 128],
                &empty_scene(),
                camera,
                &visuals,
                None,
                Some(&f),
            )
            .unwrap();
        let p = center_pixel(&renderer);
        assert_eq!(p[0] > p[2] + 20, red, "Line gradient at{x}: {p:?}");
    }
    let camera = CameraPose::top_down(4., [0., 0., 0.]);
    let mut f = PrimitiveFrame::default();
    f.push_line(
        ColoredPoint {
            position: Point3([-1., 0., 1.]),
            rgba: [1., 0., 1., 1.],
        },
        ColoredPoint {
            position: Point3([1., 0., 1.]),
            rgba: [1., 0., 1., 0.],
        },
        0.5,
        camera,
        [128, 128],
        None,
    )
    .unwrap();
    let f = f.seal().unwrap();
    assert_eq!(f.transparent_instances().len(), 1);
    renderer
        .render_combined_with_marker_primitives(
            [128, 128],
            &empty_scene(),
            camera,
            &visuals,
            None,
            Some(&f),
        )
        .unwrap();
    let p = center_pixel(&renderer);
    assert!(
        (150..220).contains(&p[2]),
        "Line endpoint alpha must blend: {p:?}"
    );
    for (y, visible) in [(0.2, true), (0.4, false)] {
        let camera = CameraPose::top_down(4., [0.25, y, 0.]);
        let mut f = PrimitiveFrame::default();
        f.push_line(
            ColoredPoint {
                position: Point3([-1., 0., 1.]),
                rgba: [1., 0., 1., 1.],
            },
            ColoredPoint {
                position: Point3([1., 0., 1.]),
                rgba: [1., 0., 1., 1.],
            },
            0.5,
            camera,
            [128, 128],
            None,
        )
        .unwrap();
        let f = f.seal().unwrap();
        renderer
            .render_combined_with_marker_primitives(
                [128, 128],
                &empty_scene(),
                camera,
                &visuals,
                None,
                Some(&f),
            )
            .unwrap();
        let p = center_pixel(&renderer);
        assert_eq!(
            p[0] > 200 && p[2] > 200,
            visible,
            "Line world width y={y}: {p:?}"
        );
    }
}
#[test]
#[ignore = "requires native GPU; serialize Marker triangle corner alpha, mixed depth and expiry"]
fn marker_triangle_corner_alpha_mixed_flat_depth_and_timed_geometry() {
    use crate::spatial::{marker_clock::MarkerClock, marker_geometry::ColoredPoint};
    let mut renderer = native_renderer();
    let camera = CameraPose::top_down(4., [0.25, 0.25, 0.]);
    let visuals = egui::Visuals::dark();
    let triangle = |z, color| {
        [
            ColoredPoint {
                position: Point3([-1., -1., z]),
                rgba: color,
            },
            ColoredPoint {
                position: Point3([1., -1., z]),
                rgba: color,
            },
            ColoredPoint {
                position: Point3([0., 1., z]),
                rgba: color,
            },
        ]
    };
    let mut f = PrimitiveFrame::default();
    let mut corners = triangle(1., [1.; 4]);
    corners[0].rgba = [1., 0., 0., 0.];
    corners[1].rgba = [0., 1., 0., 1.];
    corners[2].rgba = [0., 0., 1., 1.];
    f.push_triangle(corners, camera, [128, 128], None).unwrap();
    let f = f.seal().unwrap();
    assert_eq!(f.transparent_instances().len(), 1);
    renderer
        .render_combined_with_marker_primitives(
            [128, 128],
            &empty_scene(),
            camera,
            &visuals,
            None,
            Some(&f),
        )
        .unwrap();
    let p = center_pixel(&renderer);
    assert!(
        p[1] > 60 && p[2] > p[0] + 20,
        "Triangle per-corner alpha: {p:?}"
    );
    for (z, red) in [(1., true), (-1., false)] {
        let mut f = PrimitiveFrame::default();
        f.push_triangle(triangle(0., [0., 0., 1., 1.]), camera, [128, 128], None)
            .unwrap();
        f.push_point(
            ColoredPoint {
                position: Point3([0.25, 0.25, z]),
                rgba: [1., 0., 0., 1.],
            },
            [0.5; 2],
            camera,
            [128, 128],
            None,
        )
        .unwrap();
        let f = f.seal().unwrap();
        renderer
            .render_combined_with_marker_primitives(
                [128, 128],
                &empty_scene(),
                camera,
                &visuals,
                None,
                Some(&f),
            )
            .unwrap();
        let p = center_pixel(&renderer);
        assert_eq!(p[0] > 200, red, "Flat mixed depth z{z}: {p:?}");
        assert_eq!(p[2] > 200, !red);
    }
    for kind in 0..3 {
        let mut f = PrimitiveFrame::with_clock(MarkerClock::new());
        match kind {
            0 => f
                .push_point(
                    ColoredPoint {
                        position: Point3([0.25, 0.25, 1.]),
                        rgba: [1., 0., 0., 1.],
                    },
                    [1.; 2],
                    camera,
                    [128, 128],
                    Some(0),
                )
                .unwrap(),
            1 => {
                f.push_line(
                    ColoredPoint {
                        position: Point3([-1., 0.25, 1.]),
                        rgba: [1., 0., 0., 1.],
                    },
                    ColoredPoint {
                        position: Point3([1., 0.25, 1.]),
                        rgba: [1., 0., 0., 1.],
                    },
                    1.,
                    camera,
                    [128, 128],
                    Some(0),
                )
                .unwrap();
            }
            _ => f
                .push_triangle(triangle(1., [1., 0., 0., 1.]), camera, [128, 128], Some(0))
                .unwrap(),
        };
        f.push_triangle(triangle(0., [0., 0., 1., 1.]), camera, [128, 128], None)
            .unwrap();
        let f = f.seal().unwrap();
        renderer
            .render_combined_with_marker_primitives(
                [128, 128],
                &empty_scene(),
                camera,
                &visuals,
                None,
                Some(&f),
            )
            .unwrap();
        let p = center_pixel(&renderer);
        assert!(p[2] > 200 && p[0] == 0, "Timed kind{kind}: {p:?}");
    }
    renderer.release_frame();
    assert!(renderer.marker_primitives.is_none());
}

#[test]
#[ignore = "requires native GPU; serialize joined strip coverage, alpha, perspective and expiry"]
fn marker_joined_strip_corner_coverage_single_alpha_and_expiry() {
    use crate::spatial::{marker_clock::MarkerClock, marker_geometry::ColoredPoint};
    let mut renderer = native_renderer();
    let mut gpu = PrimitiveRenderer::new(&renderer.state);
    let p = |x, y| ColoredPoint {
        position: Point3([x, y, 0.]),
        rgba: [1., 0., 0., 0.5],
    };
    let points = [p(-1.5, 0.), p(0., 0.), p(0., 1.5)];
    // A right-angle miter occupies both halves of the shared diagonal.
    // Every covered pixel should blend once, including close to that diagonal.
    for target in [
        [-0.2, -0.15, 0.],
        [-0.15, -0.2, 0.],
        [0.15, 0.2, 0.],
        [0.2, 0.15, 0.],
        [0., 0., 0.],
    ] {
        let camera = CameraPose::top_down(2., target);
        let mut f = PrimitiveFrame::with_clock(MarkerClock::new());
        f.push_line_strip(&points, 0.5, camera, [128, 128], Some(10), || false)
            .unwrap();
        gpu.upload(&renderer.state, &f).unwrap();
        for (tick, visible) in [(9, true), (10, false)] {
            gpu.update_clock(&renderer.state, tick);
            clear(&mut renderer);
            draw(&renderer, &gpu, &f);
            let pixel = center_pixel(&renderer);
            if visible {
                assert!(
                    (180..200).contains(&pixel[0]),
                    "joined strip target={target:?}: {pixel:?}"
                );
            } else {
                assert_eq!(pixel, [0, 0, 0, 255]);
            }
        }
    }
    let camera = CameraPose::orbit(0.6, 0.8, 4., [0.; 3]);
    let mut f = PrimitiveFrame::default();
    let points = points.map(|p| ColoredPoint {
        rgba: [0., 1., 0., 1.],
        ..p
    });
    f.push_line_strip(&points, 0.5, camera, [128, 128], None, || false)
        .unwrap();
    gpu.upload(&renderer.state, &f).unwrap();
    clear(&mut renderer);
    draw(&renderer, &gpu, &f);
    assert!(
        center_pixel(&renderer)[1] > 200,
        "Perspective join must cover its source vertex"
    );
    // Opaque four-corner records participate in the same scene depth target.
    let camera = CameraPose::top_down(2., [0.; 3]);
    let points = points.map(|p| ColoredPoint {
        rgba: [1., 0., 0., 1.],
        ..p
    });
    for (z, front) in [(-1., false), (1., true)] {
        let mut f = PrimitiveFrame::default();
        f.push_line_strip(&points, 0.5, camera, [128, 128], None, || false)
            .unwrap();
        f.push_point(
            ColoredPoint {
                position: Point3([0., 0., z]),
                rgba: [0., 1., 0., 1.],
            },
            [0.5; 2],
            camera,
            [128, 128],
            None,
        )
        .unwrap();
        gpu.upload(&renderer.state, &f).unwrap();
        clear(&mut renderer);
        draw(&renderer, &gpu, &f);
        let pixel = center_pixel(&renderer);
        assert_eq!(pixel[1] > 200, front, "Joined strip depth z={z}: {pixel:?}");
        assert_eq!(
            pixel[0] > 200,
            !front,
            "Joined strip depth z={z}: {pixel:?}"
        );
    }
    gpu.clear();
    assert_eq!(gpu.allocated_instance_bytes(), 0);
}

#[test]
#[ignore = "requires native GPU; serialize prepared arrow silhouette, orientation, alpha, depth and expiry"]
fn marker_prepared_arrow_shape_orientation_and_timed_peer_depth() {
    use crate::spatial::{
        marker_clock::MarkerClock,
        marker_geometry::{Body, ColoredPoint},
    };
    let mut renderer = native_renderer();
    let mut gpu = PrimitiveRenderer::new(&renderer.state);
    let arrow = |pose, rgba| Body::Arrow {
        pose,
        shaft: 2.,
        head: 1.,
        shaft_diameters: [0.4, 0.6],
        head_diameters: [0.8, 1.2],
        rgba,
    };
    let pose = RigidTransform::new([-1.5, 0., 0.], [0., 0., 0., 1.]).unwrap();
    for (target, visible) in [
        ([-0.5, 0.15, 0.], true),
        ([-0.5, 0.3, 0.], false),
        ([0.7, 0.25, 0.], true),
        ([0.7, 0.45, 0.], false),
        ([1.4, 0.01, 0.], true),
        ([1.6, 0., 0.], false),
    ] {
        let camera = CameraPose::top_down(2., target);
        let mut f = PrimitiveFrame::default();
        assert_eq!(
            f.push_geometry(
                &arrow(pose, [1., 0., 0., 1.]),
                camera,
                [128, 128],
                None,
                || false
            )
            .unwrap(),
            2
        );
        gpu.upload(&renderer.state, &f).unwrap();
        clear(&mut renderer);
        draw(&renderer, &gpu, &f);
        let pixel = center_pixel(&renderer);
        assert_eq!(
            pixel[0] > 150,
            visible,
            "Arrow silhouette {target:?}: {pixel:?}"
        );
    }
    let pose = RigidTransform::new(
        [0., -1.5, 0.],
        [
            0.,
            0.,
            std::f64::consts::FRAC_1_SQRT_2,
            std::f64::consts::FRAC_1_SQRT_2,
        ],
    )
    .unwrap();
    let camera = CameraPose::top_down(2., [0., 1.3, 0.]);
    let mut f = PrimitiveFrame::default();
    f.push_geometry(
        &arrow(pose, [0., 0., 1., 1.]),
        camera,
        [128, 128],
        None,
        || false,
    )
    .unwrap();
    gpu.upload(&renderer.state, &f).unwrap();
    clear(&mut renderer);
    draw(&renderer, &gpu, &f);
    assert!(center_pixel(&renderer)[2] > 150, "Rotated arrow tip");
    let pose = RigidTransform::new([-1.5, 0., 0.], [0., 0., 0., 1.]).unwrap();
    let camera = CameraPose::top_down(2., [-0.5, 0., 0.]);
    let mut f = PrimitiveFrame::with_clock(MarkerClock::new());
    f.push_point(
        ColoredPoint {
            position: Point3([-0.5, 0., -1.]),
            rgba: [0., 1., 0., 1.],
        },
        [1.; 2],
        camera,
        [128, 128],
        None,
    )
    .unwrap();
    f.push_geometry(
        &arrow(pose, [1., 0., 0., 0.5]),
        camera,
        [128, 128],
        Some(10),
        || false,
    )
    .unwrap();
    gpu.upload(&renderer.state, &f).unwrap();
    let uploads = gpu.uploads;
    for (tick, visible) in [(9, true), (10, false)] {
        gpu.update_clock(&renderer.state, tick);
        clear(&mut renderer);
        draw(&renderer, &gpu, &f);
        let pixel = center_pixel(&renderer);
        assert_eq!(pixel[0] > 100, visible, "Timed arrow {tick}: {pixel:?}");
        assert!(pixel[1] > 100, "Permanent peer remains: {pixel:?}");
        if !visible {
            assert!(pixel[1] > 250);
        }
        assert_eq!(gpu.uploads, uploads);
    }
    // Opaque arrow surface participates in scene depth in front of the point.
    let mut f = PrimitiveFrame::default();
    f.push_geometry(
        &arrow(pose, [1., 0., 0., 1.]),
        camera,
        [128, 128],
        None,
        || false,
    )
    .unwrap();
    f.push_point(
        ColoredPoint {
            position: Point3([-0.5, 0., -1.]),
            rgba: [0., 1., 0., 1.],
        },
        [1.; 2],
        camera,
        [128, 128],
        None,
    )
    .unwrap();
    gpu.upload(&renderer.state, &f).unwrap();
    clear(&mut renderer);
    draw(&renderer, &gpu, &f);
    assert_eq!(
        center_pixel(&renderer)[1],
        0,
        "Behind point must not cover the arrow"
    );
    gpu.clear();
    assert_eq!(gpu.allocated_instance_bytes(), 0);
}

fn marker_text_body(s: &str, alpha: f32) -> crate::spatial::marker_geometry::Body {
    crate::spatial::marker_geometry::Body::Text {
        position: Point3([0.; 3]),
        text: s.into(),
        height: 0.5,
        space_width: None,
        rgba: [1., 0., 0., alpha],
    }
}
fn glyph_ink_target(layout: &crate::spatial::marker_text::Layout) -> [f32; 3] {
    let g = layout.glyphs[0];
    let atlas = layout.atlas();
    let size = atlas.dimensions();
    let min = [
        (g.uv[0] * size[0] as f32) as usize,
        (g.uv[1] * size[1] as f32) as usize,
    ];
    let max = [
        (g.uv[2] * size[0] as f32) as usize,
        (g.uv[3] * size[1] as f32) as usize,
    ];
    let mut best = (0u32, min[0] + 1, min[1] + 1);
    for y in min[1] + 1..max[1] - 1 {
        for x in min[0] + 1..max[0] - 1 {
            let score = (y - 1..=y + 1)
                .flat_map(|j| (x - 1..=x + 1).map(move |i| (j, i)))
                .map(|(j, i)| atlas.pixels[j * size[0] as usize + i] as u32)
                .sum();
            if score > best.0 {
                best = (score, x, y);
            }
        }
    }
    [
        (g.center[0]
            + ((best.1 as f64 + 0.5 - min[0] as f64) / (max[0] - min[0]) as f64 - 0.5)
                * g.dimensions[0]) as f32,
        (g.center[1]
            + (0.5 - (best.2 as f64 + 0.5 - min[1] as f64) / (max[1] - min[1]) as f64)
                * g.dimensions[1]) as f32,
        0.,
    ]
}
#[test]
#[ignore = "requires native GPU; serialize actual glyph coverage, depth, alpha, expiry and atlas resources"]
fn marker_text_real_coverage_depth_alpha_expiry_and_atlas_lifecycle() {
    use crate::spatial::{marker_clock::MarkerClock, marker_geometry::ColoredPoint, marker_text};
    let mut renderer = native_renderer();
    let mut gpu = PrimitiveRenderer::new(&renderer.state);
    let bodies = [
        marker_text_body("O", 1.),
        marker_text_body("中", 1.),
        marker_text_body("🤖", 1.),
    ];
    let layouts = marker_text::prepare(&bodies.iter().collect::<Vec<_>>(), || false).unwrap();
    for layout in &layouts {
        let target = glyph_ink_target(layout);
        let camera = CameraPose::top_down(1., target);
        let mut f = PrimitiveFrame::default();
        f.push_text(layout, camera, [128, 128], None, || false)
            .unwrap();
        gpu.upload(&renderer.state, &f).unwrap();
        clear(&mut renderer);
        draw(&renderer, &gpu, &f);
        let pixel = center_pixel(&renderer);
        assert!(pixel[0] > 150, "Real glyph coverage: {pixel:?}");
    }
    assert_eq!(
        gpu.atlas_uploads(),
        1,
        "Shared immutable atlas must upload once"
    );
    // Camera-facing geometry follows a tilted perspective camera as well.
    let local = glyph_ink_target(&layouts[0]);
    let basis = CameraPose::orbit(0.5, 0.4, 4., [0.; 3]);
    let target = std::array::from_fn(|i| basis.right[i] * local[0] + basis.up[i] * local[1]);
    let camera = CameraPose::orbit(0.5, 0.4, 4., target);
    let mut perspective = PrimitiveFrame::default();
    perspective
        .push_text(&layouts[0], camera, [128, 128], None, || false)
        .unwrap();
    gpu.upload(&renderer.state, &perspective).unwrap();
    clear(&mut renderer);
    draw(&renderer, &gpu, &perspective);
    assert!(
        center_pixel(&renderer)[0] > 150,
        "Text must face the perspective camera"
    );
    // The center of O is transparent rather than a filled billboard rectangle.
    let g = layouts[0].glyphs[0];
    let camera = CameraPose::top_down(1., [g.center[0] as f32, g.center[1] as f32, 0.]);
    let mut f = PrimitiveFrame::default();
    f.push_text(&layouts[0], camera, [128, 128], None, || false)
        .unwrap();
    gpu.upload(&renderer.state, &f).unwrap();
    clear(&mut renderer);
    draw(&renderer, &gpu, &f);
    assert_eq!(
        center_pixel(&renderer),
        [0, 0, 0, 255],
        "O must preserve its transparent center"
    );
    let body = marker_text_body("O", 0.5);
    let layout = marker_text::prepare(&[&body], || false).unwrap().remove(0);
    let target = glyph_ink_target(&layout);
    let camera = CameraPose::top_down(1., target);
    let mut f = PrimitiveFrame::with_clock(MarkerClock::new());
    f.push_point(
        ColoredPoint {
            position: Point3([target[0] as f64, target[1] as f64, -1.]),
            rgba: [0., 1., 0., 1.],
        },
        [2.; 2],
        camera,
        [128, 128],
        None,
    )
    .unwrap();
    f.push_text(&layout, camera, [128, 128], Some(10), || false)
        .unwrap();
    let f = f.seal().unwrap();
    gpu.upload_ready(&renderer.state, &f).unwrap();
    let uploads = gpu.uploads;
    let atlas_uploads = gpu.atlas_uploads();
    for (tick, visible) in [(9, true), (10, false)] {
        gpu.update_clock(&renderer.state, tick);
        clear(&mut renderer);
        draw(&renderer, &gpu, &f);
        let pixel = center_pixel(&renderer);
        assert_eq!(pixel[0] > 100, visible, "Timed text {tick}: {pixel:?}");
        assert!(pixel[1] > 100);
        assert_eq!(gpu.uploads, uploads);
        assert_eq!(gpu.atlas_uploads(), atlas_uploads);
    }
    let mut front = PrimitiveFrame::default();
    front
        .push_point(
            ColoredPoint {
                position: Point3([target[0] as f64, target[1] as f64, 1.]),
                rgba: [0., 0., 1., 1.],
            },
            [2.; 2],
            camera,
            [128, 128],
            None,
        )
        .unwrap();
    front
        .push_text(&layout, camera, [128, 128], None, || false)
        .unwrap();
    let front = front.seal().unwrap();
    renderer
        .render_combined_with_marker_primitives(
            [128, 128],
            &empty_scene(),
            camera,
            &egui::Visuals::dark(),
            None,
            Some(&front),
        )
        .unwrap();
    let pixel = center_pixel(&renderer);
    assert!(
        pixel[2] > 250 && pixel[0] == 0,
        "Text must be occluded by front scene geometry: {pixel:?}"
    );
    let mut plain = PrimitiveFrame::default();
    add(&mut plain, Kind::Cube, [0.; 3], [1.; 3], [1.; 4], camera);
    gpu.upload(&renderer.state, &plain).unwrap();
    assert_eq!(gpu.allocated_atlas_bytes(), 0);
    gpu.upload_ready(&renderer.state, &f).unwrap();
    assert!(gpu.allocated_atlas_bytes() > 0);
    gpu.clear();
    assert_eq!(gpu.allocated_atlas_bytes(), 0);
    assert_eq!(gpu.allocated_instance_bytes(), 0);
}
#[test]
#[ignore = "requires native GPU; serialize maximum combined non-text and glyph instance storage"]
fn marker_maximum_text_and_non_text_allowances_upload_without_reducing_source_caps() {
    use crate::spatial::marker_text;
    let renderer = native_renderer();
    let mut gpu = PrimitiveRenderer::new(&renderer.state);
    let camera = CameraPose::top_down(8., [0.; 3]);
    let mut f = PrimitiveFrame::default();
    for _ in 0..MAX_NON_TEXT_INSTANCES {
        add(&mut f, Kind::Sphere, [0.; 3], [1.; 3], [1.; 4], camera);
    }
    let body = marker_text_body(&"A".repeat(4096), 1.);
    let mut layouts = marker_text::prepare(&vec![&body; 32], || false).unwrap();
    // Synthetic maximum atlas storage accompanies the full real-font glyph
    // count. This upload fixture does not claim those pixels are font outlines.
    let maximum_atlas = std::sync::Arc::new(marker_text::Atlas {
        size: [marker_text::ATLAS_SIDE as u32; 2],
        pixels: vec![0; marker_text::MAX_ATLAS_BYTES].into_boxed_slice(),
    });
    for layout in &mut layouts {
        layout.atlas = maximum_atlas.clone();
    }
    for layout in &layouts {
        f.push_text(layout, camera, [128, 128], None, || false)
            .unwrap();
    }
    for _ in 0..2 {
        let image = std::sync::Arc::new(crate::robot::RobotTexture {
            width: 2048,
            height: 2048,
            rgba: vec![255; 16 * 1024 * 1024],
            has_transparency: false,
        });
        let mesh = gpu_mesh(
            Some(image),
            crate::spatial::marker_mesh::MAX_TRIANGLES / 2,
            [0.; 4],
            0.,
        );
        f.push_mesh(&mesh, camera, [128, 128], None, || false)
            .unwrap();
    }
    assert_eq!(f.len(), MAX_INSTANCES);
    let f = f.seal().unwrap();
    gpu.upload_ready(&renderer.state, &f).unwrap();
    assert_eq!(gpu.allocated_instance_bytes(), MAX_INSTANCE_BYTES);
    assert_eq!(gpu.allocated_atlas_bytes(), marker_text::MAX_ATLAS_BYTES);
    assert_eq!(
        gpu.allocated_mesh_texture_bytes(),
        crate::spatial::marker_mesh::MAX_TEXTURE_BYTES
    );
    gpu.clear();
    assert_eq!(gpu.allocated_instance_bytes(), 0);
    assert_eq!(gpu.allocated_atlas_bytes(), 0);
}

fn gpu_mesh(
    image: Option<std::sync::Arc<crate::robot::RobotTexture>>,
    count: usize,
    rgba: [f32; 4],
    z: f64,
) -> crate::spatial::marker_mesh::Prepared {
    use crate::{
        robot::{MeshAsset, MeshAssetPart, Triangle},
        spatial::{marker_geometry::Body, marker_mesh},
    };
    let embedded = image.is_some();
    let asset = MeshAsset {
        path: "fixture".into(),
        triangle_count: count,
        embedded_materials: embedded,
        parts: vec![MeshAssetPart {
            triangles: vec![Triangle([[-1., -1., 0.], [1., -1., 0.], [0., 1., 0.]]); count].into(),
            uvs: image
                .as_ref()
                .map(|_| vec![[[0., 0.], [1., 0.], [0.5, 1.]]; count].into()),
            texture: image,
            rgba: embedded.then_some([1., 1., 1., 0.5]),
        }]
        .into(),
    };
    let body = Body::Mesh {
        pose: RigidTransform::new([0., 0., z], [0., 0., 0., 1.]).unwrap(),
        scale: [1.; 3],
        resource: "fixture".into(),
        embedded,
        rgba,
    };
    marker_mesh::prepare(
        &[(&body, &asset)],
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap()
    .remove(0)
    .unwrap()
}
#[test]
#[ignore = "requires native GPU; serialize mesh sentinel/tint, scene depth, exact expiry and texture resources"]
fn marker_mesh_material_sentinel_overlay_depth_expiry_and_texture_lifecycle() {
    use crate::spatial::{marker_clock::MarkerClock, marker_geometry::ColoredPoint};
    let mut renderer = native_renderer();
    let mut gpu = PrimitiveRenderer::new(&renderer.state);
    let camera = CameraPose::top_down(4., [0.; 3]);
    let red = std::sync::Arc::new(crate::robot::RobotTexture {
        width: 1,
        height: 1,
        rgba: vec![255, 0, 0, 255],
        has_transparency: false,
    });
    for (rgba, expected) in [
        ([0.; 4], [188i16, 0, 0]),
        ([0., 0., 1., 0.4], [134, 0, 170]),
    ] {
        let mesh = gpu_mesh(Some(red.clone()), 1, rgba, 0.);
        let mut f = PrimitiveFrame::default();
        f.push_mesh(&mesh, camera, [128, 128], None, || false)
            .unwrap();
        let f = f.seal().unwrap();
        gpu.upload_ready(&renderer.state, &f).unwrap();
        clear(&mut renderer);
        draw(&renderer, &gpu, &f);
        let pixel = center_pixel(&renderer);
        for i in 0..3 {
            assert!(
                (pixel[i] as i16 - expected[i]).abs() <= 4,
                "Mesh material {rgba:?}: {pixel:?}"
            );
        }
    }
    assert_eq!(gpu.mesh_texture_uploads(), 1);
    let mesh = gpu_mesh(Some(red.clone()), 1, [0.; 4], 0.);
    let mut f = PrimitiveFrame::with_clock(MarkerClock::new());
    f.push_point(
        ColoredPoint {
            position: Point3([0., 0., -1.]),
            rgba: [0., 1., 0., 1.],
        },
        [2.; 2],
        camera,
        [128, 128],
        None,
    )
    .unwrap();
    f.push_mesh(&mesh, camera, [128, 128], Some(10), || false)
        .unwrap();
    let f = f.seal().unwrap();
    gpu.upload_ready(&renderer.state, &f).unwrap();
    let uploads = gpu.uploads;
    for (tick, visible) in [(9, true), (10, false)] {
        gpu.update_clock(&renderer.state, tick);
        clear(&mut renderer);
        draw(&renderer, &gpu, &f);
        let pixel = center_pixel(&renderer);
        assert_eq!(pixel[0] > 150, visible, "Mesh expiry {tick}: {pixel:?}");
        assert!(pixel[1] > 150);
        assert_eq!(gpu.uploads, uploads);
    }
    let mut front = PrimitiveFrame::default();
    front
        .push_point(
            ColoredPoint {
                position: Point3([0., 0., 1.]),
                rgba: [0., 0., 1., 1.],
            },
            [2.; 2],
            camera,
            [128, 128],
            None,
        )
        .unwrap();
    front
        .push_mesh(&mesh, camera, [128, 128], None, || false)
        .unwrap();
    let front = front.seal().unwrap();
    renderer
        .render_combined_with_marker_primitives(
            [128, 128],
            &empty_scene(),
            camera,
            &egui::Visuals::dark(),
            None,
            Some(&front),
        )
        .unwrap();
    let p = center_pixel(&renderer);
    assert!(
        p[2] > 250 && p[0] == 0,
        "Opaque scene must occlude mesh: {p:?}"
    );
    // A fully opaque textured mesh must write depth before a later rear point.
    let opaque = gpu_mesh(Some(red.clone()), 1, [1., 0., 0., 1.], 0.);
    let mut opaque_frame = PrimitiveFrame::default();
    opaque_frame
        .push_mesh(&opaque, camera, [128, 128], None, || false)
        .unwrap();
    opaque_frame
        .push_point(
            ColoredPoint {
                position: Point3([0., 0., -1.]),
                rgba: [0., 0., 1., 1.],
            },
            [2.; 2],
            camera,
            [128, 128],
            None,
        )
        .unwrap();
    gpu.upload(&renderer.state, &opaque_frame).unwrap();
    clear(&mut renderer);
    draw(&renderer, &gpu, &opaque_frame);
    let pixel = center_pixel(&renderer);
    assert!(
        pixel[0] > 250 && pixel[2] == 0,
        "Opaque textured mesh depth: {pixel:?}"
    );
    let clear_image = std::sync::Arc::new(crate::robot::RobotTexture {
        width: 1,
        height: 1,
        rgba: vec![255, 0, 0, 0],
        has_transparency: true,
    });
    let transparent = gpu_mesh(Some(clear_image), 1, [0.; 4], 0.);
    let mut f = PrimitiveFrame::default();
    f.push_mesh(&transparent, camera, [128, 128], None, || false)
        .unwrap();
    gpu.upload(&renderer.state, &f).unwrap();
    clear(&mut renderer);
    draw(&renderer, &gpu, &f);
    assert_eq!(
        center_pixel(&renderer),
        [0, 0, 0, 255],
        "Texture alpha holes must remain transparent"
    );
    let mesh = gpu_mesh(None, 1, [0., 1., 0., 1.], 0.);
    let mut flat = PrimitiveFrame::default();
    flat.push_mesh(&mesh, camera, [128, 128], None, || false)
        .unwrap();
    gpu.upload(&renderer.state, &flat).unwrap();
    assert_eq!(gpu.allocated_mesh_texture_bytes(), 0);
    clear(&mut renderer);
    draw(&renderer, &gpu, &flat);
    assert!(center_pixel(&renderer)[1] > 250);
    gpu.clear();
    assert_eq!(gpu.allocated_instance_bytes(), 0);
    assert_eq!(gpu.allocated_mesh_texture_bytes(), 0);
}
#[test]
#[ignore = "requires native GPU; serialize directly loaded OBJ/Collada mesh UV orientation and repeat"]
fn marker_loaded_meshes_keep_diffuse_uv_orientation_repeat_and_projection() {
    use crate::{
        robot::{LoadOptions, MeshAssetLoader},
        spatial::{marker_geometry::Body, marker_mesh},
    };
    let root =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/robot_visualizer");
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let mut loader = MeshAssetLoader::new(Some(&root), &LoadOptions::default(), &cancel).unwrap();
    let mut renderer = native_renderer();
    let mut gpu = PrimitiveRenderer::new(&renderer.state);
    let mut reference: Option<[u8; 4]> = None;
    for resource in ["materials/textured.obj", "materials/textured.dae"] {
        let asset = loader.load(resource, true, &cancel).unwrap();
        let body = Body::Mesh {
            pose: RigidTransform::identity(),
            scale: [1.; 3],
            resource: resource.into(),
            embedded: true,
            rgba: [0.; 4],
        };
        let mesh = marker_mesh::prepare(&[(&body, &asset)], &cancel)
            .unwrap()
            .remove(0)
            .unwrap();
        let triangle = mesh
            .parts
            .iter()
            .find(|p| p.texture.is_some())
            .unwrap()
            .triangles[0]
            .0;
        let normalized = |v: [f64; 3]| {
            let n = v.iter().map(|x| x * x).sum::<f64>().sqrt();
            v.map(|x| x / n)
        };
        let r = normalized(std::array::from_fn(|i| triangle[1][i] - triangle[0][i]));
        let e: [f64; 3] = std::array::from_fn(|i| triangle[2][i] - triangle[0][i]);
        let n = normalized([
            r[1] * e[2] - r[2] * e[1],
            r[2] * e[0] - r[0] * e[2],
            r[0] * e[1] - r[1] * e[0],
        ]);
        let up = [
            n[1] * r[2] - n[2] * r[1],
            n[2] * r[0] - n[0] * r[2],
            n[0] * r[1] - n[1] * r[0],
        ];
        let center: [f64; 3] =
            std::array::from_fn(|i| (triangle[0][i] + triangle[1][i] + triangle[2][i]) / 3.);
        let camera = CameraPose {
            eye: std::array::from_fn(|i| (center[i] + n[i] * 4.) as f32),
            right: r.map(|v| v as f32),
            up: up.map(|v| v as f32),
            forward: n.map(|v| -v as f32),
            orthographic_span: Some(2.),
            calibrated: None,
        };
        let mut f = PrimitiveFrame::default();
        f.push_mesh(&mesh, camera, [128, 128], None, || false)
            .unwrap();
        gpu.upload(&renderer.state, &f).unwrap();
        clear(&mut renderer);
        draw(&renderer, &gpu, &f);
        let pixel = center_pixel(&renderer);
        assert!(
            pixel[..3].iter().any(|v| *v > 150),
            "Loaded mesh {resource} must render: {pixel:?}"
        );
        if let Some(old) = reference {
            for i in 0..3 {
                assert!(
                    (pixel[i] as i16 - old[i] as i16).abs() <= 20,
                    "Loaded UV behavior {resource}: {pixel:?} vs {old:?}"
                );
            }
        } else {
            reference = Some(pixel);
        }
    }
    // Same image and UVs offset by integer repeats must sample identically.
    let image = std::sync::Arc::new(crate::robot::RobotTexture {
        width: 2,
        height: 2,
        rgba: vec![
            255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
        ],
        has_transparency: false,
    });
    let mut a = gpu_mesh(Some(image), 1, [0.; 4], 0.);
    let mut observed = None;
    for repeat in [0., 2.] {
        if repeat > 0. {
            let p = std::sync::Arc::make_mut(&mut a.parts);
            let uvs = std::sync::Arc::make_mut(p[0].uvs.as_mut().unwrap());
            for uv in uvs.iter_mut().flatten() {
                uv[0] += repeat;
                uv[1] += repeat;
            }
        }
        let camera = CameraPose::top_down(4., [0.1, -0.1, 0.]);
        let mut f = PrimitiveFrame::default();
        f.push_mesh(&a, camera, [128, 128], None, || false).unwrap();
        gpu.upload(&renderer.state, &f).unwrap();
        clear(&mut renderer);
        draw(&renderer, &gpu, &f);
        let pixel = center_pixel(&renderer);
        if let Some(old) = observed {
            assert_eq!(pixel, old, "Integer UV repeat");
        } else {
            assert!(
                pixel[1] > pixel[0] + 5 && (pixel[1] as i16 - pixel[2] as i16).abs() <= 2,
                "UV vertical orientation: {pixel:?}"
            );
            observed = Some(pixel);
        }
    }
    gpu.clear();
    assert_eq!(gpu.allocated_mesh_texture_bytes(), 0);
}

#[test]
#[ignore = "requires native GPU; serialize mesh texture source ordering with Robot and map peers"]
fn marker_mesh_texture_sources_merge_with_robot_map_and_scoped_clear() {
    use crate::{
        robot::{GeometryRole, RobotTexture, Triangle},
        spatial::{
            combined::RobotGeometryFrame,
            map_appearance::MapAppearance,
            map_texture::{fixture, TextureMap, TextureMapFrame},
            robot_geometry::{PreparedRobot, PreparedVisual},
        },
    };
    use std::sync::Arc;
    let mut renderer = native_renderer();
    let camera = CameraPose::top_down(4., [0.25, 0.25, 0.]);
    let visuals = egui::Visuals::dark();
    let image = Arc::new(RobotTexture {
        width: 1,
        height: 1,
        rgba: vec![255, 0, 0, 255],
        has_transparency: false,
    });
    let mut robot = PreparedRobot {
        link_names: vec!["base".into()],
        triangle_count: 1,
        visuals: vec![PreparedVisual {
            role: GeometryRole::Visual,
            visible: true,
            texture: None,
            uvs: None,
            link_index: 0,
            triangles: vec![Triangle([[-2., -2., 0.], [2., -2., 0.], [0., 2., 0.]])].into(),
            rgba: [0., 0., 1., 0.5],
        }],
    };
    let poses = [Some(RigidTransform::identity())];
    for textured in [false, true] {
        if textured {
            robot.visuals[0].texture = Some(Arc::new(RobotTexture {
                width: 1,
                height: 1,
                rgba: vec![0, 0, 255, 255],
                has_transparency: false,
            }));
            robot.visuals[0].uvs = Some(vec![[[0.5, 0.5]; 3]].into());
            robot.visuals[0].rgba = [1., 1., 1., 0.5];
        }
        for (z, front) in [(1., true), (-1., false)] {
            let mesh = gpu_mesh(Some(image.clone()), 1, [0.; 4], z);
            let mut f = PrimitiveFrame::default();
            f.push_mesh(&mesh, camera, [128, 128], None, || false)
                .unwrap();
            let f = f.seal().unwrap();
            let mut scene = empty_scene();
            scene.robot = Some(RobotGeometryFrame {
                prepared: &robot,
                poses: &poses,
            });
            renderer
                .render_combined_with_marker_primitives(
                    [128, 128],
                    &scene,
                    camera,
                    &visuals,
                    None,
                    Some(&f),
                )
                .unwrap();
            let p = center_pixel(&renderer);
            assert_eq!(
                p[0] > p[2] + 15,
                front,
                "Mesh/Robot ordering textured={textured} z={z}: {p:?}"
            );
        }
    }
    let map = Arc::new(TextureMap::prepare(fixture(2, 2, vec![0; 4])).unwrap());
    let mut greens = vec![];
    for z in [1., -1.] {
        let mesh = gpu_mesh(Some(image.clone()), 1, [0.; 4], z);
        let mut f = PrimitiveFrame::default();
        f.push_mesh(&mesh, camera, [128, 128], None, || false)
            .unwrap();
        let f = f.seal().unwrap();
        renderer
            .render_combined_with_marker_primitives(
                [128, 128],
                &empty_scene(),
                camera,
                &visuals,
                Some(TextureMapFrame {
                    id: 33,
                    map: &map,
                    appearance: MapAppearance {
                        opacity: 0.5,
                        ..Default::default()
                    },
                }),
                Some(&f),
            )
            .unwrap();
        greens.push(center_pixel(&renderer)[1]);
    }
    assert!(greens[1] > greens[0] + 20, "Mesh/Map ordering: {greens:?}");
    assert_eq!(
        renderer
            .marker_primitives
            .as_ref()
            .unwrap()
            .mesh_texture_uploads(),
        1
    );
    let uploads = renderer.map_textures.first().unwrap().uploads;
    renderer.clear_marker_primitives();
    assert!(renderer.marker_primitives.is_none());
    assert_eq!(renderer.map_textures.first().unwrap().uploads, uploads);
}

#[test]
#[ignore = "requires native GPU; serialize cached keyed visibility for primitive/text/mesh, epochs, expiry and peer cleanup"]
fn marker_keyed_visibility_hides_cached_content_without_peer_loss_or_upload() {
    use crate::spatial::{
        map_appearance::MapAppearance,
        map_texture::{fixture, TextureMap, TextureMapFrame},
        marker_clock::MarkerClock,
        marker_geometry::ColoredPoint,
        marker_primitives::{KeyIdentity, MarkerLayer},
        marker_text,
    };
    use std::sync::Arc;
    let identity = |owner, id, assets| KeyIdentity {
        owner,
        source_epoch: 1,
        scene_epoch: 1,
        asset_epoch: assets,
        key: studio_protocol::marker::Key {
            namespace: "objects".into(),
            id,
        },
        version: 1,
    };
    let image = Arc::new(crate::robot::RobotTexture {
        width: 1,
        height: 1,
        rgba: vec![255, 0, 0, 255],
        has_transparency: false,
    });
    let mesh = gpu_mesh(Some(image), 1, [1., 0., 0., 1.], 2.);
    let body = marker_text_body("O", 1.);
    let mut text = marker_text::prepare(&[&body], || false).unwrap().remove(0);
    text.position = Point3([2., 2., 2.]);
    let ink = glyph_ink_target(&text);
    let map = Arc::new(TextureMap::prepare(fixture(2, 2, vec![0; 4])).unwrap());
    let visuals = egui::Visuals::dark();
    for kind in 0..3 {
        let target = if kind == 2 {
            [ink[0] + 2., ink[1] + 2., 0.]
        } else {
            [0.25, 0.25, 0.]
        };
        let camera = CameraPose::top_down(if kind == 2 { 1. } else { 4. }, target);
        let build = |deadline| {
            let mut f = PrimitiveFrame::owned(MarkerClock::new());
            f.append_key(
                identity(1, 1, if kind == 1 { Some(1) } else { None }),
                |f| match kind {
                    0 => f
                        .push_timed(
                            Kind::Cube,
                            &Instance {
                                pose: RigidTransform::new([0., 0., 2.], [0., 0., 0., 1.]).unwrap(),
                                scale: [1.; 3],
                                rgba: [1., 0., 0., 1.],
                            },
                            camera,
                            [128, 128],
                            Some(deadline),
                        )
                        .map(|_| 1),
                    1 => f.push_mesh(&mesh, camera, [128, 128], Some(deadline), || false),
                    _ => f.push_text(&text, camera, [128, 128], Some(deadline), || false),
                },
            )
            .unwrap();
            for (owner, id, z, color) in
                [(1, 2, 1., [0., 0., 1., 1.]), (2, 1, 0., [0., 1., 0., 1.])]
            {
                f.append_key(identity(owner, id, None), |f| {
                    f.push_point(
                        ColoredPoint {
                            position: Point3([target[0] as f64, target[1] as f64, z]),
                            rgba: color,
                        },
                        [2.; 2],
                        camera,
                        [128, 128],
                        None,
                    )
                })
                .unwrap();
            }
            f.seal().unwrap()
        };
        let frame = build(1 << 60);
        let mut renderer = native_renderer();
        for (phase, permit) in [
            (0, frame.permit(|_| true)),
            (1, frame.permit(|k| !(k.owner == 1 && k.key.id == 1))),
            (2, frame.permit(|k| k.owner == 2)),
        ] {
            renderer
                .render_combined_with_marker_layer(
                    [128, 128],
                    &empty_scene(),
                    camera,
                    &visuals,
                    Some(TextureMapFrame {
                        id: 61,
                        map: &map,
                        appearance: MapAppearance {
                            draw_behind: true,
                            ..Default::default()
                        },
                    }),
                    Some(MarkerLayer::new(&frame, &permit).unwrap()),
                )
                .unwrap();
            let p = center_pixel(&renderer);
            match phase {
                0 => assert!(p[0] > 150 && p[1] < 10, "kind={kind} all: {p:?}"),
                1 => assert!(
                    p[2] > 250 && p[0] == 0,
                    "kind={kind} key hidden/same-owner peer: {p:?}"
                ),
                _ => assert!(
                    p[1] > 250 && p[2] == 0,
                    "kind={kind} owner hidden/other owner: {p:?}"
                ),
            }
            assert_eq!(
                renderer.marker_primitives.as_ref().unwrap().uploads,
                1,
                "Visibility changes must not reupload instances"
            );
        }
        if kind == 1 {
            let revoked = frame.permit(|k| k.asset_epoch.is_none() || k.asset_epoch == Some(2));
            renderer
                .render_combined_with_marker_layer(
                    [128, 128],
                    &empty_scene(),
                    camera,
                    &visuals,
                    None,
                    Some(MarkerLayer::new(&frame, &revoked).unwrap()),
                )
                .unwrap();
            assert!(
                center_pixel(&renderer)[2] > 250,
                "Revoking mesh assets must preserve same-owner non-asset geometry"
            );
            assert_eq!(renderer.marker_primitives.as_ref().unwrap().uploads, 1);
        }
        let current = frame.permit(|k| k.source_epoch == if k.owner == 1 { 2 } else { 1 });
        renderer
            .render_combined_with_marker_layer(
                [128, 128],
                &empty_scene(),
                camera,
                &visuals,
                None,
                Some(MarkerLayer::new(&frame, &current).unwrap()),
            )
            .unwrap();
        assert!(center_pixel(&renderer)[1] > 250);
        let expired = build(0);
        let allowed = expired.permit(|_| true);
        renderer
            .render_combined_with_marker_layer(
                [128, 128],
                &empty_scene(),
                camera,
                &visuals,
                None,
                Some(MarkerLayer::new(&expired, &allowed).unwrap()),
            )
            .unwrap();
        assert!(
            center_pixel(&renderer)[2] > 250,
            "Expired key must leave its same-owner peer"
        );
        let before = center_pixel(&renderer);
        let wrong = MarkerLayer {
            frame: &expired,
            permit: Some(&current),
        };
        assert!(renderer
            .render_combined_with_marker_layer(
                [128, 128],
                &empty_scene(),
                camera,
                &visuals,
                None,
                Some(wrong)
            )
            .is_err());
        assert_eq!(center_pixel(&renderer), before);
        let denied = frame.permit(|_| false);
        renderer
            .render_combined_with_marker_layer(
                [128, 128],
                &empty_scene(),
                camera,
                &visuals,
                Some(TextureMapFrame {
                    id: 61,
                    map: &map,
                    appearance: MapAppearance {
                        draw_behind: true,
                        ..Default::default()
                    },
                }),
                Some(MarkerLayer::new(&frame, &denied).unwrap()),
            )
            .unwrap();
        assert!(renderer.marker_primitives.is_none());
        assert!(!renderer.map_textures.is_empty());
        renderer
            .render_combined_with_marker_primitives(
                [128, 128],
                &empty_scene(),
                CameraPose::top_down(2., target),
                &visuals,
                None,
                Some(&frame),
            )
            .unwrap();
        assert!(
            renderer.marker_primitives.is_none(),
            "Legacy path must deny owned keys without a permit"
        );
    }
}
