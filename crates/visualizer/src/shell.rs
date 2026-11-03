#[cfg(test)]
use eframe::egui;
pub use studio_ui_foundation::theme::*;
#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::Color32;

    fn luminance(color: Color32) -> f32 {
        let linear = |byte: u8| {
            let v = f32::from(byte) / 255.0;
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
    }
    fn contrast(a: Color32, b: Color32) -> f32 {
        let a = luminance(a);
        let b = luminance(b);
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }
    #[test]
    fn normal_status_error_and_selected_text_meet_contrast_in_both_themes() {
        let ctx = egui::Context::default();
        configure(&ctx);
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            let style = ctx.style_of(theme);
            let v = &style.visuals;
            for background in [v.panel_fill, v.extreme_bg_color] {
                for text in [
                    v.text_color(),
                    v.hyperlink_color,
                    v.error_fg_color,
                    v.warn_fg_color,
                    muted(v),
                ] {
                    assert!(
                        contrast(text, background) >= 4.5,
                        "{theme:?}: {text:?} on {background:?}"
                    );
                }
            }
            assert!(contrast(v.selection.stroke.color, v.selection.bg_fill) >= 4.5);
        }
    }
}
