//! One integer texture and six vertices, independent of cell count.
use super::*;
use crate::spatial::{
    map_appearance::Palette,
    map_texture::{TextureMap, TextureMapFrame},
};
use std::sync::{Arc, Weak};
struct Entry {
    owner: Weak<TextureMap>,
    _texture: wgpu::Texture,
    bind: wgpu::BindGroup,
}
pub(super) struct MapTexture {
    pub opaque: wgpu::RenderPipeline,
    pub transparent: wgpu::RenderPipeline,
    pub vertices: wgpu::Buffer,
    colors: wgpu::Buffer,
    opacity: wgpu::Buffer,
    palette: Option<Palette>,
    alpha: Option<u32>,
    entry: Option<Entry>,
    #[cfg(test)]
    pub uploads: usize,
}
impl MapTexture {
    pub fn new(state: &egui_wgpu::RenderState) -> Self {
        let device = &state.device;
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("Integer map palette"),
            source: wgpu::ShaderSource::Wgsl(include_str!("map_texture.wgsl").into()),
        });
        let bindings = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("Map bindings"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: std::num::NonZeroU64::new(4096),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: std::num::NonZeroU64::new(16),
                    },
                    count: None,
                },
            ],
        });
        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("Map layout"),
            bind_group_layouts: &[&bindings],
            push_constant_ranges: &[],
        });
        let make = |transparent: bool| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("Map quad"),
                layout: Some(&layout),
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
                primitive: Default::default(),
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
        let buffer = |size, usage, label| {
            device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size,
                usage: usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            })
        };
        Self {
            opaque: make(false),
            transparent: make(true),
            vertices: buffer(192, wgpu::BufferUsages::VERTEX, "Six map vertices"),
            colors: buffer(4096, wgpu::BufferUsages::UNIFORM, "Map palette"),
            opacity: buffer(16, wgpu::BufferUsages::UNIFORM, "Map opacity"),
            palette: None,
            alpha: None,
            entry: None,
            #[cfg(test)]
            uploads: 0,
        }
    }
    pub fn sync(
        &mut self,
        state: &egui_wgpu::RenderState,
        frame: TextureMapFrame<'_>,
        vertices: &[RenderVertex; 6],
    ) {
        if self.palette != Some(frame.appearance.palette) {
            let colors = crate::spatial::map_palette::palette(frame.appearance.palette)
                .map(|color| color.map(|c| c as f32 / 255.));
            state
                .queue
                .write_buffer(&self.colors, 0, bytemuck::cast_slice(&colors));
            self.palette = Some(frame.appearance.palette);
        }
        if self.alpha != Some(frame.appearance.opacity.to_bits()) {
            state.queue.write_buffer(
                &self.opacity,
                0,
                bytemuck::cast_slice(&[frame.appearance.opacity, 0., 0., 0.]),
            );
            self.alpha = Some(frame.appearance.opacity.to_bits());
        }
        if self
            .entry
            .as_ref()
            .is_none_or(|e| e.owner.as_ptr() != Arc::as_ptr(frame.map))
        {
            let [width, height] = frame.map.dimensions();
            let texture = state.device.create_texture(&wgpu::TextureDescriptor {
                label: Some("Raw signed map cells"),
                size: wgpu::Extent3d {
                    width,
                    height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::R8Uint,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            });
            state.queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d::ZERO,
                    aspect: wgpu::TextureAspect::All,
                },
                frame.map.bytes(),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(width),
                    rows_per_image: Some(height),
                },
                texture.size(),
            );
            let view = texture.create_view(&Default::default());
            let bind = state.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Map cells and palette"),
                layout: &self.opaque.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.colors.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.opacity.as_entire_binding(),
                    },
                ],
            });
            self.entry = Some(Entry {
                owner: Arc::downgrade(frame.map),
                _texture: texture,
                bind,
            });
            #[cfg(test)]
            {
                self.uploads += 1;
            }
        }
        state
            .queue
            .write_buffer(&self.vertices, 0, bytemuck::cast_slice(vertices));
    }
    pub fn bind(&self) -> &wgpu::BindGroup {
        &self.entry.as_ref().unwrap().bind
    }
}
