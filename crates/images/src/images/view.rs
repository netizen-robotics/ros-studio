use super::{DecodedFrame, FrameMetadata, LatestFrames};
use eframe::egui;

/// One reusable texture allocation for worker-converted Image pixels. Image
/// companions and calibrated scene consumers use the same upload convention.
pub fn update_texture(
    ctx: &egui::Context,
    texture: &mut Option<egui::TextureHandle>,
    frame: &DecodedFrame,
) -> egui::TextureId {
    if let Some(texture) = texture {
        texture.set(frame.color_image(), egui::TextureOptions::NEAREST);
    } else {
        *texture = Some(ctx.load_texture(
            "camera-frame",
            frame.color_image(),
            egui::TextureOptions::NEAREST,
        ));
    }
    texture.as_ref().expect("texture created above").id()
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub enum DisplayMode {
    #[default]
    Fit,
    ActualSize,
}

/// Owns at most one displayed texture and a shared one-frame mailbox. All
/// lifecycle transitions must use these methods, not the mailbox alone.
#[derive(Default)]
pub struct ImageView {
    frames: LatestFrames,
    texture: Option<egui::TextureHandle>,
    metadata: Option<([usize; 2], String)>,
    displayed_metadata: Option<FrameMetadata>,
    active: bool,
    paused: bool,
    error: Option<String>,
    mode: DisplayMode,
}
impl ImageView {
    pub fn frames(&self) -> LatestFrames {
        self.frames.clone()
    }
    /// Returns generation for the worker; clears the previous live image.
    pub fn start(&mut self) -> u64 {
        self.clear_display();
        self.active = true;
        self.paused = false;
        self.frames.start()
    }
    /// Use for stop, replacement, disconnect and failure cleanup.
    pub fn stop(&mut self) {
        self.frames.stop();
        self.active = false;
        self.paused = false;
        self.clear_display();
    }
    pub fn fail(&mut self, message: impl Into<String>) {
        self.stop();
        self.error = Some(message.into());
    }
    fn clear_display(&mut self) {
        self.texture = None;
        self.metadata = None;
        self.displayed_metadata = None;
        self.error = None;
    }
    pub fn set_paused(&mut self, paused: bool) {
        self.paused = self.active && paused;
        self.frames.set_paused(self.paused);
    }
    pub fn set_display_mode(&mut self, mode: DisplayMode) {
        self.mode = mode;
    }
    pub fn display_mode(&self) -> DisplayMode {
        self.mode
    }
    pub fn is_paused(&self) -> bool {
        self.paused
    }
    pub fn has_displayed_frame(&self) -> bool {
        self.texture.is_some()
    }
    pub fn displayed_metadata(&self) -> Option<&FrameMetadata> {
        self.displayed_metadata.as_ref()
    }
    fn upload(
        &mut self,
        ctx: &egui::Context,
        frame: DecodedFrame,
        metadata: Option<FrameMetadata>,
    ) {
        self.displayed_metadata = metadata;
        update_texture(ctx, &mut self.texture, &frame);
        self.metadata = Some((frame.size, frame.encoding));
    }
    /// Thread/texture-bound operation; `decode` and raw receipt belong to worker.
    pub fn show(&mut self, ui: &mut egui::Ui) {
        // Standalone hosts also own the details toggle below the canvas.
        let details_height = ui.spacing().interact_size.y.max(
            ui.text_style_height(&egui::TextStyle::Button) + 2.0 * ui.spacing().button_padding.y,
        ) + ui.spacing().item_spacing.y;
        ui.scope(|ui| {
            ui.set_max_height((ui.available_height() - details_height).max(0.0));
            self.show_content(ui);
        });
        self.show_details(ui);
    }
    /// Canvas and local tools only; workspace inspector owns metadata.
    pub fn show_content(&mut self, ui: &mut egui::Ui) {
        self.show_content_with_guidance(ui, true);
    }
    /// Compact hosts keep background explanations in tooltips.
    pub fn show_content_with_guidance(&mut self, ui: &mut egui::Ui, guidance: bool) {
        if self.active && !self.paused {
            if let Some((frame, metadata)) = self.frames.take_with_metadata() {
                self.upload(ui.ctx(), frame, metadata);
            }
        }
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.mode, DisplayMode::Fit, "Fit to view");
            ui.selectable_value(&mut self.mode, DisplayMode::ActualSize, "Actual size");
            if self.active {
                let label = if self.paused {
                    "Resume image"
                } else {
                    "Pause image"
                };
                if ui.button(label).clicked() {
                    self.set_paused(!self.paused);
                }
            }
        });
        if let Some(message) = &self.error {
            ui.label(format!("Image unavailable: {message}"));
            return;
        }
        if !self.active {
            ui.label(if guidance {
                "Image stopped. Subscribe explicitly to receive frames."
            } else {
                "Image stopped"
            });
            return;
        }

        if self.paused {
            ui.label(if guidance {
                "Paused. Retained frame; incoming frames skipped."
            } else {
                "Paused"
            })
            .on_hover_text("Retained frame; incoming frames are skipped.");
        } else if self.texture.is_none() {
            ui.label("Waiting for image samples…");
        }
        let Some(texture) = &self.texture else {
            return;
        };
        let Some((size, encoding)) = &self.metadata else {
            return;
        };
        let natural = egui::vec2(size[0] as f32, size[1] as f32);
        // Reserve the actual one-line metadata row, then fit the remaining
        // viewport. Fixed extra reservations made short hosts shrink to a pixel.
        let metadata_height =
            ui.text_style_height(&egui::TextStyle::Body) + ui.spacing().item_spacing.y;
        let available = egui::vec2(
            ui.available_width(),
            (ui.available_height() - metadata_height).max(1.0),
        );
        let scale = match self.mode {
            DisplayMode::ActualSize => 1.0,
            DisplayMode::Fit => (available.x / natural.x).min(available.y / natural.y),
        };
        let metadata_label = format!(
            "{} × {} · {}{} · {:.1}% (logical pixels)",
            size[0],
            size[1],
            encoding,
            if matches!(encoding.as_str(), "16UC1" | "32FC1") {
                " · Depth 0–10 m"
            } else {
                ""
            },
            scale * 100.0
        );
        ui.add(egui::Label::new(&metadata_label).truncate())
            .on_hover_text(&metadata_label);
        egui::ScrollArea::both()
            .id_salt("image-pixels")
            .max_height(ui.available_height().max(1.0))
            .show(ui, |ui| {
                egui::Frame::new()
                    .fill(ui.visuals().extreme_bg_color)
                    .show(ui, |ui| {
                        let frame = self
                            .displayed_metadata
                            .as_ref()
                            .map_or("unknown", |metadata| metadata.frame_id.as_str());
                        ui.add(egui::Image::new((texture.id(), natural * scale)).alt_text(
                            format!(
                                "{} image, {} by {} pixels, frame {}{}",
                                encoding,
                                size[0],
                                size[1],
                                frame,
                                if self.paused {
                                    ", paused retained frame"
                                } else {
                                    ""
                                },
                            ),
                        ));
                    });
            });
        // Adapter must request repaint after publishing a frame; no polling loop.
    }
    pub fn show_details(&self, ui: &mut egui::Ui) {
        let counters = self.frames.counters();
        ui.collapsing("Image details", |ui| {
            ui.small(format!(
                "Superseded before display: {} · Skipped while paused: {} · Stale rejected: {}",
                counters.superseded, counters.paused_skips, counters.stale_rejected
            ));
            if let Some(metadata) = &self.displayed_metadata {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!(
                            "Frame: {} · Header sec: {} · nanosec: {:09} · Clock provenance: {}",
                            metadata.frame_id,
                            metadata.header_stamp.seconds,
                            metadata.header_stamp.nanoseconds,
                            if metadata.source_time.is_some() {
                                "supplied"
                            } else {
                                "unknown"
                            }
                        ))
                        .monospace(),
                    )
                    .selectable(true)
                    .wrap(),
                );
            }
        });
    }
}
impl Drop for ImageView {
    fn drop(&mut self) {
        self.frames.stop();
    }
}

#[cfg(test)]
mod presentation_tests {
    use super::*;

    #[test]
    fn image_details_follow_the_image_stage_without_hiding_its_toggle() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut view = ImageView::default();
        let generation = view.start();
        assert!(view.frames().publish(
            generation,
            DecodedFrame {
                size: [64, 48],
                encoding: "rgba8".into(),
                image: egui::ColorImage::new([64, 48], egui::Color32::WHITE).into(),
            }
        ));
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(480.0, 400.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| view.show(ui));
            },
        );
        let texture = view.texture.as_ref().unwrap().id();
        let primitives = context.tessellate(output.shapes.clone(), output.pixels_per_point);
        let image_bottom = primitives
            .iter()
            .find_map(|primitive| match &primitive.primitive {
                egui::epaint::Primitive::Mesh(mesh) if mesh.texture_id == texture => {
                    Some(mesh.calc_bounds().bottom())
                }
                _ => None,
            })
            .expect("Image stage must be painted");
        let details_rect = output
            .shapes
            .iter()
            .find_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.text() == "Image details" => {
                    Some(shape.shape.visual_bounding_rect())
                }
                _ => None,
            })
            .expect("Details must remain visible");
        assert!(details_rect.top() >= image_bottom);
        assert!(details_rect.bottom() <= 400.0);
        assert!(output
            .platform_output
            .accesskit_update
            .as_ref()
            .unwrap()
            .nodes
            .iter()
            .any(|(_, node)| node.role() == egui::accesskit::Role::Image
                && node.label().is_some_and(
                    |label| label.contains("rgba8 image, 64 by 48 pixels, frame unknown")
                )));
        view.set_paused(true);
        let paused = context.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| view.show(ui));
        });
        assert!(paused
            .platform_output
            .accesskit_update
            .unwrap()
            .nodes
            .iter()
            .any(|(_, node)| node.role() == egui::accesskit::Role::Image
                && node
                    .label()
                    .is_some_and(|label| label.contains("paused retained frame"))));
        assert_eq!(view.texture.as_ref().unwrap().id(), texture);
    }
}
