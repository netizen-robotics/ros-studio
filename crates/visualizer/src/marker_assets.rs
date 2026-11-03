//! Pure asset-draft controls. Only explicit approval changes mesh authority.
use super::*;
use crate::components::robot::{MAX_NAME_BYTES, MAX_PACKAGE_ROOTS, MAX_PATH_BYTES};

fn edit(ui: &mut egui::Ui, label: &str, value: &mut String, limit: usize, hint: &str) {
    let caption = ui.label(label);
    ui.add_sized(
        [
            ui.available_width(),
            studio_ui_foundation::theme::CONTROL_HEIGHT,
        ],
        studio_ui_foundation::theme::input(ui, value)
            .id_salt(label)
            .char_limit(limit)
            .desired_width(ui.available_width())
            .clip_text(true)
            .hint_text(studio_ui_foundation::theme::hint(ui, hint)),
    )
    .labelled_by(caption.id);
}
fn read_only(ui: &mut egui::Ui, label: &str, value: &str) {
    let caption = ui.label(label);
    let mut text = value;
    ui.add_sized(
        [
            ui.available_width(),
            studio_ui_foundation::theme::CONTROL_HEIGHT,
        ],
        studio_ui_foundation::theme::input(ui, &mut text)
            .id_salt(label)
            .desired_width(ui.available_width())
            .clip_text(true),
    )
    .labelled_by(caption.id);
}
fn action_pair(
    ui: &mut egui::Ui,
    left: (&str, bool),
    right: (&str, bool),
    primary: bool,
) -> [bool; 2] {
    let columns = if ui.available_width() >= 240.0 { 2 } else { 1 };
    let width = (ui.available_width() - (columns - 1) as f32 * ui.spacing().item_spacing.x)
        / columns as f32;
    let mut clicked = [false; 2];
    let mut button = |ui: &mut egui::Ui, index: usize, (label, enabled): (&str, bool)| {
        let widget = if primary && index == 0 {
            shell::primary(ui.visuals(), label)
        } else {
            egui::Button::new(label)
        };
        clicked[index] = ui
            .add_enabled_ui(enabled, |ui| {
                ui.add_sized(
                    [width, studio_ui_foundation::theme::CONTROL_HEIGHT],
                    widget.truncate(),
                )
            })
            .inner
            .clicked();
    };
    if columns == 2 {
        ui.horizontal(|ui| {
            button(ui, 0, left);
            button(ui, 1, right);
        });
    } else {
        button(ui, 0, left);
        button(ui, 1, right);
    }
    clicked
}
impl App {
    pub(super) fn marker_assets_ui(&mut self, ui: &mut egui::Ui, frozen: bool) {
        if ui
            .add_enabled_ui(!frozen, |ui| {
                ui.add_sized(
                    [
                        ui.available_width(),
                        studio_ui_foundation::theme::CONTROL_HEIGHT,
                    ],
                    egui::Button::new("Mesh assets…"),
                )
            })
            .inner
            .clicked()
        {
            self.markers.asset_editor_open = true;
        }
        ui.add_sized(
            [ui.available_width(), 20.0],
            egui::Label::new(if self.markers.approved.is_some() {
                "Shared scene · mesh roots approved"
            } else {
                "Shared scene · mesh access not approved"
            })
            .truncate(),
        );
    }
    pub(crate) fn marker_asset_window_ui(&mut self, ctx: &egui::Context, frozen: bool) {
        let mut open = self.markers.asset_editor_open;
        egui::Window::new("Marker mesh assets")
            .id(egui::Id::new("marker-asset-window"))
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_width(520.0)
            .default_height(520.0)
            .min_width(300.0)
            .min_height(360.0)
            .max_size(egui::vec2(
                (ctx.screen_rect().width() - 32.0).max(300.0),
                (ctx.screen_rect().height() - 32.0).max(360.0),
            ))
            .show(ctx, |ui| self.marker_asset_form_ui(ui, frozen));
        self.markers.asset_editor_open = open;
    }
    fn marker_asset_form_ui(&mut self, ui: &mut egui::Ui, frozen: bool) {
        ui.small("Session-only access for Marker meshes. Other shapes keep receiving. Workspaces do not save these roots.");
        let changed = self.markers.asset_draft_changed();
        let invalid = self.markers.draft.policy(1).err();
        ui.add_sized(
            [ui.available_width(), 22.0],
            egui::Label::new(
                egui::RichText::new(if changed {
                    "Unapplied changes"
                } else if self.markers.approved.is_some() {
                    "Approved mesh roots"
                } else {
                    "Mesh access not approved"
                })
                .family(studio_ui_foundation::fonts::semibold_family(ui.ctx())),
            )
            .truncate(),
        );
        let message = self
            .markers
            .asset_error
            .as_deref()
            .or(invalid.as_deref())
            .or(self.markers.asset_notice.as_deref())
            .unwrap_or(if changed && self.markers.approved.is_some() {
                "Draft not applied. Approved meshes remain active."
            } else {
                "Edit the draft below, then approve its roots."
            });
        let is_error = self.markers.asset_error.is_some() || (changed && invalid.is_some());
        // Fixed feedback space: validation never moves the fields or action targets.
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 60.0), egui::Sense::hover());
        let mut feedback = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        feedback.set_clip_rect(rect.intersect(ui.clip_rect()));
        feedback.add(
            egui::Label::new(egui::RichText::new(message).color(if is_error {
                ui.visuals().error_fg_color
            } else {
                shell::muted(ui.visuals())
            }))
            .wrap()
            .selectable(true),
        );
        let approved = self.markers.approved.is_some();
        let [approve, discard] = action_pair(
            ui,
            ("Approve roots", !frozen && changed && invalid.is_none()),
            ("Discard draft", !frozen && changed),
            true,
        );
        let [reload, revoke] = action_pair(
            ui,
            ("Reload meshes", !frozen && approved),
            ("Revoke access", !frozen && approved),
            false,
        );
        let before = self.markers.draft.clone();
        egui::ScrollArea::vertical()
            .id_salt("marker-asset-fields")
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .min_scrolled_height(80.0)
            .max_height(ui.available_height().max(80.0))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.add_enabled_ui(!frozen, |ui| {
                    ui.separator();
                    edit(
                        ui,
                        "Asset folder (optional)",
                        &mut self.markers.draft.root,
                        MAX_PATH_BYTES,
                        "/absolute/local/folder",
                    );
                    ui.small("For relative or file meshes. Package meshes use the mappings below.");
                    studio_ui_foundation::theme::strong(ui, "Package mappings");
                    ui.small(format!(
                        "{} of {MAX_PACKAGE_ROOTS} mappings",
                        self.markers.draft.packages.len()
                    ));
                    if ui
                        .add_enabled_ui(
                            self.markers.draft.packages.len() < MAX_PACKAGE_ROOTS,
                            |ui| {
                                ui.add_sized(
                                    [
                                        ui.available_width(),
                                        studio_ui_foundation::theme::CONTROL_HEIGHT,
                                    ],
                                    egui::Button::new("Add package"),
                                )
                            },
                        )
                        .inner
                        .clicked()
                    {
                        self.markers.draft.add_package();
                    }
                    let mut remove = None;
                    for (index, mapping) in self.markers.draft.packages.iter_mut().enumerate() {
                        ui.push_id(index, |ui| {
                            ui.separator();
                            let number = index + 1;
                            ui.horizontal(|ui| {
                                studio_ui_foundation::theme::strong(
                                    ui,
                                    format!("Package {number}"),
                                );
                                let response = ui.add_sized(
                                    [80.0, studio_ui_foundation::theme::CONTROL_HEIGHT],
                                    egui::Button::new("Remove"),
                                );
                                response.widget_info(|| {
                                    egui::WidgetInfo::labeled(
                                        egui::WidgetType::Button,
                                        response.enabled(),
                                        format!("Remove package {number}"),
                                    )
                                });
                                if response.clicked() {
                                    remove = Some(index);
                                }
                            });
                            edit(
                                ui,
                                &format!("Package {number} name"),
                                &mut mapping.package,
                                MAX_NAME_BYTES,
                                "my_robot_description",
                            );
                            edit(
                                ui,
                                &format!("Package {number} folder"),
                                &mut mapping.path,
                                MAX_PATH_BYTES,
                                "/absolute/package/folder",
                            );
                        });
                    }
                    if let Some(index) = remove {
                        self.markers.draft.packages.remove(index);
                    }
                });
                ui.collapsing("Approved roots (read only)", |ui| {
                    if let Some(approved) = &self.markers.approved {
                        ui.small("These active roots do not change when you edit the draft.");
                        if !approved.root.trim().is_empty() {
                            read_only(ui, "Approved asset folder", approved.root.trim());
                        }
                        for (index, mapping) in approved.packages.iter().enumerate() {
                            read_only(
                                ui,
                                &format!("Approved package {} name", index + 1),
                                mapping.package.trim(),
                            );
                            read_only(
                                ui,
                                &format!("Approved package {} folder", index + 1),
                                mapping.path.trim(),
                            );
                        }
                    } else {
                        ui.small("No roots are approved.");
                    }
                });
                crate::ui::reveal_focused_control(ui);
            });
        if before != self.markers.draft {
            self.markers.asset_error = None;
            self.markers.asset_notice = None;
        }
        // Process fields before applying an explicit action from this frame.
        if approve {
            if let Err(error) = self.markers.review(self.markers.draft.clone()) {
                self.markers.asset_error = Some(error);
            }
        }
        if discard {
            self.markers.discard_asset_draft();
        }
        if reload {
            if let Err(error) = self.markers.reload_assets() {
                self.markers.asset_error = Some(error);
            }
        }
        if revoke {
            self.markers.revoke_assets();
        }
    }
}

#[cfg(test)]
#[path = "marker_assets_tests.rs"]
mod tests;
