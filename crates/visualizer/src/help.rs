//! Task guidance and passive navigation; never owns ROS commands or file I/O.
use super::*;
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(crate) enum Topic {
    #[default]
    Start,
    Workspaces,
    Troubleshooting,
}
#[derive(Default)]
pub(crate) struct HelpUi {
    pub open: bool,
    topic: Topic,
}
#[derive(Clone, Copy)]
pub(crate) enum Destination {
    Workspace,
    Scene,
    Diagnostics,
}
impl App {
    pub(crate) fn show_help(&mut self, topic: Topic) {
        self.help.open = true;
        self.help.topic = topic;
    }
    pub(crate) fn help_navigate(&mut self, destination: Destination) {
        match destination {
            Destination::Workspace => {
                self.workspace.open = true;
                self.workspace.focus_path = true;
            }
            Destination::Scene => {
                if self.workspace_frozen() {
                    return;
                }
                self.open_scene_settings();
            }
            Destination::Diagnostics => self.diagnostics_open = true,
        }
        self.help.open = false;
    }
    pub(crate) fn help_window_ui(&mut self, ctx: &egui::Context) {
        let mut open = self.help.open;
        let mut destination = None;
        egui::Window::new("Visualizer guide")
            .open(&mut open)
            .collapsible(false)
            .resizable(true)
            .default_size(egui::vec2(560.0, 490.0))
            .min_size(egui::vec2(340.0, 340.0))
            .max_size((ctx.screen_rect().size() - egui::vec2(32.0, 32.0)).max(egui::vec2(340.0, 340.0)))
            .show(ctx, |ui| {
                let columns = if ui.available_width() >= 460.0 { 3 } else { 1 };
                let width = (ui.available_width() - (columns - 1) as f32 * ui.spacing().item_spacing.x) / columns as f32;
                egui::Grid::new("help-topics").num_columns(columns).show(ui, |ui| {
                    for (i, (topic, label)) in [(Topic::Start, "Start viewing"), (Topic::Workspaces, "Workspaces"), (Topic::Troubleshooting, "Troubleshooting")].into_iter().enumerate() {
                        let selected = self.help.topic == topic;
                        let widget = if selected { shell::primary(ui.visuals(), label) } else { egui::Button::new(label) };
                        let response = ui.add_sized([width, studio_ui_foundation::theme::CONTROL_HEIGHT], widget);
                        response.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, selected, label));
                        if response.clicked() { self.help.topic = topic; }
                        if (i + 1) % columns == 0 { ui.end_row(); }
                    }
                });
                ui.separator();
                egui::ScrollArea::vertical().id_salt(("help-content", self.help.topic as u8)).auto_shrink([false, false]).show(ui, |ui| {
                    let mut section = |title: &str, body: &str| {
                        studio_ui_foundation::theme::strong(ui, title);
                        ui.add(egui::Label::new(body).wrap().selectable(true));
                        ui.add_space(8.0);
                    };
                    match self.help.topic {
                        Topic::Start => {
                            section("1. Connect to your ROS graph", "Choose the ROS Domain and matching Middleware, then Connect. Connection alone discovers topics; it does not enable displays. This app observes data and sends no robot motion commands.");
                            section("2. Show a topic", "Find a topic in Available topics and use its eye action (Show). Show starts the default compatible display; using it again selects that same source. Display offers alternatives, including calibrated Camera. Inspect and selecting an In view display open its properties without starting or replacing reception. Its reception checkbox starts or stops data.");
                            section("3. Adjust the scene", "The applied frame stays visible on the left. The Scene tab shows the fixed frame and transform form in the Inspector. Edits remain drafts until Apply. Applying changed scene settings stops affected displays and waits for cleanup; enable them again explicitly. Image displays continue receiving. Appearance controls update locally without restarting reception.");
                            section("Move around", "Views stays above the right inspector: choose Orbit or Top down, adjust distance or height to zoom, and fit or focus geometry. Camera… opens precise settings and saved views. In a compact window, Views stays above the scene and framing actions move into Camera…. Display details opens an inspector below Views. Image and Camera companions appear below the scene; switching tabs does not stop their reception.");
                            section("Robot and mesh assets", "Load robot model opens explicit setup. Review local asset folders before loading. Marker Mesh assets has separate drafts and approved roots. Approval lasts for this session and is never saved in a workspace.");
                            if ui.add_enabled(!self.workspace_frozen(), egui::Button::new("Review Scene settings")).clicked() { destination = Some(Destination::Scene); }
                        }
                        Topic::Workspaces => {
                            section("Save your viewing setup", "File > Save workspace saves display sources, appearance and layout. The window title shows the filename and an asterisk for unsaved changes. Workspace name is a label inside the file, separate from its filename. Runtime data and active subscriptions are not saved.");
                            section("Open, review, then replace", "Open previews a workspace without replacing your current setup. Save unsaved changes first, or explicitly discard them when replacing. Replacement waits for current display cleanup. Restored displays stay disabled and Robot stays unloaded. Domain and middleware do not change.");
                            section("Recover a file error", "Manage workspace shows full error details and a local file path alternative to the chooser. For Open, check the path and choose a compatible Visualizer file. For Save, choose a writable destination. A canceled save may already have changed its destination, so check the file before trying again.");
                            section("Cancel or close", "Cancel operation abandons replacement and keeps your current configuration. Stops already issued are not undone. Closing the workspace window preserves the path draft and does not cancel a running operation. Open it again to check progress or cancel. Reconnect never restarts displays automatically.");
                            #[cfg(target_os = "macos")]
                            section("File chooser on macOS", "Keyboard Quit can be ignored while the native chooser is pending. Cancel the chooser first, then press Cmd-Q. Manual local paths are available in Manage workspace.");
                            if ui.button("Open workspace management").clicked() { destination = Some(Destination::Workspace); }
                        }
                        Topic::Troubleshooting => {
                            section("No topics", "Check Domain and Middleware, then the publisher and ROS network. Changing connection settings requires Disconnect. Search and topic filters can hide available topics. Needs attention lists current waiting and recovery causes. Diagnostics keeps earlier history separately; acknowledgment never hides an unresolved cause. Topics defaults to displayable sources. Topic filters can include nonvisual sources with their role or limitation.");
                            section("Starting, waiting or stopped", "Starting reserves resources until the backend confirms the display. Waiting means reception has started but usable data may still be missing. Inspect the selected layer status and Advanced source/QoS. Stop and cancellation wait for cleanup. Reconnect and workspace restore leave reception disabled.");
                            section("Missing spatial geometry", "Check the applied fixed frame, transform topics and publisher frames. Data needs a transform at its acquisition time. Missing or incompatible transforms hide affected geometry rather than silently placing it at the origin. Source and Scene edits require Apply; canceled Apply does not undo issued Stops.");
                            section("Image or calibrated Camera", "Image shows supported raw pixels without scene alignment. Camera also needs an already rectified Image, compatible CameraInfo and transforms at Image time. Inspect its waiting reason and source inputs. Invalid calibration or transforms hide the alignment; ordinary incoming frames retain the last completed compatible image during replacement. Raw undistortion and simulation-time synchronization are unavailable.");
                            section("Missing Marker mesh", "Open Mesh assets from a Marker layer. Add package names and absolute folder paths, then Approve roots. Editing alone grants no access. Reload uses the approved roots; Discard restores them; Revoke withdraws access. Missing or outside-root assets stay denied while other Marker shapes can continue receiving.");
                            section("Cleanup or backend failure", "Review the selected layer or workspace cleanup status. Retry failed stops explicitly where offered. If cleanup cannot finish, Disconnect and review the setup again. Never assume cancellation has re-enabled stopped displays.");
                            if ui.button("Review diagnostics").clicked() { destination = Some(Destination::Diagnostics); }
                            if ui.add_enabled(!self.workspace_frozen(), egui::Button::new("Review Scene settings")).clicked() { destination = Some(Destination::Scene); }
                        }
                    }
                    crate::ui::reveal_focused_control(ui);
                });
            });
        self.help.open = open;
        if let Some(destination) = destination {
            self.help_navigate(destination);
        }
    }
}
