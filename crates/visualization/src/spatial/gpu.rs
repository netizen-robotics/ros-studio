//! Bounded offscreen perspective rasterization; no blocking GPU waits on UI frames.
mod draw_order;
#[cfg(test)]
mod map_tests;
mod map_texture;
use draw_order::{Draw, Source};
#[cfg(test)]
mod camera_tests;
#[cfg(test)]
mod marker_tests;
mod textures;
#[cfg(test)]
mod uncertainty_tests;
use super::map_texture::TextureMapFrame;
use super::{camera3d::CameraPose, Point3, Scene, MAX_LAYERS, MAX_POINTS};
use eframe::{
    egui,
    egui_wgpu::{self, wgpu},
};
pub const MAX_DIMENSION: u32 = 2048;
const MAX_LEGACY_SCENE_VERTICES: usize = (MAX_LAYERS * MAX_POINTS + 45) * 6;
pub const MAX_SCENE_VERTICES: usize = (super::combined::MAX_TOTAL_POINTS + 45) * 6;
pub const MAX_VERTICES: usize = MAX_SCENE_VERTICES
    + crate::robot::MAX_ROBOT_TRIANGLES * 3
    + super::map_geometry::MAX_MAP_VERTICES
    + super::overlay_geometry::MAX_PATH_VERTICES
    + super::overlay_geometry::POSE_VERTICES;
pub const MAX_HISTORY_VERTICES: usize = MAX_VERTICES
    + (super::combined::MAX_HISTORY_TOTAL_POINTS - super::combined::MAX_TOTAL_POINTS) * 6
    + crate::tf_graph::MAX_VERTICES
    + super::uncertainty::MAX_VERTICES;
/// Uniformly downsample attachments while retaining the logical viewport aspect.
pub fn bounded_dimensions(size: [u32; 2]) -> [u32; 2] {
    let size = size.map(|s| s.max(1));
    let scale = (MAX_DIMENSION as f64 / size[0].max(size[1]) as f64).min(1.0);
    size.map(|s| ((s as f64 * scale).round() as u32).clamp(1, MAX_DIMENSION))
}
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct RenderVertex {
    pub clip: [f32; 4],
    pub color: [f32; 4],
}
/// Portable little-endian fallback, and the independent byte-layout test oracle.
#[cfg(any(test, target_endian = "big"))]
fn vertex_bytes(vertices: &[RenderVertex]) -> Vec<u8> {
    let mut bytes = vec![0; vertices.len() * 32];
    for (vertex, dst) in vertices.iter().zip(bytes.chunks_exact_mut(32)) {
        dst[0..4].copy_from_slice(&vertex.clip[0].to_le_bytes());
        dst[4..8].copy_from_slice(&vertex.clip[1].to_le_bytes());
        dst[8..12].copy_from_slice(&vertex.clip[2].to_le_bytes());
        dst[12..16].copy_from_slice(&vertex.clip[3].to_le_bytes());
        dst[16..20].copy_from_slice(&vertex.color[0].to_le_bytes());
        dst[20..24].copy_from_slice(&vertex.color[1].to_le_bytes());
        dst[24..28].copy_from_slice(&vertex.color[2].to_le_bytes());
        dst[28..32].copy_from_slice(&vertex.color[3].to_le_bytes());
    }
    bytes
}
struct Frame {
    size: [u32; 2],
    color: wgpu::Texture,
    color_view: wgpu::TextureView,
    _depth: wgpu::Texture,
    depth_view: wgpu::TextureView,
    id: egui::TextureId,
}
pub struct Renderer3d {
    state: egui_wgpu::RenderState,
    pipeline: wgpu::RenderPipeline,
    transparent: wgpu::RenderPipeline,
    max_vertices: usize,
    history_enabled: bool,
    textures: textures::Textures,
    map_textures: Vec<map_texture::MapTexture>,
    marker_primitives: Option<super::marker_primitives::Renderer>,
    vertices: Option<wgpu::Buffer>,
    frame: Option<Frame>,
    map_cache: Option<(MapCacheKey, Vec<RenderVertex>)>,
}
#[derive(PartialEq)]
struct MapCacheKey {
    projection: Option<u32>,
    calibrated: Option<super::camera3d::CalibratedView>,
    owner: u64,
    generation: u64,
    sequence: u64,
    camera: [u32; 12],
    size: [u32; 2],
    corners: usize,
    cells: usize,
}
/// Logical Marker GPU allocations retained by this scene renderer.
/// Excludes driver overhead, framebuffer and shared unit meshes; not process RSS.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct MarkerAllocations {
    pub instance_bytes: usize,
    pub glyph_atlas_bytes: usize,
    pub mesh_texture_bytes: usize,
}
impl Renderer3d {
    pub fn new(state: egui_wgpu::RenderState) -> Self {
        Self::create(state, false)
    }
    pub fn with_sensor_history(state: egui_wgpu::RenderState) -> Self {
        Self::create(state, true)
    }
    fn create(state: egui_wgpu::RenderState, history_enabled: bool) -> Self {
        let device = &state.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("ROS scene 3D"),
            source: wgpu::ShaderSource::Wgsl(include_str!("scene3d.wgsl").into()),
        });
        let make_pipeline = |transparent: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("ROS scene perspective depth pipeline"),
                layout: None,
                vertex: wgpu::VertexState {
                    module: &shader,
                    entry_point: Some("vs_main"),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: 32,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &wgpu::vertex_attr_array![0=>Float32x4,1=>Float32x4],
                    }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &shader,
                    entry_point: Some("fs_main"),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: wgpu::TextureFormat::Rgba8UnormSrgb,
                        blend: transparent.then_some(wgpu::BlendState::ALPHA_BLENDING),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: !transparent,
                    depth_compare: wgpu::CompareFunction::LessEqual,
                    stencil: Default::default(),
                    bias: Default::default(),
                }),
                multisample: Default::default(),
                multiview: None,
                cache: None,
            })
        };
        let pipeline = make_pipeline(false);
        let transparent = make_pipeline(true);
        let textures = textures::Textures::new(&state);
        Self {
            textures,
            map_textures: Vec::new(),
            marker_primitives: None,
            state,
            pipeline,
            transparent,
            history_enabled,
            max_vertices: if history_enabled {
                MAX_HISTORY_VERTICES
            } else {
                MAX_VERTICES
            },
            vertices: None,
            frame: None,
            map_cache: None,
        }
    }
    pub fn clear_texture_map(&mut self) {
        self.map_textures.clear();
    }
    pub fn marker_repaint_after(
        &self,
        frame: &super::marker_primitives::ReadyFrame,
    ) -> Option<std::time::Duration> {
        self.marker_primitives
            .as_ref()?
            .rendered_clock_for(frame)
            .and_then(|tick| frame.next_expiry_after(tick))
    }
    pub fn marker_allocations(&self) -> MarkerAllocations {
        self.marker_primitives
            .as_ref()
            .map_or_else(MarkerAllocations::default, |m| MarkerAllocations {
                instance_bytes: m.allocated_instance_bytes(),
                glyph_atlas_bytes: m.allocated_atlas_bytes(),
                mesh_texture_bytes: m.allocated_mesh_texture_bytes(),
            })
    }
    pub fn clear_marker_primitives(&mut self) {
        self.marker_primitives = None;
    }
    pub fn release_frame(&mut self) {
        self.textures.clear();
        self.marker_primitives = None;
        self.map_textures.clear();
        self.vertices = None;
        self.map_cache = None;
        if let Some(frame) = self.frame.take() {
            self.state.renderer.write().free_texture(&frame.id);
        }
    }
    pub fn color_texture(&self) -> Option<&wgpu::Texture> {
        self.frame.as_ref().map(|f| &f.color)
    }
    pub fn frame_size(&self) -> Option<[u32; 2]> {
        self.frame.as_ref().map(|f| f.size)
    }
    fn ensure_frame(&mut self, size: [u32; 2]) {
        if self.frame.as_ref().is_some_and(|f| f.size == size) {
            return;
        }
        let map_textures = std::mem::take(&mut self.map_textures);
        self.release_frame();
        self.map_textures = map_textures;
        self.vertices = Some(self.state.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Bounded scene vertices"),
            size: (self.max_vertices * 32) as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        let extent = wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        };
        let texture = |format, usage, label| {
            self.state.device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: extent,
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage,
                view_formats: &[],
            })
        };
        let color = texture(
            wgpu::TextureFormat::Rgba8UnormSrgb,
            wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            "Scene color",
        );
        let depth = texture(
            wgpu::TextureFormat::Depth32Float,
            wgpu::TextureUsages::RENDER_ATTACHMENT,
            "Scene depth",
        );
        let color_view = color.create_view(&Default::default());
        let depth_view = depth.create_view(&Default::default());
        let id = self.state.renderer.write().register_native_texture(
            &self.state.device,
            &color_view,
            wgpu::FilterMode::Linear,
        );
        self.frame = Some(Frame {
            size,
            color,
            color_view,
            _depth: depth,
            depth_view,
            id,
        });
    }
    /// Public primitive seam for deterministic depth readback tests. Reject invalid
    /// inputs before allocation; no truncation silently masquerades as complete data.
    pub fn render_vertices(
        &mut self,
        size: [u32; 2],
        vertices: &[RenderVertex],
    ) -> egui::TextureId {
        self.map_cache = None;
        self.render_vertices_clear(size, vertices, [0.018, 0.024, 0.035, 1.0])
    }
    fn render_vertices_clear(
        &mut self,
        size: [u32; 2],
        vertices: &[RenderVertex],
        background: [f32; 4],
    ) -> egui::TextureId {
        self.render_vertices_textured(size, vertices, background, &[], None)
    }
    fn render_vertices_textured(
        &mut self,
        size: [u32; 2],
        vertices: &[RenderVertex],
        background: [f32; 4],
        batches: &[super::robot_geometry::TextureBatch],
        map: Option<(TextureMapFrame<'_>, [RenderVertex; 6])>,
    ) -> egui::TextureId {
        self.render_vertices_textured_with_primitives(
            size,
            vertices,
            background,
            batches,
            map.as_slice(),
            None,
        )
    }
    fn render_vertices_textured_with_primitives(
        &mut self,
        size: [u32; 2],
        vertices: &[RenderVertex],
        background: [f32; 4],
        batches: &[super::robot_geometry::TextureBatch],
        maps: &[(TextureMapFrame<'_>, [RenderVertex; 6])],
        layer: Option<super::marker_primitives::MarkerLayer<'_>>,
    ) -> egui::TextureId {
        let primitives = layer.map(|l| l.frame);
        let permit = layer.and_then(|l| l.permit);
        assert!(
            vertices.len() + batches.iter().map(|b| b.vertices.len()).sum::<usize>()
                <= self.max_vertices
        );
        assert!(
            vertices.len() <= self.max_vertices && vertices.len().is_multiple_of(3),
            "scene vertex budget"
        );
        assert!(
            vertices
                .iter()
                .all(|v| v.clip.iter().all(|n| n.is_finite())
                    && v.color.iter().all(|n| n.is_finite())),
            "finite vertices"
        );
        let primitives = primitives.filter(|p| !p.all_expired() && p.has_visible(permit));
        let size = bounded_dimensions(size);
        self.ensure_frame(size);
        self.textures.sync(&self.state, batches);
        if let Some(frame) = primitives.filter(|f| !f.is_empty()) {
            self.marker_primitives
                .get_or_insert_with(|| super::marker_primitives::Renderer::new(&self.state))
                .upload_ready(&self.state, frame)
                .expect("validated primitive frame/device");
        } else {
            self.marker_primitives = None;
        }

        self.map_textures.truncate(maps.len());
        for (index, (layer, vertices)) in maps.iter().enumerate() {
            if self.map_textures.len() == index {
                self.map_textures
                    .push(map_texture::MapTexture::new(&self.state));
            }
            self.map_textures[index].sync(&self.state, *layer, vertices);
        }

        // repr(C)+Pod proves the 32-byte vertex has no padding/uninitialized
        // bytes. Little-endian hosts upload the existing immutable slice with
        // no full-frame serialization allocation; other hosts keep LE packing.
        #[cfg(target_endian = "little")]
        let bytes: &[u8] = bytemuck::cast_slice(vertices);
        #[cfg(target_endian = "big")]
        let storage = vertex_bytes(vertices);
        #[cfg(target_endian = "big")]
        let bytes = storage.as_slice();
        if !bytes.is_empty() {
            self.state
                .queue
                .write_buffer(self.vertices.as_ref().unwrap(), 0, bytes);
        }
        let mut offset = vertices.len() * 32;
        for b in batches {
            self.state.queue.write_buffer(
                self.vertices.as_ref().unwrap(),
                offset as u64,
                bytemuck::cast_slice(&b.vertices),
            );
            offset += b.vertices.len() * 32;
        }
        let frame = self.frame.as_ref().unwrap();
        let mut encoder =
            self.state
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("Scene frame"),
                });
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Scene depth pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &frame.color_view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: background[0] as f64,
                            g: background[1] as f64,
                            b: background[2] as f64,
                            a: background[3] as f64,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &frame.depth_view,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            for (map_index, (layer, _)) in maps.iter().enumerate() {
                if layer.appearance.draw_behind {
                    let gpu = &self.map_textures[map_index];
                    pass.set_pipeline(&gpu.transparent);
                    pass.set_bind_group(0, gpu.bind(), &[]);
                    pass.set_vertex_buffer(0, gpu.vertices.slice(..));
                    pass.draw(0..6, 0..1);
                }
            }
            pass.set_pipeline(&self.pipeline);
            pass.set_vertex_buffer(0, self.vertices.as_ref().unwrap().slice(..));
            // Opaque runs write depth; translucent sensor triangles join the
            // same normalized-depth order as Robot materials below.
            let transparent_count = vertices
                .chunks_exact(3)
                .filter(|chunk| {
                    chunk.iter().any(|v| v.color[3] < 1.) && chunk.iter().any(|v| v.color[3] > 0.)
                })
                .count()
                + batches
                    .iter()
                    .filter(|b| b.transparent)
                    .map(|b| b.vertices.len() / 3)
                    .sum::<usize>();
            let mut transparent = Vec::with_capacity(transparent_count);
            let mut opaque_start = None;
            for (triangle, chunk) in vertices.chunks_exact(3).enumerate() {
                let start = (triangle * 3) as u32;
                if chunk.iter().all(|v| v.color[3] >= 1.) {
                    opaque_start.get_or_insert(start);
                } else {
                    if let Some(first) = opaque_start.take() {
                        pass.draw(first..start, 0..1);
                    }
                    if chunk.iter().any(|v| v.color[3] > 0.) {
                        let depth = chunk.iter().map(|v| v.clip[2] / v.clip[3]).sum::<f32>() / 3.;
                        transparent.push(Draw {
                            source: Source::Vertices,
                            index: start,
                            depth,
                        });
                    }
                }
            }
            if let Some(first) = opaque_start {
                pass.draw(first..vertices.len() as u32, 0..1);
            }
            pass.set_pipeline(&self.textures.pipeline);
            let mut start = vertices.len() as u32;
            for (i, batch) in batches.iter().enumerate() {
                let end = start + batch.vertices.len() as u32;
                if batch.transparent {
                    for (triangle, chunk) in batch.vertices.chunks_exact(3).enumerate() {
                        let depth = chunk.iter().map(|v| v.clip[2] / v.clip[3]).sum::<f32>() / 3.;
                        transparent.push(Draw {
                            source: Source::Texture(i),
                            index: start + triangle as u32 * 3,
                            depth,
                        });
                    }
                } else {
                    pass.set_bind_group(0, self.textures.bind(batch), &[]);
                    pass.draw(start..end, 0..1);
                }
                start = end;
            }
            for (map_index, (layer, vertices)) in maps.iter().enumerate() {
                if !layer.appearance.draw_behind && layer.appearance.opacity > 0. {
                    if layer.appearance.opacity < 1. {
                        for (i, chunk) in vertices.chunks_exact(3).enumerate() {
                            let depth =
                                chunk.iter().map(|v| v.clip[2] / v.clip[3]).sum::<f32>() / 3.;
                            transparent.push(Draw {
                                source: Source::Map(map_index),
                                index: i as u32 * 3,
                                depth,
                            });
                        }
                    } else {
                        let gpu = &self.map_textures[map_index];
                        pass.set_pipeline(&gpu.opaque);
                        pass.set_bind_group(0, gpu.bind(), &[]);
                        pass.set_vertex_buffer(0, gpu.vertices.slice(..));
                        pass.draw(0..6, 0..1);
                    }
                }
            }
            if let Some(primitives) = primitives {
                for (mesh, range) in primitives.opaque_visible(permit) {
                    self.marker_primitives
                        .as_ref()
                        .unwrap()
                        .draw(&mut pass, mesh, range, false);
                }
            }
            draw_order::sort(&mut transparent);
            let markers = primitives.map_or(&[][..], |p| p.transparent_instances());
            for (source, range) in draw_order::merge_visible(&transparent, markers, permit) {
                match source {
                    Source::Marker(kind) => self
                        .marker_primitives
                        .as_ref()
                        .unwrap()
                        .draw(&mut pass, kind, range, true),
                    Source::Map(map_index) => {
                        let gpu = &self.map_textures[map_index];
                        pass.set_pipeline(&gpu.transparent);
                        pass.set_bind_group(0, gpu.bind(), &[]);
                        pass.set_vertex_buffer(0, gpu.vertices.slice(..));
                        pass.draw(range, 0..1);
                    }
                    Source::Texture(batch) => {
                        pass.set_vertex_buffer(0, self.vertices.as_ref().unwrap().slice(..));
                        pass.set_pipeline(&self.textures.transparent);
                        pass.set_bind_group(0, self.textures.bind(&batches[batch]), &[]);
                        pass.draw(range, 0..1);
                    }
                    Source::Vertices => {
                        pass.set_vertex_buffer(0, self.vertices.as_ref().unwrap().slice(..));
                        pass.set_pipeline(&self.transparent);
                        pass.draw(range, 0..1);
                    }
                }
            }
        }
        self.state.queue.submit([encoder.finish()]);
        frame.id
    }
    /// Borrow one immutable scene snapshot. Validate all layers before replacing
    /// the framebuffer; no selection-dependent camera or retained geometry.
    pub fn render_combined(
        &mut self,
        size: [u32; 2],
        frame: &super::combined::CombinedSceneFrame<'_>,
        camera: CameraPose,
        visuals: &egui::Visuals,
    ) -> Result<egui::TextureId, super::combined::CombinedSceneError> {
        self.render_combined_with_map(size, frame, camera, visuals, None)
    }
    /// Texture-map path; preparation belongs on the data worker. Validation
    /// precedes any GPU replacement and legacy scene roots remain unchanged.
    pub fn render_combined_with_map(
        &mut self,
        size: [u32; 2],
        frame: &super::combined::CombinedSceneFrame<'_>,
        camera: CameraPose,
        visuals: &egui::Visuals,
        map: Option<TextureMapFrame<'_>>,
    ) -> Result<egui::TextureId, super::combined::CombinedSceneError> {
        self.render_combined_with_marker_primitives(size, frame, camera, visuals, map, None)
    }
    /// Marker projection is prepared outside active UI frames. Exact camera/
    /// viewport validation precedes any framebuffer or resource replacement.
    pub fn render_combined_with_marker_primitives(
        &mut self,
        size: [u32; 2],
        frame: &super::combined::CombinedSceneFrame<'_>,
        camera: CameraPose,
        visuals: &egui::Visuals,
        map: Option<TextureMapFrame<'_>>,
        primitives: Option<&super::marker_primitives::ReadyFrame>,
    ) -> Result<egui::TextureId, super::combined::CombinedSceneError> {
        self.render_combined_with_marker_layer(
            size,
            frame,
            camera,
            visuals,
            map,
            primitives.map(super::marker_primitives::MarkerLayer::unkeyed),
        )
    }
    pub fn render_combined_with_marker_layer(
        &mut self,
        size: [u32; 2],
        frame: &super::combined::CombinedSceneFrame<'_>,
        camera: CameraPose,
        visuals: &egui::Visuals,
        map: Option<TextureMapFrame<'_>>,
        layer: Option<super::marker_primitives::MarkerLayer<'_>>,
    ) -> Result<egui::TextureId, super::combined::CombinedSceneError> {
        self.render_combined_with_uncertainty(size, frame, camera, visuals, map, layer, &[])
    }
    #[allow(clippy::too_many_arguments)]
    pub fn render_combined_with_uncertainty(
        &mut self,
        size: [u32; 2],
        frame: &super::combined::CombinedSceneFrame<'_>,
        camera: CameraPose,
        visuals: &egui::Visuals,
        map: Option<TextureMapFrame<'_>>,
        layer: Option<super::marker_primitives::MarkerLayer<'_>>,
        uncertainty: &[super::uncertainty::Layer<'_>],
    ) -> Result<egui::TextureId, super::combined::CombinedSceneError> {
        self.render_combined_with_maps(
            size,
            frame,
            camera,
            visuals,
            map.as_slice(),
            layer,
            uncertainty,
        )
    }
    #[allow(clippy::too_many_arguments)]
    pub fn render_combined_with_maps(
        &mut self,
        size: [u32; 2],
        frame: &super::combined::CombinedSceneFrame<'_>,
        camera: CameraPose,
        visuals: &egui::Visuals,
        maps: &[TextureMapFrame<'_>],
        layer: Option<super::marker_primitives::MarkerLayer<'_>>,
        uncertainty: &[super::uncertainty::Layer<'_>],
    ) -> Result<egui::TextureId, super::combined::CombinedSceneError> {
        let primitives = layer.map(|l| l.frame);
        let permit = layer.and_then(|l| l.permit);
        use super::combined::{content_vertices, CombinedSceneError};
        if maps.len() > studio_protocol::mapping::MAX_MAP_LAYERS {
            return Err(CombinedSceneError::new(
                "Map display limit reached (3 maps)",
            ));
        }
        for (index, map) in maps.iter().enumerate() {
            if maps[..index].iter().any(|other| other.id == map.id) {
                return Err(CombinedSceneError::new("Duplicate map display identity"));
            }
        }
        if let Some(permit) = permit {
            permit
                .validate_for(
                    primitives
                        .ok_or_else(|| CombinedSceneError::new("Marker permit has no frame"))?,
                )
                .map_err(CombinedSceneError::new)?;
        }
        // Fully denied/expired cached Marker projections cannot block a new
        // camera for surviving non-Marker scene content.
        let primitives = primitives.filter(|f| !f.all_expired() && f.has_visible(permit));
        if let Some(primitives) = primitives {
            primitives
                .validate_projection(camera, bounded_dimensions(size))
                .map_err(CombinedSceneError::new)?;
            let limits = self.state.device.limits();
            if primitives
                .image_dimensions()
                .any(|s| s.into_iter().any(|n| n > limits.max_texture_dimension_2d))
            {
                return Err(CombinedSceneError::new(
                    "Marker mesh image exceeds GPU texture dimensions",
                ));
            }
            if primitives
                .atlas_dimensions()
                .is_some_and(|s| s.into_iter().any(|n| n > limits.max_texture_dimension_2d))
            {
                return Err(CombinedSceneError::new(
                    "Marker glyph atlas exceeds GPU texture dimensions",
                ));
            }
            if !primitives.is_empty()
                && (primitives.len() * super::marker_primitives::INSTANCE_BYTES
                    > limits.max_buffer_size as usize
                    || limits.max_vertex_attributes < 10
                    || limits.max_vertex_buffer_array_stride < 128)
            {
                return Err(CombinedSceneError::new(
                    "Marker primitives exceed GPU device limits",
                ));
            }
        }
        let maps = maps
            .iter()
            .copied()
            .map(|layer| {
                layer.validate_scene(frame)?;
                if layer
                    .map
                    .dimensions()
                    .into_iter()
                    .any(|n| n > self.state.device.limits().max_texture_dimension_2d)
                {
                    return Err(CombinedSceneError::new(
                        "Map exceeds GPU texture dimensions",
                    ));
                }
                let size = bounded_dimensions(size);
                let aspect = size[0] as f32 / size[1] as f32;
                let corners = layer.map.corners().map(|p| camera.clip(p, aspect));
                if !corners.iter().flatten().all(|v| v.is_finite()) {
                    return Err(CombinedSceneError::new("Map projection is nonfinite"));
                }
                let uv = [[0., 0.], [1., 0.], [1., 1.], [0., 1.]];
                let vertices = [0, 1, 2, 0, 2, 3].map(|i| RenderVertex {
                    clip: corners[i],
                    color: [uv[i][0], uv[i][1], 0., 0.],
                });
                Ok((layer, vertices))
            })
            .collect::<Result<Vec<_>, CombinedSceneError>>()?;
        if !self.history_enabled
            && frame
                .layers
                .iter()
                .any(|l| l.samples.is_some() || l.appearance.is_some())
        {
            return Err(CombinedSceneError::new(
                "Sensor history requires explicit renderer opt-in",
            ));
        }
        let size = bounded_dimensions(size);
        let batches = if let Some(robot) = frame.robot {
            super::robot_geometry::texture_batches(robot.prepared, robot.poses, camera, size)?
        } else {
            Vec::new()
        };
        let mut geometry = content_vertices(frame, camera, size)?;
        if uncertainty.len() > studio_protocol::pose_snapshot::MAX_OWNERS
            || (!self.history_enabled && !uncertainty.is_empty())
        {
            return Err(CombinedSceneError::new(
                "Uncertainty layers require the enhanced renderer and at most two owners",
            ));
        }
        for layer in uncertainty {
            if !layer.is_current() {
                continue;
            }
            if layer.prepared.snapshot.owner.fixed_frame != frame.fixed_frame {
                return Err(CombinedSceneError::new(
                    "Uncertainty layer uses another scene frame",
                ));
            }
            let projected = layer.geometry.vertices(camera, size, layer.width)?;
            if layer.is_current() {
                geometry.extend(projected);
            }
        }
        let mut vertices = scene_vertices(&Scene::default(), camera, size, visuals);
        if let Some(layer) = frame.map {
            if let Some(map) = layer.map {
                let key = MapCacheKey {
                    projection: camera.orthographic_span.map(f32::to_bits),
                    calibrated: camera.calibrated,
                    owner: layer.id,
                    generation: layer.local_generation,
                    sequence: layer.sequence,
                    camera: std::array::from_fn(|i| {
                        [camera.eye, camera.right, camera.up, camera.forward][i / 3][i % 3]
                            .to_bits()
                    }),
                    size,
                    corners: map.corners.as_ptr() as usize,
                    cells: map.cells.as_ptr() as usize,
                };
                if self.map_cache.as_ref().is_none_or(|(old, _)| *old != key) {
                    let projected = super::map_geometry::map_vertices(map, camera, size)?;
                    self.map_cache = Some((key, projected));
                }
                vertices.extend_from_slice(&self.map_cache.as_ref().unwrap().1);
            } else {
                self.map_cache = None;
            }
        } else {
            self.map_cache = None;
        }
        let total = vertices
            .len()
            .checked_add(geometry.len())
            .filter(|n| *n <= self.max_vertices)
            .ok_or_else(|| CombinedSceneError::new("Combined scene vertex budget exceeded"))?;
        if !vertices
            .iter()
            .all(|v| v.clip.iter().all(|n| n.is_finite()))
        {
            return Err(CombinedSceneError::new(
                "Combined scene grid projection is nonfinite",
            ));
        }
        vertices.reserve_exact(total - vertices.len());
        vertices.append(&mut geometry);
        // Resize retires the attachments via release_frame; preserve this newly
        // validated cache across that internal resize only. Explicit release
        // still drops the cache and all live geometry.
        let cache = self.map_cache.take();
        let texture = self.render_vertices_textured_with_primitives(
            size,
            &vertices,
            if camera.calibrated.is_some() {
                [0.; 4]
            } else {
                egui::Rgba::from(visuals.extreme_bg_color).to_array()
            },
            &batches,
            &maps,
            layer,
        );
        self.map_cache = cache;
        Ok(texture)
    }
    /// Render one externally posed robot plus world grid/axes. All model/pose
    /// validation completes before touching the previous framebuffer.
    pub fn render_robot(
        &mut self,
        size: [u32; 2],
        prepared: &super::robot_geometry::PreparedRobot,
        poses: &[Option<super::RigidTransform>],
        camera: CameraPose,
        visuals: &egui::Visuals,
    ) -> Result<egui::TextureId, crate::robot::RobotError> {
        self.map_cache = None;
        let size = bounded_dimensions(size);
        let batches = super::robot_geometry::texture_batches(prepared, poses, camera, size)?;
        let mut robot = super::robot_geometry::robot_vertices(prepared, poses, camera, size)?;
        let mut vertices = scene_vertices(&Scene::default(), camera, size, visuals);
        let count = vertices
            .len()
            .checked_add(robot.len())
            .filter(|n| *n <= self.max_vertices)
            .ok_or_else(|| {
                crate::robot::RobotError::new(
                    crate::robot::RobotErrorKind::Limit,
                    "Robot renderer vertex budget exceeded",
                )
            })?;
        if !vertices
            .iter()
            .all(|v| v.clip.iter().all(|n| n.is_finite()))
        {
            return Err(crate::robot::RobotError::new(
                crate::robot::RobotErrorKind::Pose,
                "Robot camera/grid projection is nonfinite",
            ));
        }
        // Reserve once rather than growing geometrically beyond the bound.
        vertices.reserve_exact(count - vertices.len());
        vertices.append(&mut robot);
        Ok(self.render_vertices_textured(
            size,
            &vertices,
            egui::Rgba::from(visuals.extreme_bg_color).to_array(),
            &batches,
            None,
        ))
    }
    pub(super) fn render_scene(
        &mut self,
        size: [u32; 2],
        scene: &Scene,
        camera: CameraPose,
        visuals: &egui::Visuals,
    ) -> egui::TextureId {
        let size = bounded_dimensions(size);
        let vertices = scene_vertices(scene, camera, size, visuals);
        self.render_vertices_clear(
            size,
            &vertices,
            egui::Rgba::from(visuals.extreme_bg_color).to_array(),
        )
    }
}
impl Drop for Renderer3d {
    fn drop(&mut self) {
        self.release_frame();
    }
}
fn quad(out: &mut Vec<RenderVertex>, corners: [[f32; 4]; 4], color: [f32; 4]) {
    for index in [0, 1, 2, 0, 2, 3] {
        out.push(RenderVertex {
            clip: corners[index],
            color,
        });
    }
}
fn line(
    out: &mut Vec<RenderVertex>,
    camera: CameraPose,
    size: [u32; 2],
    a: Point3,
    b: Point3,
    color: [f32; 4],
) {
    let aspect = size[0] as f32 / size[1] as f32;
    let mut a = camera.clip(a, aspect);
    let mut b = camera.clip(b, aspect);
    // Clip the segment against the near plane before forming its screen-space ribbon.
    if a[2] < 0.0 && b[2] < 0.0 {
        return;
    }
    if a[2] < 0.0 || b[2] < 0.0 {
        let t = a[2] / (a[2] - b[2]);
        let c = std::array::from_fn(|i| a[i] + t * (b[i] - a[i]));
        if a[2] < 0.0 {
            a = c;
        } else {
            b = c;
        }
    }
    if a[3] <= 0.0 || b[3] <= 0.0 {
        return;
    }
    let dx = (b[0] / b[3] - a[0] / a[3]) * size[0] as f32;
    let dy = (b[1] / b[3] - a[1] / a[3]) * size[1] as f32;
    let len = dx.hypot(dy).max(0.001);
    let offset = [-dy / len / size[0] as f32, dx / len / size[1] as f32];
    let shift = |mut p: [f32; 4], sign: f32| {
        p[0] += offset[0] * p[3] * sign;
        p[1] += offset[1] * p[3] * sign;
        p
    };
    quad(
        out,
        [shift(a, -1.0), shift(b, -1.0), shift(b, 1.0), shift(a, 1.0)],
        color,
    );
}
fn scene_vertices(
    scene: &Scene,
    camera: CameraPose,
    size: [u32; 2],
    visuals: &egui::Visuals,
) -> Vec<RenderVertex> {
    let mut out = Vec::with_capacity(MAX_LEGACY_SCENE_VERTICES);
    for n in -10..=10 {
        if n == 0 {
            continue;
        }
        let n = f64::from(n);
        for (a, b) in [
            (Point3([n, -10.0, 0.0]), Point3([n, 10.0, 0.0])),
            (Point3([-10.0, n, 0.0]), Point3([10.0, n, 0.0])),
        ] {
            line(
                &mut out,
                camera,
                size,
                a,
                b,
                egui::Rgba::from(visuals.widgets.noninteractive.bg_stroke.color).to_array(),
            );
        }
    }
    for (p, c) in [
        (Point3([1.0, 0.0, 0.0]), [0.95, 0.15, 0.12, 1.0]),
        (Point3([0.0, 1.0, 0.0]), [0.18, 0.9, 0.25, 1.0]),
        (Point3([0.0, 0.0, 1.0]), [0.25, 0.5, 1.0, 1.0]),
    ] {
        line(&mut out, camera, size, Point3([0.0; 3]), p, c);
    }
    let aspect = size[0] as f32 / size[1] as f32;
    for layer in scene.layers().iter().filter(|l| l.visible) {
        for &point in layer.points() {
            let p = camera.clip(point, aspect);
            if !p.iter().all(|v| v.is_finite()) || p[3] <= 0.0 || p[2] < 0.0 || p[2] > p[3] {
                continue;
            }
            let corner = |x: f32, y: f32| {
                [
                    p[0] + x * 3.0 / size[0] as f32 * p[3],
                    p[1] + y * 3.0 / size[1] as f32 * p[3],
                    p[2],
                    p[3],
                ]
            };
            quad(
                &mut out,
                [
                    corner(-1.0, -1.0),
                    corner(1.0, -1.0),
                    corner(1.0, 1.0),
                    corner(-1.0, 1.0),
                ],
                egui::Rgba::from(visuals.hyperlink_color).to_array(),
            );
        }
    }
    debug_assert!(out.len() <= MAX_LEGACY_SCENE_VERTICES);
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn packed_vertex_bytes_preserve_exact_little_endian_layout() {
        let vertices = [
            RenderVertex {
                clip: [-0.0, 1.5, -2.25, f32::MIN_POSITIVE],
                color: [0.1, 0.2, 0.3, 1.],
            },
            RenderVertex {
                clip: [10., -20., 30., 40.],
                color: [1., 0., 0.5, -0.0],
            },
        ];
        let reference: Vec<u8> = vertices
            .iter()
            .flat_map(|v| v.clip.into_iter().chain(v.color))
            .flat_map(f32::to_le_bytes)
            .collect();
        assert_eq!(vertex_bytes(&vertices), reference);
        assert_eq!(std::mem::size_of::<RenderVertex>(), 32);
        assert_eq!(
            std::mem::align_of::<RenderVertex>(),
            std::mem::align_of::<f32>()
        );
        #[cfg(target_endian = "little")]
        {
            let borrowed: &[u8] = bytemuck::cast_slice(&vertices);
            assert_eq!(borrowed, reference);
            assert_eq!(
                borrowed.as_ptr(),
                vertices.as_ptr().cast::<u8>(),
                "GPU bytes must borrow existing vertices without allocation"
            );
        }
        assert_eq!(vertex_bytes(&vertices).len(), vertices.len() * 32);
        assert!(vertex_bytes(&[]).is_empty());
    }
    #[test]
    fn near_plane_crossing_line_is_clipped_in_both_endpoint_orders() {
        let camera = CameraPose::orbit(0.0, 0.0, 10.0, [0.0; 3]);
        // Offset the eye-plane endpoint from the eye: a segment starting at
        // the eye projects to one point, whose ribbon normal is undefined.
        let eye_plane = Point3([10.0, 0.05, -0.02]);
        let visible = Point3([9.0, 0.3, 0.2]);
        let mut ordered_bounds = Vec::new();
        for (a, b) in [(eye_plane, visible), (visible, eye_plane)] {
            let mut vertices = Vec::new();
            line(&mut vertices, camera, [640, 480], a, b, [1.0; 4]);
            assert_eq!(
                vertices.len(),
                6,
                "crossing segment must retain its visible ribbon"
            );
            assert!(vertices
                .iter()
                .all(|v| v.clip.iter().all(|n| n.is_finite())));
            assert!(
                vertices.iter().all(|v| v.clip[2] >= 0.0 && v.clip[3] > 0.0),
                "clip before division: no behind-near or nonpositive-w vertices"
            );
            let min_w = vertices
                .iter()
                .map(|v| v.clip[3])
                .fold(f32::INFINITY, f32::min);
            let max_w = vertices
                .iter()
                .map(|v| v.clip[3])
                .fold(f32::NEG_INFINITY, f32::max);
            assert!((min_w - super::super::camera3d::NEAR).abs() < 1e-5);
            assert!((max_w - 1.0).abs() < 1e-5);
            assert!(
                max_w > min_w * 10.0,
                "retain perspective w, not a flat screen line"
            );
            let bounds: [[f32; 2]; 4] = std::array::from_fn(|axis| {
                [
                    vertices
                        .iter()
                        .map(|v| v.clip[axis])
                        .fold(f32::INFINITY, f32::min),
                    vertices
                        .iter()
                        .map(|v| v.clip[axis])
                        .fold(f32::NEG_INFINITY, f32::max),
                ]
            });
            ordered_bounds.push(bounds);
        }
        for (forward, reverse) in ordered_bounds[0]
            .iter()
            .flatten()
            .zip(ordered_bounds[1].iter().flatten())
        {
            assert!(
                (forward - reverse).abs() < 1e-5,
                "endpoint order must preserve clipped ribbon bounds: forward={forward}, reverse={reverse}, diff={}, bounds={ordered_bounds:?}", (forward-reverse).abs()
            );
        }
        // Preserve coverage for the original degenerate viewing-ray segment.
        // Its screen-space direction is undefined; both endpoint orders must
        // still remain finite, clipped and bounded rather than forming NaNs.
        for (a, b) in [
            (Point3([10.0, 0.0, 0.0]), visible),
            (visible, Point3([10.0, 0.0, 0.0])),
        ] {
            let mut vertices = Vec::new();
            line(&mut vertices, camera, [640, 480], a, b, [1.0; 4]);
            assert_eq!(vertices.len(), 6);
            assert!(vertices.iter().all(|v| v.clip.iter().all(|n| n.is_finite())
                && v.clip[2] >= 0.0
                && v.clip[3] > 0.0));
            for vertex in vertices {
                let p = vertex.clip;
                assert!(
                    (p[0] / p[3]).abs() < 1.0 && (p[1] / p[3]).abs() < 1.0,
                    "viewing-ray ribbon stays within viewport"
                );
            }
        }
        for (a, b) in [
            (Point3([9.99, 0.0, 0.0]), Point3([9.98, 0.1, 0.1])),
            (Point3([10.1, 0.0, 0.0]), Point3([10.2, 0.1, 0.1])),
        ] {
            let mut vertices = Vec::new();
            line(&mut vertices, camera, [640, 480], a, b, [1.0; 4]);
            assert!(
                vertices.is_empty(),
                "segment fully behind near plane must disappear"
            );
        }
    }
    #[test]
    fn empty_scene_has_finite_bounded_grid_axes() {
        let v = scene_vertices(
            &Scene::default(),
            CameraPose::orbit(0.5, 0.8, 12.0, [0.0; 3]),
            [2048, 2048],
            &egui::Visuals::dark(),
        );
        assert!(!v.is_empty() && v.len() <= 45 * 6);
        assert!(v.iter().all(|v| v.clip.iter().all(|n| n.is_finite())));
        assert_eq!(MAX_VERTICES * 32, 23_880_000);
        assert_eq!(MAX_HISTORY_VERTICES * 32, 49_203_648);
        assert_eq!(bounded_dimensions([4096, 1024]), [2048, 512]);
        assert_eq!(bounded_dimensions([1024, 4096]), [512, 2048]);
        assert_eq!(bounded_dimensions([0, 0]), [1, 1]);
    }
}

#[cfg(test)]
mod gpu_readback {
    use super::*;
    fn block_on<F: std::future::Future>(future: F) -> F::Output {
        struct Wake(std::thread::Thread);
        impl std::task::Wake for Wake {
            fn wake(self: std::sync::Arc<Self>) {
                self.0.unpark();
            }
        }
        let waker = std::task::Waker::from(std::sync::Arc::new(Wake(std::thread::current())));
        let mut context = std::task::Context::from_waker(&waker);
        let mut future = std::pin::pin!(future);
        loop {
            match future.as_mut().poll(&mut context) {
                std::task::Poll::Ready(v) => return v,
                std::task::Poll::Pending => std::thread::park(),
            }
        }
    }
    #[test]
    #[ignore = "requires native GPU; serialize texture orientation/depth/resource readback"]
    fn opaque_texture_orientation_tint_depth_and_release() {
        use crate::{
            robot::{RobotTexture, Triangle},
            spatial::{
                robot_geometry::{PreparedRobot, PreparedVisual},
                RigidTransform,
            },
        };
        use std::sync::Arc;
        let image = Arc::new(RobotTexture {
            width: 2,
            height: 2,
            rgba: vec![
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 255, 255,
            ],
            has_transparency: false,
        });
        let triangles: Arc<[Triangle]> =
            vec![Triangle([[0., -2., -2.], [0., 2., -2.], [0., 0., 2.]])].into();
        let make = |link_index, uv, rgba| PreparedVisual {
            role: crate::robot::GeometryRole::Visual,
            visible: true,
            texture: Some(Arc::clone(&image)),
            uvs: Some(vec![[uv; 3]].into()),
            link_index,
            triangles: Arc::clone(&triangles),
            rgba,
        };
        let mut model = PreparedRobot {
            link_names: vec!["near".into(), "far".into()],
            visuals: vec![
                make(0, [0.25, 0.25], [1., 1., 0.5, 1.]),
                make(1, [0.75, 0.75], [1.; 4]),
            ],
            triangle_count: 2,
        };
        let near = Some(RigidTransform::new([2., 0., 3.], [0., 0., 0., 1.]).unwrap());
        let far = Some(RigidTransform::new([0., 0., 3.], [0., 0., 0., 1.]).unwrap());
        let mut renderer = native_renderer();
        let camera = CameraPose::orbit(0., 0., 10., [0., 0., 3.]);
        let visuals = egui::Visuals::dark();
        renderer
            .render_robot([128, 128], &model, &[near, far], camera, &visuals)
            .unwrap();
        let pixel = center_pixel(&renderer);
        eprintln!("Textured near linear-blue tint readback {pixel:?}");
        assert!(pixel[0] < 5 && pixel[1] < 5 && (185..=191).contains(&pixel[2]));
        assert_eq!(renderer.textures.counts(), (1, 2));
        let poses = [near, far];
        let frame = super::super::combined::CombinedSceneFrame {
            fixed_frame: "map",
            robot: Some(super::super::combined::RobotGeometryFrame {
                prepared: &model,
                poses: &poses,
            }),
            layers: &[],
            map: None,
            pose: None,
            path: None,
            tf: None,
        };
        renderer
            .render_combined([128, 128], &frame, camera, &visuals)
            .unwrap();
        assert_eq!(center_pixel(&renderer), pixel);
        model.visuals.reverse();
        renderer
            .render_robot([128, 128], &model, &[near, far], camera, &visuals)
            .unwrap();
        assert_eq!(center_pixel(&renderer), pixel);
        renderer
            .render_robot([128, 128], &model, &[None, far], camera, &visuals)
            .unwrap();
        let p = center_pixel(&renderer);
        assert!(p[1] > 245 && p[0] < 5 && p[2] < 5);
        renderer
            .render_robot([128, 128], &model, &[None, None], camera, &visuals)
            .unwrap();
        assert_eq!(renderer.textures.counts(), (0, 0));
        renderer.release_frame();
        assert_eq!(renderer.textures.counts(), (0, 0));
    }
    #[test]
    #[ignore = "requires native GPU; serialize transparent depth and blend readback"]
    fn transparent_color_texture_sorting_and_opaque_depth() {
        use crate::{
            robot::{RobotTexture, Triangle},
            spatial::{
                robot_geometry::{PreparedRobot, PreparedVisual},
                RigidTransform,
            },
        };
        use std::sync::Arc;
        let triangle: Arc<[Triangle]> =
            vec![Triangle([[0., -2., -2.], [0., 2., -2.], [0., 0., 2.]])].into();
        let make = |i, rgba| PreparedVisual {
            role: crate::robot::GeometryRole::Visual,
            visible: true,
            link_index: i,
            triangles: Arc::clone(&triangle),
            rgba,
            uvs: None,
            texture: None,
        };
        let mut robot = PreparedRobot {
            link_names: vec!["near".into(), "far".into(), "opaque".into()],
            visuals: vec![
                make(0, [1., 0., 0., 0.5]),
                make(1, [0., 1., 0., 0.5]),
                make(2, [0., 0., 1., 1.]),
            ],
            triangle_count: 3,
        };
        let pose = |x| Some(RigidTransform::new([x, 0., 3.], [0., 0., 0., 1.]).unwrap());
        let poses = [pose(2.), pose(1.), pose(0.)];
        let camera = CameraPose::orbit(0., 0., 10., [0., 0., 3.]);
        let visuals = egui::Visuals::dark();
        let mut renderer = native_renderer();
        renderer
            .render_robot([128, 128], &robot, &poses, camera, &visuals)
            .unwrap();
        let pixel = center_pixel(&renderer);
        eprintln!("Straight-alpha ordered RGB readback {pixel:?}");
        assert!(
            (185..=191).contains(&pixel[0])
                && (134..=140).contains(&pixel[1])
                && (134..=140).contains(&pixel[2])
        );
        robot.visuals.reverse();
        renderer
            .render_robot([128, 128], &robot, &poses, camera, &visuals)
            .unwrap();
        assert_eq!(center_pixel(&renderer), pixel);
        renderer
            .render_robot(
                [128, 128],
                &robot,
                &[pose(2.), pose(1.), pose(4.)],
                camera,
                &visuals,
            )
            .unwrap();
        assert_eq!(center_pixel(&renderer), [0, 0, 255, 255]);
        let near = robot
            .visuals
            .iter_mut()
            .find(|v| v.link_index == 0)
            .unwrap();
        near.rgba = [1.; 4];
        near.texture = Some(Arc::new(RobotTexture {
            width: 1,
            height: 1,
            rgba: vec![255, 0, 0, 128],
            has_transparency: true,
        }));
        near.uvs = Some(vec![[[0.5, 0.5]; 3]].into());
        renderer
            .render_robot([128, 128], &robot, &poses, camera, &visuals)
            .unwrap();
        let textured = center_pixel(&renderer);
        assert!(pixel.iter().zip(textured).all(|(a, b)| a.abs_diff(b) <= 1));
        // Orthographic depth has constant clip-w, so sorting must use depth, not w.
        let top = CameraPose::top_down(8., [0., 0., 0.]);
        for v in &mut robot.visuals {
            v.triangles = vec![Triangle([[-2., -2., 0.], [2., -2., 0.], [0., 2., 0.]])].into();
        }
        let zpose = |z| Some(RigidTransform::new([0., 0., z], [0., 0., 0., 1.]).unwrap());
        renderer
            .render_robot(
                [128, 128],
                &robot,
                &[zpose(2.), zpose(1.), zpose(0.)],
                top,
                &visuals,
            )
            .unwrap();
        let ortho = center_pixel(&renderer);
        assert!(textured.iter().zip(ortho).all(|(a, b)| a.abs_diff(b) <= 1));
    }
    #[test]
    #[ignore = "requires native GPU; serialized maximum transparent material-switch measurement"]
    fn maximum_transparent_material_switch_measurement() {
        use crate::{
            robot::Triangle,
            spatial::{
                robot_geometry::{PreparedRobot, PreparedVisual},
                RigidTransform,
            },
        };
        let began = std::time::Instant::now();
        let mut parts = Vec::new();
        for part in 0..256 {
            let mut triangles = Vec::new();
            for index in (part..crate::robot::MAX_ROBOT_TRIANGLES).step_by(256) {
                let x = index as f64 / crate::robot::MAX_ROBOT_TRIANGLES as f64 * 2. - 1.;
                triangles.push(if index == 0 {
                    Triangle([[x, -2., -2.], [x, 2., -2.], [x, 0., 2.]])
                } else {
                    Triangle([[x, 100., 100.], [x, 101., 100.], [x, 100., 101.]])
                });
            }
            parts.push(PreparedVisual {
                role: crate::robot::GeometryRole::Visual,
                visible: true,
                texture: None,
                uvs: None,
                link_index: 0,
                triangles: triangles.into(),
                rgba: [(part % 128) as f32 / 127., 0.2, 0.7, 0.5],
            });
        }
        let model = PreparedRobot {
            link_names: vec!["base".into()],
            visuals: parts,
            triangle_count: crate::robot::MAX_ROBOT_TRIANGLES,
        };
        eprintln!(
            "MEASURE maximum_model_construction_seconds={}",
            began.elapsed().as_secs_f64()
        );
        let mut renderer = native_renderer();
        let camera = CameraPose::orbit(0., 0., 10., [0., 0., 0.]);
        for frame in 0..6 {
            let start = std::time::Instant::now();
            renderer
                .render_robot(
                    [1024, 768],
                    &model,
                    &[Some(RigidTransform::identity())],
                    camera,
                    &egui::Visuals::dark(),
                )
                .unwrap();
            let encoded = start.elapsed().as_secs_f64();
            let _ = center_pixel(&renderer);
            eprintln!("MEASURE maximum_transparent_frame={frame} encode_seconds={encoded} completed_seconds={}",start.elapsed().as_secs_f64());
        }
        assert_eq!(renderer.textures.counts(), (1, 128));
        renderer.release_frame();
        assert_eq!(renderer.textures.counts(), (0, 0));
        eprintln!("SCOPE 50000triangles/256parts/128materials; one onscreen triangle, others offscreen; maximum material-switch/geometry cost, not maximum fill or sustained native UI");
    }
    #[test]
    #[ignore = "requires native GPU; serialize maximum decoded texture lifecycle readback"]
    fn maximum_decoded_texture_lifecycle() {
        use crate::{
            robot::{RobotTexture, Triangle},
            spatial::{
                robot_geometry::{PreparedRobot, PreparedVisual},
                RigidTransform,
            },
        };
        use std::sync::Arc;
        let images: Vec<_> = [[255, 0, 0, 255], [0, 255, 0, 255]]
            .into_iter()
            .map(|pixel| {
                Arc::new(RobotTexture {
                    width: 2048,
                    height: 2048,
                    rgba: pixel.repeat(2048 * 2048),
                    has_transparency: false,
                })
            })
            .collect();
        assert_eq!(
            images.iter().map(|image| image.rgba.len()).sum::<usize>(),
            32 * 1024 * 1024
        );
        let triangles: Arc<[Triangle]> =
            vec![Triangle([[0., -2., -2.], [0., 2., -2.], [0., 0., 2.]])].into();
        let mut model = PreparedRobot {
            link_names: vec!["near".into(), "far".into()],
            visuals: images
                .iter()
                .enumerate()
                .map(|(link_index, image)| PreparedVisual {
                    role: crate::robot::GeometryRole::Visual,
                    visible: true,
                    texture: Some(Arc::clone(image)),
                    uvs: Some(vec![[[0.5, 0.5]; 3]].into()),
                    link_index,
                    triangles: Arc::clone(&triangles),
                    rgba: [1.; 4],
                })
                .collect(),
            triangle_count: 2,
        };
        let near = Some(RigidTransform::new([2., 0., 3.], [0., 0., 0., 1.]).unwrap());
        let far = Some(RigidTransform::new([0., 0., 3.], [0., 0., 0., 1.]).unwrap());
        let camera = CameraPose::orbit(0., 0., 10., [0., 0., 3.]);
        let mut renderer = native_renderer();
        for size in [[128, 128], [256, 192], [128, 128]] {
            renderer
                .render_robot(size, &model, &[near, far], camera, &egui::Visuals::dark())
                .unwrap();
            assert_eq!(renderer.textures.counts(), (2, 2));
            let pixel = center_pixel(&renderer);
            assert!(pixel[0] > 245 && pixel[1] < 5 && pixel[2] < 5, "{pixel:?}");
        }
        model.visuals.remove(0);
        model.triangle_count = 1;
        renderer
            .render_robot(
                [128, 128],
                &model,
                &[near, far],
                camera,
                &egui::Visuals::dark(),
            )
            .unwrap();
        assert_eq!(renderer.textures.counts(), (1, 1));
        let pixel = center_pixel(&renderer);
        assert!(pixel[1] > 245 && pixel[0] < 5 && pixel[2] < 5, "{pixel:?}");
        renderer
            .render_robot(
                [128, 128],
                &model,
                &[None, None],
                camera,
                &egui::Visuals::dark(),
            )
            .unwrap();
        assert_eq!(renderer.textures.counts(), (0, 0));
        renderer.release_frame();
        assert_eq!(renderer.textures.counts(), (0, 0));
        eprintln!("SCOPE exact32MiB decoded textures uploaded, depth readback, resize/replacement/missing-TF/release; not driver RSS or sustained UI timing");
    }
    #[test]
    #[ignore = "requires native GPU; serialize collision/link appearance and release readback"]
    fn collision_visibility_opacity_and_missing_tf_release() {
        use crate::robot::{
            load_model, present_robot, LinkAppearance, LoadOptions, RobotAppearance,
        };
        use crate::spatial::{robot_geometry::prepare_robot, RigidTransform};
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/robot_visualizer/collision.urdf");
        let cancel = std::sync::atomic::AtomicBool::new(false);
        let source = prepare_robot(
            &load_model(&path, &LoadOptions::default(), &cancel).unwrap(),
            &cancel,
        )
        .unwrap();
        let mut renderer = native_renderer();
        let camera = CameraPose::orbit(0., 0., 5., [0., 0., 0.6]);
        let poses = [Some(RigidTransform::identity()), None];
        let visuals = egui::Visuals::dark();
        renderer
            .render_robot([128, 128], &source, &poses, camera, &visuals)
            .unwrap();
        let visual = center_pixel(&renderer);
        assert!(visual[1] > 200 && visual[0] < 110, "{visual:?}");
        let appearance = RobotAppearance {
            visual: false,
            collision: true,
            ..Default::default()
        };
        let collision = present_robot(&source, &appearance).unwrap();
        renderer
            .render_robot([128, 128], &collision, &poses, camera, &visuals)
            .unwrap();
        let amber = center_pixel(&renderer);
        assert!(
            amber[0] > 235 && amber[1] > 185 && amber[2] < 120,
            "{amber:?}"
        );
        let mut alpha = appearance.clone();
        alpha.opacity_permille = 500;
        alpha.links = vec![LinkAppearance {
            name: "base".into(),
            visible: true,
            opacity_permille: 500,
        }];
        let faded = present_robot(&source, &alpha).unwrap();
        renderer
            .render_robot([128, 128], &faded, &poses, camera, &visuals)
            .unwrap();
        assert_ne!(center_pixel(&renderer), amber);
        assert_eq!(renderer.textures.counts(), (1, 1));
        alpha.links[0].visible = false;
        let hidden = present_robot(&source, &alpha).unwrap();
        renderer
            .render_robot([128, 128], &hidden, &poses, camera, &visuals)
            .unwrap();
        assert_eq!(renderer.textures.counts(), (0, 0));
        renderer
            .render_robot([128, 128], &faded, &[None, None], camera, &visuals)
            .unwrap();
        assert_eq!(renderer.textures.counts(), (0, 0));
        renderer.release_frame();
        assert_eq!(renderer.textures.counts(), (0, 0));
    }
    pub(super) fn native_renderer() -> Renderer3d {
        let instance = wgpu::Instance::new(&Default::default());
        let adapter = block_on(instance.request_adapter(&wgpu::RequestAdapterOptions::default()))
            .expect("native GPU adapter required");
        eprintln!("3D readback adapter: {:?}", adapter.get_info());
        let (device, queue) = block_on(adapter.request_device(
            &wgpu::DeviceDescriptor {
                label: Some("Depth acceptance device"),
                required_features: wgpu::Features::empty(),
                required_limits: wgpu::Limits::downlevel_defaults(),
                memory_hints: Default::default(),
            },
            None,
        ))
        .unwrap();
        let state = egui_wgpu::RenderState {
            adapter,
            available_adapters: Vec::new(),
            renderer: std::sync::Arc::new(egui::mutex::RwLock::new(egui_wgpu::Renderer::new(
                &device,
                wgpu::TextureFormat::Rgba8Unorm,
                None,
                1,
                false,
            ))),
            device,
            queue,
            target_format: wgpu::TextureFormat::Rgba8Unorm,
        };
        Renderer3d::new(state)
    }

    #[test]
    #[ignore = "requires native GPU; serialize attributed sensor/material blending and expanded resources"]
    fn attributed_sensor_material_transparency_and_explicit_history_resources() {
        use super::super::{
            combined::{CombinedSceneFrame, PointLayerFrame, PointStyle, RobotGeometryFrame},
            robot_geometry::{PreparedRobot, PreparedVisual},
            sensor_appearance::{ColorMode, SensorAppearance},
            sensor_history::{SensorPoint, MAX_HISTORY_POINTS},
            RigidTransform,
        };
        use crate::robot::{GeometryRole, Triangle};
        use std::sync::Arc;
        let mut legacy = native_renderer();
        let mut renderer = Renderer3d::with_sensor_history(legacy.state.clone());
        let settings = SensorAppearance {
            color: ColorMode::Rgb,
            pixels: 32,
            ..Default::default()
        };
        let points = [
            SensorPoint {
                position: Point3([4., 0., 3.]),
                rgba: Some([255, 0, 0, 128]),
                intensity: None,
            },
            SensorPoint {
                position: Point3([0., 0., 3.]),
                rgba: Some([0, 0, 255, 255]),
                intensity: None,
            },
        ];
        let make = |id, samples| PointLayerFrame {
            id,
            points: &[],
            samples: Some(samples),
            appearance: Some(&settings),
            style: PointStyle::for_id(id),
            status: "Receiving",
            error: None,
        };
        let layers = [make(1, points.as_slice())];
        let model = PreparedRobot {
            link_names: vec!["material".into()],
            visuals: vec![PreparedVisual {
                role: GeometryRole::Visual,
                visible: true,
                texture: None,
                uvs: None,
                link_index: 0,
                triangles: Arc::from([Triangle([[0., -2., -2.], [0., 2., -2.], [0., 0., 2.]])]),
                rgba: [0., 1., 0., 0.5],
            }],
            triangle_count: 1,
        };
        let poses = [Some(
            RigidTransform::new([2., 0., 3.], [0., 0., 0., 1.]).unwrap(),
        )];
        let scene = CombinedSceneFrame {
            fixed_frame: "map",
            robot: Some(RobotGeometryFrame {
                prepared: &model,
                poses: &poses,
            }),
            layers: &layers,
            map: None,
            pose: None,
            path: None,
            tf: None,
        };
        let camera = CameraPose::orbit(0., 0., 10., [0., 0., 3.]);
        assert!(legacy
            .render_combined([128, 128], &scene, camera, &egui::Visuals::dark())
            .is_err());
        assert!(legacy.vertices.is_none());
        renderer
            .render_combined([128, 128], &scene, camera, &egui::Visuals::dark())
            .unwrap();
        let pixel = center_pixel(&renderer);
        eprintln!("MEASURE attributed_sensor_material_pixel={pixel:?}");
        assert!(
            (186..=190).contains(&pixel[0])
                && (135..=139).contains(&pixel[1])
                && (135..=139).contains(&pixel[2])
        );
        let reversed = [points[1], points[0]];
        let other = [make(1, reversed.as_slice())];
        let alternate = CombinedSceneFrame {
            layers: &other,
            ..scene
        };
        renderer
            .render_combined([128, 128], &alternate, camera, &egui::Visuals::dark())
            .unwrap();
        assert_eq!(center_pixel(&renderer), pixel);
        renderer.release_frame();
        assert!(renderer.vertices.is_none() && renderer.frame.is_none());
        let settings = SensorAppearance {
            pixels: 1,
            ..Default::default()
        };
        let maximum: Vec<_> = (0..MAX_HISTORY_POINTS)
            .map(|i| SensorPoint {
                position: Point3([(i % 256) as f64 - 128., (i / 256) as f64 - 64., 1.]),
                ..Default::default()
            })
            .collect();
        let layers = std::array::from_fn::<_, 4, _>(|i| PointLayerFrame {
            id: i as u64 + 1,
            points: &[],
            samples: Some(&maximum),
            appearance: Some(&settings),
            style: PointStyle::for_id(i as u64 + 1),
            status: "Receiving",
            error: None,
        });
        let full = CombinedSceneFrame {
            layers: &layers,
            robot: None,
            ..scene
        };
        renderer
            .render_combined(
                [256, 256],
                &full,
                CameraPose::top_down(256., [0.; 3]),
                &egui::Visuals::dark(),
            )
            .unwrap();
        assert_eq!(renderer.vertices.as_ref().unwrap().size(), 42_926_400);
        eprintln!(
            "MEASURE attributed_history_points={} vertex_buffer_bytes={}",
            4 * MAX_HISTORY_POINTS,
            renderer.vertices.as_ref().unwrap().size()
        );
        renderer.release_frame();
        assert!(renderer.vertices.is_none() && renderer.frame.is_none());
    }

    pub(super) fn center_pixel(renderer: &Renderer3d) -> [u8; 4] {
        let texture = renderer.color_texture().unwrap();
        let size = texture.size();
        texture_pixel(&renderer.state, texture, [size.width / 2, size.height / 2])
    }
    pub(super) fn texture_pixel(
        state: &egui_wgpu::RenderState,
        texture: &wgpu::Texture,
        at: [u32; 2],
    ) -> [u8; 4] {
        let device = &state.device;
        let buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("Robot center readback"),
            size: 256,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            wgpu::TexelCopyTextureInfo {
                texture,
                mip_level: 0,
                origin: wgpu::Origin3d {
                    x: at[0],
                    y: at[1],
                    z: 0,
                },
                aspect: wgpu::TextureAspect::All,
            },
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: Some(1),
                },
            },
            wgpu::Extent3d {
                width: 1,
                height: 1,
                depth_or_array_layers: 1,
            },
        );
        state.queue.submit([encoder.finish()]);
        let (tx, rx) = std::sync::mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
        device.poll(wgpu::Maintain::Wait);
        rx.recv().unwrap().unwrap();
        let pixel = {
            let bytes = buffer.slice(..).get_mapped_range();
            <[u8; 4]>::try_from(&bytes[..4]).unwrap()
        };
        buffer.unmap();
        pixel
    }
    #[test]
    #[ignore = "requires a native wgpu adapter; run explicitly for orthographic map projection/cache acceptance"]
    fn top_down_zoom_invalidates_cached_map_projection_and_preserves_depth() {
        use super::super::{combined::*, map_geometry::prepare_map, RigidTransform};
        let mut renderer = native_renderer();
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
            tf: None,
            fixed_frame: "map",
            robot: None,
            layers: &[],
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
        renderer
            .render_combined(
                [128, 128],
                &frame,
                CameraPose::top_down(10., [0.; 3]),
                &egui::Visuals::dark(),
            )
            .unwrap();
        let before = renderer.map_cache.as_ref().unwrap().1[0].clip[0];
        renderer
            .render_combined(
                [128, 128],
                &frame,
                CameraPose::top_down(5., [0.; 3]),
                &egui::Visuals::dark(),
            )
            .unwrap();
        let after = renderer.map_cache.as_ref().unwrap().1[0].clip[0];
        assert_eq!(after,before*2.,"A span change must regenerate map clip coordinates even with identical camera basis/eye and map sequence");
        assert_eq!(
            renderer.map_cache.as_ref().unwrap().0.projection,
            Some(5f32.to_bits())
        );
        assert!(renderer
            .map_cache
            .as_ref()
            .unwrap()
            .1
            .iter()
            .all(|v| v.clip[3] == 1. && (0.0..=1.0).contains(&v.clip[2])));
        let free = center_pixel(&renderer);
        assert!(free[0] > 230 && free[1] > 230);
        let behind = [Point3([-0.5, 0., -1.]), Point3([0.5, 0., -1.])];
        let front = [Point3([-0.5, 0., 1.]), Point3([0.5, 0., 1.])];
        let mut overlay = frame;
        for (points, visible) in [(&behind, true), (&front, false)] {
            overlay.path = Some(PathLayerFrame {
                id: 2,
                points,
                style: PointStyle {
                    rgba: [1., 0., 0., 1.],
                    size_px: 4.,
                },
                status: "Receiving",
                error: None,
            });
            renderer
                .render_combined(
                    [128, 128],
                    &overlay,
                    CameraPose::top_down(5., [0.; 3]),
                    &egui::Visuals::dark(),
                )
                .unwrap();
            if visible {
                assert_eq!(center_pixel(&renderer), free, "Map occludes the lower path");
            } else {
                let p = center_pixel(&renderer);
                assert!(
                    p[0] > 240 && p[1] < 20,
                    "Higher path occludes the map: {p:?}"
                );
            }
        }
        renderer.release_frame();
        assert!(renderer.map_cache.is_none());
    }
    #[test]
    #[ignore = "requires a native Metal/wgpu adapter; run explicitly and serialize"]
    fn tf_axes_are_depth_tested_with_map_and_stop_clears_geometry() {
        use super::super::{combined::*, map_geometry::prepare_map, RigidTransform};
        use studio_protocol::tf_graph::{EdgeKind, Frame, LatestPose, Parent, Sample};
        let mut renderer = native_renderer();
        let map = prepare_map(
            1,
            1,
            1.,
            RigidTransform::new([-0.5, -0.5, 0.], [0., 0., 0., 1.]).unwrap(),
            RigidTransform::identity(),
            &[0],
        )
        .unwrap();
        let mut view = crate::tf_graph::TfView::default();
        let camera = CameraPose::top_down(2., [0.1, 0.1, 0.]);
        for (z, occluded) in [(-1., true), (1., false)] {
            view.apply(Sample {
                contract_version: 1,
                fixed_frame: "map".into(),
                counters: Default::default(),
                frames: vec![
                    Frame {
                        name: "base".into(),
                        parent: Some(Parent {
                            frame: "map".into(),
                            kind: EdgeKind::Dynamic,
                            stamp_ns: Some(1),
                        }),
                        pose: LatestPose::Ready {
                            resolved_stamp_ns: 1,
                            components: [-0.075, 0.1, z, 0., 0., 0., 1.],
                        },
                    },
                    Frame {
                        name: "map".into(),
                        parent: None,
                        pose: LatestPose::Ready {
                            resolved_stamp_ns: 0,
                            components: [0., 0., 0., 0., 0., 0., 1.],
                        },
                    },
                ],
            })
            .unwrap();
            view.select("base");
            let frame = CombinedSceneFrame {
                tf: Some(TfLayerFrame { id: 2, view: &view }),
                fixed_frame: "map",
                robot: None,
                layers: &[],
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
            renderer
                .render_combined([128, 128], &frame, camera, &egui::Visuals::dark())
                .unwrap();
            let pixel = center_pixel(&renderer);
            if occluded {
                assert!(
                    pixel[0] > 230 && pixel[1] > 230,
                    "Map occludes lower TF axis: {pixel:?}"
                );
            } else {
                assert!(
                    pixel[0] > 230 && pixel[1] < 130 && pixel[2] < 120,
                    "Upper TF axis occludes map: {pixel:?}"
                );
            }
        }
        view.stop();
        let frame = CombinedSceneFrame {
            tf: Some(TfLayerFrame { id: 2, view: &view }),
            fixed_frame: "map",
            robot: None,
            layers: &[],
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
        renderer
            .render_combined([128, 128], &frame, camera, &egui::Visuals::dark())
            .unwrap();
        let pixel = center_pixel(&renderer);
        assert!(
            pixel[0] > 230 && pixel[1] > 230,
            "Stopped TF clears its old axis"
        );
        renderer.release_frame();
        assert!(renderer.frame.is_none());
    }
    #[test]
    #[ignore = "requires a native wgpu adapter; run explicitly for mapping depth/cache/resource acceptance"]
    fn mapping_palette_coplanar_order_depth_cache_restart_and_maximum_resources() {
        use super::super::{
            combined::*,
            map_geometry::prepare_map,
            robot_geometry::{PreparedRobot, PreparedVisual},
            RigidTransform,
        };
        use crate::robot::{Triangle, MAX_ROBOT_TRIANGLES};
        let mut renderer = native_renderer();
        let camera = CameraPose::orbit(0., 1.5, 12., [2., 2., 0.]);
        let origin = RigidTransform::new([1., 1., 0.], [0., 0., 0., 1.]).unwrap();
        let mut pixels = Vec::new();
        for value in [0, 100, -1] {
            let map = prepare_map(1, 1, 2., origin, RigidTransform::identity(), &[value]).unwrap();
            let frame = CombinedSceneFrame {
                tf: None,
                fixed_frame: "map",
                robot: None,
                layers: &[],
                map: Some(MapLayerFrame {
                    id: 5,
                    local_generation: 1,
                    sequence: (value as i16 + 2) as u64,
                    map: Some(&map),
                    status: "Receiving",
                    error: None,
                }),
                pose: None,
                path: None,
            };
            renderer
                .render_combined([128, 128], &frame, camera, &egui::Visuals::dark())
                .unwrap();
            pixels.push(center_pixel(&renderer));
            assert_eq!(renderer.map_cache.as_ref().unwrap().1.len(), 6);
        }
        assert!(
            pixels[0][0] > 230 && pixels[1][0] < 80,
            "free/occupied must cover coplanar grid: {pixels:?}"
        );
        assert!(
            pixels[2][2] > pixels[2][0] && pixels[2] != pixels[0] && pixels[2] != pixels[1],
            "unknown palette: {pixels:?}"
        );
        let map = prepare_map(1, 1, 2., origin, RigidTransform::identity(), &[0]).unwrap();
        // Exact coplanarity: Path is later under LessEqual and wins deliberately.
        let points = [Point3([1., 2., 0.]), Point3([3., 2., 0.])];
        let path = PathLayerFrame {
            id: 7,
            points: &points,
            style: PointStyle {
                rgba: [1., 0., 0., 1.],
                size_px: 4.,
            },
            status: "Receiving",
            error: None,
        };
        let map_layer = MapLayerFrame {
            id: 5,
            local_generation: 2,
            sequence: 1,
            map: Some(&map),
            status: "Receiving",
            error: None,
        };
        let mut frame = CombinedSceneFrame {
            tf: None,
            fixed_frame: "map",
            robot: None,
            layers: &[],
            map: Some(map_layer),
            pose: None,
            path: Some(path),
        };
        renderer
            .render_combined([128, 128], &frame, camera, &egui::Visuals::dark())
            .unwrap();
        let path_pixel = center_pixel(&renderer);
        assert!(
            path_pixel[0] > 240 && path_pixel[1] < 20,
            "coplanar Path should win: {path_pixel:?}"
        );
        // A later Path behind the map must still fail depth rather than paint over it.
        let behind = [Point3([1., 2., -1.]), Point3([3., 2., -1.])];
        frame.path = Some(PathLayerFrame {
            points: &behind,
            ..path
        });
        renderer
            .render_combined([128, 128], &frame, camera, &egui::Visuals::dark())
            .unwrap();
        assert_eq!(center_pixel(&renderer), pixels[0]);
        assert_eq!(renderer.map_cache.as_ref().unwrap().0.generation, 2);
        frame.map = Some(MapLayerFrame {
            local_generation: 3,
            ..map_layer
        });
        frame.path = None;
        renderer
            .render_combined([128, 128], &frame, camera, &egui::Visuals::dark())
            .unwrap();
        assert_eq!(
            renderer.map_cache.as_ref().unwrap().0.generation,
            3,
            "restart must rebuild projected cache"
        );
        frame.map = Some(MapLayerFrame {
            map: None,
            error: Some("Missing acquisition TF"),
            ..map_layer
        });
        renderer
            .render_combined([128, 128], &frame, camera, &egui::Visuals::dark())
            .unwrap();
        assert!(renderer.map_cache.is_none());
        // Maximum upload, bounded repeated offscreen geometry limits fill work.
        let map = prepare_map(
            256,
            256,
            0.001,
            RigidTransform::new([100., 100., 0.], [0., 0., 0., 1.]).unwrap(),
            RigidTransform::identity(),
            &vec![-1; 65536],
        )
        .unwrap();
        let robot = PreparedRobot {
            link_names: vec!["base".into()],
            visuals: vec![PreparedVisual {
                role: crate::robot::GeometryRole::Visual,
                visible: true,
                texture: None,
                uvs: None,
                link_index: 0,
                triangles: vec![
                    Triangle([[0., 100., 0.], [0.01, 100., 0.], [0., 100.01, 0.]]);
                    MAX_ROBOT_TRIANGLES
                ]
                .into(),
                rgba: [1., 0., 0., 1.],
            }],
            triangle_count: MAX_ROBOT_TRIANGLES,
        };
        let robot_poses = [Some(RigidTransform::identity())];
        let sensor_points = vec![Point3([0., 100., 0.]); 8192];
        let layers = std::array::from_fn::<_, 4, _>(|i| PointLayerFrame {
            samples: None,
            appearance: None,
            id: i as u64 + 1,
            points: &sensor_points,
            style: PointStyle::for_id(i as u64 + 1),
            status: "Receiving",
            error: None,
        });
        let path_points = vec![Point3([0., 100., 0.]); 1024];
        let frame = CombinedSceneFrame {
            tf: None,
            fixed_frame: "map",
            robot: Some(RobotGeometryFrame {
                prepared: &robot,
                poses: &robot_poses,
            }),
            layers: &layers,
            map: Some(MapLayerFrame {
                id: 5,
                local_generation: 4,
                sequence: 1,
                map: Some(&map),
                status: "Receiving",
                error: None,
            }),
            pose: Some(PoseLayerFrame {
                id: 6,
                pose: Some(RigidTransform::new([0., 100., 0.], [0., 0., 0., 1.]).unwrap()),
                status: "Receiving",
                error: None,
            }),
            path: Some(PathLayerFrame {
                id: 7,
                points: &path_points,
                style: PointStyle::for_id(7),
                status: "Receiving",
                error: None,
            }),
        };
        let geometry = combined_vertices(&frame, camera, [2048, 2048]).unwrap();
        assert_eq!(geometry.len(), MAX_VERTICES - 45 * 6);
        drop(geometry);
        let id = renderer
            .render_combined([2048, 2048], &frame, camera, &egui::Visuals::dark())
            .unwrap();
        assert_eq!(renderer.vertices.as_ref().unwrap().size(), 23_880_000);
        assert_eq!(
            renderer.map_cache.as_ref().unwrap().1.len() * 32,
            12_582_912
        );
        assert!(renderer.vertices.as_ref().unwrap().size() < 32 * 1024 * 1024);
        // The explicit history path must fit the same maximum Robot/Map/Path/
        // Pose alongside all four full histories, including translucent samples.
        let settings = super::super::sensor_appearance::SensorAppearance {
            opacity: 128,
            ..Default::default()
        };
        let history = vec![
            super::super::sensor_history::SensorPoint {
                position: Point3([0., 100., 0.]),
                rgba: Some([1, 2, 3, 4]),
                intensity: Some(42.)
            };
            super::super::sensor_history::MAX_HISTORY_POINTS
        ];
        let expanded_layers = std::array::from_fn::<_, 4, _>(|i| PointLayerFrame {
            id: i as u64 + 1,
            points: &[],
            samples: Some(&history),
            appearance: Some(&settings),
            style: PointStyle::for_id(i as u64 + 1),
            status: "Receiving",
            error: None,
        });
        let mut graph = crate::tf_graph::TfView::default();
        graph
            .apply(studio_protocol::tf_graph::Sample {
                contract_version: 1,
                fixed_frame: "map".into(),
                counters: Default::default(),
                frames: (0..studio_protocol::tf_graph::MAX_FRAMES)
                    .map(|i| studio_protocol::tf_graph::Frame {
                        name: format!("f{i:03}"),
                        parent: if (1..=studio_protocol::tf_graph::MAX_EDGES).contains(&i) {
                            Some(studio_protocol::tf_graph::Parent {
                                frame: "f000".into(),
                                kind: studio_protocol::tf_graph::EdgeKind::Static,
                                stamp_ns: Some(0),
                            })
                        } else {
                            None
                        },
                        pose: studio_protocol::tf_graph::LatestPose::Ready {
                            resolved_stamp_ns: 0,
                            components: [0., 100. + i as f64 * 0.01, 0., 0., 0., 0., 1.],
                        },
                    })
                    .collect(),
            })
            .unwrap();
        let graph_vertices = graph.vertices(camera, [2048, 2048]).unwrap().len();
        assert_eq!(
            graph_vertices,
            studio_protocol::tf_graph::MAX_FRAMES * super::super::overlay_geometry::POSE_VERTICES
                + studio_protocol::tf_graph::MAX_EDGES * 6
        );
        let expanded_frame = CombinedSceneFrame {
            layers: &expanded_layers,
            tf: Some(super::super::combined::TfLayerFrame {
                id: 8,
                view: &graph,
            }),
            ..frame
        };
        let expanded_geometry = combined_vertices(&expanded_frame, camera, [2048, 2048]).unwrap();
        assert_eq!(
            expanded_geometry.len(),
            MAX_HISTORY_VERTICES - 45 * 6 - super::super::uncertainty::MAX_VERTICES
        );
        drop(expanded_geometry);
        let mut expanded = Renderer3d::with_sensor_history(renderer.state.clone());
        let started = std::time::Instant::now();
        expanded
            .render_combined(
                [2048, 2048],
                &expanded_frame,
                camera,
                &egui::Visuals::dark(),
            )
            .unwrap();
        let _completed = center_pixel(&expanded);
        eprintln!("MEASURE expanded_history_robot_map_path_pose_seconds={} vertex_buffer_bytes={} projected_map_bytes={}",started.elapsed().as_secs_f64(),expanded.vertices.as_ref().unwrap().size(),expanded.map_cache.as_ref().unwrap().1.len()*32);
        assert_eq!(expanded.vertices.as_ref().unwrap().size(), 42_926_400);
        // The replacement path fits the same maximum peers with a 2048-square
        // raw texture and six separate vertices, without legacy cell geometry.
        let mut complete = super::super::map_texture::fixture(2048, 2048, vec![50; 2048 * 2048]);
        std::sync::Arc::get_mut(&mut complete)
            .unwrap()
            .header
            .owner
            .fixed_frame = "map".into();
        let texture_map =
            std::sync::Arc::new(super::super::map_texture::TextureMap::prepare(complete).unwrap());
        let texture_frame = CombinedSceneFrame {
            map: None,
            ..expanded_frame
        };
        let started = std::time::Instant::now();
        expanded
            .render_combined_with_map(
                [2048, 2048],
                &texture_frame,
                camera,
                &egui::Visuals::dark(),
                Some(TextureMapFrame {
                    id: 5,
                    map: &texture_map,
                    appearance: super::super::map_appearance::MapAppearance {
                        opacity: 0.5,
                        ..Default::default()
                    },
                }),
            )
            .unwrap();
        let _completed = center_pixel(&expanded);
        assert_eq!(expanded.vertices.as_ref().unwrap().size(), 42_926_400);
        assert_eq!(expanded.map_textures.first().unwrap().vertices.size(), 192);
        assert!(expanded.map_cache.is_none());
        eprintln!("MEASURE maximum_texture_map_history_robot_tf_path_pose_seconds={} raw_texture_bytes=4194304 map_vertices_bytes=192 vertex_buffer_bytes=42926400",started.elapsed().as_secs_f64());
        expanded.release_frame();
        assert!(expanded.map_textures.is_empty());
        assert!(
            expanded.vertices.is_none() && expanded.frame.is_none() && expanded.map_cache.is_none()
        );
        let empty = CombinedSceneFrame {
            tf: None,
            fixed_frame: "map",
            robot: None,
            layers: &[],
            map: None,
            pose: None,
            path: None,
        };
        renderer
            .render_combined([64, 64], &empty, camera, &egui::Visuals::dark())
            .unwrap();
        assert!(renderer.state.renderer.read().texture(&id).is_none());
        assert!(renderer.map_cache.is_none());
        renderer.release_frame();
        assert!(
            renderer.frame.is_none() && renderer.vertices.is_none() && renderer.map_cache.is_none()
        );
        renderer
            .render_combined([64, 64], &empty, camera, &egui::Visuals::dark())
            .unwrap();
        assert_ne!(
            center_pixel(&renderer),
            path_pixel,
            "release/empty cannot retain mapping pixels"
        );
    }
    #[test]
    #[ignore = "requires a native wgpu adapter; run explicitly for combined scene depth/resource acceptance"]
    fn combined_robot_sensor_depth_clearing_and_maximum_resources() {
        use super::super::{
            combined::{
                CombinedSceneFrame, PointLayerFrame, PointStyle, RobotGeometryFrame,
                MAX_LAYER_POINTS, MAX_SENSOR_LAYERS,
            },
            robot_geometry::{PreparedRobot, PreparedVisual},
            RigidTransform,
        };
        use crate::robot::{Triangle, MAX_ROBOT_TRIANGLES};
        use std::sync::Arc;
        let mut renderer = native_renderer();
        let camera = CameraPose::orbit(0.0, 0.0, 10.0, [0.0, 0.0, 3.0]);
        let triangle = Triangle([[0.0, -2.0, -2.0], [0.0, 2.0, -2.0], [0.0, 0.0, 2.0]]);
        let prepared = PreparedRobot {
            link_names: vec!["base".into()],
            visuals: vec![PreparedVisual {
                role: crate::robot::GeometryRole::Visual,
                visible: true,
                texture: None,
                uvs: None,
                link_index: 0,
                triangles: Arc::from([triangle]),
                rgba: [1.0, 0.0, 0.0, 1.0],
            }],
            triangle_count: 1,
        };
        let poses = [Some(
            RigidTransform::new([2.0, 0.0, 3.0], [0.0, 0.0, 0.0, 1.0]).unwrap(),
        )];
        let robot = Some(RobotGeometryFrame {
            prepared: &prepared,
            poses: &poses,
        });
        let near = [Point3([4.0, 0.0, 3.0])];
        let far = [Point3([0.0, 0.0, 3.0])];
        let mut layers = [
            PointLayerFrame {
                samples: None,
                appearance: None,
                id: 1,
                points: &near,
                style: PointStyle {
                    rgba: [0.0, 0.0, 1.0, 1.0],
                    size_px: 5.0,
                },
                status: "Receiving scan",
                error: None,
            },
            PointLayerFrame {
                samples: None,
                appearance: None,
                id: 2,
                points: &far,
                style: PointStyle {
                    rgba: [0.0, 1.0, 0.0, 1.0],
                    size_px: 5.0,
                },
                status: "Receiving cloud",
                error: None,
            },
        ];
        renderer
            .render_combined(
                [64, 64],
                &CombinedSceneFrame {
                    tf: None,
                    map: None,
                    pose: None,
                    path: None,
                    fixed_frame: "map",
                    robot,
                    layers: &layers,
                },
                camera,
                &egui::Visuals::dark(),
            )
            .unwrap();
        assert_eq!(
            center_pixel(&renderer),
            [0, 0, 255, 255],
            "near sensor wins across robot and later far sensor"
        );
        layers[0].points = &[];
        layers[0].error = Some("Missing acquisition transform");
        renderer
            .render_combined(
                [64, 64],
                &CombinedSceneFrame {
                    tf: None,
                    map: None,
                    pose: None,
                    path: None,
                    fixed_frame: "map",
                    robot,
                    layers: &layers,
                },
                camera,
                &egui::Visuals::dark(),
            )
            .unwrap();
        assert_eq!(
            center_pixel(&renderer),
            [255, 0, 0, 255],
            "clearing only near sensor reveals robot; far sensor is depth-occluded"
        );
        let no_poses = [None];
        renderer
            .render_combined(
                [64, 64],
                &CombinedSceneFrame {
                    tf: None,
                    map: None,
                    pose: None,
                    path: None,
                    fixed_frame: "map",
                    robot: Some(RobotGeometryFrame {
                        prepared: &prepared,
                        poses: &no_poses,
                    }),
                    layers: &layers,
                },
                camera,
                &egui::Visuals::dark(),
            )
            .unwrap();
        assert_eq!(
            center_pixel(&renderer),
            [0, 255, 0, 255],
            "missing robot link leaves far sensor active"
        );
        let mut triangles = vec![
            Triangle([
                [0.0, 100.0, 100.0],
                [0.0, 101.0, 100.0],
                [0.0, 100.0, 101.0]
            ]);
            MAX_ROBOT_TRIANGLES
        ];
        triangles[0] = triangle;
        let maximum = PreparedRobot {
            link_names: vec!["base".into()],
            visuals: vec![PreparedVisual {
                role: crate::robot::GeometryRole::Visual,
                visible: true,
                texture: None,
                uvs: None,
                link_index: 0,
                triangles: triangles.into(),
                rgba: [1.0, 0.0, 0.0, 1.0],
            }],
            triangle_count: MAX_ROBOT_TRIANGLES,
        };
        let points = vec![Point3([0.0, 100.0, 100.0]); MAX_LAYER_POINTS];
        let maximum_layers = std::array::from_fn::<_, MAX_SENSOR_LAYERS, _>(|i| PointLayerFrame {
            samples: None,
            appearance: None,
            id: i as u64 + 1,
            points: &points,
            style: PointStyle::for_id(i as u64 + 1),
            status: "Receiving",
            error: None,
        });
        let maximum_frame = CombinedSceneFrame {
            tf: None,
            map: None,
            pose: None,
            path: None,
            fixed_frame: "map",
            robot: Some(RobotGeometryFrame {
                prepared: &maximum,
                poses: &poses,
            }),
            layers: &maximum_layers,
        };
        let id = renderer
            .render_combined(
                [MAX_DIMENSION; 2],
                &maximum_frame,
                camera,
                &egui::Visuals::dark(),
            )
            .unwrap();
        assert_eq!(renderer.frame_size(), Some([MAX_DIMENSION; 2]));
        assert_eq!(renderer.vertices.as_ref().unwrap().size(), 23_880_000);
        assert!(renderer.vertices.as_ref().unwrap().size() < 32 * 1024 * 1024);
        assert_eq!(center_pixel(&renderer), [255, 0, 0, 255]);
        eprintln!("Combined maximum: 50000 robot triangles, 32768 sensor points, 23880000-byte vertices, 2048-square color/depth; offscreen repeated geometry bounds fill cost");
        renderer
            .render_combined(
                [64, 64],
                &CombinedSceneFrame {
                    tf: None,
                    map: None,
                    pose: None,
                    path: None,
                    fixed_frame: "map",
                    robot: None,
                    layers: &[],
                },
                camera,
                &egui::Visuals::dark(),
            )
            .unwrap();
        assert!(renderer.state.renderer.read().texture(&id).is_none());
        let empty = center_pixel(&renderer);
        assert_ne!(empty, [255, 0, 0, 255]);
        assert_ne!(empty, [0, 255, 0, 255]);
        assert_ne!(empty, [0, 0, 255, 255]);
        renderer.release_frame();
        assert!(renderer.frame.is_none() && renderer.vertices.is_none());
    }

    #[test]
    #[ignore = "requires a native wgpu adapter; run explicitly for robot depth/resource acceptance"]
    fn opaque_robot_depth_missing_links_and_resource_cleanup() {
        use super::super::{
            robot_geometry::{PreparedRobot, PreparedVisual},
            RigidTransform,
        };
        use crate::robot::{Triangle, MAX_ROBOT_TRIANGLES};
        use std::sync::Arc;
        let mut renderer = native_renderer();
        let triangle = Triangle([[0.0, -2.0, -2.0], [0.0, 2.0, -2.0], [0.0, 0.0, 2.0]]);
        let camera = CameraPose::orbit(0.0, 0.0, 10.0, [0.0, 0.0, 3.0]);
        let near = RigidTransform::new([2.0, 0.0, 3.0], [0.0, 0.0, 0.0, 1.0]).unwrap();
        let far = RigidTransform::new([0.0, 0.0, 3.0], [0.0, 0.0, 0.0, 1.0]).unwrap();
        // Maximum upload/resource case, not a worst-case fill-rate benchmark:
        // one center triangle and 49,999 offscreen triangles still all upload.
        let mut maximum_triangles = vec![
            Triangle([
                [0.0, 100.0, 100.0],
                [0.0, 101.0, 100.0],
                [0.0, 100.0, 101.0]
            ]);
            MAX_ROBOT_TRIANGLES
        ];
        maximum_triangles[0] = triangle;
        let maximum = PreparedRobot {
            link_names: vec!["maximum".into()],
            visuals: vec![PreparedVisual {
                role: crate::robot::GeometryRole::Visual,
                visible: true,
                texture: None,
                uvs: None,
                link_index: 0,
                triangles: maximum_triangles.into(),
                rgba: [1.0, 0.0, 0.0, 1.0],
            }],
            triangle_count: MAX_ROBOT_TRIANGLES,
        };
        let maximum_id = renderer
            .render_robot(
                [MAX_DIMENSION; 2],
                &maximum,
                &[Some(near)],
                camera,
                &egui::Visuals::dark(),
            )
            .unwrap();
        assert_eq!(renderer.frame_size(), Some([MAX_DIMENSION; 2]));
        assert_eq!(renderer.vertices.as_ref().unwrap().size(), 23_880_000);
        assert!(renderer.vertices.as_ref().unwrap().size() < 32 * 1024 * 1024);
        assert_eq!(center_pixel(&renderer), [255, 0, 0, 255]);
        eprintln!("Robot maximum: {MAX_ROBOT_TRIANGLES} triangles, 150000 robot vertices, 2048-square target, 23880000-byte total scene vertex capacity; offscreen triangles limit fill cost");
        let mut robot = PreparedRobot {
            link_names: vec!["near".into(), "far".into()],
            visuals: vec![
                PreparedVisual {
                    role: crate::robot::GeometryRole::Visual,
                    visible: true,
                    texture: None,
                    uvs: None,
                    link_index: 0,
                    triangles: Arc::from([triangle]),
                    rgba: [1.0, 0.0, 0.0, 1.0],
                },
                PreparedVisual {
                    role: crate::robot::GeometryRole::Visual,
                    visible: true,
                    texture: None,
                    uvs: None,
                    link_index: 1,
                    triangles: Arc::from([triangle]),
                    rgba: [0.0, 1.0, 0.0, 1.0],
                },
            ],
            triangle_count: 2,
        };
        renderer
            .render_robot(
                [64, 64],
                &robot,
                &[None, None],
                camera,
                &egui::Visuals::dark(),
            )
            .unwrap();
        assert!(renderer
            .state
            .renderer
            .read()
            .texture(&maximum_id)
            .is_none());
        let baseline = center_pixel(&renderer);
        assert_ne!(baseline, [255, 0, 0, 255]);
        assert_ne!(baseline, [0, 255, 0, 255]);
        for reverse in [false, true] {
            if reverse {
                robot.visuals.reverse();
            }
            renderer
                .render_robot(
                    [64, 64],
                    &robot,
                    &[Some(near), Some(far)],
                    camera,
                    &egui::Visuals::dark(),
                )
                .unwrap();
            assert_eq!(
                center_pixel(&renderer),
                [255, 0, 0, 255],
                "external near pose wins independent of visual submission order"
            );
        }
        renderer
            .render_robot(
                [64, 64],
                &robot,
                &[None, Some(far)],
                camera,
                &egui::Visuals::dark(),
            )
            .unwrap();
        assert_eq!(
            center_pixel(&renderer),
            [0, 255, 0, 255],
            "missing near link reveals receiving far link"
        );
        robot
            .visuals
            .iter_mut()
            .find(|v| v.link_index == 0)
            .unwrap()
            .rgba[3] = 0.0;
        renderer
            .render_robot(
                [64, 64],
                &robot,
                &[Some(near), Some(far)],
                camera,
                &egui::Visuals::dark(),
            )
            .unwrap();
        assert_eq!(
            center_pixel(&renderer),
            [0, 255, 0, 255],
            "alpha zero is hidden rather than opaque black"
        );
        let active_id = renderer.frame.as_ref().unwrap().id;
        robot
            .visuals
            .iter_mut()
            .find(|v| v.link_index == 0)
            .unwrap()
            .rgba[3] = 1.5;
        assert!(renderer
            .render_robot(
                [64, 64],
                &robot,
                &[Some(near), Some(far)],
                camera,
                &egui::Visuals::dark()
            )
            .is_err());
        assert_eq!(renderer.frame.as_ref().unwrap().id, active_id);
        assert_eq!(
            center_pixel(&renderer),
            [0, 255, 0, 255],
            "invalid replacement must not paint a partial frame"
        );
        robot
            .visuals
            .iter_mut()
            .find(|v| v.link_index == 0)
            .unwrap()
            .rgba[3] = 1.0;
        renderer
            .render_robot(
                [64, 64],
                &robot,
                &[None, None],
                camera,
                &egui::Visuals::dark(),
            )
            .unwrap();
        assert_eq!(
            center_pixel(&renderer),
            baseline,
            "missing all links clears old robot pixels"
        );
        renderer.release_frame();
        assert!(renderer.vertices.is_none() && renderer.frame.is_none());
        assert!(renderer.state.renderer.read().texture(&active_id).is_none());
        renderer
            .render_robot(
                [64, 64],
                &robot,
                &[None, None],
                camera,
                &egui::Visuals::dark(),
            )
            .unwrap();
        assert_eq!(
            center_pixel(&renderer),
            baseline,
            "release then missing poses cannot revive old geometry"
        );
        let final_id = renderer.frame.as_ref().unwrap().id;
        let egui_renderer = renderer.state.renderer.clone();
        drop(renderer);
        assert!(
            egui_renderer.read().texture(&final_id).is_none(),
            "renderer Drop unregisters its last frame"
        );
    }

    /// Deliberately opt-in: fails if no real adapter is available; never records
    /// lack of a GPU as a passing software/headless rendering result.
    #[test]
    #[ignore = "requires a native wgpu adapter; run explicitly for depth readback acceptance"]
    fn near_quad_occludes_far_quad_in_both_submission_orders() {
        let mut renderer = native_renderer();
        // Actual maximum offline Scene; the live transport stays at 2048 rays.
        let mut maximum_scene = Scene::default();
        maximum_scene.set_fixed_frame("map").unwrap();
        for id in 1..=MAX_LAYERS as u64 {
            maximum_scene.add_layer(id).unwrap();
        }
        maximum_scene.start().unwrap();
        let ranges = vec![1.0; MAX_POINTS];
        for id in 1..=MAX_LAYERS as u64 {
            let token = maximum_scene.layer_token(id).unwrap();
            maximum_scene
                .update(
                    token,
                    super::super::Scan {
                        frame: "laser",
                        stamp_ns: 1,
                        clock: super::super::Clock::System,
                        complete: true,
                        angle_min: 0.0,
                        angle_increment: 0.001,
                        range_min: 0.1,
                        range_max: 10.0,
                        ranges: &ranges,
                    },
                    super::super::TfOutcome::Ready {
                        source: "laser",
                        target: "map",
                        stamp_ns: 1,
                        transform: super::super::RigidTransform::identity(),
                    },
                )
                .unwrap();
        }
        assert_eq!(
            maximum_scene
                .layers()
                .iter()
                .map(|l| l.points().len())
                .sum::<usize>(),
            MAX_LAYERS * MAX_POINTS
        );
        let maximum_vertices = scene_vertices(
            &maximum_scene,
            CameraPose::orbit(0.5, 0.8, 12.0, [0.0; 3]),
            [MAX_DIMENSION; 2],
            &egui::Visuals::dark(),
        );
        assert!(maximum_vertices.len() >= MAX_LAYERS * MAX_POINTS * 6);
        assert!(maximum_vertices.len() <= MAX_VERTICES);
        let maximum_id = renderer.render_vertices([MAX_DIMENSION; 2], &maximum_vertices);
        assert_eq!(renderer.frame_size(), Some([MAX_DIMENSION; 2]));
        let frame = renderer.frame.as_ref().unwrap();
        assert_eq!(frame.color.size().width, MAX_DIMENSION);
        assert_eq!(frame.color.size().height, MAX_DIMENSION);
        assert_eq!(frame._depth.size(), frame.color.size());
        assert_eq!(frame.color.format(), wgpu::TextureFormat::Rgba8UnormSrgb);
        assert_eq!(frame._depth.format(), wgpu::TextureFormat::Depth32Float);
        assert_eq!(
            u64::from(frame.color.size().width) * u64::from(frame.color.size().height) * 8,
            32 * 1024 * 1024
        );
        assert_eq!(
            renderer.vertices.as_ref().unwrap().size(),
            (MAX_VERTICES * 32) as u64
        );
        assert!(renderer
            .state
            .renderer
            .read()
            .texture(&maximum_id)
            .is_some());
        eprintln!("Maximum resource case: {} points, {} vertices, {}x{} color+depth (32 MiB logical), vertex buffer {} bytes",MAX_LAYERS*MAX_POINTS, maximum_vertices.len(),MAX_DIMENSION,MAX_DIMENSION,MAX_VERTICES*32);
        let quad_at = |z: f32, color: [f32; 4]| {
            let mut out = Vec::new();
            quad(
                &mut out,
                [
                    [-0.8, -0.8, z, 1.0],
                    [0.8, -0.8, z, 1.0],
                    [0.8, 0.8, z, 1.0],
                    [-0.8, 0.8, z, 1.0],
                ],
                color,
            );
            out
        };
        let near = quad_at(0.2, [1.0, 0.0, 0.0, 1.0]);
        let far = quad_at(0.8, [0.0, 1.0, 0.0, 1.0]);
        for mode in 0..3 {
            let reverse = mode == 1;
            let vertices = if reverse {
                far.iter().chain(&near)
            } else {
                near.iter().chain(&far)
            }
            .copied()
            .collect::<Vec<_>>();
            if mode == 2 {
                let old_id = renderer.frame.as_ref().unwrap().id;
                renderer.release_frame();
                assert!(renderer.vertices.is_none());
                assert!(renderer.color_texture().is_none());
                assert!(renderer.state.renderer.read().texture(&old_id).is_none());
                renderer.render_vertices([64, 64], &[]);
            } else {
                renderer.render_vertices([64, 64], &vertices);
            }
            assert_eq!(renderer.frame_size(), Some([64, 64]));
            assert_eq!(renderer.frame.as_ref().unwrap().color.size().width, 64);
            assert_eq!(renderer.frame.as_ref().unwrap()._depth.size().height, 64);
            assert_ne!(renderer.frame.as_ref().unwrap().id, maximum_id);
            assert!(
                renderer
                    .state
                    .renderer
                    .read()
                    .texture(&maximum_id)
                    .is_none(),
                "resize unregisters old maximum target"
            );
            let device = &renderer.state.device;
            let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Acceptance readback"),
                size: 64 * 256,
                usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                mapped_at_creation: false,
            });
            let mut encoder = device.create_command_encoder(&Default::default());
            encoder.copy_texture_to_buffer(
                wgpu::TexelCopyTextureInfo {
                    texture: renderer.color_texture().unwrap(),
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                wgpu::TexelCopyBufferInfo {
                    buffer: &buffer,
                    layout: wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(256),
                        rows_per_image: Some(64),
                    },
                },
                wgpu::Extent3d {
                    width: 64,
                    height: 64,
                    depth_or_array_layers: 1,
                },
            );
            renderer.state.queue.submit([encoder.finish()]);
            let (tx, rx) = std::sync::mpsc::channel();
            buffer
                .slice(..)
                .map_async(wgpu::MapMode::Read, move |result| tx.send(result).unwrap());
            device.poll(wgpu::Maintain::Wait);
            rx.recv().unwrap().unwrap();
            {
                let data = buffer.slice(..).get_mapped_range();
                let pixel = &data[32 * 256 + 32 * 4..32 * 256 + 32 * 4 + 4];
                if mode == 2 {
                    assert_ne!(pixel, [255, 0, 0, 255], "released scan must not reappear");
                    assert_ne!(pixel, [0, 255, 0, 255]);
                    assert_eq!(pixel[3], 255);
                } else {
                    assert_eq!(
                        pixel,
                        [255, 0, 0, 255],
                        "near red must occlude far green; reversed={reverse}"
                    );
                }
            }
            buffer.unmap();
        }
        assert_eq!(renderer.frame_size(), Some([64, 64]));
        let final_id = renderer.frame.as_ref().unwrap().id;
        renderer.release_frame();
        assert!(renderer.state.renderer.read().texture(&final_id).is_none());
        assert!(renderer.color_texture().is_none());
        assert!(renderer.vertices.is_none());
    }
}
