//! Shared sensor inspector; edits presentation only, never subscription inputs.
use super::{
    sensor_appearance::{ColorMode, SensorAppearance, SizeMode},
    sensor_history::SensorHistory,
};
use eframe::egui;
#[derive(Default)]
pub struct AppearanceAction {
    pub changed: bool,
    pub clear: bool,
}
fn history_duration(text: &str) -> Option<f64> {
    let text = text.trim();
    if text.eq_ignore_ascii_case("latest") {
        return Some(0.);
    }
    let (number, scale) = if let Some(value) = text.strip_suffix("ms") {
        (value, 1.)
    } else {
        (text.strip_suffix('s').unwrap_or(text), 1000.)
    };
    let value = number.trim().parse::<f64>().ok()? * scale;
    (value.is_finite() && (0.0..=10_000.).contains(&value)).then_some(value)
}
fn history_label(value: f64) -> String {
    if value == 0. {
        "Latest".into()
    } else if value < 1000. {
        format!("{value:.0} ms")
    } else {
        format!("{:.1} s", value / 1000.)
    }
}
fn attribute_hint(
    mode: ColorMode,
    points: &[super::sensor_history::SensorPoint],
) -> Option<&'static str> {
    if !matches!(mode, ColorMode::Rgb | ColorMode::Intensity) {
        return None;
    }
    if points.is_empty() {
        return Some("No retained points. Source color choices update as valid samples arrive.");
    }
    let available = |p: &super::sensor_history::SensorPoint| {
        if mode == ColorMode::Rgb {
            p.rgba.is_some()
        } else {
            p.intensity.is_some()
        }
    };
    if !points.iter().any(available) {
        Some("Retained samples contain no usable values for this color mode. Missing values use grey.")
    } else if points.iter().any(|p| !available(p)) {
        Some("Points with missing values use grey.")
    } else {
        None
    }
}
pub fn show(
    ui: &mut egui::Ui,
    settings: &mut SensorAppearance,
    history: &SensorHistory,
) -> AppearanceAction {
    let before = *settings;
    let mut action = AppearanceAction::default();
    egui::CollapsingHeader::new("Appearance").default_open(true).show(ui,|ui| {
        ui.horizontal(|ui| {
            ui.add(studio_ui_foundation::theme::slider(egui::Slider::new(&mut settings.history_ms,0..=10_000).text("History").step_by(100.).custom_formatter(|n,_|history_label(n)).custom_parser(history_duration)));
            action.clear=ui.add_enabled(history.frame_count()>0,egui::Button::new("Clear")).on_hover_text("Remove retained points. Reception continues; old samples are not replayed.").clicked();
        });
        ui.small(format!("{} points in {} {}",history.points().len(),history.frame_count(),if history.frame_count()==1 {"sample"} else {"samples"}));
        egui::ComboBox::from_label("Size units").selected_text(if settings.size_mode==SizeMode::Pixels {"Pixels"} else {"Millimetres"}).show_ui(ui,|ui| {
            ui.selectable_value(&mut settings.size_mode,SizeMode::Pixels,"Pixels");ui.selectable_value(&mut settings.size_mode,SizeMode::World,"Millimetres");
        });
        if settings.size_mode==SizeMode::Pixels {ui.add(studio_ui_foundation::theme::slider(egui::Slider::new(&mut settings.pixels,1..=32).text("Point size").suffix(" px")));}
        else {ui.add(studio_ui_foundation::theme::slider(egui::Slider::new(&mut settings.diameter_mm,1..=1000).text("Point size").suffix(" mm")));}
        let mut opacity=(u16::from(settings.opacity)*100+127)/255;
        if ui.add(studio_ui_foundation::theme::slider(egui::Slider::new(&mut opacity,0..=100).text("Opacity").suffix("%"))).changed() {settings.opacity=((opacity*255+50)/100) as u8;}
        let rgb=history.points().iter().any(|p|p.rgba.is_some());
        let intensity=history.points().iter().any(|p|p.intensity.is_some());
        let label=|mode|match mode {ColorMode::Flat=>"Flat color",ColorMode::Height=>"Height in fixed frame",ColorMode::Rgb=>"Source RGB",ColorMode::Intensity=>"Intensity"};
        egui::ComboBox::from_label("Color").selected_text(label(settings.color)).show_ui(ui,|ui| {
            for (mode,available) in [(ColorMode::Flat,true),(ColorMode::Height,true),(ColorMode::Rgb,rgb),(ColorMode::Intensity,intensity)] {
                let choice=ui.add_enabled_ui(available,|ui|ui.selectable_value(&mut settings.color,mode,label(mode)));
                if choice.inner.clicked() {ui.close_menu();}
                choice.response.on_hover_text(if available {"Changes color locally; reception continues"} else {"Unavailable until retained samples contain usable source values"});
            }
        });
        if settings.color==ColorMode::Flat {ui.horizontal(|ui|{ui.label("Color");ui.color_edit_button_srgb(&mut settings.flat_rgb);});}
        if let Some(hint)=attribute_hint(settings.color,history.points()) {ui.small(hint);}
        if matches!(settings.color,ColorMode::Height|ColorMode::Intensity) {
            ui.checkbox(&mut settings.automatic_range,"Automatic range");
            if !settings.automatic_range {
                let values=if settings.color==ColorMode::Height {&mut settings.height_mm} else {&mut settings.intensity_milli};
                let limit=if settings.color==ColorMode::Height {1_000_000.} else {1_000_000_000_000.};
                for (i,name) in ["Minimum","Maximum"].into_iter().enumerate() {
                    let mut value=values[i] as f64/1000.;
                    ui.horizontal(|ui| {let label=ui.label(name);if ui.add(egui::DragValue::new(&mut value).speed(0.1).range(-limit..=limit)).labelled_by(label.id).changed() && value.is_finite() {values[i]=(value*1000.).round() as i64;}});
                }
                if settings.color==ColorMode::Height {ui.small("Height range is in metres in the fixed frame.");}
            }
            if let Some(range)=super::sensor_appearance::range(settings,history.points().iter().copied()) {ui.small(format!("Teal {:.3} → amber {:.3}",range.minimum,range.maximum));}
            else {ui.small("Range unavailable until samples arrive.");}
        }

        ui.collapsing("Retention details",|ui| {
            let c=history.counters();ui.small(format!("Expired samples: {} · Replaced: {} · Budget evictions: {} samples / {} points",c.expired_frames,c.replaced_frames,c.budget_evicted_frames,c.budget_evicted_points));
            ui.small("Retention limits: 10 seconds, 32 samples and 32,768 points. Expiry and eviction are separate from reception drops.");
        });
    });
    action.changed = *settings != before;
    action
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn numeric_history_entry_uses_displayed_seconds_and_explicit_millisecond_units() {
        assert_eq!(history_duration("1.5"), Some(1500.));
        assert_eq!(history_duration("1.5 s"), Some(1500.));
        assert_eq!(history_duration("250 ms"), Some(250.));
        assert_eq!(history_duration("Latest"), Some(0.));
        assert_eq!(history_label(1.), "1 ms");
        for text in ["NaN", "inf", "-1", "10.1 s", "10001 ms", ""] {
            assert!(history_duration(text).is_none(), "{text}");
        }
    }
    #[test]
    fn attribute_hints_distinguish_no_samples_missing_values_and_complete_data() {
        use super::super::{sensor_history::SensorPoint, Point3};
        assert!(attribute_hint(ColorMode::Rgb, &[])
            .unwrap()
            .contains("No retained points"));
        let absent = SensorPoint {
            position: Point3([0.; 3]),
            ..Default::default()
        };
        assert!(attribute_hint(ColorMode::Rgb, &[absent])
            .unwrap()
            .contains("Retained samples"));
        let present = SensorPoint {
            rgba: Some([1, 2, 3, 255]),
            intensity: Some(42.),
            ..absent
        };
        assert_eq!(attribute_hint(ColorMode::Rgb, &[present]), None);
        assert_eq!(
            attribute_hint(ColorMode::Intensity, &[present, absent]),
            Some("Points with missing values use grey.")
        );
    }
}
