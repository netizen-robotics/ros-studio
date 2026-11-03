//! Shared, local-only Robot appearance inspector. The app owns persistence.
use super::{LinkAppearance, RobotAppearance};
use crate::spatial::robot_geometry::PreparedRobot;
use eframe::egui;

#[derive(Default)]
pub struct RobotAppearanceView {
    search: String,
    pub error: Option<String>,
}
fn opacity(ui: &mut egui::Ui, value: &mut u16, label: &str) -> bool {
    let mut alpha = f32::from(*value) / 1000.;
    let changed = ui
        .add(
            egui::Slider::new(&mut alpha, 0.0..=1.0)
                .text(label)
                .custom_formatter(|value, _| format!("{:.0}%", value * 100.)),
        )
        .changed();
    if changed {
        *value = (alpha * 1000.).round() as u16;
    }
    changed
}
fn link_style(settings: &RobotAppearance, name: &str) -> LinkAppearance {
    settings
        .links
        .iter()
        .find(|link| link.name == name)
        .cloned()
        .unwrap_or_else(|| LinkAppearance {
            name: name.into(),
            visible: true,
            opacity_permille: 1000,
        })
}
fn store(settings: &mut RobotAppearance, link: LinkAppearance) {
    settings.links.retain(|old| old.name != link.name);
    if !link.visible || link.opacity_permille != 1000 {
        settings.links.push(link);
    }
}
impl RobotAppearanceView {
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        settings: &mut RobotAppearance,
        model: Option<&PreparedRobot>,
    ) -> bool {
        let before = settings.clone();
        egui::CollapsingHeader::new("Appearance").default_open(true).show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.checkbox(&mut settings.visual, "Visual geometry");
                ui.checkbox(&mut settings.collision, "Collision geometry");
            });
            opacity(ui, &mut settings.opacity_permille, "Robot opacity");
            if ui.button("Reset appearance").clicked() { *settings = Default::default(); self.error = None; }
            if let Some(error) = &self.error { ui.colored_label(ui.visuals().error_fg_color, error); }
            if let Some(model) = model {
                ui.collapsing("Links", |ui| {
                    let caption = ui.label("Search links");
                    ui.add(studio_ui_foundation::theme::input(ui, &mut self.search).hint_text(studio_ui_foundation::theme::hint(ui, "Name contains…")).desired_width(f32::INFINITY)).labelled_by(caption.id);
                    ui.horizontal_wrapped(|ui| {
                        let show = ui.button("Show all links").clicked();
                        let hide = ui.button("Hide all links").clicked();
                        if show || hide { for name in &model.link_names { let mut link=link_style(settings,name);link.visible=show;store(settings,link); } }
                    });
                    let query=self.search.to_lowercase();
                    let mut found=false;
                    for name in model.link_names.iter().filter(|name|name.to_lowercase().contains(&query)) {
                        found=true;
                        ui.push_id(name,|ui| {
                            let mut link=link_style(settings,name);
                            let changed=ui.checkbox(&mut link.visible,name).on_hover_text(name).changed();
                            let alpha=opacity(ui,&mut link.opacity_permille,"Link opacity");
                            if changed || alpha {store(settings,link);}
                        });
                    }
                    if !found {ui.label("No links match this search.");}
                });
            } else { ui.small("Load and review a model to inspect links. Saved appearance stays inactive until Enable."); }
        });
        *settings != before
    }
}
