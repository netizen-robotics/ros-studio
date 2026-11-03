//! One App-owned camera and viewport for the combined immutable scene. This
//! presentation layer performs no activation, TF/FK or configuration mutation.
use super::{
    combined::{CombinedSceneError, CombinedSceneFrame},
    Camera, CameraMode, Renderer3d,
};
use eframe::egui;
#[derive(Clone, Copy)]
pub struct MarkerPresentation<'a> {
    pub camera: super::CameraPose,
    pub size: [u32; 2],
    pub layer: super::marker_primitives::MarkerLayer<'a>,
    pub bounds: Option<super::camera_fit::Bounds>,
    pub selected_bounds: Option<super::camera_fit::Bounds>,
}
#[derive(Clone, Copy, Default)]
pub struct SceneOverlays<'a> {
    pub maps: &'a [super::map_texture::TextureMapFrame<'a>],
    pub markers: Option<MarkerPresentation<'a>>,
    pub uncertainty: &'a [super::uncertainty::Layer<'a>],
}
#[derive(Default)]
pub struct SharedSceneView {
    pub camera: Camera,
    pub selected: Option<u64>,
    pub robot_id: Option<u64>,
    pub saved_views: Vec<studio_workspace::presentation::SavedView>,
    /// Desired camera/physical viewport for off-thread Marker projection.
    pub marker_request: Option<(super::CameraPose, [u32; 2])>,
    enhanced: bool,
    pub external_controls: bool,
    fit_requested: bool,
    focus_requested: bool,
    view_name: String,
    message: Option<String>,
    drag: Option<(egui::PointerButton, egui::Pos2)>,
}
fn row(ui: &mut egui::Ui, text: &str, error: bool) {
    let color = if error {
        ui.visuals().error_fg_color
    } else {
        ui.visuals().text_color()
    };
    ui.add_sized(
        [
            ui.available_width(),
            ui.text_style_height(&egui::TextStyle::Body),
        ],
        egui::Label::new(
            egui::RichText::new(if text.is_empty() { " " } else { text }).color(color),
        )
        .truncate(),
    )
    .on_hover_text(text);
}
impl SharedSceneView {
    /// Consume the ordered pointer events, including move+release in one frame.
    /// Response::dragged_by can be false after that release, losing the entire
    /// short gesture. Respect clipping and the topmost UI layer before capture.
    fn pointer_input(&mut self, ui: &egui::Ui, rect: egui::Rect, response: &egui::Response) {
        if !ui.is_enabled() {
            self.drag = None;
            return;
        }
        let events = ui.input(|i| i.events.clone());
        if let Some((button, _)) = self.drag {
            if !ui.input(|i|i.pointer.button_down(button)) && !events.iter().any(|e|matches!(e,egui::Event::PointerButton{button:b,pressed:false,..} if *b==button)) { self.drag=None; }
        }
        for event in events {
            match event {
                egui::Event::PointerButton {
                    pos,
                    button,
                    pressed: true,
                    ..
                } if matches!(
                    button,
                    egui::PointerButton::Primary | egui::PointerButton::Secondary
                ) && ui.is_enabled()
                    && rect.intersect(ui.clip_rect()).contains(pos)
                    && ui.ctx().layer_id_at(pos) == Some(ui.layer_id()) =>
                {
                    self.drag = Some((button, pos));
                    response.request_focus();
                }
                egui::Event::PointerMoved(pos) => {
                    if let Some((button, previous)) = self.drag {
                        self.drag_delta(ui, button, pos - previous);
                        self.drag = Some((button, pos));
                    }
                }
                egui::Event::PointerButton {
                    pos,
                    button,
                    pressed: false,
                    ..
                } => {
                    if let Some((captured, previous)) = self.drag {
                        if captured == button {
                            self.drag_delta(ui, button, pos - previous);
                            self.drag = None;
                        }
                    }
                }
                egui::Event::PointerGone => self.drag = None,
                _ => (),
            }
        }
    }
    fn drag_delta(&mut self, ui: &egui::Ui, button: egui::PointerButton, delta: egui::Vec2) {
        if button == egui::PointerButton::Secondary
            || self.camera.mode == CameraMode::TopDown
            || ui.input(|i| i.modifiers.shift)
        {
            self.camera.pan_delta([delta.x, delta.y]);
        } else {
            self.camera.orbit_delta([delta.x, delta.y]);
        }
    }
    pub fn visualizer() -> Self {
        Self {
            enhanced: true,
            ..Default::default()
        }
    }
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        frame: &CombinedSceneFrame<'_>,
        status: &str,
        error: Option<&str>,
        renderer: Option<&mut Renderer3d>,
    ) -> Result<(), CombinedSceneError> {
        self.show_with_map(ui, frame, status, error, renderer, None)
    }
    /// The active owner supplies a worker-prepared texture map. This view never
    /// starts reception or decodes source cells while fitting/rendering.
    pub fn show_with_map(
        &mut self,
        ui: &mut egui::Ui,
        frame: &CombinedSceneFrame<'_>,
        status: &str,
        error: Option<&str>,
        renderer: Option<&mut Renderer3d>,
        map: Option<super::map_texture::TextureMapFrame<'_>>,
    ) -> Result<(), CombinedSceneError> {
        self.show_with_overlays(
            ui,
            frame,
            status,
            error,
            renderer,
            SceneOverlays {
                maps: map.as_slice(),
                markers: None,
                uncertainty: &[],
            },
        )
    }
    pub fn show_with_overlays(
        &mut self,
        ui: &mut egui::Ui,
        frame: &CombinedSceneFrame<'_>,
        status: &str,
        error: Option<&str>,
        renderer: Option<&mut Renderer3d>,
        overlays: SceneOverlays<'_>,
    ) -> Result<(), CombinedSceneError> {
        for map in overlays.maps {
            map.validate_scene(frame)?;
        }
        ui.push_id("shared-scene-view", |ui| {
            self.show_inner(ui, frame, status, error, renderer, overlays)
        })
        .inner
    }
    fn toolbar(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            if self.enhanced {
                ui.selectable_value(&mut self.camera.mode, CameraMode::Orbit, "Orbit");
                ui.selectable_value(&mut self.camera.mode, CameraMode::TopDown, "Top down");
                self.fit_requested |= ui
                    .button("Fit visible")
                    .on_hover_text("Fit available geometry to this viewport (F)")
                    .clicked();
                self.focus_requested |= ui
                    .button("Focus selected")
                    .on_hover_text("Fit the selected display's geometry (Shift+F)")
                    .clicked();
            }
            if ui.button("Zoom in").clicked() {
                self.camera.zoom(1.25);
            }
            if ui.button("Zoom out").clicked() {
                self.camera.zoom(0.8);
            }
            if ui.button("Reset camera").clicked() {
                self.camera.reset();
            }
            ui.small(if self.enhanced {
                "Drag: orbit / top-down pan · Right-drag: pan · Scroll: zoom"
            } else {
                "Drag: orbit · Right-drag: pan · X red / Y green / Z blue"
            });
        });
    }
    pub fn controls_ui(&mut self, ui: &mut egui::Ui) {
        if self.enhanced {
            self.view_controls(ui);
            return;
        }
        self.toolbar(ui);
        self.camera_details(ui);
    }
    /// Keep primary controls compact. Secondary settings never displace content.
    pub fn compact_controls_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let label = ui.strong("Views");
            egui::ComboBox::from_id_salt("compact-camera-mode")
                .width(86.0)
                .selected_text(match self.camera.mode {
                    CameraMode::Orbit => "Orbit",
                    CameraMode::TopDown => "Top down",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.camera.mode, CameraMode::Orbit, "Orbit");
                    ui.selectable_value(&mut self.camera.mode, CameraMode::TopDown, "Top down");
                })
                .response
                .labelled_by(label.id);
            let value = match self.camera.mode {
                CameraMode::Orbit => &mut self.camera.distance,
                CameraMode::TopDown => &mut self.camera.vertical_span,
            };
            ui.spacing_mut().slider_width = 70.0;
            let distance = *value;
            ui.add(studio_ui_foundation::theme::slider(egui::Slider::new(value, 0.2..=2000.0).logarithmic(true).show_value(false).text("Zoom")))
                .on_hover_text(
                    format!("Distance / height: {distance:.1} m. Drag to zoom; open Camera… for precise values."),
                )
                .widget_info(|| {
                    egui::WidgetInfo::labeled(egui::WidgetType::Slider, ui.is_enabled(), "Zoom (m)")
                });
            ui.menu_button("Camera…", |ui| {
                self.fit_requested |= ui.button("Fit visible").clicked();
                self.focus_requested |= ui
                    .add_enabled(self.selected.is_some(), egui::Button::new("Focus selected"))
                    .clicked();
                if ui.button("Reset camera").clicked() {
                    self.camera.reset();
                }
                ui.separator();
                ui.set_width(270.0);
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                ui.spacing_mut().slider_width = 100.0;
                egui::ScrollArea::vertical()
                    .max_height(300.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.camera_details(ui));
            });
        });
    }

    fn view_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let label = ui.label("Mode");
            egui::ComboBox::from_id_salt("camera-mode")
                .selected_text(match self.camera.mode {
                    CameraMode::Orbit => "Orbit",
                    CameraMode::TopDown => "Top down",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.camera.mode, CameraMode::Orbit, "Orbit");
                    ui.selectable_value(&mut self.camera.mode, CameraMode::TopDown, "Top down");
                })
                .response
                .labelled_by(label.id);
            ui.menu_button("Camera…", |ui| {
                ui.set_width(270.0);
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Wrap);
                ui.spacing_mut().slider_width = 100.0;
                egui::ScrollArea::vertical()
                    .max_height(320.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.camera_details(ui));
            });
        });
        ui.horizontal(|ui| {
            let (value, caption) = match self.camera.mode {
                CameraMode::Orbit => (&mut self.camera.distance, "Distance (m)"),
                CameraMode::TopDown => (&mut self.camera.vertical_span, "Height (m)"),
            };
            ui.spacing_mut().slider_width = (ui.available_width() - 160.0).max(60.0);
            ui.add(studio_ui_foundation::theme::slider(
                egui::Slider::new(value, 0.2..=2000.0)
                    .logarithmic(true)
                    .text(caption),
            ))
            .on_hover_text("Drag to zoom, or edit the value. Scroll over the scene to zoom.");
        });
        ui.horizontal_wrapped(|ui| {
            self.fit_requested |= ui
                .button("Fit visible")
                .on_hover_text("Fit available geometry (F)")
                .clicked();
            self.focus_requested |= ui
                .add_enabled(self.selected.is_some(), egui::Button::new("Focus selected"))
                .on_hover_text("Fit the selected display (Shift+F)")
                .clicked();
            if ui
                .button("Reset")
                .on_hover_text("Reset camera (R)")
                .clicked()
            {
                self.camera.reset();
            }
        });
    }
    fn camera_details(&mut self, ui: &mut egui::Ui) {
        egui::CollapsingHeader::new("Camera details")
            .id_salt("shared-camera-details")
            .show(ui, |ui| {
                ui.add(studio_ui_foundation::theme::slider(
                    egui::Slider::new(&mut self.camera.azimuth, 0.0..=std::f32::consts::TAU)
                        .text("Orbit (rad)")),);
                if self.enhanced {
                    ui.add(studio_ui_foundation::theme::slider(egui::Slider::new(&mut self.camera.vertical_span,0.2..=2000.).logarithmic(true).text("Top-down height (m)")));
                    ui.small("Viewport keyboard: O Orbit, T Top down, F Fit, Shift+F Focus, R Reset, +/- Zoom, arrows Orbit/Pan, Shift+arrows Pan.");
                }
                ui.add(studio_ui_foundation::theme::slider(
                    egui::Slider::new(&mut self.camera.elevation, -1.5..=1.5)
                        .text("Elevation (rad)")),);
                ui.add(studio_ui_foundation::theme::slider(
                    egui::Slider::new(&mut self.camera.distance, 0.2..=2000.0)
                        .logarithmic(true)
                        .text("Distance (m)")),);
                for (i, label) in ["Target X (m)", "Target Y (m)", "Target Z (m)"]
                    .into_iter()
                    .enumerate()
                {
                    ui.horizontal(|ui| {
                        let caption = ui.label(label);
                        ui.add(
                            egui::DragValue::new(&mut self.camera.target[i])
                                .speed(0.1)
                                .range(-1_000_000.0..=1_000_000.0),
                        ).labelled_by(caption.id);
                    });
                }
            });
        if self.enhanced {
            ui.collapsing("Saved camera views", |ui| {
                let caption = ui.label("View name");
                ui.add(
                    studio_ui_foundation::theme::input(ui, &mut self.view_name)
                        .desired_width(f32::INFINITY)
                        .char_limit(64),
                )
                .labelled_by(caption.id);
                let valid = !self.view_name.trim().is_empty()
                    && self.view_name.len() <= 64
                    && !self.view_name.chars().any(char::is_control);
                let existing = self
                    .saved_views
                    .iter()
                    .position(|v| v.name == self.view_name);
                if ui
                    .add_enabled(
                        valid && (existing.is_some() || self.saved_views.len() < 8),
                        egui::Button::new(if existing.is_some() {
                            "Replace saved view"
                        } else {
                            "Save camera view"
                        }),
                    )
                    .clicked()
                {
                    let saved = studio_workspace::presentation::SavedView {
                        name: self.view_name.clone(),
                        camera: self.camera.snapshot(),
                    };
                    if let Some(index) = existing {
                        self.saved_views[index] = saved;
                    } else {
                        self.saved_views.push(saved);
                    }
                }
                let mut remove = None;
                for (index, view) in self.saved_views.iter().enumerate() {
                    ui.push_id(index, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            if ui.button(format!("Restore {}", view.name)).clicked() {
                                self.message = self.camera.restore(&view.camera).err();
                            }
                            if ui.button("Remove saved view").clicked() {
                                remove = Some(index);
                            }
                        });
                    });
                }
                if let Some(index) = remove {
                    self.saved_views.remove(index);
                }
                ui.small("At most eight named views. Save the workspace to retain them.");
            });
        }
    }
    fn show_inner(
        &mut self,
        ui: &mut egui::Ui,
        frame: &CombinedSceneFrame<'_>,
        status: &str,
        error: Option<&str>,
        renderer: Option<&mut Renderer3d>,
        overlays: SceneOverlays<'_>,
    ) -> Result<(), CombinedSceneError> {
        let maps = overlays.maps;
        self.camera.zoom(1.0);
        let mut fit = std::mem::take(&mut self.fit_requested);
        let mut focus = std::mem::take(&mut self.focus_requested);
        if !self.external_controls {
            ui.label("Spatial scene · shared 3D");
            row(ui, &format!("Fixed frame: {}", frame.fixed_frame), false);
            row(ui, status, false);
            self.toolbar(ui);
            fit |= std::mem::take(&mut self.fit_requested);
            focus |= std::mem::take(&mut self.focus_requested);
        }
        let size = egui::vec2(
            ui.available_width().max(1.0),
            if self.external_controls {
                (ui.available_height() - 22.0).max(1.0)
            } else {
                (ui.available_height() - 64.0).clamp(120.0, 700.0)
            },
        );
        let (_, rect) = ui.allocate_space(size);
        let response = ui.interact(
            rect,
            ui.make_persistent_id("shared-scene-viewport"),
            if self.enhanced {
                egui::Sense::click_and_drag()
            } else {
                egui::Sense::drag()
            },
        );
        if self.enhanced && response.clicked() {
            response.request_focus();
        }
        response.widget_info(||egui::WidgetInfo::labeled(egui::WidgetType::Other,ui.is_enabled(),
            format!("Combined 3D scene. Fixed frame: {}. {} sensor layers. Map {}, Pose {}, Path {}. Drag to orbit; use Camera details for keyboard controls.",frame.fixed_frame,frame.layers.len(),(frame.map.is_some() || !maps.is_empty()),frame.pose.is_some(),frame.path.is_some())));
        if self.enhanced {
            self.pointer_input(ui, rect, &response);
        } else if response.dragged_by(egui::PointerButton::Secondary) {
            let d = ui.input(|i| i.pointer.delta());
            self.camera.pan_delta([d.x, d.y]);
        } else if response.dragged_by(egui::PointerButton::Primary) {
            let d = ui.input(|i| i.pointer.delta());
            if self.enhanced
                && (self.camera.mode == CameraMode::TopDown || ui.input(|i| i.modifiers.shift))
            {
                self.camera.pan_delta([d.x, d.y]);
            } else {
                self.camera.orbit_delta([d.x, d.y]);
            }
        }
        if self.enhanced && ui.is_enabled() && response.hovered() {
            let (scroll, pinch) = ui.input(|i| (i.smooth_scroll_delta.y, i.zoom_delta()));
            if scroll != 0. {
                self.camera.zoom((scroll * 0.005).clamp(-1., 1.).exp());
                ui.input_mut(|i| i.smooth_scroll_delta = egui::Vec2::ZERO);
            }
            if pinch != 1. {
                self.camera.zoom(pinch);
            }
        }
        if self.enhanced && ui.is_enabled() && response.has_focus() {
            use egui::{Key, Modifiers};
            ui.input_mut(|i| {
                if i.consume_key(Modifiers::NONE, Key::O) {
                    self.camera.mode = CameraMode::Orbit;
                }
                if i.consume_key(Modifiers::NONE, Key::T) {
                    self.camera.mode = CameraMode::TopDown;
                }
                if i.consume_key(Modifiers::SHIFT, Key::F) {
                    focus = true;
                } else if i.consume_key(Modifiers::NONE, Key::F) {
                    fit = true;
                }
                if i.consume_key(Modifiers::NONE, Key::R) {
                    self.camera.reset();
                }
                if i.consume_key(Modifiers::NONE, Key::Plus)
                    || i.consume_key(Modifiers::NONE, Key::Equals)
                {
                    self.camera.zoom(1.25);
                }
                if i.consume_key(Modifiers::NONE, Key::Minus) {
                    self.camera.zoom(0.8);
                }
                for (key, delta) in [
                    (Key::ArrowLeft, [-12., 0.]),
                    (Key::ArrowRight, [12., 0.]),
                    (Key::ArrowUp, [0., -12.]),
                    (Key::ArrowDown, [0., 12.]),
                ] {
                    if i.consume_key(Modifiers::SHIFT, key) {
                        self.camera.pan_delta(delta);
                    } else if i.consume_key(Modifiers::NONE, key) {
                        if self.camera.mode == CameraMode::TopDown {
                            self.camera.pan_delta(delta);
                        } else {
                            self.camera.orbit_delta(delta);
                        }
                    }
                }
            });
        }
        if self.enhanced && (fit || focus) {
            let mut bounds = if focus && self.selected.is_none() {
                None
            } else {
                super::camera_fit::visible_bounds_with_maps(
                    frame,
                    if focus { self.selected } else { None },
                    self.robot_id,
                    maps,
                )
            };
            let marker_bounds =
                overlays
                    .markers
                    .and_then(|m| if focus { m.selected_bounds } else { m.bounds });
            if let Some(b) = marker_bounds {
                if let Some(all) = &mut bounds {
                    for i in 0..3 {
                        all.min[i] = all.min[i].min(b.min[i]);
                        all.max[i] = all.max[i].max(b.max[i]);
                    }
                } else {
                    bounds = Some(b);
                }
            }
            for layer in overlays
                .uncertainty
                .iter()
                .filter(|layer| !focus || self.selected == Some(layer.id))
            {
                if let Some(b) = layer.bounds() {
                    if let Some(all) = &mut bounds {
                        for i in 0..3 {
                            all.min[i] = all.min[i].min(b.min[i]);
                            all.max[i] = all.max[i].max(b.max[i]);
                        }
                    } else {
                        bounds = Some(b);
                    }
                }
            }
            self.message = self.camera.fit(bounds, size.x / size.y).err();
        }
        let painter = ui.painter_at(rect);
        let mut result = Ok(());
        let mut presented = false;
        let scale = ui.ctx().pixels_per_point();
        let desired_size = [
            (size.x * scale).round() as u32,
            (size.y * scale).round() as u32,
        ];
        self.marker_request = Some((self.camera.pose(), desired_size));
        let mut presented_projection = (rect, self.camera.pose());
        if let Some(renderer) = renderer {
            let presented_markers = overlays.markers.filter(|m| {
                m.layer.frame.has_visible(m.layer.permit) && !m.layer.frame.all_expired()
            });
            let (camera, render_size) = presented_markers
                .map_or((self.camera.pose(), desired_size), |m| (m.camera, m.size));
            let draw_rect = if presented_markers.is_some() {
                let aspect = render_size[0] as f32 / render_size[1].max(1) as f32;
                let fitted = egui::vec2(size.x.min(size.y * aspect), size.y.min(size.x / aspect));
                painter.rect_filled(rect, 0., ui.visuals().extreme_bg_color);
                egui::Rect::from_center_size(rect.center(), fitted)
            } else {
                rect
            };
            presented_projection = (draw_rect, camera);
            match renderer.render_combined_with_maps(
                render_size,
                frame,
                camera,
                ui.visuals(),
                maps,
                overlays.markers.map(|m| m.layer),
                overlays.uncertainty,
            ) {
                Ok(id) => {
                    painter.image(
                        id,
                        draw_rect,
                        egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                        egui::Color32::WHITE,
                    );
                    presented = true;
                }
                Err(error) => result = Err(error),
            }
            if let Some(markers) = overlays.markers {
                if let Some(after) = renderer.marker_repaint_after(markers.layer.frame) {
                    ui.ctx().request_repaint_after(after);
                }
            }
        } else {
            result = frame.validate();
        }
        if presented {
            if let Some(tf) = frame.tf {
                tf.view
                    .paint_names(ui, presented_projection.0, presented_projection.1);
            }
        }
        if !presented {
            painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
            painter.text(rect.center(),egui::Align2::CENTER_CENTER,
                if result.is_err(){"Combined scene unavailable\nReview the diagnostic below."}else{"3D GPU presentation unavailable\nRun the native wgpu desktop to view the scene."},
                egui::FontId::proportional(14.0),ui.visuals().text_color());
        }
        if response.has_focus() {
            painter.rect_stroke(
                rect.shrink(1.0),
                0.0,
                egui::Stroke::new(2.0, ui.visuals().hyperlink_color),
                egui::StrokeKind::Inside,
            );
        }
        let diagnostic = result
            .as_ref()
            .err()
            .map(|e| e.message.as_str())
            .or(error)
            .or(self.message.as_deref())
            .unwrap_or("");
        row(ui, diagnostic, !diagnostic.is_empty());
        if !self.external_controls {
            self.camera_details(ui);
        }
        if !self.external_controls {
            egui::CollapsingHeader::new("Scene details")
                .id_salt("shared-layer-details")
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("shared-layer-list")
                        .max_height(140.0)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            if let Some(robot) = frame.robot {
                                row(
                                    ui,
                                    &format!(
                                        "Robot: {} / {} link poses",
                                        robot.poses.iter().filter(|p| p.is_some()).count(),
                                        robot.prepared.link_names.len()
                                    ),
                                    false,
                                );
                                for (index, name) in robot
                                    .prepared
                                    .link_names
                                    .iter()
                                    .take(crate::robot::MAX_LINKS)
                                    .enumerate()
                                {
                                    row(
                                        ui,
                                        &format!(
                                            "{name}: {}",
                                            if robot.poses.get(index).is_some_and(Option::is_some) {
                                                "Ready"
                                            } else {
                                                "Missing transform · link hidden"
                                            }
                                        ),
                                        false,
                                    );
                                }
                            }
                            for layer in
                                frame.layers.iter().take(super::combined::MAX_SENSOR_LAYERS)
                            {
                                ui.push_id(layer.id, |ui| {
                                    row(
                                        ui,
                                        &format!(
                                            "Layer {}: {} · {} points",
                                            layer.id,
                                            layer.status,
                                            layer.point_count()
                                        ),
                                        false,
                                    );
                                    row(ui, layer.error.unwrap_or(""), layer.error.is_some());
                                });
                            }
                            for (name, layer) in [
                                ("Map", frame.map.map(|m| (m.id, m.status, m.error))),
                                ("Pose", frame.pose.map(|m| (m.id, m.status, m.error))),
                                ("Path", frame.path.map(|m| (m.id, m.status, m.error))),
                            ] {
                                if let Some((id, status, error)) = layer {
                                    ui.push_id(id, |ui| {
                                        row(ui, &format!("{name}: {status}"), false);
                                        row(ui, error.unwrap_or(""), error.is_some());
                                    });
                                }
                            }
                        });
                });
        }
        self.camera.zoom(1.0);
        result
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::spatial::{
        combined::{PointLayerFrame, PointStyle},
        Point3,
    };
    #[test]
    fn enhanced_viewport_handles_release_frame_drag_and_focus_scoped_keyboard() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut view = SharedSceneView::visualizer();
        let frame = CombinedSceneFrame {
            tf: None,
            fixed_frame: "map",
            robot: None,
            layers: &[],
            map: None,
            pose: None,
            path: None,
        };
        let draw = |view: &mut SharedSceneView, events: Vec<egui::Event>| {
            let output = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1200., 1000.),
                    )),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        view.show(ui, &frame, "Waiting", None, None).unwrap()
                    });
                },
            );
            output
                .platform_output
                .accesskit_update
                .unwrap()
                .nodes
                .into_iter()
                .find(|(_, node)| {
                    node.label()
                        .is_some_and(|label| label.starts_with("Combined 3D scene."))
                })
                .unwrap()
                .1
                .bounds()
                .unwrap()
        };
        let rect = draw(&mut view, vec![]);
        let start = egui::pos2(
            (rect.x0 + rect.x1) as f32 / 2.,
            (rect.y0 + rect.y1) as f32 / 2.,
        );
        let end = start + egui::vec2(40., 20.);
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        draw(
            &mut view,
            vec![egui::Event::PointerMoved(start), button(start, true)],
        );
        let before = view.camera.snapshot();
        draw(
            &mut view,
            vec![egui::Event::PointerMoved(end), button(end, false)],
        );
        assert_ne!(
            view.camera.snapshot(),
            before,
            "A release frame containing the final pointer move must update the camera"
        );
        // Explicit focus through a click, then keyboard T and '-' change only presentation.
        draw(&mut view, vec![button(end, true)]);
        draw(&mut view, vec![button(end, false)]);
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        draw(&mut view, vec![key(egui::Key::T), key(egui::Key::Minus)]);
        assert_eq!(view.camera.mode, CameraMode::TopDown);
        assert!(view.camera.vertical_span > 12.);
    }
    #[test]
    fn changing_layer_count_errors_and_summary_keeps_viewport_and_camera_stable() {
        for size in [egui::vec2(1280.0, 800.0), egui::vec2(360.0, 640.0)] {
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            let mut view = SharedSceneView::default();
            let initial = view.camera.pose();
            let draw = |view: &mut SharedSceneView,
                        frame: &CombinedSceneFrame<'_>,
                        status: &str,
                        error: Option<&str>| {
                let output = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default()
                            .show(ctx, |ui| view.show(ui, frame, status, error, None).unwrap());
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
                            .is_some_and(|label| label.starts_with("Combined 3D scene."))
                    })
                    .unwrap()
                    .1
                    .bounds()
                    .unwrap()
            };
            let baseline = draw(
                &mut view,
                &CombinedSceneFrame {
                    tf: None,
                    map: None,
                    pose: None,
                    path: None,
                    fixed_frame: "map",
                    robot: None,
                    layers: &[],
                },
                "Waiting",
                None,
            );
            let long = "Changing source/transform diagnostic ".repeat(12);
            for count in 1..=4 {
                let layers = (0..count)
                    .map(|id| PointLayerFrame {
                        samples: None,
                        appearance: None,
                        id: id + 1,
                        points: &[],
                        style: PointStyle::for_id(id + 1),
                        status: &long,
                        error: Some("Missing transform"),
                    })
                    .collect::<Vec<_>>();
                assert_eq!(
                    draw(
                        &mut view,
                        &CombinedSceneFrame {
                            tf: None,
                            map: None,
                            pose: None,
                            path: None,
                            fixed_frame: "map",
                            robot: None,
                            layers: &layers
                        },
                        &long,
                        Some(&long)
                    ),
                    baseline
                );
            }
            let points = [Point3([0.0, 0.0, 0.5])];
            let layers = [PointLayerFrame {
                samples: None,
                appearance: None,
                id: 1,
                points: &points,
                style: PointStyle::default(),
                status: "Receiving",
                error: None,
            }];
            assert_eq!(
                draw(
                    &mut view,
                    &CombinedSceneFrame {
                        tf: None,
                        map: None,
                        pose: None,
                        path: None,
                        fixed_frame: "map",
                        robot: None,
                        layers: &layers
                    },
                    "Receiving",
                    None
                ),
                baseline
            );
            assert_eq!(view.camera.pose().eye, initial.eye);
        }
    }
    #[test]
    fn camera_focus_survives_layer_replacement_and_headless_gpu_is_explicit() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        ctx.style_mut(|s| s.animation_time = 0.0);
        let mut view = SharedSceneView::default();
        let draw = |view: &mut SharedSceneView,
                    event: Option<egui::Event>,
                    layers: &[PointLayerFrame<'_>]| {
            ctx.run(
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
                        view.show(
                            ui,
                            &CombinedSceneFrame {
                                tf: None,
                                map: None,
                                pose: None,
                                path: None,
                                fixed_frame: "map",
                                robot: None,
                                layers,
                            },
                            "Waiting",
                            None,
                            None,
                        )
                        .unwrap()
                    });
                },
            )
        };
        let initial = draw(&mut view, None, &[]);
        assert!(initial.shapes.iter().any(|shape|matches!(&shape.shape,egui::Shape::Text(text) if text.galley.text().contains("3D GPU presentation unavailable"))));
        let header = initial
            .platform_output
            .accesskit_update
            .unwrap()
            .nodes
            .iter()
            .find(|(_, node)| node.label() == Some("Camera details"))
            .unwrap()
            .0;
        let action = |target, action| {
            egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
                action,
                target,
                data: None,
            })
        };
        draw(
            &mut view,
            Some(action(header, egui::accesskit::Action::Click)),
            &[],
        );
        let output = draw(&mut view, None, &[]);
        let orbit = output
            .platform_output
            .accesskit_update
            .unwrap()
            .nodes
            .iter()
            .find(|(_, node)| {
                node.label() == Some("Orbit (rad)") && node.role() == egui::accesskit::Role::Slider
            })
            .unwrap()
            .0;
        draw(
            &mut view,
            Some(action(orbit, egui::accesskit::Action::Focus)),
            &[],
        );
        let layers = [PointLayerFrame {
            samples: None,
            appearance: None,
            id: 42,
            points: &[],
            style: PointStyle::for_id(42),
            status: "Waiting",
            error: Some("Lookup timeout"),
        }];
        let output = draw(&mut view, None, &layers);
        let tree = output.platform_output.accesskit_update.unwrap();
        assert_eq!(tree.focus, orbit);
        assert!(tree.nodes.iter().any(|(id, _)| *id == orbit));
    }
}
