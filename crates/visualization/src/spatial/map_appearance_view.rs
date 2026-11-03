//! Shared local map appearance controls. Source configuration is owned by roots.
use eframe::egui;
use studio_workspace::map::{Appearance, Palette};
pub fn show(ui: &mut egui::Ui, settings: &mut Appearance) -> bool {
    let before = *settings;
    egui::CollapsingHeader::new("Appearance").default_open(true).show(ui,|ui| {
        let response=egui::ComboBox::from_label("Palette").selected_text(match settings.palette {Palette::Map=>"Map",Palette::Costmap=>"Costmap",Palette::Raw=>"Raw"}).show_ui(ui,|ui| {
            for (value,label) in [(Palette::Map,"Map"),(Palette::Costmap,"Costmap"),(Palette::Raw,"Raw")] {if ui.selectable_value(&mut settings.palette,value,label).clicked() {ui.close_menu();}}
        }).response;
        response.widget_info(||{let mut info=egui::WidgetInfo::labeled(egui::WidgetType::ComboBox,response.enabled(),"Palette");info.current_text_value=Some(match settings.palette {Palette::Map=>"Map",Palette::Costmap=>"Costmap",Palette::Raw=>"Raw"}.into());info});
        let mut percent=settings.opacity_per_mille as f32/10.;
        if ui.add(studio_ui_foundation::theme::slider(egui::Slider::new(&mut percent,0.0..=100.).text("Opacity").suffix("%").step_by(1.))).changed() {settings.opacity_per_mille=(percent*10.).round() as u16;}
        ui.checkbox(&mut settings.draw_behind,"Draw behind other displays").on_hover_text("Render the map first without writing depth, so Robot and sensor geometry can remain visible.");
    });
    before != *settings
}
