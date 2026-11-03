//! Robot presentation only. The caller owns activation, model generations and TF;
//! absent poses are never replaced by identity or forward kinematics here.
use super::{RobotError, RobotErrorKind, MAX_LINKS};
use crate::spatial::{robot_geometry::PreparedRobot, Camera, Renderer3d, RigidTransform};
use eframe::egui;
use studio_protocol::robot::{LinkPose, LinkResult};

#[derive(Clone, Copy, Default)]
pub struct RobotViewState<'a> {
    pub fixed_frame: &'a str,
    pub status: &'a str,
    pub error: Option<&'a str>,
    /// Empty while waiting, otherwise exact stable description order.
    pub links: &'a [LinkResult],
}
#[derive(Default)]
pub struct RobotView {
    pub camera: Camera,
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
fn check_identity(
    prepared: &PreparedRobot,
    poses: &[Option<RigidTransform>],
    links: &[LinkResult],
) -> Result<(), RobotError> {
    if prepared.link_names.is_empty()
        || prepared.link_names.len() > MAX_LINKS
        || poses.len() != prepared.link_names.len()
    {
        return Err(RobotError::new(
            RobotErrorKind::Pose,
            "Robot presentation requires a pose slot for each stable model link",
        ));
    }
    if !links.is_empty()
        && (links.len() != prepared.link_names.len()
            || links
                .iter()
                .zip(&prepared.link_names)
                .any(|(link, name)| &link.frame != name))
    {
        return Err(RobotError::new(
            RobotErrorKind::Pose,
            "Robot diagnostic link identities/order do not match the loaded model",
        ));
    }
    Ok(())
}
fn pose_label(pose: &LinkPose) -> String {
    match pose {
        LinkPose::Ready {
            resolved_stamp_ns, ..
        } => format!("Ready · native stamp {resolved_stamp_ns} ns"),
        LinkPose::Missing => "Missing transform · link hidden".into(),
        LinkPose::Stale => "Stale transform · link hidden".into(),
        LinkPose::Extrapolated => "Transform extrapolated · link hidden".into(),
        LinkPose::LookupTimeout => "Transform lookup timed out · link hidden".into(),
    }
}
impl RobotView {
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        prepared: &PreparedRobot,
        poses: &[Option<RigidTransform>],
        state: RobotViewState<'_>,
        renderer: Option<&mut Renderer3d>,
    ) -> Result<(), RobotError> {
        ui.push_id("robot-view", |ui| {
            self.show_inner(ui, prepared, poses, state, renderer)
        })
        .inner
    }
    fn show_inner(
        &mut self,
        ui: &mut egui::Ui,
        prepared: &PreparedRobot,
        poses: &[Option<RigidTransform>],
        state: RobotViewState<'_>,
        renderer: Option<&mut Renderer3d>,
    ) -> Result<(), RobotError> {
        // Neutral zoom applies Camera's existing finite/bounds policy.
        self.camera.zoom(1.0);
        ui.label("Robot model · perspective 3D");
        row(
            ui,
            &format!(
                "Fixed frame: {}",
                if state.fixed_frame.is_empty() {
                    "choose a frame"
                } else {
                    state.fixed_frame
                }
            ),
            false,
        );
        row(ui, state.status, false);
        ui.horizontal_wrapped(|ui| {
            if ui.button("Zoom in").clicked() {
                self.camera.zoom(1.25);
            }
            if ui.button("Zoom out").clicked() {
                self.camera.zoom(0.8);
            }
            if ui.button("Reset camera").clicked() {
                self.camera.reset();
            }
            ui.small("Drag: orbit · Right-drag: pan · X red / Y green / Z blue");
        });
        let size = egui::vec2(
            ui.available_width().max(1.0),
            (ui.available_height() - 64.0).clamp(120.0, 700.0),
        );
        let (_, rect) = ui.allocate_space(size);
        let response = ui.interact(
            rect,
            ui.make_persistent_id("robot-viewport"),
            egui::Sense::drag(),
        );
        response.widget_info(||egui::WidgetInfo::labeled(egui::WidgetType::Other,ui.is_enabled(),
            format!("Robot 3D scene. Fixed frame: {}. {} model links. Drag to orbit; use Camera details for keyboard controls.",state.fixed_frame,prepared.link_names.len())));
        if response.dragged_by(egui::PointerButton::Secondary) {
            let delta = ui.input(|i| i.pointer.delta());
            self.camera.pan_delta([delta.x, delta.y]);
        } else if response.dragged_by(egui::PointerButton::Primary) {
            let delta = ui.input(|i| i.pointer.delta());
            self.camera.orbit_delta([delta.x, delta.y]);
        }
        let painter = ui.painter_at(rect);
        let mut result = check_identity(prepared, poses, state.links);
        let mut presented = false;
        if result.is_ok() {
            if let Some(renderer) = renderer {
                let scale = ui.ctx().pixels_per_point();
                match renderer.render_robot(
                    [
                        (size.x * scale).round() as u32,
                        (size.y * scale).round() as u32,
                    ],
                    prepared,
                    poses,
                    self.camera.pose(),
                    ui.visuals(),
                ) {
                    Ok(id) => {
                        painter.image(
                            id,
                            rect,
                            egui::Rect::from_min_max(egui::Pos2::ZERO, egui::pos2(1.0, 1.0)),
                            egui::Color32::WHITE,
                        );
                        presented = true;
                    }
                    Err(error) => result = Err(error),
                }
            }
        }
        if !presented {
            painter.rect_filled(rect, 0.0, ui.visuals().extreme_bg_color);
            let text = if result.is_err() {
                "Robot presentation unavailable\nReview the diagnostic below."
            } else {
                "3D GPU presentation unavailable\nRun the native wgpu desktop to view the robot."
            };
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                text,
                egui::FontId::proportional(14.0),
                ui.visuals().text_color(),
            );
        }
        if response.has_focus() {
            painter.rect_stroke(
                rect.shrink(1.0),
                0.0,
                egui::Stroke::new(2.0, ui.visuals().hyperlink_color),
                egui::StrokeKind::Inside,
            );
        }
        // Always reserve the diagnostic row, including the no-error state.
        let diagnostic = result
            .as_ref()
            .err()
            .map(|e| e.message.as_str())
            .or(state.error)
            .unwrap_or("");
        row(ui, diagnostic, !diagnostic.is_empty());
        egui::CollapsingHeader::new("Camera details")
            .id_salt("robot-camera-details")
            .show(ui, |ui| {
                ui.add(studio_ui_foundation::theme::slider(
                    egui::Slider::new(&mut self.camera.azimuth, 0.0..=std::f32::consts::TAU)
                        .text("Orbit (rad)"),
                ));
                ui.add(studio_ui_foundation::theme::slider(
                    egui::Slider::new(&mut self.camera.elevation, -1.5..=1.5)
                        .text("Elevation (rad)"),
                ));
                ui.add(studio_ui_foundation::theme::slider(
                    egui::Slider::new(&mut self.camera.distance, 0.2..=2000.0)
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
                            egui::DragValue::new(&mut self.camera.target[i])
                                .speed(0.1)
                                .range(-1_000_000.0..=1_000_000.0),
                        );
                    });
                }
            });
        egui::CollapsingHeader::new("Link diagnostics")
            .id_salt("robot-link-diagnostics")
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("robot-link-list")
                    .max_height(140.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        for (index, name) in prepared.link_names.iter().take(MAX_LINKS).enumerate()
                        {
                            let detail = if result.is_err() {
                                "Invalid pose batch · not presented".into()
                            } else {
                                state
                                    .links
                                    .get(index)
                                    .map(|l| pose_label(&l.pose))
                                    .unwrap_or_else(|| "Waiting for transform · link hidden".into())
                            };
                            row(ui, &format!("{name}: {detail}"), false);
                        }
                    });
            });
        self.camera.zoom(1.0);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui::accesskit::{Action, ActionRequest, Role};
    fn prepared() -> PreparedRobot {
        PreparedRobot {
            link_names: vec!["robot/base_link".into()],
            visuals: vec![],
            triangle_count: 0,
        }
    }
    #[test]
    fn status_error_frame_and_link_text_do_not_move_the_robot_viewport() {
        for dimensions in [egui::vec2(1280.0, 800.0), egui::vec2(360.0, 640.0)] {
            let context = egui::Context::default();
            context.enable_accesskit();
            let mut view = RobotView::default();
            let model = prepared();
            let before = view.camera.pose();
            let draw = |view: &mut RobotView, state: RobotViewState<'_>| {
                let output = context.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, dimensions)),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            view.show(ui, &model, &[None], state, None).unwrap()
                        });
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
                            .is_some_and(|label| label.starts_with("Robot 3D scene."))
                    })
                    .unwrap()
                    .1
                    .bounds()
                    .unwrap()
            };
            let initial = draw(
                &mut view,
                RobotViewState {
                    fixed_frame: "map",
                    status: "Waiting for links",
                    ..Default::default()
                },
            );
            let long = "a_long_diagnostic_or_frame_".repeat(12);
            for pose in [
                LinkPose::Missing,
                LinkPose::Stale,
                LinkPose::Extrapolated,
                LinkPose::LookupTimeout,
            ] {
                let links = [LinkResult {
                    frame: model.link_names[0].clone(),
                    pose,
                }];
                let bounds = draw(
                    &mut view,
                    RobotViewState {
                        fixed_frame: &long,
                        status: &long,
                        error: Some(&long),
                        links: &links,
                    },
                );
                assert_eq!(
                    bounds, initial,
                    "dynamic diagnostics must not move viewport at {dimensions:?}"
                );
            }
            assert_eq!(
                draw(
                    &mut view,
                    RobotViewState {
                        fixed_frame: "map",
                        status: "Stopped",
                        ..Default::default()
                    }
                ),
                initial
            );
            assert_eq!(
                view.camera.pose().eye,
                before.eye,
                "presentation does not move camera or invent a pose"
            );
        }
    }
    #[test]
    fn camera_focus_and_expansion_survive_changing_link_diagnostics() {
        let context = egui::Context::default();
        context.enable_accesskit();
        context.style_mut(|style| style.animation_time = 0.0);
        let mut view = RobotView::default();
        let model = prepared();
        let draw = |view: &mut RobotView, event: Option<egui::Event>, error: Option<&str>| {
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
                        view.show(
                            ui,
                            &model,
                            &[None],
                            RobotViewState {
                                fixed_frame: "map",
                                status: "Waiting",
                                error,
                                links: &[],
                            },
                            None,
                        )
                        .unwrap()
                    });
                },
            )
        };
        let initial = draw(&mut view, None, None);
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
            egui::Event::AccessKitActionRequest(ActionRequest {
                action,
                target,
                data: None,
            })
        };
        draw(&mut view, Some(action(header, Action::Click)), None);
        let opened = draw(&mut view, None, None);
        let orbit = opened
            .platform_output
            .accesskit_update
            .unwrap()
            .nodes
            .iter()
            .find(|(_, node)| node.label() == Some("Orbit (rad)") && node.role() == Role::Slider)
            .expect("Camera details expanded")
            .0;
        let focused = draw(&mut view, Some(action(orbit, Action::Focus)), None);
        assert_eq!(
            focused.platform_output.accesskit_update.unwrap().focus,
            orbit
        );
        for error in [Some("Transform missing: base link"),None,Some("The latest transform lookup timed out; missing link visuals remain hidden until recovery")] {
            let output=draw(&mut view,None,error);let tree=output.platform_output.accesskit_update.unwrap();
            assert_eq!(tree.focus,orbit);
            assert!(tree.nodes.iter().any(|(id,node)|*id==orbit && node.role()==Role::Slider));
        }
    }
    #[test]
    fn mismatched_model_identity_fails_and_headless_presentation_is_explicit() {
        let context = egui::Context::default();
        let mut view = RobotView::default();
        let model = prepared();
        let mut kind = None;
        let output = context.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                kind = view
                    .show(ui, &model, &[], RobotViewState::default(), None)
                    .err()
                    .map(|e| e.kind);
            });
        });
        assert_eq!(kind, Some(RobotErrorKind::Pose));
        assert!(output.shapes.iter().any(|shape|matches!(&shape.shape,egui::Shape::Text(text) if text.galley.text().contains("Robot presentation unavailable"))));
        let mut wrong = [LinkResult {
            frame: "different_model_link".into(),
            pose: LinkPose::Missing,
        }];
        assert!(check_identity(&model, &[None], &wrong).is_err());
        wrong[0].frame = model.link_names[0].clone();
        assert!(check_identity(&model, &[None], &wrong).is_ok());
        let output = context.run(egui::RawInput::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                view.show(ui, &model, &[None], RobotViewState::default(), None)
                    .unwrap()
            });
        });
        assert!(output.shapes.iter().any(|shape|matches!(&shape.shape,egui::Shape::Text(text) if text.galley.text().contains("3D GPU presentation unavailable"))));
    }
}
