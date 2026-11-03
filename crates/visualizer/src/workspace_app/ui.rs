//! Workspace recovery UI. File I/O and cleanup remain controller-owned.
use super::*;

fn button(ui: &mut egui::Ui, label: &str, enabled: bool, width: f32) -> bool {
    ui.add_enabled_ui(enabled, |ui| {
        ui.add_sized(
            [width, studio_ui_foundation::theme::CONTROL_HEIGHT],
            egui::Button::new(label).truncate(),
        )
    })
    .inner
    .clicked()
}
fn text(ui: &mut egui::Ui, value: impl Into<egui::WidgetText>) {
    ui.add(egui::Label::new(value).selectable(true).wrap());
}
impl App {
    pub(crate) fn workspace_render(&mut self, ui: &mut egui::Ui) {
        let mut action = None;
        let available = self.workspace_file_available();
        let busy = self.workspace.controller.is_busy();
        let applying = self.workspace.controller.is_applying();
        let candidate = self.workspace.controller.candidate().is_some();
        ui.horizontal(|ui| {
            studio_ui_foundation::theme::strong(ui, "Workspace");
            ui.label(if self.workspace_dirty() {
                "Unsaved changes"
            } else {
                "Saved"
            });
        });
        let width = ((ui.available_width() - 2.0 * ui.spacing().item_spacing.x) / 3.0).max(80.0);
        ui.horizontal(|ui| {
            if button(ui, "Open…", available, width) {
                action = Some("open");
            }
            if button(ui, "Save", available, width) {
                action = Some("save");
            }
            if button(ui, "Save as…", available, width) {
                action = Some("save_as");
            }
        });
        let failed_stops = matches!(self.workspace.controller.restore_state(), file::RestoreState::Stopping { failures, .. } if !failures.is_empty());
        ui.horizontal(|ui| {
            if button(ui, "Cancel operation", busy || applying || candidate, width) {
                action = Some("cancel");
            }
            if button(ui, "Retry failed stops", failed_stops, width) {
                action = Some("retry_stops");
            }
            if button(ui, "Workspace help", true, width) {
                self.show_help(crate::help::Topic::Workspaces);
            }
        });
        // Reserve feedback geometry. Full errors remain selectable in the body.
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 54.0), egui::Sense::hover());
        let mut feedback = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(rect)
                .layout(egui::Layout::top_down(egui::Align::Min)),
        );
        feedback.set_clip_rect(rect.intersect(ui.clip_rect()));
        text(
            &mut feedback,
            if applying {
                "Waiting for display cleanup. Cancel keeps the current workspace; issued Stops are not undone."
            } else if self.workspace_needs_attention() {
                "Workspace needs attention. Review the error and recovery steps below."
            } else if self.workspace.restored_notice {
                "Workspace restored inactive. Enable displays explicitly; reload Robot and approve mesh roots as needed."
            } else {
                self.workspace.controller.status()
            },
        );
        ui.separator();
        let error = self.workspace_error().map(str::to_owned);
        let new_error = error.is_some() && error != self.workspace.last_rendered_error;
        self.workspace.last_rendered_error = error;
        let mut scroll = egui::ScrollArea::vertical().id_salt("workspace-files-scroll");
        if new_error {
            scroll = scroll.vertical_scroll_offset(0.0);
        }
        scroll
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysVisible)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                self.workspace_recovery_ui(ui);
                ui.add_enabled_ui(!self.workspace_frozen(), |ui| {
                    let label = ui.label("Workspace name (inside the file)");
                    ui.add_sized([ui.available_width(), studio_ui_foundation::theme::CONTROL_HEIGHT], studio_ui_foundation::theme::input(ui, &mut self.workspace.name).id(egui::Id::new("workspace-name")).clip_text(true).char_limit(256))
                        .labelled_by(label.id);
                });
                if let Some(path) = self.workspace.controller.path() { workspace_path_ui(ui, path, "Saved workspace"); }
                else { ui.small("No saved file yet. Save chooses a filename for the window title."); }
                ui.separator();
                studio_ui_foundation::theme::strong(ui, "Local file path");
                ui.small("Use an absolute path as an alternative to the file chooser. Open previews a file before replacement.");
                let label = ui.label("Workspace file path");
                let response = ui.add_sized([ui.available_width(), studio_ui_foundation::theme::CONTROL_HEIGHT], studio_ui_foundation::theme::input(ui, &mut self.workspace.path_entry).id(egui::Id::new("workspace-path")).clip_text(true))
                    .labelled_by(label.id);
                if self.workspace.focus_path {
                    response.request_focus();
                    self.workspace.focus_path = false;
                    response.scroll_to_me(Some(egui::Align::Center));
                }
                let half = (ui.available_width() - ui.spacing().item_spacing.x) / 2.0;
                ui.horizontal(|ui| {
                    let enabled = available && !self.workspace.path_entry.trim().is_empty();
                    if button(ui, "Open path", enabled, half) { action = Some("load_path"); }
                    if button(ui, "Save to path", enabled, half) { action = Some("save_path"); }
                });
                #[cfg(target_os = "macos")]
                ui.small("If keyboard Quit is ignored while a chooser is open, cancel the chooser first, then press Cmd-Q.");
                self.workspace_preview_ui(ui, &mut action);
                ui.collapsing("Presets and connection suggestion", |ui| {
                    ui.small("Presets preview inactive configuration. Connection suggestions never change the active connection.");
                    if ui.add_enabled(available, egui::Button::new("Visualize preset")).clicked() { action = Some("visualize"); }
                    if let Some(suggestion) = &self.workspace.connection_suggestion {
                        text(ui, format!("Saved suggestion: domain {} · {}", suggestion.domain, match suggestion.middleware {
                            Some(file::Middleware::FastDds) => "Fast DDS",
                            Some(file::Middleware::CycloneDds) => "Cyclone DDS",
                            None => "middleware unspecified",
                        }));
                    } else { ui.label("No connection suggestion saved."); }
                    if ui.add_enabled(!self.workspace_frozen(), egui::Button::new("Save current settings as suggestion")).clicked() {
                        match self.connection.domain_id() {
                            Ok(domain) => self.workspace.connection_suggestion = Some(file::ConnectionSuggestion { domain, middleware: Some(if self.connection.cyclone { file::Middleware::CycloneDds } else { file::Middleware::FastDds }) }),
                            Err(error) => self.diagnostic(error),
                        }
                    }
                    if ui.add_enabled(!self.workspace_frozen(), egui::Button::new("Remove saved suggestion")).clicked() { self.workspace.connection_suggestion = None; }
                });
                ui.collapsing("Advanced file recovery", |ui| {
                    text(ui, "If the file worker has failed, restart it, then explicitly repeat Open or Save. Restart does not retry or replace anything.");
                    if ui.add_enabled(available, egui::Button::new("Restart file worker")).clicked() { action = Some("restart_worker"); }
                });
                crate::ui::reveal_focused_control(ui);
            });
        if let Some(action) = action {
            self.workspace_action(action);
        }
    }
    fn workspace_recovery_ui(&mut self, ui: &mut egui::Ui) {
        if let Some(error) = self.workspace_error().map(str::to_owned) {
            studio_ui_foundation::theme::strong(ui, "Recover a file operation");
            text(
                ui,
                egui::RichText::new(&error).color(ui.visuals().error_fg_color),
            );
            text(ui, "For Open, check that the file exists and belongs to Visualizer. For Save, choose a writable destination and check the file before retrying a canceled save. Your current configuration stays available.");
            ui.horizontal_wrapped(|ui| {
                if ui.button("Copy error details").clicked() {
                    ui.ctx().copy_text(error);
                }
                if ui.button("Dismiss error").clicked() {
                    self.workspace.controller.acknowledge_error();
                    self.workspace.recovery_error = None;
                }
            });
            ui.separator();
        }
        if let file::RestoreState::Stopping {
            pending, failures, ..
        } = self.workspace.controller.restore_state()
        {
            text(ui, format!("Waiting for {} display cleanup acknowledgements. Replacement has not completed.", pending.len()));
            for failure in failures {
                text(
                    ui,
                    egui::RichText::new(&failure.message).color(ui.visuals().error_fg_color),
                );
            }
            text(ui, "Retry failed stops above, or cancel replacement. If cleanup cannot finish, disconnect, then open and review the file again. Stopped displays require explicit re-enabling.");
        }
    }
    fn workspace_preview_ui(&self, ui: &mut egui::Ui, action: &mut Option<&str>) {
        let Some(candidate) = self.workspace.controller.candidate() else {
            return;
        };
        ui.separator();
        studio_ui_foundation::theme::strong(ui, "Review replacement");
        text(
            ui,
            format!(
                "{} · {} displays",
                candidate.workspace.name,
                candidate.workspace.views.len()
            ),
        );
        if let Some(path) = candidate.path.as_deref() {
            workspace_path_ui(ui, path, "Preview");
        }
        text(ui, "Replacement stops current displays and waits for cleanup. Restored displays stay disabled and Robot stays unloaded. Domain and middleware remain unchanged. Marker roots require fresh approval.");
        if self.workspace_dirty() {
            text(ui, "You have unsaved changes. Save first, or explicitly discard them using the replacement button below.");
        }
        if let Some(issue) = &candidate.issue {
            text(
                ui,
                egui::RichText::new(issue).color(ui.visuals().error_fg_color),
            );
        }
        for excluded in &candidate.excluded {
            text(ui, format!("Excluded from this app: {excluded}"));
        }
        let ready = candidate.preview.is_some() && self.workspace_file_available();
        let width = (ui.available_width() - ui.spacing().item_spacing.x) / 2.0;
        ui.horizontal(|ui| {
            if button(ui, "Review again", self.workspace_file_available(), width) {
                *action = Some("review");
            }
            if button(
                ui,
                "Cancel replacement",
                !self.workspace.controller.is_busy(),
                width,
            ) {
                *action = Some("cancel");
            }
        });
        let label = if self.workspace_dirty() {
            "Discard changes and replace"
        } else {
            "Replace workspace"
        };
        if button(ui, label, ready, ui.available_width()) {
            *action = Some("apply");
        }
        ui.collapsing("Displays in this file", |ui| {
            for view in &candidate.workspace.views {
                text(ui, format!("{} · {}", view.id, view.source_label()));
            }
        });
        ui.collapsing("Configuration details", |ui| {
            let encoded = studio_workspace::application::encode(
                &candidate.workspace,
                studio_workspace::application::Application::Visualizer,
            )
            .ok()
            .and_then(|bytes| String::from_utf8(bytes).ok())
            .unwrap_or_else(|| "Preview unavailable".into());
            text(ui, egui::RichText::new(encoded).monospace());
        });
    }
    pub(crate) fn workspace_action(&mut self, action: &str) {
        self.workspace.restored_notice = false;
        let result = match action {
            "open" => self.workspace_file(FileAction::Open),
            "save" => self.workspace_file(FileAction::Save),
            "save_as" => self.workspace_file(FileAction::SaveAs),
            "load_path" | "save_path" => {
                let path = std::path::PathBuf::from(&self.workspace.path_entry);
                if !path.is_absolute() {
                    Err("Enter an absolute workspace file path, then choose Open path or Save to path.".into())
                } else {
                    self.workspace_observe().and_then(|_| {
                        if action == "load_path" {
                            self.workspace.controller.load_path(path)
                        } else {
                            self.workspace.controller.save_path(path)
                        }
                    })
                }
            }
            "review" => self
                .workspace_observe()
                .and_then(|_| self.workspace.controller.review_candidate()),
            "apply" => self.workspace_begin_apply(if self.workspace_dirty() {
                file::ApplyApproval::DiscardUnsaved
            } else {
                file::ApplyApproval::CleanOrSaved
            }),
            "cancel" => {
                self.workspace.controller.cancel();
                self.workspace.prepared = None;
                Ok(())
            }
            "retry_stops" => self.workspace_retry_stops(),
            "restart_worker" => self.workspace.controller.restart_file_worker(),
            "visualize" => (|| {
                let bytes = file::Preset::Visualize
                    .workspace()
                    .encode()
                    .map_err(|e| e.to_string())?;
                let imported = studio_workspace::application::parse(
                    &bytes,
                    studio_workspace::application::Application::Visualizer,
                )
                .map_err(|e| e.to_string())?;
                self.workspace_review_configuration(imported.workspace)
            })(),
            _ => Ok(()),
        };
        match result {
            Ok(()) => {
                self.workspace.controller.acknowledge_error();
                self.workspace.recovery_error = None;
            }
            Err(error) => {
                self.workspace.recovery_error = Some(error.clone());
                self.diagnostic(error);
            }
        }
    }
}
