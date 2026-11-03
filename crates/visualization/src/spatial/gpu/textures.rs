use super::*;
use crate::spatial::robot_geometry::TextureBatch;
use std::{collections::BTreeMap, sync::Arc};
type Key = (usize, [u32; 4]);
struct ImageEntry {
    _image: Arc<crate::robot::RobotTexture>,
    texture: wgpu::Texture,
}
struct Entry {
    _tint: wgpu::Buffer,
    bind: wgpu::BindGroup,
}
pub(super) struct Textures {
    pub pipeline: wgpu::RenderPipeline,
    pub transparent: wgpu::RenderPipeline,
    entries: BTreeMap<Key, Entry>,
    images: BTreeMap<usize, ImageEntry>,
}
fn key(b: &TextureBatch) -> Key {
    (Arc::as_ptr(&b.image) as usize, b.rgba.map(f32::to_bits))
}
impl Textures {
    pub fn new(state: &egui_wgpu::RenderState) -> Self {
        let shader = state
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("Robot diffuse texture"),
                source: wgpu::ShaderSource::Wgsl(include_str!("texture.wgsl").into()),
            });
        let bindings = state
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Shared sampled material bindings"),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D2,
                            multisampled: false,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
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
        let layout = state
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("Shared sampled material layout"),
                bind_group_layouts: &[&bindings],
                push_constant_ranges: &[],
            });
        let make = |transparent: bool| {
            state
                .device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("Opaque robot texture"),
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
        let pipeline = make(false);
        let transparent = make(true);
        Self {
            pipeline,
            transparent,
            entries: BTreeMap::new(),
            images: BTreeMap::new(),
        }
    }
    #[cfg(test)]
    pub fn counts(&self) -> (usize, usize) {
        (self.images.len(), self.entries.len())
    }
    pub fn clear(&mut self) {
        self.entries.clear();
        self.images.clear();
    }
    pub fn sync(&mut self, state: &egui_wgpu::RenderState, batches: &[TextureBatch]) {
        self.entries
            .retain(|k, _| batches.iter().any(|b| key(b) == *k));
        self.images
            .retain(|k, _| batches.iter().any(|b| Arc::as_ptr(&b.image) as usize == *k));
        for b in batches {
            let k = key(b);
            if self.entries.contains_key(&k) {
                continue;
            }
            let image = &b.image;
            self.images.entry(k.0).or_insert_with(|| {
                let texture = state.device.create_texture(&wgpu::TextureDescriptor {
                    label: Some("Bounded robot diffuse image"),
                    size: wgpu::Extent3d {
                        width: image.width,
                        height: image.height,
                        depth_or_array_layers: 1,
                    },
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba8UnormSrgb,
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
                    &image.rgba,
                    wgpu::TexelCopyBufferLayout {
                        offset: 0,
                        bytes_per_row: Some(image.width * 4),
                        rows_per_image: Some(image.height),
                    },
                    texture.size(),
                );
                ImageEntry {
                    _image: Arc::clone(image),
                    texture,
                }
            });
            let view = self.images[&k.0].texture.create_view(&Default::default());
            let sampler = state.device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("Robot repeating linear sampler"),
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::Repeat,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                ..Default::default()
            });
            let tint = state.device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("Robot texture tint"),
                size: 16,
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            state
                .queue
                .write_buffer(&tint, 0, bytemuck::cast_slice(&b.rgba));
            let bind = state.device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("Robot diffuse material"),
                layout: &self.pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::Sampler(&sampler),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: tint.as_entire_binding(),
                    },
                ],
            });
            self.entries.insert(k, Entry { _tint: tint, bind });
        }
    }
    pub fn bind(&self, batch: &TextureBatch) -> &wgpu::BindGroup {
        &self.entries[&key(batch)].bind
    }
}
