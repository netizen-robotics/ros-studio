//! Pure Robot setup widgets. Loading, validation, cleanup and subscription
//! activation belong to the caller; editing a field never emits Apply.
use super::setup::{RobotDraft, RobotSetupAction, RobotSetupState};
use crate::{
    robot::{MAX_NAME_BYTES, MAX_PACKAGE_ROOTS, MAX_PATH_BYTES},
    workspace::PackageRoot,
};
use eframe::egui;

fn fixed_row(ui: &mut egui::Ui, text: &str, error: bool) {
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
fn edit(
    ui: &mut egui::Ui,
    label: &str,
    value: &mut String,
    limit: usize,
    hint: &str,
    id: impl std::hash::Hash,
) -> bool {
    let caption = ui.label(label);
    ui.add(
        studio_ui_foundation::theme::input(ui, value)
            .id_salt(id)
            .char_limit(limit)
            .desired_width(ui.available_width())
            .hint_text(studio_ui_foundation::theme::hint(ui, hint)),
    )
    .labelled_by(caption.id)
    .changed()
}
fn tf_choice(ui: &mut egui::Ui, label: &str, selected: &mut String, topics: &[String]) {
    let caption = ui.label(label);
    let response = egui::ComboBox::from_id_salt(label)
        .width((ui.available_width() - 12.0).max(1.0))
        .truncate()
        .selected_text(if selected.is_empty() {
            format!("Choose {}", label.to_lowercase())
        } else {
            selected.clone()
        })
        .show_ui(ui, |ui| {
            for topic in topics
                .iter()
                .take(studio_protocol::session::MAX_GRAPH_TOPICS)
            {
                ui.selectable_value(selected, topic.clone(), topic)
                    .on_hover_text(topic);
            }
            if topics.is_empty() {
                ui.label(if label == "Description topic" {
                    "No String topics discovered. Connect or wait for discovery."
                } else {
                    "Connect to discover TF topics."
                });
            }
        })
        .response
        .labelled_by(caption.id)
        .on_hover_text(selected.as_str());
    response.widget_info(|| {
        let mut info =
            egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, response.enabled(), label);
        info.current_text_value = Some(selected.clone());
        info
    });
}
/// Returns only an explicit button action. Every field remains a draft; Add and
/// Apply save inactive configuration even when reviewing a replacement model.
pub fn show_robot_setup(
    ui: &mut egui::Ui,
    draft: &mut RobotDraft,
    state: RobotSetupState<'_>,
) -> Option<RobotSetupAction> {
    show_robot_setup_inner(ui, draft, false, state)
}
pub fn show_robot_setup_in_scene(
    ui: &mut egui::Ui,
    draft: &mut RobotDraft,
    state: RobotSetupState<'_>,
) -> Option<RobotSetupAction> {
    show_robot_setup_inner(ui, draft, true, state)
}
fn show_robot_setup_inner(
    ui: &mut egui::Ui,
    draft: &mut RobotDraft,
    scene_owned: bool,
    state: RobotSetupState<'_>,
) -> Option<RobotSetupAction> {
    ui.push_id("robot-setup", |ui| {
        let mut action = None;
        let tf_before = draft.tf.clone();
        ui.heading("Robot model");
        ui.small("Read a description or load an exported URDF, then review its visuals.");
        let mut assets_changed = false;
        ui.add_enabled_ui(!state.busy, |ui| {
            if scene_owned {
                let was_topic = draft.description.is_some();
                let mut topic = was_topic;
                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(&mut topic, true, "Description topic");
                    ui.selectable_value(&mut topic, false, "File");
                });
                if topic != was_topic {
                    draft.description = topic.then(Default::default);
                    assets_changed = true;
                }
            }
            if let Some(description) = &mut draft.description {
                let before = description.clone();
                tf_choice(ui, "Description topic", &mut description.topic, state.description_topics);
                ui.small("std_msgs/msg/String · one complete description per Read");
                egui::CollapsingHeader::new("Description QoS and assets").id_salt("description-options").show(ui, |ui| {
                    use crate::workspace::{Reliability, Durability};
                    let mut best = description.qos.reliability == Reliability::BestEffort;
                    let mut retained = description.qos.durability == Durability::TransientLocal;
                    ui.checkbox(&mut best, "Description best effort");
                    ui.checkbox(&mut retained, "Request retained description");
                    description.qos.reliability = if best { Reliability::BestEffort } else { Reliability::Reliable };
                    description.qos.durability = if retained { Durability::TransientLocal } else { Durability::Volatile };
                    let mut root = description.asset_root.clone().unwrap_or_default();
                    edit(ui, "Asset directory (optional)", &mut root, MAX_PATH_BYTES, "/path/to/robot/assets", "description-asset-root");
                    description.asset_root = (!root.is_empty()).then_some(root);
                    ui.small("Relative meshes need this directory. Package meshes use the mappings below.");
                });
                assets_changed |= before != *description;
            } else {
            assets_changed |= edit(
                ui,
                "URDF path",
                &mut draft.assets.urdf_path,
                MAX_PATH_BYTES,
                "/path/to/robot.urdf",
                "urdf-path",
            );
            }
            let roots = if let Some(description) = &mut draft.description { &mut description.package_roots } else { &mut draft.assets.package_roots };
            egui::CollapsingHeader::new("Package roots (optional)")
                .id_salt("robot-package-roots")
                .show(ui, |ui| {
                    ui.small("Map each package name to its local directory.");
                    let mut remove = None;
                    for (index, root) in roots
                        .iter_mut()
                        .take(MAX_PACKAGE_ROOTS)
                        .enumerate()
                    {
                        ui.push_id(index, |ui| {
                            ui.add_space(4.0);
                            assets_changed |= edit(
                                ui,
                                "Package name",
                                &mut root.package,
                                MAX_NAME_BYTES,
                                "my_robot_description",
                                "package-name",
                            );
                            assets_changed |= edit(
                                ui,
                                "Package directory",
                                &mut root.path,
                                MAX_PATH_BYTES,
                                "/path/to/my_robot_description",
                                "package-directory",
                            );
                            if ui.button("Remove package root").clicked() {
                                remove = Some(index);
                            }
                        });
                    }
                    if let Some(index) = remove {
                        roots.remove(index);
                        assets_changed = true;
                    }
                    if ui
                        .add_enabled(
                            roots.len() < MAX_PACKAGE_ROOTS,
                            egui::Button::new("Add package root"),
                        )
                        .clicked()
                    {
                        roots.push(PackageRoot {
                            package: String::new(),
                            path: String::new(),
                        });
                        assets_changed = true;
                    }
                    if roots.len() > MAX_PACKAGE_ROOTS {
                        fixed_row(
                            ui,
                            "Too many package roots. At most 16 are supported.",
                            true,
                        );
                    }
                });
        });
        ui.horizontal_wrapped(|ui| {
            let response = ui
                .add_enabled(
                    !state.busy && (if let Some(description) = &draft.description { state.connected && !description.topic.is_empty() } else { !draft.assets.urdf_path.trim().is_empty() }),
                    egui::Button::new(if draft.description.is_some() { "Read description" } else { "Load model" }),
                );
            let focus_key = ui.make_persistent_id("robot-read-keyboard-focus");
            if response.clicked() {
                let had_focus = response.has_focus();
                ui.data_mut(|data| data.insert_temp(focus_key, had_focus));
                action = Some(RobotSetupAction::Load);
            } else if !state.busy && ui.data(|data| data.get_temp::<bool>(focus_key).unwrap_or(false)) {
                // Disabled progress controls temporarily release egui focus.
                // Restore only this explicit keyboard action's focus, and only
                // if the user has not focused another control in the meantime.
                if (state.review.is_some() || state.error.is_some()) && ui.memory(|memory| memory.focused().is_none()) {
                    response.request_focus();
                }
                ui.data_mut(|data| data.insert_temp(focus_key, false));
            }
            if ui
                .add_enabled(state.busy, egui::Button::new(if draft.description.is_some() { "Cancel read or load" } else { "Cancel load" }))
                .clicked()
            {
                action = Some(RobotSetupAction::CancelLoad);
            }
            if state.busy && ui.memory(|memory| memory.focused().is_some_and(|id| id != response.id)) {
                ui.data_mut(|data| data.insert_temp(focus_key, false));
            }
        });
        // Live worker/error summaries never add/remove rows or wrap the panel.
        let status = if state.busy {
            state.operation.unwrap_or("Loading model…")
        } else if state.review.is_some() && state.assets_match && !assets_changed {
            "Model ready for review"
        } else if state.review.is_some() {
            "Source changed. Read or load it again before applying."
        } else {
            "No model loaded"
        };
        fixed_row(ui, status, false);
        fixed_row(ui, state.error.unwrap_or(""), state.error.is_some());
        if scene_owned { fixed_row(ui, if draft.description.is_some() { state.source_summary.unwrap_or("Description: not captured") } else { "Source: local URDF" }, false); }
        if let Some(review) = state.review {
            fixed_row(ui, &format!("Model: {}", review.name), false);
            fixed_row(
                ui,
                &format!(
                    "{} links · {} visuals · {} collision parts · {} triangles",
                    review.links, review.visuals, review.collisions, review.triangles
                ),
                false,
            );
        } else {
            fixed_row(ui, "Model: awaiting load", false);
            fixed_row(
                ui,
                "Load model to inspect its link and visual summary.",
                false,
            );
        }
        ui.add_space(8.0);
        ui.strong("Placement");
        if scene_owned {
            ui.label(format!("Shared scene frame: {}", if draft.tf.fixed_frame.is_empty() { "not configured" } else { &draft.tf.fixed_frame }));
            if draft.tf.fixed_frame.is_empty() {
                ui.small("Choose a fixed frame in Scene settings before adding this display.");
            }
            ui.small("Frame, TF inputs and clock are configured in Scene settings.");
            ui.small("Pose policy: Latest available TF. Missing transforms hide affected links.");
        } else {
            edit(
                ui,
                "Fixed frame",
                &mut draft.tf.fixed_frame,
                MAX_NAME_BYTES,
                "For example: map",
                "robot-fixed-frame",
            );
            fixed_row(ui, "Pose policy: Latest available TF", false);
            if !state.connected {
                draft.clock_verified = false;
            }
            ui.add_enabled(
                state.connected,
                egui::Checkbox::new(
                    &mut draft.clock_verified,
                    "I verified these publishers use system time",
                ),
            );
            ui.small(if state.connected {
                "Verify again after each connection. Simulated time is unsupported."
            } else {
                "Local model review works offline. Connect before verifying time or enabling."
            });
            egui::CollapsingHeader::new("Transform inputs")
                .id_salt("robot-transform-inputs")
                .show(ui, |ui| {
                    tf_choice(
                        ui,
                        "Dynamic TF topic",
                        &mut draft.tf.dynamic_tf_topic,
                        state.tf_topics,
                    );
                    tf_choice(
                        ui,
                        "Static TF topic",
                        &mut draft.tf.static_tf_topic,
                        state.tf_topics,
                    );
                    ui.checkbox(
                        &mut draft.tf.dynamic_tf_best_effort,
                        "Dynamic TF best effort",
                    );
                    ui.small("Static TF uses Reliable + Transient local retained data.");
                    ui.small("Missing link transforms hide only those links.");
                });
        }
        if draft.tf != tf_before {
            draft.clock_verified = false;
        }
        ui.add_space(8.0);
        fixed_row(
            ui,
            if state.target_active {
                "Applying stops this display. Enable it again when ready."
            } else {
                "Added or changed displays stay disabled until you enable them."
            },
            false,
        );
        let can_apply = !state.busy
            && state.review.is_some()
            && state.assets_match
            && !assets_changed
            && !draft.tf.fixed_frame.trim().is_empty()
            && !draft.tf.dynamic_tf_topic.is_empty()
            && !draft.tf.static_tf_topic.is_empty();
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    can_apply,
                    egui::Button::new(if state.is_replacement {
                        "Apply changes"
                    } else {
                        "Add display"
                    }),
                )
                .clicked()
            {
                action = Some(RobotSetupAction::Apply);
            }
            if ui.button("Cancel setup").clicked() {
                action = Some(RobotSetupAction::Cancel);
            }
        });
        action
    })
    .inner
}

#[cfg(test)]
mod tests {
    use super::super::setup::RobotReview;
    use super::*;
    use egui::accesskit::{Action, ActionRequest, NodeId, Role};
    fn draw(
        ctx: &egui::Context,
        draft: &mut RobotDraft,
        review: Option<&RobotReview>,
        busy: bool,
        connected: bool,
        error: Option<&str>,
        event: Option<egui::Event>,
    ) -> (egui::FullOutput, Option<RobotSetupAction>) {
        let mut action = None;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(320.0, 1200.0),
                )),
                events: event.into_iter().collect(),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    action = show_robot_setup(
                        ui,
                        draft,
                        RobotSetupState {
                            busy,
                            review,
                            assets_match: true,
                            error,
                            target_active: false,
                            is_replacement: false,
                            connected,
                            tf_topics: &[],
                            description_topics: &[],
                            operation: None,
                            source_summary: None,
                        },
                    );
                });
            },
        );
        (output, action)
    }
    fn action(target: NodeId, action: Action) -> egui::Event {
        egui::Event::AccessKitActionRequest(ActionRequest {
            action,
            target,
            data: None,
        })
    }
    fn button(output: &egui::FullOutput, label: &str) -> NodeId {
        output
            .platform_output
            .accesskit_update
            .as_ref()
            .unwrap()
            .nodes
            .iter()
            .find(|(_, node)| node.label() == Some(label) && node.role() == Role::Button)
            .unwrap()
            .0
    }
    fn review() -> RobotReview {
        RobotReview {
            name: "Articulated fixture".into(),
            links: 4,
            visuals: 5,
            collisions: 0,
            triangles: 128,
        }
    }
    fn draft() -> RobotDraft {
        let mut draft = RobotDraft::default();
        draft.assets.urdf_path = "/tmp/robot.urdf".into();
        draft.tf.fixed_frame = "map".into();
        draft
    }
    #[test]
    fn model_loading_and_inactive_install_require_explicit_buttons_even_offline() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut draft = draft();
        let (first, result) = draw(&ctx, &mut draft, None, false, false, None, None);
        assert!(result.is_none());
        let load = button(&first, "Load model");
        let (_, result) = draw(
            &ctx,
            &mut draft,
            None,
            false,
            false,
            None,
            Some(action(load, Action::Click)),
        );
        assert_eq!(result, Some(RobotSetupAction::Load));
        let (busy, result) = draw(&ctx, &mut draft, None, true, false, None, None);
        assert!(result.is_none());
        let cancel_load = button(&busy, "Cancel load");
        let (_, result) = draw(
            &ctx,
            &mut draft,
            None,
            true,
            false,
            None,
            Some(action(cancel_load, Action::Click)),
        );
        assert_eq!(result, Some(RobotSetupAction::CancelLoad));
        let review = review();
        draft.clock_verified = true;
        let (ready, result) = draw(&ctx, &mut draft, Some(&review), false, false, None, None);
        assert!(result.is_none());
        assert!(
            !draft.clock_verified,
            "offline review cannot authorize a live clock"
        );
        let apply = button(&ready, "Add display");
        let (_, result) = draw(
            &ctx,
            &mut draft,
            Some(&review),
            false,
            false,
            None,
            Some(action(apply, Action::Click)),
        );
        assert_eq!(result, Some(RobotSetupAction::Apply));
        assert!(
            !draft.clock_verified,
            "inactive install never verifies time or activates reception"
        );
    }
    #[test]
    fn keyboard_load_focus_returns_after_async_completion_without_stealing_other_focus() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut draft = draft();
        let (initial, _) = draw(&ctx, &mut draft, None, false, false, None, None);
        let load = button(&initial, "Load model");
        draw(
            &ctx,
            &mut draft,
            None,
            false,
            false,
            None,
            Some(action(load, Action::Focus)),
        );
        let focused = ctx.memory(|memory| memory.focused());
        assert!(focused.is_some());
        let (_, triggered) = draw(
            &ctx,
            &mut draft,
            None,
            false,
            false,
            None,
            Some(action(load, Action::Click)),
        );
        assert_eq!(triggered, Some(RobotSetupAction::Load));
        draw(&ctx, &mut draft, None, true, false, None, None);
        let review = review();
        draw(&ctx, &mut draft, Some(&review), false, false, None, None);
        assert_eq!(
            ctx.memory(|memory| memory.focused()),
            focused,
            "Async load completion must preserve the initiating keyboard focus"
        );
        let (ready, _) = draw(&ctx, &mut draft, Some(&review), false, false, None, None);
        let load = button(&ready, "Load model");
        draw(
            &ctx,
            &mut draft,
            Some(&review),
            false,
            false,
            None,
            Some(action(load, Action::Click)),
        );
        let (busy, _) = draw(&ctx, &mut draft, None, true, false, None, None);
        let cancel = button(&busy, "Cancel setup");
        draw(
            &ctx,
            &mut draft,
            None,
            true,
            false,
            None,
            Some(action(cancel, Action::Focus)),
        );
        let moved = ctx.memory(|memory| memory.focused());
        assert!(moved.is_some() && moved != focused);
        draw(&ctx, &mut draft, Some(&review), false, false, None, None);
        assert_eq!(
            ctx.memory(|memory| memory.focused()),
            moved,
            "Completion cannot steal focus after the user moves elsewhere"
        );
    }
    #[test]
    fn setup_controls_stay_stable_across_live_messages_and_tf_edits_revoke_clock() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut draft = draft();
        let review = review();
        let (initial, result) = draw(&ctx, &mut draft, Some(&review), false, true, None, None);
        assert!(result.is_none());
        let apply = button(&initial, "Add display");
        let bounds = initial
            .platform_output
            .accesskit_update
            .as_ref()
            .unwrap()
            .nodes
            .iter()
            .find(|(id, _)| *id == apply)
            .unwrap()
            .1
            .bounds();
        let long = "A long model loading diagnostic requiring correction ".repeat(12);
        for (busy, error) in [
            (true, Some(long.as_str())),
            (false, None),
            (
                false,
                Some("Missing STL asset. Correct the package mapping and load again."),
            ),
        ] {
            let (output, result) = draw(&ctx, &mut draft, Some(&review), busy, true, error, None);
            assert!(result.is_none());
            let next = button(&output, "Add display");
            assert_eq!(next, apply);
            assert_eq!(
                output
                    .platform_output
                    .accesskit_update
                    .unwrap()
                    .nodes
                    .iter()
                    .find(|(id, _)| *id == next)
                    .unwrap()
                    .1
                    .bounds(),
                bounds,
                "single-row loading/error text must not move actions at narrow width"
            );
        }
        let (output, _) = draw(&ctx, &mut draft, Some(&review), false, true, None, None);
        let frame = output
            .platform_output
            .accesskit_update
            .unwrap()
            .nodes
            .iter()
            .find(|(_, node)| node.role() == Role::TextInput && node.value() == Some("map"))
            .expect("fixed-frame text input")
            .0;
        draw(
            &ctx,
            &mut draft,
            Some(&review),
            false,
            true,
            None,
            Some(action(frame, Action::Focus)),
        );
        draft.clock_verified = true;
        let (_, result) = draw(
            &ctx,
            &mut draft,
            Some(&review),
            false,
            true,
            None,
            Some(egui::Event::Text("updated".into())),
        );
        assert!(
            result.is_none(),
            "editing a TF source never installs or activates a display"
        );
        assert_ne!(draft.tf.fixed_frame, "map");
        assert!(
            !draft.clock_verified,
            "changing fixed frame requires fresh explicit verification"
        );
    }
}
