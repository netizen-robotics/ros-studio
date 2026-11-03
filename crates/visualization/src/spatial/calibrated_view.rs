//! Dedicated calibrated viewport. Image conversion and Marker preparation stay
//! on workers; this view only uploads shared pixels and projects scene content.
use super::{
    calibrated_camera::RectifiedProjection, combined::CombinedSceneFrame,
    scene_view::SceneOverlays, CameraPose, Renderer3d, RigidTransform,
};
use eframe::egui;
use std::sync::{Arc, Weak};
use studio_images::TypedImage;
use studio_protocol::{
    camera_snapshot::{transport::Permission, Snapshot, Status},
    latest_pose::LatestPose,
};
use studio_workspace::camera::{Appearance, ImagePlacement};

pub struct CalibratedFrame<'a> {
    pub snapshot: &'a Arc<Snapshot>,
    pub permission: &'a Permission,
    pub image: &'a Arc<TypedImage>,
    pub projection: RectifiedProjection,
    pub scene: &'a CombinedSceneFrame<'a>,
    pub overlays: SceneOverlays<'a>,
}

#[cfg(test)]
pub(crate) mod tests;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CameraPresentation {
    Presented,
    Superseded,
    GpuUnavailable,
}
#[derive(Default)]
pub struct CalibratedSceneView {
    pub appearance: Appearance,
    /// Feed the existing off-thread Marker projector; never reuse an orbit
    /// projection for this image. Mismatched cached Marker frames stay hidden.
    pub marker_request: Option<(CameraPose, [u32; 2])>,
    texture: Option<egui::TextureHandle>,
    displayed: Weak<TypedImage>,
    #[cfg(test)]
    pub(crate) before_present: Option<Box<dyn FnOnce()>>,
}
impl CalibratedSceneView {
    pub fn clear(&mut self) {
        self.texture = None;
        self.displayed = Weak::new();
        self.marker_request = None;
    }
    fn camera(&self, frame: &CalibratedFrame<'_>) -> Result<CameraPose, String> {
        self.appearance.validate()?;
        if frame.snapshot.status != Status::Receiving
            || frame.snapshot.owner.fixed_frame != frame.scene.fixed_frame
        {
            return Err("Camera is waiting for a current aligned frame".into());
        }
        let captured = frame
            .snapshot
            .frame
            .as_ref()
            .ok_or("Camera has no captured frame")?;
        if frame.image.frame.size()
            != [
                captured.image.metadata.width as usize,
                captured.image.metadata.height as usize,
            ]
        {
            return Err("Prepared Camera image dimensions disagree with capture".into());
        }
        let LatestPose::Ready { components, .. } = captured.transform else {
            return Err("Camera transform is unavailable".into());
        };
        let transform = RigidTransform::new(
            [components[0], components[1], components[2]],
            [components[3], components[4], components[5], components[6]],
        )
        .ok_or("Invalid captured Camera transform")?;
        CameraPose::calibrated(
            transform,
            frame.projection,
            f64::from(self.appearance.zoom_milli) / 1000.,
        )
    }
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        frame: &CalibratedFrame<'_>,
        renderer: Option<&mut Renderer3d>,
        height: f32,
    ) -> Result<CameraPresentation, String> {
        // Continuity may be revoked between App polling and presentation.
        // Hide invalid alignment without turning a calibration/TF wait into Stop.
        if !frame.permission.authorizes(frame.snapshot) {
            self.clear();
            if let Some(renderer) = renderer {
                renderer.release_frame();
            }
            ui.allocate_space(egui::vec2(ui.available_width().max(1.), height.max(1.)));
            ui.ctx().request_repaint();
            return Ok(CameraPresentation::Superseded);
        }
        let camera = match self.camera(frame) {
            Ok(camera) => camera,
            Err(error) => {
                self.clear();
                if let Some(renderer) = renderer {
                    renderer.release_frame();
                }
                return Err(error);
            }
        };
        let size = egui::vec2(ui.available_width().max(1.), height.max(1.));
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
        if response.clicked() {
            response.request_focus();
        }
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Other,
                ui.is_enabled(),
                "Calibrated rectified camera scene",
            )
        });
        let physical = super::gpu::bounded_dimensions([
            (rect.width() * ui.ctx().pixels_per_point()).round().max(1.) as u32,
            (rect.height() * ui.ctx().pixels_per_point())
                .round()
                .max(1.) as u32,
        ]);
        self.marker_request = Some((camera, physical));
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        painter.rect_filled(rect, 0., ui.visuals().extreme_bg_color);
        let Some(renderer) = renderer else {
            self.texture = None;
            self.displayed = Weak::new();
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "Calibrated GPU presentation unavailable",
                egui::FontId::proportional(14.),
                ui.visuals().text_color(),
            );
            return Ok(CameraPresentation::GpuUnavailable);
        };
        let markers = frame
            .overlays
            .markers
            .filter(|m| m.camera == camera && m.size == physical);
        let scene_id = renderer
            .render_combined_with_maps(
                physical,
                frame.scene,
                camera,
                ui.visuals(),
                frame.overlays.maps,
                markers.map(|m| m.layer),
                frame.overlays.uncertainty,
            )
            .map_err(|e| e.to_string())?;
        // Exercise the exact publication race deterministically in the Metal
        // regression; production never stores or invokes this test-only hook.
        #[cfg(test)]
        if let Some(before_present) = self.before_present.take() {
            before_present();
        }
        // Supersession, replacement and Stop all hide this exact retired frame.
        // Only source/configuration/renderer faults are fatal to the display.
        if !frame.permission.authorizes(frame.snapshot) {
            self.clear();
            renderer.release_frame();
            ui.ctx().request_repaint();
            return Ok(CameraPresentation::Superseded);
        }
        if !self
            .displayed
            .upgrade()
            .is_some_and(|old| Arc::ptr_eq(&old, frame.image))
        {
            studio_images::update_texture(ui.ctx(), &mut self.texture, &frame.image.frame);
            self.displayed = Arc::downgrade(frame.image);
        }
        let image_id = self.texture.as_ref().expect("image uploaded above").id();
        let scale = frame
            .projection
            .image_scale(physical)
            .ok_or("Invalid Camera viewport")?;
        let zoom = f32::from(self.appearance.zoom_milli) / 1000.;
        let image_rect = egui::Rect::from_center_size(
            rect.center(),
            egui::vec2(
                rect.width() * scale[0] as f32 * zoom,
                rect.height() * scale[1] as f32 * zoom,
            ),
        );
        let uv = egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1., 1.));
        if matches!(
            self.appearance.placement,
            ImagePlacement::Background | ImagePlacement::Both
        ) {
            painter.image(image_id, image_rect, uv, egui::Color32::WHITE);
        }
        painter.image(scene_id, rect, uv, egui::Color32::WHITE);
        if matches!(
            self.appearance.placement,
            ImagePlacement::Overlay | ImagePlacement::Both
        ) {
            let alpha = (u32::from(self.appearance.overlay_alpha_milli) * 255 / 1000) as u8;
            painter.image(
                image_id,
                image_rect,
                uv,
                egui::Color32::from_white_alpha(alpha),
            );
        }
        if response.has_focus() {
            painter.rect_stroke(
                rect.shrink(1.),
                0.,
                egui::Stroke::new(2., ui.visuals().hyperlink_color),
                egui::StrokeKind::Inside,
            );
        }
        Ok(CameraPresentation::Presented)
    }
}
