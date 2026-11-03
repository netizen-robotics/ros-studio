use super::{
    gpu_readback::{native_renderer, texture_pixel},
    *,
};
use crate::spatial::{
    calibrated_camera::RectifiedProjection,
    calibrated_view::{tests::fixture, CalibratedFrame, CalibratedSceneView, CameraPresentation},
    combined::{CombinedSceneFrame, MapLayerFrame},
    map_geometry::prepare_map,
    RigidTransform,
};
use studio_workspace::camera::ImagePlacement;

fn composite(
    ctx: &egui::Context,
    view: &mut CalibratedSceneView,
    frame: &CalibratedFrame<'_>,
    renderer: &mut Renderer3d,
    target: &wgpu::Texture,
    active: bool,
) -> usize {
    let output = ctx.run(
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(128., 128.),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default()
                .frame(egui::Frame::NONE)
                .show(ctx, |ui| {
                    assert_eq!(
                        view.show(ui, frame, Some(renderer), 128.).unwrap(),
                        if active {
                            CameraPresentation::Presented
                        } else {
                            CameraPresentation::Superseded
                        }
                    );
                });
        },
    );
    let uploads = output
        .textures_delta
        .set
        .iter()
        .filter(|(_, delta)| delta.image.size() == [2, 2])
        .count();
    let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
    let screen = egui_wgpu::ScreenDescriptor {
        size_in_pixels: [128, 128],
        pixels_per_point: output.pixels_per_point,
    };
    let state = &renderer.state;
    let mut gpu = state.renderer.write();
    for (id, delta) in &output.textures_delta.set {
        gpu.update_texture(&state.device, &state.queue, *id, delta);
    }
    let mut encoder = state.device.create_command_encoder(&Default::default());
    let mut commands =
        gpu.update_buffers(&state.device, &state.queue, &mut encoder, &jobs, &screen);
    {
        let attachment = target.create_view(&Default::default());
        let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Calibrated complete egui composition"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: &attachment,
                resolve_target: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            occlusion_query_set: None,
            timestamp_writes: None,
        });
        gpu.render(&mut pass.forget_lifetime(), &jobs, &screen);
    }
    commands.push(encoder.finish());
    state.queue.submit(commands);
    for id in output.textures_delta.free {
        gpu.free_texture(&id);
    }
    uploads
}

#[test]
#[ignore = "requires native Metal/wgpu; serialize calibrated complete composition/depth/cache/Stop readback"]
fn calibrated_camera_complete_composition_orientation_alpha_cache_and_revocation() {
    let base = native_renderer();
    assert_eq!(base.state.adapter.get_info().backend, wgpu::Backend::Metal);
    let mut renderer = Renderer3d::with_sensor_history(base.state.clone());
    let target = renderer
        .state
        .device
        .create_texture(&wgpu::TextureDescriptor {
            label: Some("Complete calibrated composite readback"),
            size: wgpu::Extent3d {
                width: 128,
                height: 128,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
    let map = prepare_map(
        1,
        1,
        2.,
        RigidTransform::new([-1., -1., 2.], [0., 0., 0., 1.]).unwrap(),
        RigidTransform::identity(),
        &[0],
    )
    .unwrap();
    let scene = CombinedSceneFrame {
        fixed_frame: "world",
        robot: None,
        layers: &[],
        tf: None,
        pose: None,
        path: None,
        map: Some(MapLayerFrame {
            id: 1,
            local_generation: 1,
            sequence: 1,
            map: Some(&map),
            status: "Receiving",
            error: None,
        }),
    };
    let (mut receiver, accepted, image, projection) = fixture();
    let frame = CalibratedFrame {
        snapshot: &accepted.snapshot,
        permission: &accepted.permission,
        image: &image,
        projection,
        scene: &scene,
        overlays: Default::default(),
    };
    let ctx = egui::Context::default();
    ctx.set_pixels_per_point(1.);
    let mut view = CalibratedSceneView::default();
    let uploads = composite(&ctx, &mut view, &frame, &mut renderer, &target, true);
    assert_eq!(uploads, 1);
    let pixel = |at| texture_pixel(&renderer.state, &target, at);
    let top = pixel([16, 16]);
    let bottom = pixel([16, 112]);
    let geometry = pixel([48, 48]);
    eprintln!("Calibrated background: top={top:?}, bottom={bottom:?}, geometry={geometry:?}");
    assert!(top[0] > 245 && top[1] < 5 && top[2] < 5);
    assert!(bottom[2] > 245 && bottom[0] < 5 && bottom[1] < 5);
    assert!(geometry[..3].iter().all(|v| *v > 200));
    view.appearance.placement = ImagePlacement::Overlay;
    view.appearance.overlay_alpha_milli = 1000;
    assert_eq!(
        composite(&ctx, &mut view, &frame, &mut renderer, &target, true),
        0
    );
    let overlay = texture_pixel(&renderer.state, &target, [48, 48]);
    assert!(overlay[0] > 245 && overlay[1] < 5 && overlay[2] < 5);
    view.appearance.overlay_alpha_milli = 0;
    assert_eq!(
        composite(&ctx, &mut view, &frame, &mut renderer, &target, true),
        0
    );
    assert_eq!(texture_pixel(&renderer.state, &target, [48, 48]), geometry);
    view.appearance.placement = ImagePlacement::Both;
    view.appearance.overlay_alpha_milli = 500;
    assert_eq!(
        composite(&ctx, &mut view, &frame, &mut renderer, &target, true),
        0
    );
    let mixed = texture_pixel(&renderer.state, &target, [48, 48]);
    assert!(mixed[0] > mixed[1] + 30 && mixed[1] > 50 && mixed[1] < geometry[1]);
    // Same map/camera basis and source identity, changed principal point: cached
    // legacy map vertices must be regenerated for the new intrinsics.
    let shifted = CameraPose::calibrated(
        RigidTransform::identity(),
        RectifiedProjection::new(
            [2, 2],
            [1., 0., 1.5, 0., 0., 1., 1., 0., 0., 0., 1., 0.],
            0.05,
            10000.,
        )
        .unwrap(),
        1.,
    )
    .unwrap();
    renderer
        .render_combined([128, 128], &scene, shifted, &egui::Visuals::dark())
        .unwrap();
    assert_eq!(
        texture_pixel(&renderer.state, renderer.color_texture().unwrap(), [48, 48]),
        [0; 4]
    );
    // A near-plane/behind-camera map cannot cover the camera image.
    let behind = prepare_map(
        1,
        1,
        2.,
        RigidTransform::new([-1., -1., -2.], [0., 0., 0., 1.]).unwrap(),
        RigidTransform::identity(),
        &[100],
    )
    .unwrap();
    let behind_scene = CombinedSceneFrame {
        map: Some(MapLayerFrame {
            map: Some(&behind),
            ..scene.map.unwrap()
        }),
        ..scene
    };
    let behind_frame = CalibratedFrame {
        scene: &behind_scene,
        ..frame
    };
    composite(&ctx, &mut view, &behind_frame, &mut renderer, &target, true);
    let uncovered = texture_pixel(&renderer.state, &target, [48, 48]);
    assert!(uncovered[0] > 245 && uncovered[1] < 5);
    // A source frame can retire authority after GPU preparation but before
    // publication. Hide obsolete output, then recover on the next fresh frame.
    let revocation = receiver.revocation();
    view.before_present = Some(Box::new(move || revocation.revoke()));
    composite(
        &ctx,
        &mut view,
        &behind_frame,
        &mut renderer,
        &target,
        false,
    );
    assert!(renderer.color_texture().is_none());
    assert!(view.marker_request.is_none());
    assert_ne!(texture_pixel(&renderer.state, &target, [48, 48]), uncovered);
    let mut fresh = accepted.snapshot.as_ref().clone();
    fresh.revision += 1;
    fresh.counters.images += 1;
    fresh.frame.as_mut().unwrap().image_sequence += 1;
    let recovered = receiver.apply(fresh).unwrap().unwrap();
    let recovered_frame = CalibratedFrame {
        snapshot: &recovered.snapshot,
        permission: &recovered.permission,
        ..behind_frame
    };
    composite(
        &ctx,
        &mut view,
        &recovered_frame,
        &mut renderer,
        &target,
        true,
    );
    assert!(recovered.permission.authorizes(&recovered.snapshot));
    assert_eq!(texture_pixel(&renderer.state, &target, [48, 48]), uncovered);
    receiver.stop();
    composite(
        &ctx,
        &mut view,
        &recovered_frame,
        &mut renderer,
        &target,
        false,
    );
    assert!(renderer.color_texture().is_none());
    assert!(view.marker_request.is_none());
    assert_eq!(
        texture_pixel(&renderer.state, &target, [48, 48]),
        [0, 0, 0, 255]
    );
}
