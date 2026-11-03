//! Menus, fixed connection geometry and the Workbench navigation rail.
use super::*;
impl Tools {
    pub(crate) fn workbench_shell(&mut self, ctx: &egui::Context, frozen: bool) {
        egui::TopBottomPanel::top("tools-menu")
            .frame(
                egui::Frame::new()
                    .fill(ctx.style().visuals.panel_fill)
                    .inner_margin(egui::Margin::symmetric(theme::PADDING, 4)),
            )
            .show(ctx, |ui| {
                ui.spacing_mut().interact_size.y = theme::COMPACT_HEIGHT;
                egui::menu::bar(ui, |ui| {
                    ui.menu_button("File", |ui| {
                        if ui.button("Manage workspace…").clicked() {
                            self.files_open = true;
                            ui.close_menu();
                        }
                        ui.add_enabled_ui(!self.files.is_busy() && !frozen, |ui| {
                            if ui.button("Open workspace…").clicked() {
                                if let Err(e) = self.files.open() {
                                    self.status = e;
                                }
                                ui.close_menu();
                            }
                            if ui.button("Save workspace").clicked() {
                                if let Err(e) = self.files.save() {
                                    self.status = e;
                                }
                                ui.close_menu();
                            }
                            if ui.button("Save workspace as…").clicked() {
                                if let Err(e) = self.files.save_as() {
                                    self.status = e;
                                }
                                ui.close_menu();
                            }
                        });
                        ui.separator();
                        theme::exit_menu_item(ui);
                    });
                    ui.menu_button("Help", |ui| {
                        if ui.button("Tools workflow").clicked() {
                            self.help_open = true;
                            ui.close_menu();
                        }
                        if ui.button("About / licenses").clicked() {
                            self.about = true;
                            ui.close_menu();
                        }
                    });
                });
            });
        egui::TopBottomPanel::top("tools-connection")
            .frame(
                egui::Frame::side_top_panel(&ctx.style())
                    .inner_margin(egui::Margin::symmetric(theme::PADDING, 4)),
            )
            .show(ctx, |ui| {
                theme::appearance_toolbar(ui, |ui| {
                    theme::connection_fields(
                        ui,
                        "middleware",
                        &mut self.domain,
                        &mut self.cyclone,
                        !self.connected && !self.connecting && !frozen && self.retiring.is_none(),
                    );
                    let label = if self.connecting {
                        "Cancel startup"
                    } else if self.session.is_some() {
                        "Disconnect"
                    } else if self.retiring.is_some() {
                        "Cleaning up"
                    } else {
                        "Connect"
                    };
                    let button = if self.session.is_some() {
                        theme::disconnect(ui.visuals(), label)
                    } else if self.connecting || self.retiring.is_some() {
                        egui::Button::new(label)
                    } else {
                        theme::primary(ui.visuals(), label)
                    };
                    if theme::action_slot(
                        ui,
                        !frozen && (self.session.is_some() || self.retiring.is_none()),
                        theme::CONNECTION_ACTION_WIDTH,
                        button,
                    )
                    .clicked()
                    {
                        if self.session.is_some() {
                            self.disconnect();
                        } else {
                            self.connect(ctx);
                        }
                    }
                    self.network.control(ui);
                });
            });
        self.network.panel_with_guidance(
            ctx,
            self.session.is_some() || self.retiring.is_some() || frozen,
            false,
        );
        egui::TopBottomPanel::bottom("tools-status")
            .exact_height(theme::CONTROL_HEIGHT + 8.0)
            .frame(theme::strip_frame(&ctx.style().visuals))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    let width = (ui.available_width() - 148.0).max(120.0);
                    ui.allocate_ui_with_layout(
                        egui::vec2(width, theme::CONTROL_HEIGHT),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.add(
                                egui::Label::new(egui::RichText::new(&self.status).small())
                                    .truncate(),
                            )
                            .on_hover_text(&self.status);
                        },
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .add_sized(
                                [132., theme::CONTROL_HEIGHT],
                                egui::Button::new("Diagnostics"),
                            )
                            .clicked()
                        {
                            self.diagnostics.request(false);
                        }
                        if self.health.running() {
                            ui.small(format!("ROS health: {}", self.health.label()));
                        }
                    });
                });
            });
        if ctx.screen_rect().width() < 900.0 {
            egui::TopBottomPanel::top("tools-navigation-compact").show(ctx, |ui| {
                ui.add_enabled_ui(!frozen, |ui| self.workbench.navigation(ui, true));
            });
        } else {
            egui::SidePanel::left("tools-navigation")
                .exact_width(132.0)
                .resizable(false)
                .frame(
                    egui::Frame::side_top_panel(&ctx.style())
                        .fill(ctx.style().visuals.extreme_bg_color)
                        .inner_margin(8),
                )
                .show(ctx, |ui| {
                    ui.add_space(8.0);
                    ui.add_enabled_ui(!frozen, |ui| self.workbench.navigation(ui, false));
                });
        }
        if self.help_open {
            egui::Window::new("Tools workflow").open(&mut self.help_open).default_width(480.0).vscroll(true).show(ctx,|ui|{
                crate::presentation::section_heading(ui, "Inspect data");
                ui.label("Connect, choose an interface, then Start receiving. Use Messages, Plot or Image to inspect its data. Browsing leaves existing views unchanged.");
                crate::presentation::section_heading(ui, "Send requests");
                ui.label("Review the draft, then explicitly publish, call a service, send a goal or apply parameters. Nothing is sent by selecting an interface or reconnecting.");
                ui.label("Stop waiting and Stop tracking affect local monitoring. Cancel this goal requests remote cancellation; check the terminal result.");
                crate::presentation::section_heading(ui, "Save and reconnect");
                ui.label("Workspaces save layout and drafts. Restored and reconnected sessions start inactive. Legacy imports preserve the original file.");
                crate::presentation::section_heading(ui, "Network access");
                ui.label("Network settings apply on the next connection. An empty list can also mean discovery or middleware configuration needs attention.");
                #[cfg(target_os="macos")]
                ui.label("If local-network access is denied, allow Tools in System Settings → Privacy & Security → Local Network, then reconnect. Close native file choosers before Cmd-Q.");
            });
        }
    }
}
