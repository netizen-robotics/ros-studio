use super::*;
use crate::spatial::marker_text::Atlas;
struct Entry {
    source: Weak<Atlas>,
    _texture: wgpu::Texture,
    bind: wgpu::BindGroup,
    bytes: usize,
}
pub(super) struct Textures {
    pub layout: wgpu::BindGroupLayout,
    _fallback: wgpu::Texture,
    fallback_bind: wgpu::BindGroup,
    entry: Option<Entry>,
    #[cfg(test)]
    pub uploads: usize,
}
impl Textures {
    pub fn new(state: &egui_wgpu::RenderState) -> Self {
        let layout = state
            .device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("Marker glyph atlas layout"),
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
                ],
            });
        let (texture, bind) = create(state, &layout, [1, 1], &[255]);
        Self {
            layout,
            _fallback: texture,
            fallback_bind: bind,
            entry: None,
            #[cfg(test)]
            uploads: 0,
        }
    }
    pub fn sync(
        &mut self,
        state: &egui_wgpu::RenderState,
        atlas: Option<&Arc<Atlas>>,
    ) -> Result<(), String> {
        let Some(atlas) = atlas else {
            self.clear();
            return Ok(());
        };
        if atlas
            .size
            .into_iter()
            .any(|n| n > state.device.limits().max_texture_dimension_2d)
        {
            return Err("Marker glyph atlas exceeds GPU dimensions".into());
        }
        if self
            .entry
            .as_ref()
            .and_then(|e| e.source.upgrade())
            .is_some_and(|a| Arc::ptr_eq(&a, atlas))
        {
            return Ok(());
        }
        let (texture, bind) = create(state, &self.layout, atlas.size, &atlas.pixels);
        #[cfg(test)]
        {
            self.uploads += 1;
        }
        self.entry = Some(Entry {
            source: Arc::downgrade(atlas),
            _texture: texture,
            bind,
            bytes: atlas.bytes(),
        });
        Ok(())
    }
    pub fn clear(&mut self) {
        self.entry = None;
    }
    pub fn bind(&self) -> &wgpu::BindGroup {
        self.entry.as_ref().map_or(&self.fallback_bind, |e| &e.bind)
    }
    pub fn bytes(&self) -> usize {
        self.entry.as_ref().map_or(0, |e| e.bytes)
    }
}
fn create(
    state: &egui_wgpu::RenderState,
    layout: &wgpu::BindGroupLayout,
    size: [u32; 2],
    pixels: &[u8],
) -> (wgpu::Texture, wgpu::BindGroup) {
    sampled(
        state,
        layout,
        size,
        pixels,
        wgpu::TextureFormat::R8Unorm,
        false,
    )
}
pub(super) fn sampled(
    state: &egui_wgpu::RenderState,
    layout: &wgpu::BindGroupLayout,
    size: [u32; 2],
    pixels: &[u8],
    format: wgpu::TextureFormat,
    repeat: bool,
) -> (wgpu::Texture, wgpu::BindGroup) {
    let texture = state.device.create_texture(&wgpu::TextureDescriptor {
        label: Some("Bounded Marker glyph coverage"),
        size: wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format,
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
        pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(
                size[0]
                    * if format == wgpu::TextureFormat::R8Unorm {
                        1
                    } else {
                        4
                    },
            ),
            rows_per_image: Some(size[1]),
        },
        wgpu::Extent3d {
            width: size[0],
            height: size[1],
            depth_or_array_layers: 1,
        },
    );
    let sampler = state.device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("Marker glyph linear coverage"),
        address_mode_u: if repeat {
            wgpu::AddressMode::Repeat
        } else {
            wgpu::AddressMode::ClampToEdge
        },
        address_mode_v: if repeat {
            wgpu::AddressMode::Repeat
        } else {
            wgpu::AddressMode::ClampToEdge
        },
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        ..Default::default()
    });
    let view = texture.create_view(&Default::default());
    let bind = state.device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("Marker glyph atlas binding"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    });
    (texture, bind)
}
