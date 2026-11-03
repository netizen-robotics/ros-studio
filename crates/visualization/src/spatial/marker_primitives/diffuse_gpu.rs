use super::*;
#[derive(Default)]
pub(super) struct Textures {
    entries: Vec<Entry>,
    #[cfg(test)]
    pub uploads: usize,
}
struct Entry {
    source: Weak<crate::robot::RobotTexture>,
    _texture: wgpu::Texture,
    bind: wgpu::BindGroup,
    bytes: usize,
}
impl Textures {
    pub fn sync(
        &mut self,
        state: &egui_wgpu::RenderState,
        layout: &wgpu::BindGroupLayout,
        images: &[Arc<crate::robot::RobotTexture>],
    ) {
        let mut next = Vec::with_capacity(images.len());
        for image in images {
            if let Some(i) = self.entries.iter().position(|e| {
                e.source
                    .upgrade()
                    .is_some_and(|old| Arc::ptr_eq(&old, image))
            }) {
                next.push(self.entries.swap_remove(i));
            } else {
                #[cfg(test)]
                {
                    self.uploads += 1;
                }
                let (texture, bind) = atlas_gpu::sampled(
                    state,
                    layout,
                    [image.width, image.height],
                    &image.rgba,
                    wgpu::TextureFormat::Rgba8UnormSrgb,
                    true,
                );
                next.push(Entry {
                    source: Arc::downgrade(image),
                    _texture: texture,
                    bind,
                    bytes: image.rgba.len(),
                });
            }
        }
        self.entries = next;
    }
    pub fn bind(&self, slot: u8) -> &wgpu::BindGroup {
        &self.entries[slot as usize].bind
    }
    pub fn clear(&mut self) {
        self.entries.clear();
    }
    pub fn bytes(&self) -> usize {
        self.entries.iter().map(|e| e.bytes).sum()
    }
}
