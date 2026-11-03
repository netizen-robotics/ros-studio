use super::{camera3d::CameraPose, gpu::Renderer3d, Point3, Scene};
use eframe::egui;
pub use studio_workspace::presentation::CameraMode;
/// Shared orbit/orthographic camera. Legacy projection fields remain for source
/// compatibility; live rendering uses GPU depth rather than the old 2D painter.
#[derive(Clone, Debug)]
pub struct Camera {
    pub mode: CameraMode,
    pub vertical_span: f32,
    pub azimuth: f32,
    pub elevation: f32,
    pub pixels_per_meter: f32,
    pub pan: [f32; 2],
    pub target: [f64; 3],
    pub distance: f32,
}
impl Default for Camera {
    fn default() -> Self {
        Self {
            mode: CameraMode::Orbit,
            vertical_span: 12.0,
            azimuth: 0.5,
            elevation: 0.8,
            pixels_per_meter: 50.0,
            pan: [0.0; 2],
            target: [0.0; 3],
            distance: 12.0,
        }
    }
}
impl Camera {
    /// Legacy mathematical preview only; not used for presented geometry.
    pub fn project(&self, point: Point3, center: egui::Pos2) -> egui::Pos2 {
        let [x, y, z] = point.0.map(|v| v as f32);
        let (sa, ca) = self.azimuth.sin_cos();
        let (se, ce) = self.elevation.sin_cos();
        egui::pos2(
            center.x + self.pan[0] + (-sa * x + ca * y) * self.pixels_per_meter,
            center.y + self.pan[1] - (-se * (ca * x + sa * y) + ce * z) * self.pixels_per_meter,
        )
    }
    pub fn reset(&mut self) {
        *self = Self::default();
    }
    pub fn pose(&self) -> CameraPose {
        match self.mode {
            CameraMode::Orbit => CameraPose::orbit(
                self.azimuth,
                self.elevation,
                self.distance,
                self.target.map(|v| v as f32),
            ),
            CameraMode::TopDown => {
                CameraPose::top_down(self.vertical_span, self.target.map(|v| v as f32))
            }
        }
    }
    pub fn orbit_delta(&mut self, delta: [f32; 2]) {
        self.azimuth += delta[0] * 0.01;
        self.elevation -= delta[1] * 0.01;
        self.clamp();
    }
    pub fn pan_delta(&mut self, delta: [f32; 2]) {
        self.clamp();
        let pose = self.pose();
        let scale = if self.mode == CameraMode::TopDown {
            self.vertical_span
        } else {
            self.distance
        } * 0.002;
        for i in 0..3 {
            self.target[i] +=
                f64::from((-pose.right[i] * delta[0] + pose.up[i] * delta[1]) * scale);
        }
        self.clamp();
    }
    pub fn zoom(&mut self, factor: f32) {
        if factor.is_finite() && factor > 0.0 {
            if self.mode == CameraMode::TopDown {
                self.vertical_span /= factor;
            } else {
                self.distance /= factor;
            }
        }
        self.clamp();
    }
    fn clamp(&mut self) {
        let finite = |v: f32, default| if v.is_finite() { v } else { default };
        self.azimuth = finite(self.azimuth, 0.5).rem_euclid(std::f32::consts::TAU);
        self.elevation = finite(self.elevation, 0.8).clamp(-1.5, 1.5);
        self.distance = finite(self.distance, 12.0).clamp(0.2, 2000.0);
        self.vertical_span = finite(self.vertical_span, 12.0).clamp(0.2, 2000.0);
        self.pixels_per_meter = finite(self.pixels_per_meter, 50.0).clamp(5.0, 500.0);
        for p in &mut self.pan {
            *p = finite(*p, 0.0).clamp(-10000.0, 10000.0);
        }
        for t in &mut self.target {
            *t = if t.is_finite() {
                t.clamp(-1_000_000.0, 1_000_000.0)
            } else {
                0.
            };
        }
    }
    pub fn show(&mut self, ui: &mut egui::Ui, scene: &Scene) {
        self.show_inner(ui, scene, None);
    }
    pub fn show_live(&mut self, ui: &mut egui::Ui, scene: &Scene) {
        self.show_inner(ui, scene, None);
    }
    pub fn show_live_gpu(&mut self, ui: &mut egui::Ui, scene: &Scene, renderer: &mut Renderer3d) {
        self.show_inner(ui, scene, Some(renderer));
    }
    fn show_inner(&mut self, ui: &mut egui::Ui, scene: &Scene, renderer: Option<&mut Renderer3d>) {
        self.clamp();
        ui.label("Spatial scene · perspective 3D");
        ui.add(
            egui::Label::new(format!(
                "Fixed frame: {} · {}",
                scene.fixed_frame().unwrap_or("choose a frame"),
                if scene.is_active() {
                    "Active"
                } else {
                    "Stopped"
                }
            ))
            .selectable(true)
            .wrap(),
        );
        ui.horizontal_wrapped(|ui| {
            if ui.button("Zoom in").clicked() {
                self.zoom(1.25);
            }
            if ui.button("Zoom out").clicked() {
                self.zoom(0.8);
            }
            if ui.button("Reset camera").clicked() {
                self.reset();
            }
            ui.small("Drag: orbit · Right-drag: pan · X red / Y green / Z blue");
        });
        // Dynamic TF/status rows must consume a fixed parent ID slot. Otherwise
        // camera widget IDs change when a missing-transform message appears.
        ui.push_id("spatial-status", |ui| {
            for layer in scene.layers() {
                let text = match layer.status() {
                    super::LayerStatus::Error(error) => error.to_string(),
                    super::LayerStatus::Waiting => "Waiting for scan and TF".to_owned(),
                    super::LayerStatus::Receiving => "Receiving scan · transform ready".to_owned(),
                    super::LayerStatus::Stopped => "Stopped".to_owned(),
                };
                let text = if layer.visible {
                    text
                } else {
                    format!("Hidden · {text}")
                };
                let color = if matches!(layer.status(), super::LayerStatus::Error(_)) {
                    ui.visuals().error_fg_color
                } else {
                    ui.visuals().text_color()
                };
                // Every layer always occupies one text row, independent of TF status.
                // Truncation preserves the full diagnostic in AX and its hover text.
                ui.add_sized(
                    [
                        ui.available_width(),
                        ui.text_style_height(&egui::TextStyle::Body),
                    ],
                    egui::Label::new(egui::RichText::new(&text).color(color)).truncate(),
                )
                .on_hover_text(&text);
            }
        });
        let size = egui::vec2(
            ui.available_width().max(1.0),
            (ui.available_height() - 32.0).clamp(120.0, 700.0),
        );
        let (_, rect) = ui.allocate_space(size);
        let response = ui.interact(
            rect,
            ui.make_persistent_id("spatial-viewport"),
            egui::Sense::drag(),
        );
        let painter = ui.painter_at(rect);
        response.widget_info(||egui::WidgetInfo::labeled(egui::WidgetType::Other,ui.is_enabled(),
            format!("Perspective 3D spatial scene. Fixed frame: {}. {} visible layers. Drag to orbit; use Camera details below for keyboard camera controls.",
                scene.fixed_frame().unwrap_or("not selected"),scene.layers().iter().filter(|l|l.visible).count())));
        if response.dragged() {
            let d = ui.input(|i| i.pointer.delta());
            if response.dragged_by(egui::PointerButton::Secondary) {
                self.pan_delta([d.x, d.y]);
            } else {
                self.orbit_delta([d.x, d.y]);
            }
        }
        if let Some(renderer) = renderer {
            let scale = ui.ctx().pixels_per_point();
            let dimensions = [
                (size.x * scale).round() as u32,
                (size.y * scale).round() as u32,
            ];
            let id = renderer.render_scene(dimensions, scene, self.pose(), ui.visuals());
            painter.image(
                id,
                response.rect,
                egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                egui::Color32::WHITE,
            );
        } else {
            painter.rect_filled(response.rect, 0.0, ui.visuals().extreme_bg_color);
            painter.text(
                response.rect.center(),
                egui::Align2::CENTER_CENTER,
                "3D GPU presentation unavailable\nRun the native wgpu desktop to view the scene.",
                egui::FontId::proportional(14.0),
                ui.visuals().text_color(),
            );
        }
        if response.has_focus() {
            painter.rect_stroke(
                response.rect.shrink(1.0),
                0.0,
                egui::Stroke::new(2.0, ui.visuals().hyperlink_color),
                egui::StrokeKind::Inside,
            );
        }
        egui::CollapsingHeader::new("Camera details")
            .id_salt("spatial-camera-details")
            .show(ui, |ui| {
                ui.add(studio_ui_foundation::theme::slider(
                    egui::Slider::new(&mut self.azimuth, 0.0..=std::f32::consts::TAU)
                        .text("Orbit (rad)"),
                ));
                ui.add(studio_ui_foundation::theme::slider(
                    egui::Slider::new(&mut self.elevation, -1.5..=1.5).text("Elevation (rad)"),
                ));
                ui.add(studio_ui_foundation::theme::slider(
                    egui::Slider::new(&mut self.distance, 0.2..=2000.0)
                        .logarithmic(true)
                        .text("Distance (m)"),
                ));
                for (i, label) in ["Target X (m)", "Target Y (m)", "Target Z (m)"]
                    .into_iter()
                    .enumerate()
                {
                    ui.horizontal(|ui| {
                        ui.label(label);
                        ui.add(
                            egui::DragValue::new(&mut self.target[i])
                                .speed(0.1)
                                .range(-1_000_000.0..=1_000_000.0),
                        );
                    });
                }
                for layer in scene.layers() {
                    let text = format!(
                        "Layer {}: {:?} · {} points · {} rays skipped{}",
                        layer.id,
                        layer.status(),
                        layer.points().len(),
                        layer.skipped_rays,
                        if layer.visible { "" } else { " · Hidden" }
                    );
                    ui.add(egui::Label::new(&text).truncate())
                        .on_hover_text(&text);
                }
            });
        self.clamp();
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn camera_handles_nonfinite_controls_and_reset() {
        let mut camera = Camera {
            azimuth: f32::NAN,
            distance: f32::INFINITY,
            target: [f64::NAN; 3],
            ..Default::default()
        };
        camera.orbit_delta([f32::INFINITY, f32::NAN]);
        camera.pan_delta([f32::INFINITY, 0.0]);
        camera.zoom(0.0);
        assert!(camera.pose().eye.iter().all(|x| x.is_finite()));
        camera.reset();
        assert_eq!(camera.target, [0.0; 3]);
        assert_eq!(camera.distance, 12.0);
    }
}
#[cfg(test)]
mod accessibility_tests {
    use super::*;
    #[test]
    fn scene_focus_is_named_and_paints_an_outline_without_moving_camera() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut camera = Camera::default();
        let mut scene = Scene::default();
        scene.set_fixed_frame("odom").unwrap();
        scene.add_layer(1).unwrap();
        let draw = |camera: &mut Camera, event: Option<egui::Event>| {
            context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(640.0, 600.0),
                    )),
                    events: event.into_iter().collect(),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| camera.show_live(ui, &scene));
                },
            )
        };
        let output = draw(&mut camera, None);
        let target = output
            .platform_output
            .accesskit_update
            .unwrap()
            .nodes
            .iter()
            .find(|(_, node)| {
                node.label().is_some_and(|label| {
                    label.starts_with("Perspective 3D spatial scene. Fixed frame: odom.")
                })
            })
            .expect("Focusable scene needs an accessible name")
            .0;
        let before = (
            camera.azimuth,
            camera.elevation,
            camera.target,
            camera.distance,
        );
        let focused = draw(
            &mut camera,
            Some(egui::Event::AccessKitActionRequest(
                egui::accesskit::ActionRequest {
                    action: egui::accesskit::Action::Focus,
                    target,
                    data: None,
                },
            )),
        );
        assert!(focused.shapes.iter().any(|shape|matches!(&shape.shape,egui::Shape::Rect(rect) if rect.stroke.width==2.0 && rect.stroke.color==context.style().visuals.hyperlink_color)));
        assert_eq!(
            (
                camera.azimuth,
                camera.elevation,
                camera.target,
                camera.distance
            ),
            before
        );
    }
}

#[cfg(test)]
mod stable_camera_tests {
    use super::*;
    use crate::spatial::{Clock, RigidTransform, Scan, TfOutcome};
    #[test]
    fn tf_status_rows_preserve_camera_expansion_focus_and_activation() {
        let context = egui::Context::default();
        context.enable_accesskit();
        context.style_mut(|style| style.animation_time = 0.0);
        let mut camera = Camera::default();
        let mut scene = Scene::default();
        scene.set_fixed_frame("odom").unwrap();
        scene.add_layer(1).unwrap();
        scene.start().unwrap();
        let token = scene.layer_token(1).unwrap();
        let draw = |camera: &mut Camera, scene: &Scene, event: Option<egui::Event>| {
            context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(900.0, 1400.0),
                    )),
                    events: event.into_iter().collect(),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        camera.show_live(ui, scene);
                    });
                },
            )
        };
        let initial = draw(&mut camera, &scene, None);
        let header = initial
            .platform_output
            .accesskit_update
            .unwrap()
            .nodes
            .iter()
            .find(|(_, node)| node.label() == Some("Camera details"))
            .unwrap()
            .0;
        let click = egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
            action: egui::accesskit::Action::Click,
            target: header,
            data: None,
        });
        draw(&mut camera, &scene, Some(click));
        let output = draw(&mut camera, &scene, None);
        let orbit = output
            .platform_output
            .accesskit_update
            .unwrap()
            .nodes
            .iter()
            .find(|(_, node)| {
                node.label() == Some("Orbit (rad)") && node.role() == egui::accesskit::Role::Slider
            })
            .expect("camera details expanded")
            .0;
        let action = egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
            action: egui::accesskit::Action::Focus,
            target: orbit,
            data: None,
        });
        let focused = draw(&mut camera, &scene, Some(action));
        assert_eq!(
            focused.platform_output.accesskit_update.unwrap().focus,
            orbit
        );
        let before = (
            camera.azimuth,
            camera.elevation,
            camera.target,
            camera.distance,
        );
        for (stamp, missing) in [(1, true), (2, false), (3, true), (4, false)] {
            let scan = Scan {
                frame: "laser",
                stamp_ns: stamp,
                clock: Clock::System,
                complete: true,
                angle_min: 0.0,
                angle_increment: 0.1,
                range_min: 0.1,
                range_max: 10.0,
                ranges: &[1.0],
            };
            let tf = if missing {
                TfOutcome::Missing
            } else {
                TfOutcome::Ready {
                    source: "laser",
                    target: "odom",
                    stamp_ns: stamp,
                    transform: RigidTransform::new([0.0; 3], [0.0, 0.0, 0.0, 1.0]).unwrap(),
                }
            };
            let result = scene.update(token, scan, tf);
            assert_eq!(result.is_err(), missing);
            let output = draw(&mut camera, &scene, None);
            let tree = output.platform_output.accesskit_update.unwrap();
            assert!(
                tree.nodes.iter().any(|(id, node)| *id == orbit
                    && node.label() == Some("Orbit (rad)")
                    && node.role() == egui::accesskit::Role::Slider),
                "same expanded Orbit control survives TF transition"
            );
            assert_eq!(
                tree.focus, orbit,
                "TF diagnostic must not steal camera focus"
            );
            assert!(scene.is_active(), "presentation must not change activation");
        }
        assert_eq!(
            (
                camera.azimuth,
                camera.elevation,
                camera.target,
                camera.distance
            ),
            before
        );
        scene.stop();
        draw(&mut camera, &scene, None);
        assert!(
            !scene.is_active(),
            "render must not reactivate a stopped scene"
        );
    }
}

#[cfg(test)]
mod viewport_geometry_tests {
    use super::*;
    use crate::spatial::{Clock, RigidTransform, Scan, TfOutcome};
    #[test]
    fn viewport_bounds_do_not_jitter_with_live_layer_diagnostics() {
        for dimensions in [egui::vec2(1280.0, 800.0), egui::vec2(360.0, 640.0)] {
            let context = egui::Context::default();
            context.enable_accesskit();
            context.style_mut(|style| style.animation_time = 0.0);
            let mut camera = Camera::default();
            let mut scene = Scene::default();
            scene.set_fixed_frame("odom").unwrap();
            scene.add_layer(1).unwrap();
            scene.start().unwrap();
            let token = scene.layer_token(1).unwrap();
            let draw = |camera: &mut Camera, scene: &Scene| {
                let output = context.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, dimensions)),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| camera.show_live(ui, scene));
                    },
                );
                output
                    .platform_output
                    .accesskit_update
                    .unwrap()
                    .nodes
                    .iter()
                    .find(|(_, node)| {
                        node.label()
                            .is_some_and(|label| label.starts_with("Perspective 3D spatial scene."))
                    })
                    .unwrap()
                    .1
                    .bounds()
                    .unwrap()
            };
            draw(&mut camera, &scene);
            let expected = draw(&mut camera, &scene);
            for (stamp, tf) in [
                (1, TfOutcome::Missing),
                (
                    2,
                    TfOutcome::Ready {
                        source: "laser",
                        target: "odom",
                        stamp_ns: 2,
                        transform: RigidTransform::new([0.0; 3], [0.0, 0.0, 0.0, 1.0]).unwrap(),
                    },
                ),
                (3, TfOutcome::Extrapolated),
                (4, TfOutcome::Stale),
                (5, TfOutcome::InvalidGraph),
                (6, TfOutcome::OutOfOrder),
                (
                    7,
                    TfOutcome::Ready {
                        source: "wrong-source",
                        target: "odom",
                        stamp_ns: 7,
                        transform: RigidTransform::new([0.0; 3], [0.0, 0.0, 0.0, 1.0]).unwrap(),
                    },
                ),
            ] {
                let _ = scene.update(
                    token,
                    Scan {
                        frame: "laser",
                        stamp_ns: stamp,
                        clock: Clock::System,
                        complete: true,
                        angle_min: 0.0,
                        angle_increment: 0.1,
                        range_min: 0.1,
                        range_max: 10.0,
                        ranges: &[1.0],
                    },
                    tf,
                );
                assert_eq!(
                    draw(&mut camera, &scene),
                    expected,
                    "live diagnostic moved viewport at {dimensions:?}, stamp {stamp}"
                );
            }
            scene.stop();
            assert_eq!(
                draw(&mut camera, &scene),
                expected,
                "stopped layer moved viewport"
            );
            scene.start().unwrap();
            assert_eq!(
                draw(&mut camera, &scene),
                expected,
                "waiting layer moved viewport"
            );
        }
    }
}
