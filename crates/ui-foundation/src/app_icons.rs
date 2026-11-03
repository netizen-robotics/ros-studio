//! Original suite identity, shared by window chrome and the Launcher library.
use eframe::egui;
use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppIcon {
    Launcher,
    Visualizer,
    Tools,
}
impl AppIcon {
    fn bytes(self) -> &'static [u8] {
        match self {
            Self::Launcher => include_bytes!("../assets/app-icons/launcher.png"),
            Self::Visualizer => include_bytes!("../assets/app-icons/visualizer.png"),
            Self::Tools => include_bytes!("../assets/app-icons/tools.png"),
        }
    }
    pub fn window_icon(self) -> Arc<egui::IconData> {
        Arc::new(
            eframe::icon_data::from_png_bytes(self.bytes()).expect("validated bundled app icon"),
        )
    }
    pub fn show(self, ui: &mut egui::Ui, size: f32) {
        let id = egui::Id::new(format!("app-icon-{self:?}"));
        let texture = ui
            .ctx()
            .data_mut(|data| data.get_temp::<egui::TextureHandle>(id));
        let texture = texture.unwrap_or_else(|| {
            let icon = self.window_icon();
            let image = egui::ColorImage::from_rgba_unmultiplied(
                [icon.width as usize, icon.height as usize],
                &icon.rgba,
            );
            let texture = ui.ctx().load_texture(
                format!("app-icon-{self:?}"),
                image,
                egui::TextureOptions::LINEAR,
            );
            ui.ctx()
                .data_mut(|data| data.insert_temp(id, texture.clone()));
            texture
        });
        ui.add(egui::Image::new((texture.id(), egui::vec2(size, size))));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_icons_decode_to_distinct_transparent_256_pixel_assets() {
        let icons =
            [AppIcon::Launcher, AppIcon::Visualizer, AppIcon::Tools].map(AppIcon::window_icon);
        for icon in &icons {
            assert_eq!((icon.width, icon.height), (256, 256));
            assert_eq!(icon.rgba.len(), 256 * 256 * 4);
            assert_eq!(&icon.rgba[..4], &[0, 0, 0, 0]);
            assert!(icon.rgba.chunks_exact(4).any(|pixel| pixel[3] == 255));
        }
        assert_ne!(icons[0].rgba, icons[1].rgba);
        assert_ne!(icons[1].rgba, icons[2].rgba);
    }
}
