use super::gpu_readback::{center_pixel, native_renderer};
use super::*;
use crate::spatial::{
    combined::CombinedSceneFrame,
    map_appearance::{MapAppearance, Palette},
    map_texture::{fixture, TextureMap},
};
use std::sync::Arc;
fn quad(map: &TextureMap, camera: CameraPose, size: [u32; 2]) -> [RenderVertex; 6] {
    let corners = map
        .corners()
        .map(|p| camera.clip(p, size[0] as f32 / size[1] as f32));
    let uv = [[0., 0.], [1., 0.], [1., 1.], [0., 1.]];
    [0, 1, 2, 0, 2, 3].map(|i| RenderVertex {
        clip: corners[i],
        color: [uv[i][0], uv[i][1], 0., 0.],
    })
}
fn image(renderer: &Renderer3d) -> Vec<u8> {
    let texture = renderer.color_texture().unwrap();
    let size = texture.size();
    let stride = (size.width * 4).div_ceil(256) * 256;
    let buffer = renderer
        .state
        .device
        .create_buffer(&wgpu::BufferDescriptor {
            label: Some("Map palette readback"),
            size: (stride * size.height) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
    let mut encoder = renderer
        .state
        .device
        .create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(stride),
                rows_per_image: Some(size.height),
            },
        },
        size,
    );
    renderer.state.queue.submit([encoder.finish()]);
    let (tx, rx) = std::sync::mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
    renderer.state.device.poll(wgpu::Maintain::Wait);
    rx.recv().unwrap().unwrap();
    let bytes = buffer.slice(..).get_mapped_range().to_vec();
    buffer.unmap();
    bytes
}
fn scene() -> CombinedSceneFrame<'static> {
    CombinedSceneFrame {
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
#[ignore = "requires native GPU; serialize integer-map palette and orientation readback"]
fn integer_map_all_palettes_rows_and_raw_signed_values() {
    let mut renderer = native_renderer();
    let map = Arc::new(
        TextureMap::prepare(fixture(16, 16, (0..256).map(|i| i as u8 as i8).collect())).unwrap(),
    );
    let camera = CameraPose::top_down(16., [8., 8., 0.]);
    for palette in [Palette::Map, Palette::Costmap, Palette::Raw] {
        renderer
            .render_combined_with_map(
                [256, 256],
                &scene(),
                camera,
                &egui::Visuals::dark(),
                Some(TextureMapFrame {
                    id: 1,
                    map: &map,
                    appearance: MapAppearance {
                        palette,
                        ..Default::default()
                    },
                }),
            )
            .unwrap();
        let bytes = image(&renderer);
        let colors = crate::spatial::map_palette::palette(palette);
        for (i, expected) in colors.iter().enumerate() {
            if expected[3] == 0 {
                continue;
            }
            let x = (i % 16) * 16 + 8;
            let y = (15 - i / 16) * 16 + 8;
            let actual = &bytes[(y * 256 + x) * 4..(y * 256 + x) * 4 + 4];
            assert!(
                actual
                    .iter()
                    .zip(expected)
                    .all(|(a, b)| a.abs_diff(*b) <= 1),
                "{palette:?} signed={} at{x},{y}: {actual:?} != {expected:?}",
                i as u8 as i8
            );
        }
        assert_eq!(renderer.map_textures.first().unwrap().uploads, 1);
    }
    eprintln!("PASS Metal readback:256 signed-byte indices, Map/Costmap/Raw palettes, row-zero at world origin, nearest cells, sRGB colors");
}
#[test]
#[ignore = "requires native GPU; serialize map depth, alpha and draw-behind readback"]
fn integer_map_alpha_depth_and_draw_behind() {
    let mut renderer = native_renderer();
    let map = Arc::new(TextureMap::prepare(fixture(2, 2, vec![100; 4])).unwrap());
    let camera = CameraPose::top_down(2., [1., 1., 0.]);
    let vertices = quad(&map, camera, [64, 64]);
    let triangles = |z, rgba| {
        [
            [-1., -1.],
            [1., -1.],
            [1., 1.],
            [-1., -1.],
            [1., 1.],
            [-1., 1.],
        ]
        .map(|p| RenderVertex {
            clip: [p[0], p[1], z, 1.],
            color: rgba,
        })
    };
    for (z, appearance, expected) in [
        (0.75, MapAppearance::default(), [0, 0, 0]),
        (0.25, MapAppearance::default(), [0, 0, 255]),
        (
            0.75,
            MapAppearance {
                draw_behind: true,
                ..Default::default()
            },
            [0, 0, 255],
        ),
        (
            0.75,
            MapAppearance {
                opacity: 0.5,
                ..Default::default()
            },
            [0, 0, 188],
        ),
        (
            0.75,
            MapAppearance {
                opacity: 0.,
                ..Default::default()
            },
            [0, 0, 255],
        ),
    ] {
        renderer.render_vertices_textured(
            [64, 64],
            &triangles(z, [0., 0., 1., 1.]),
            [1.; 4],
            &[],
            Some((
                TextureMapFrame {
                    id: 1,
                    map: &map,
                    appearance,
                },
                vertices,
            )),
        );
        let actual = center_pixel(&renderer);
        assert!(
            actual[..3]
                .iter()
                .zip(expected)
                .all(|(a, b)| a.abs_diff(b) <= 1),
            "z={z} {appearance:?}: {actual:?}"
        );
    }
    // Translucent peer in front must compose after the half-opacity map.
    renderer.render_vertices_textured(
        [64, 64],
        &triangles(0.25, [1., 0., 0., 0.5]),
        [1.; 4],
        &[],
        Some((
            TextureMapFrame {
                id: 1,
                map: &map,
                appearance: MapAppearance {
                    opacity: 0.5,
                    ..Default::default()
                },
            },
            vertices,
        )),
    );
    let actual = center_pixel(&renderer);
    assert!(
        actual[0].abs_diff(225) <= 1
            && actual[1].abs_diff(137) <= 1
            && actual[2].abs_diff(137) <= 1,
        "{actual:?}"
    );
    // Free costmap cells discard rather than occluding a farther opaque peer.
    let free = Arc::new(TextureMap::prepare(fixture(2, 2, vec![0; 4])).unwrap());
    renderer.render_vertices_textured(
        [64, 64],
        &triangles(0.75, [0., 0., 1., 1.]),
        [1.; 4],
        &[],
        Some((
            TextureMapFrame {
                id: 1,
                map: &free,
                appearance: MapAppearance {
                    palette: Palette::Costmap,
                    ..Default::default()
                },
            },
            quad(&free, camera, [64, 64]),
        )),
    );
    assert_eq!(center_pixel(&renderer), [0, 0, 255, 255]);
    eprintln!("PASS Metal readback: opaque depth, normal alpha ordering, zero-opacity, draw-behind, free costmap cell discard");
}
#[test]
#[ignore = "requires native GPU; serialize maximum map reuse, weak ownership and release"]
fn integer_map_maximum_reuse_resize_binding_and_release() {
    let mut renderer = native_renderer();
    let complete = fixture(2048, 2048, vec![50; 2048 * 2048]);
    let weak = Arc::downgrade(&complete);
    let map = Arc::new(TextureMap::prepare(complete).unwrap());
    let camera = CameraPose::top_down(2000., [1024., 1024., 0.]);
    for (size, palette) in [
        ([64, 64], Palette::Map),
        ([128, 64], Palette::Raw),
        ([64, 128], Palette::Costmap),
    ] {
        renderer
            .render_combined_with_map(
                size,
                &scene(),
                camera,
                &egui::Visuals::dark(),
                Some(TextureMapFrame {
                    id: 1,
                    map: &map,
                    appearance: MapAppearance {
                        palette,
                        opacity: 0.7,
                        ..Default::default()
                    },
                }),
            )
            .unwrap();
        assert_eq!(renderer.map_textures.first().unwrap().uploads, 1);
        assert_eq!(renderer.map_textures.first().unwrap().vertices.size(), 192);
        assert_eq!(
            renderer.vertices.as_ref().unwrap().size(),
            (MAX_VERTICES * 32) as u64
        );
        assert_eq!(Arc::strong_count(&map), 1);
        renderer.state.device.poll(wgpu::Maintain::Wait);
    }
    let shifted = CameraPose::top_down(1500., [1000., 1000., 0.]);
    renderer
        .render_combined_with_map(
            [64, 128],
            &scene(),
            shifted,
            &egui::Visuals::dark(),
            Some(TextureMapFrame {
                id: 1,
                map: &map,
                appearance: Default::default(),
            }),
        )
        .unwrap();
    assert_eq!(renderer.map_textures.first().unwrap().uploads, 1);
    let id = renderer.frame.as_ref().unwrap().id;
    let mut invalid = scene();
    invalid.fixed_frame = "other";
    assert!(renderer
        .render_combined_with_map(
            [64, 64],
            &invalid,
            camera,
            &egui::Visuals::dark(),
            Some(TextureMapFrame {
                id: 1,
                map: &map,
                appearance: Default::default()
            })
        )
        .is_err());
    assert_eq!(renderer.frame.as_ref().unwrap().id, id);
    drop(map);
    assert!(weak.upgrade().is_none(), "GPU cache retained raw map cells");
    renderer
        .render_combined([64, 64], &scene(), camera, &egui::Visuals::dark())
        .unwrap();
    assert!(renderer.map_textures.is_empty());
    let replacement = Arc::new(TextureMap::prepare(fixture(1, 1, vec![100])).unwrap());
    renderer
        .render_combined_with_map(
            [64, 64],
            &scene(),
            camera,
            &egui::Visuals::dark(),
            Some(TextureMapFrame {
                id: 1,
                map: &replacement,
                appearance: Default::default(),
            }),
        )
        .unwrap();
    assert!(!renderer.map_textures.is_empty());
    renderer.release_frame();
    assert!(renderer.map_textures.is_empty());
    assert!(renderer.vertices.is_none() && renderer.frame.is_none());
    eprintln!("PASS Metal resources:2048-square R8Uint,4MiB raw upload,192B vertices, unchanged legacy budget, palette/resize reuse, weak CPU identity, invalid-frame preservation, Clear/release");
}

#[test]
#[ignore = "requires native GPU; serialize three-map texture and independent cleanup readback"]
fn three_maps_render_independent_textures_and_survive_peer_removal() {
    let mut renderer = native_renderer();
    let maps: Vec<_> = (0..3)
        .map(|index| {
            let mut complete = fixture(2, 2, vec![index * 50; 4]);
            Arc::get_mut(&mut complete).unwrap().header.transform[0] = index as f64 * 3.;
            Arc::new(TextureMap::prepare(complete).unwrap())
        })
        .collect();
    let camera = CameraPose::top_down(10., [4., 1., 0.]);
    let layers: Vec<_> = maps
        .iter()
        .enumerate()
        .map(|(index, map)| TextureMapFrame {
            id: index as u64 + 1,
            map,
            appearance: Default::default(),
        })
        .collect();
    renderer
        .render_combined_with_maps(
            [300, 300],
            &scene(),
            camera,
            &egui::Visuals::dark(),
            &layers,
            None,
            &[],
        )
        .unwrap();
    assert_eq!(renderer.map_textures.len(), 3);
    let pixels = image(&renderer);
    let palette = crate::spatial::map_palette::palette(Palette::Map);
    for index in 0..3 {
        let clip = camera.clip(crate::spatial::Point3([index as f64 * 3. + 1., 1., 0.]), 1.);
        let x = ((clip[0] / clip[3] + 1.) * 150.) as usize;
        let y = ((1. - clip[1] / clip[3]) * 150.) as usize;
        let stride = (300usize * 4).div_ceil(256) * 256;
        let actual = &pixels[y * stride + x * 4..y * stride + x * 4 + 4];
        assert!(
            actual
                .iter()
                .zip(palette[index * 50])
                .all(|(a, b)| a.abs_diff(b) <= 1),
            "map{index}: {actual:?}"
        );
    }
    let uploads: Vec<_> = renderer
        .map_textures
        .iter()
        .map(|gpu| gpu.uploads)
        .collect();
    let mut edited = layers.clone();
    edited[1].appearance.opacity = 0.4;
    edited[1].appearance.palette = Palette::Costmap;
    renderer
        .render_combined_with_maps(
            [320, 300],
            &scene(),
            camera,
            &egui::Visuals::dark(),
            &edited,
            None,
            &[],
        )
        .unwrap();
    assert_eq!(
        renderer
            .map_textures
            .iter()
            .map(|gpu| gpu.uploads)
            .collect::<Vec<_>>(),
        uploads
    );
    assert!(renderer
        .render_combined_with_maps(
            [300, 300],
            &scene(),
            camera,
            &egui::Visuals::dark(),
            &[layers[0]; 4],
            None,
            &[]
        )
        .is_err());
    assert!(renderer
        .render_combined_with_maps(
            [300, 300],
            &scene(),
            camera,
            &egui::Visuals::dark(),
            &[layers[0]; 2],
            None,
            &[]
        )
        .is_err());
    assert_eq!(
        renderer.map_textures.len(),
        3,
        "Invalid candidate preserves completed resources"
    );
    renderer
        .render_combined_with_maps(
            [300, 300],
            &scene(),
            camera,
            &egui::Visuals::dark(),
            &[layers[0], layers[2]],
            None,
            &[],
        )
        .unwrap();
    assert_eq!(renderer.map_textures.len(), 2);
    assert_eq!(renderer.map_textures[0].uploads, uploads[0]);
    renderer
        .render_combined_with_maps(
            [300, 300],
            &scene(),
            camera,
            &egui::Visuals::dark(),
            &[],
            None,
            &[],
        )
        .unwrap();
    assert!(renderer.map_textures.is_empty());
}
