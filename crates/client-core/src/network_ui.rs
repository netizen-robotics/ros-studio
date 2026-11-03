//! Shared network settings. Host enumeration and preferences IO live on one worker.
use eframe::egui;
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::JoinHandle,
    time::Duration,
};
use studio_network::{
    adapters::{self, Adapter},
    preferences::{self, Preferences},
    profiles, ConnectionConfig, Middleware, Selection,
};

#[derive(Clone, Default)]
struct Snapshot {
    loaded: bool,
    preferences: Preferences,
    adapters: Vec<Adapter>,
    error: Option<String>,
    revision: u64,
    #[cfg(windows)]
    firewall_result: Option<Result<String, String>>,
    #[cfg(windows)]
    firewall_busy: bool,
}
enum Job {
    Save(Preferences),
    #[cfg(windows)]
    Firewall(studio_network::firewall::Request),
}
struct Worker {
    jobs: mpsc::SyncSender<Job>,
    snapshot: Arc<Mutex<Snapshot>>,
    cancel: Arc<AtomicBool>,
    join: Option<JoinHandle<()>>,
}
impl Worker {
    fn new(application: &'static str) -> Self {
        let (jobs, receive) = mpsc::sync_channel::<Job>(1);
        let snapshot = Arc::new(Mutex::new(Snapshot::default()));
        let state = snapshot.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let stopped = cancel.clone();
        let join = std::thread::spawn(move || {
            let path = preferences::path(application);
            let loaded = path
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|path| preferences::load(path));
            {
                let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
                match loaded {
                    Ok(p) => s.preferences = p,
                    Err(error) => s.error = Some(error),
                }
                s.loaded = true;
                s.revision += 1;
            }
            let mut next = std::time::Instant::now();
            while !stopped.load(Ordering::Relaxed) {
                if std::time::Instant::now() >= next {
                    let list = adapters::enumerate();
                    let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
                    match list {
                        Ok(adapters) => s.adapters = adapters,
                        Err(error) => s.error = Some(error),
                    }
                    next = std::time::Instant::now() + Duration::from_secs(2);
                }
                match receive.recv_timeout(Duration::from_millis(100)) {
                    #[cfg(windows)]
                    Ok(Job::Firewall(request)) => {
                        let result = studio_network::windows::request(&request, &stopped);
                        let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
                        s.firewall_result = Some(result);
                        s.firewall_busy = false;
                    }
                    Ok(Job::Save(p)) => {
                        let result = path
                            .as_ref()
                            .map_err(Clone::clone)
                            .and_then(|path| preferences::save(path, &p));
                        let mut s = state.lock().unwrap_or_else(|e| e.into_inner());
                        match result {
                            Ok(()) => {
                                s.preferences = p;
                                s.error = None;
                            }
                            Err(error) => {
                                s.error = Some(format!("Could not save network settings: {error}"))
                            }
                        }
                        s.revision += 1;
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => (),
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
        });
        Self {
            jobs,
            snapshot,
            cancel,
            join: Some(join),
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

pub struct NetworkSettings {
    worker: Option<Worker>,
    #[cfg(windows)]
    application: &'static str,
    #[cfg(windows)]
    public_access: bool,
    #[cfg(windows)]
    public_peers: String,
    snapshot: Snapshot,
    draft: Preferences,
    revision: u64,
    open: bool,
    control_rect: Option<egui::Rect>,
    advanced: bool,
    message: String,
    custom_path: String,
    pending_save: bool,
    overrides: Vec<String>,
}
impl NetworkSettings {
    pub fn new(application: &'static str) -> Self {
        Self {
            worker: Some(Worker::new(application)),
            #[cfg(windows)]
            application,
            #[cfg(windows)]
            public_access: false,
            #[cfg(windows)]
            public_peers: String::new(),
            snapshot: Snapshot::default(),
            draft: Preferences::default(),
            revision: 0,
            open: false,
            control_rect: None,
            advanced: false,
            message: String::new(),
            custom_path: String::new(),
            pending_save: false,
            overrides: profiles::inherited_overrides(),
        }
    }
    /// Safe local-only state for headless App regression harnesses; never activates IO.
    pub fn local_fixture() -> Self {
        let preferences = Preferences {
            selection: Selection::Local,
            managed_environment_approved: true,
            ..Default::default()
        };
        Self {
            worker: None,
            #[cfg(windows)]
            application: "tools",
            #[cfg(windows)]
            public_access: false,
            #[cfg(windows)]
            public_peers: String::new(),
            snapshot: Snapshot {
                loaded: true,
                preferences: preferences.clone(),
                ..Default::default()
            },
            draft: preferences,
            revision: 0,
            open: false,
            control_rect: None,
            advanced: false,
            message: String::new(),
            custom_path: String::new(),
            pending_save: false,
            overrides: vec![],
        }
    }
    pub fn poll(&mut self) {
        if let Some(worker) = &self.worker {
            self.snapshot = worker
                .snapshot
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone();
        }
        self.accept_snapshot();
    }
    fn accept_snapshot(&mut self) {
        if self.snapshot.revision != self.revision {
            let replace_draft = self.revision == 0 || self.snapshot.error.is_none();
            if self.revision > 0 && self.snapshot.error.is_none() {
                self.message = "Network settings saved".into();
            }
            self.revision = self.snapshot.revision;
            self.pending_save = false;
            if replace_draft {
                self.draft = self.snapshot.preferences.clone();
                self.custom_path = match &self.draft.selection {
                    Selection::Custom {
                        profile: Some(path),
                    } => path.clone(),
                    _ => String::new(),
                };
            }
        }
    }
    fn save(&mut self) {
        if matches!(self.draft.selection, Selection::Custom { .. }) {
            self.draft.selection = Selection::Custom {
                profile: (!self.custom_path.trim().is_empty())
                    .then(|| self.custom_path.trim().into()),
            };
        }
        if let Some(worker) = &self.worker {
            self.message = match worker.jobs.try_send(Job::Save(self.draft.clone())) {
                Ok(()) => {
                    self.pending_save = true;
                    "Saving settings…".into()
                }
                Err(_) => "Settings worker is busy. Choose Save again.".into(),
            };
        } else {
            self.snapshot.preferences = self.draft.clone();
        }
    }
    pub fn config(&mut self, domain: u16, cyclone: bool) -> Result<ConnectionConfig, String> {
        self.poll();
        #[cfg(windows)]
        if self.snapshot.firewall_busy {
            return Err("Finish network access setup before connecting".into());
        }
        if self.pending_save {
            return Err("Saving network settings. Wait, then Connect explicitly.".into());
        }
        if !self.snapshot.loaded {
            self.open = true;
            return Err("Preparing network adapters…".into());
        }
        if let Some(error) = &self.snapshot.error {
            self.open = true;
            return Err(error.clone());
        }
        let mut preferences = self.snapshot.preferences.clone();
        if !matches!(preferences.selection, Selection::Custom { .. })
            && !preferences.managed_environment_approved
            && !self.overrides.is_empty()
        {
            self.open = true;
            return Err(
                "Inherited DDS configuration detected. Review Network settings before connecting."
                    .into(),
            );
        }
        let resolved = adapters::resolve(&preferences.selection, &self.snapshot.adapters)
            .inspect_err(|_| self.open = true)?;
        if matches!(preferences.selection, Selection::Lan) {
            if let Some((id, _)) = resolved {
                preferences.selection = Selection::Adapter { id };
                self.draft = preferences.clone();
                self.save();
            }
        }
        Ok(ConnectionConfig {
            domain,
            middleware: Middleware::from_cyclone(cyclone),
            selection: preferences.selection,
            managed_environment_approved: preferences.managed_environment_approved,
        })
    }
    pub fn control(&mut self, ui: &mut egui::Ui) {
        self.poll();
        let control = studio_ui_foundation::theme::action_slot(
            ui,
            true,
            132.0,
            egui::Button::new("Network settings"),
        );
        self.control_rect = Some(control.rect);
        if control.clicked() {
            self.open = !self.open;
        }
        let summary = if !self.snapshot.loaded {
            "Preparing network…".into()
        } else {
            match adapters::resolve(
                &self.snapshot.preferences.selection,
                &self.snapshot.adapters,
            ) {
                Ok(Some((id, ip))) => {
                    let name = self
                        .snapshot
                        .adapters
                        .iter()
                        .find(|a| a.id == id)
                        .map(|a| a.name.as_str())
                        .unwrap_or("Local computer");
                    format!("{name} · {ip}")
                }
                Ok(None) => "Custom configuration".into(),
                Err(_) => "Choose a network adapter".into(),
            }
        };
        ui.add_sized([200.0, 30.0], egui::Label::new(&summary).truncate())
            .on_hover_text(summary);
    }
    #[cfg(windows)]
    fn firewall(&mut self, operation: studio_network::firewall::Operation) {
        let result = (|| {
            let (id, _) = adapters::resolve(&self.draft.selection, &self.snapshot.adapters)?
                .ok_or("Choose a managed LAN adapter for network setup")?;
            if id == "loopback" {
                return Err("Local computer mode needs no LAN firewall rule".into());
            }
            let request = studio_network::firewall::Request {
                application: if self.application == "tools" {
                    studio_network::firewall::Application::Tools
                } else {
                    studio_network::firewall::Application::Visualizer
                },
                operation,
                adapter_id: id,
                scope: if self.public_access {
                    studio_network::firewall::Scope::Public {
                        peers: self
                            .public_peers
                            .split(',')
                            .map(|p| p.trim().to_owned())
                            .filter(|p| !p.is_empty())
                            .collect(),
                    }
                } else {
                    studio_network::firewall::Scope::Private
                },
            };
            request.validate()?;
            let worker = self.worker.as_ref().ok_or("Network worker unavailable")?;
            // Mark busy before queueing; the UI cannot submit overlapping prompts.
            {
                let mut s = worker.snapshot.lock().unwrap_or_else(|e| e.into_inner());
                s.firewall_busy = true;
                s.firewall_result = None;
            }
            if let Err(error) = worker.jobs.try_send(Job::Firewall(request)) {
                worker
                    .snapshot
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .firewall_busy = false;
                return Err(error.to_string());
            }
            Ok(())
        })();
        if let Err(error) = result {
            self.message = error;
        }
    }
    pub fn finish_shutdown(&mut self) {
        self.worker.take();
    }
    pub fn panel(&mut self, ctx: &egui::Context, active: bool) {
        self.panel_with_guidance(ctx, active, true);
    }
    /// Hosts can move background guidance to Help while retaining actionable errors.
    pub fn panel_with_guidance(&mut self, ctx: &egui::Context, active: bool, guidance: bool) {
        self.poll();
        if !self.open {
            return;
        }
        let mut open = self.open;
        let anchor = self.control_rect.map_or(egui::pos2(12.0, 90.0), |rect| {
            egui::pos2(
                rect.left().min(ctx.screen_rect().right() - 414.0).max(12.0),
                rect.bottom() + 8.0,
            )
        });
        let panel_height = (ctx.screen_rect().bottom() - anchor.y - 70.0).clamp(120.0, 320.0);
        egui::Window::new("Network settings").id(egui::Id::new("studio-network-settings"))
            .open(&mut open).collapsible(false).resizable(false).fixed_pos(anchor)
            .default_width(384.0).max_width(384.0).show(ctx,|ui| {
            ui.set_width(384.0);
            ui.style_mut().wrap_mode=Some(egui::TextWrapMode::Wrap);
            if active {ui.small("Disconnect to edit these settings.");}
            if !self.snapshot.loaded {ui.label("Preparing adapters…");return;}
            egui::ScrollArea::vertical().max_height(panel_height).show(ui,|ui| {
                ui.add_enabled_ui(!active,|ui| {
                    egui::Grid::new("network-fields").num_columns(2).min_col_width(76.0).spacing(egui::vec2(12.0, 8.0)).show(ui, |ui| {
                        let label=ui.add(egui::Label::new("Discovery").extend());
                        egui::ComboBox::from_id_salt("network-discovery").width(280.0).selected_text(match self.draft.selection {Selection::Local=>"Local computer",Selection::Custom{..}=>"Custom configuration",_=>"LAN"}).show_ui(ui,|ui| {
                        let lan=matches!(self.draft.selection,Selection::Lan|Selection::Adapter{..});
                        if ui.selectable_label(lan,"LAN").clicked(){self.draft.selection=Selection::Lan;}
                        ui.selectable_value(&mut self.draft.selection,Selection::Local,"Local computer");
                        if ui.selectable_label(matches!(self.draft.selection,Selection::Custom{..}),"Custom configuration").clicked(){self.draft.selection=Selection::Custom{profile:None};}
                        }).response.labelled_by(label.id);
                        ui.end_row();
                    if matches!(self.draft.selection,Selection::Lan|Selection::Adapter{..}) {
                        ui.add(egui::Label::new("Adapter").extend());
                        let selected=match &self.draft.selection {Selection::Adapter{id}=>self.snapshot.adapters.iter().find(|a|&a.id==id).map(|a|a.name.as_str()).unwrap_or("Selected adapter unavailable"),_=>"Automatic physical LAN adapter"};
                        let choice=egui::ComboBox::from_id_salt("network-adapter").selected_text(selected).truncate().width(280.0).show_ui(ui,|ui| {
                            ui.selectable_value(&mut self.draft.selection,Selection::Lan,"Automatic physical LAN adapter");
                            for a in &self.snapshot.adapters {if a.usable() && (a.physical||self.advanced) {ui.selectable_value(&mut self.draft.selection,Selection::Adapter{id:a.id.clone()},format!("{} ({})",a.name,a.addresses.iter().map(ToString::to_string).collect::<Vec<_>>().join(", ")));}}
                        });
                        choice.response.widget_info(|| {
                            let mut info=egui::WidgetInfo::labeled(egui::WidgetType::ComboBox,choice.response.enabled(),"Network adapter");
                            info.current_text_value=Some(selected.into());info
                        });
                        ui.end_row();
                    }
                    });
                    if matches!(self.draft.selection,Selection::Lan|Selection::Adapter{..}) {
                        match adapters::resolve(&self.draft.selection,&self.snapshot.adapters) {Ok(Some((_,ip)))=>{ui.small(format!("Address: {ip}"));},Err(error)=>{ui.colored_label(ui.visuals().error_fg_color,error);},_=>()}
                        ui.collapsing("Adapter options", |ui| {ui.checkbox(&mut self.advanced,"Include VPN and virtual adapters");});
                    }
                    if guidance && matches!(self.draft.selection,Selection::Local){ui.label("Discover ROS processes on this computer through loopback only.");}
                    if matches!(self.draft.selection,Selection::Custom{..}) {
                        if guidance { ui.label("Use inherited discovery settings. Optionally select a middleware XML file; it is never modified."); }
                        let label=ui.label("XML file");ui.add(studio_ui_foundation::theme::input(ui, &mut self.custom_path).char_limit(4096).desired_width(f32::INFINITY).hint_text(studio_ui_foundation::theme::hint(ui, "Optional absolute path"))).labelled_by(label.id);
                    } else if !self.overrides.is_empty() && !self.draft.managed_environment_approved {
                        ui.label("Inherited DDS settings detected. Managed mode replaces these only in this app's backend process.");
                        ui.label(self.overrides.join(", "));
                        if ui.button("Use managed settings").clicked(){self.draft.managed_environment_approved=true;}
                        if ui.button("Use inherited configuration").clicked(){self.draft.selection=Selection::Custom{profile:None};}
                    }
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {if ui.add_enabled(!self.pending_save,egui::Button::new("Save settings")).clicked(){self.save();}ui.label(&self.message);});
                });
                #[cfg(windows)]
                ui.add_enabled_ui(!active && !self.snapshot.firewall_busy,|ui| {
                    ui.separator();ui.strong("Windows network access");
                    if studio_network::MANAGED_FIREWALL_AVAILABLE { ui.label("Default: inbound backend UDP on the selected adapter, Private networks, local subnet. Setup requests administrator approval.");
                    ui.checkbox(&mut self.public_access,"Advanced: Public network access");
                    if self.public_access { ui.label("Explicitly approve IPv4 peers or subnets, separated by commas. Your network category is unchanged.");ui.add(studio_ui_foundation::theme::input(ui, &mut self.public_peers).char_limit(2048).hint_text(studio_ui_foundation::theme::hint(ui, "192.0.2.10, 192.0.2.11"))); }
                    ui.horizontal_wrapped(|ui| {if ui.button("Check network access").clicked(){self.firewall(studio_network::firewall::Operation::Inspect);}
                        if ui.button("Check/repair network access").clicked(){self.firewall(studio_network::firewall::Operation::Repair);}}); } else { ui.label("Network permissions are managed by Windows. If LAN access is blocked, review Windows Security or contact your administrator."); }
                });
                #[cfg(windows)] {
                    if self.snapshot.firewall_busy {ui.label("Waiting for network access setup…");}
                    if let Some(result)=&self.snapshot.firewall_result {match result {Ok(message)=>{ui.label(message);},Err(error)=>{ui.colored_label(ui.visuals().error_fg_color,error);}}}
                }
                if let Some(error)=&self.snapshot.error {ui.colored_label(ui.visuals().error_fg_color,error);}
                if guidance {
                ui.small("Saved settings apply on the next Connect.");
                ui.collapsing("Connection help", |ui| {
                    ui.label("Match the publisher’s domain and middleware. An empty topic list alone does not identify a firewall problem.");
                    #[cfg(target_os="macos")]
                    ui.label("If local-network access is denied, allow this app in System Settings → Privacy & Security → Local Network, then reconnect.");
                });
                }
            });
        });
        self.open = open;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn settings_overlay_preserves_work_area_and_locks_active_session_edits() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        studio_ui_foundation::theme::configure(&ctx);
        let mut settings = NetworkSettings::local_fixture();
        let mut baseline = None;
        for open in [false, true] {
            settings.open = open;
            for _ in 0..3 {
                let mut work_area = egui::Rect::NOTHING;
                let out = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(800., 600.),
                        )),
                        ..Default::default()
                    },
                    |ctx| {
                        egui::TopBottomPanel::top("test-connection")
                            .show(ctx, |ui| settings.control(ui));
                        settings.panel(ctx, true);
                        egui::CentralPanel::default().show(ctx, |ui| work_area = ui.max_rect());
                    },
                );
                if let Some(previous) = baseline {
                    assert_eq!(work_area, previous, "settings displaced the scene");
                }
                baseline = Some(work_area);
                if open {
                    let tree = out.platform_output.accesskit_update.unwrap();
                    let save = tree
                        .nodes
                        .iter()
                        .find(|(_, node)| node.label() == Some("Save settings"))
                        .expect("save remains visible");
                    assert!(
                        save.1.is_disabled(),
                        "active sessions cannot change settings"
                    );
                }
            }
        }
        assert!(matches!(
            settings.config(187, false).unwrap().selection,
            Selection::Local
        ));
    }
    #[test]
    fn unsaved_and_failed_save_drafts_do_not_replace_active_intent() {
        let mut settings = NetworkSettings::local_fixture();
        settings.draft.selection = Selection::Custom { profile: None };
        assert!(matches!(
            settings.config(187, false).unwrap().selection,
            Selection::Local
        ));
        settings.revision = 1;
        settings.snapshot.revision = 2;
        settings.snapshot.error = Some("Permission denied".into());
        settings.accept_snapshot();
        assert!(matches!(settings.draft.selection, Selection::Custom { .. }));
        settings.snapshot.error = None;
        settings.save();
        assert!(matches!(
            settings.config(187, true).unwrap().selection,
            Selection::Custom { .. }
        ));
    }
}
