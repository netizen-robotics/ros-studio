//! Tools-only Workbench presentation. No ROS or document state lives here.
use eframe::egui::{self, Color32, RichText, Stroke};
pub fn action(
    ui: &mut egui::Ui,
    label: &str,
    width: f32,
    enabled: bool,
    primary: bool,
) -> egui::Response {
    let button = if primary {
        studio_ui_foundation::theme::primary(ui.visuals(), label)
    } else {
        egui::Button::new(label)
    };
    let response = studio_ui_foundation::theme::action_slot(ui, enabled, width, button);
    crate::diagnostics::response(ui.ctx(), label, &response);
    response
}
pub fn tab(ui: &mut egui::Ui, label: &str, selected: bool) -> egui::Response {
    let accent = ui.visuals().hyperlink_color;
    let fill = if selected {
        studio_ui_foundation::theme::palette(ui.visuals()).selected
    } else {
        Color32::TRANSPARENT
    };
    let response = ui.add(
        egui::Button::new(RichText::new(label).color(if selected {
            accent
        } else {
            ui.visuals().text_color()
        }))
        .fill(fill)
        .stroke(Stroke::NONE)
        .corner_radius(egui::CornerRadius {
            nw: studio_ui_foundation::theme::RADIUS,
            ne: studio_ui_foundation::theme::RADIUS,
            sw: 0,
            se: 0,
        })
        .min_size(egui::vec2(0.0, studio_ui_foundation::theme::CONTROL_HEIGHT)),
    );
    if selected || response.hovered() {
        ui.painter().hline(
            response.rect.x_range(),
            response.rect.bottom() - 1.0,
            Stroke::new(2.0, accent),
        );
    }
    if response.has_focus() || response.hovered() {
        ui.painter().rect_stroke(
            response.rect.shrink(2.0),
            3,
            Stroke::new(2.0, accent),
            egui::StrokeKind::Inside,
        );
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            selected,
            label,
        )
    });
    crate::diagnostics::response(ui.ctx(), label, &response);
    response
}
/// A single connected row. Overflow scrolls without changing the page topology.
pub fn tab_strip(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash,
    labels: &[&str],
    current: &str,
) -> Option<usize> {
    let mut chosen = None;
    egui::ScrollArea::horizontal()
        .id_salt(id)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                for (index, label) in labels.iter().enumerate() {
                    if tab(ui, label, current == *label).clicked() {
                        chosen = Some(index);
                    }
                }
            });
        });
    let rect = ui.available_rect_before_wrap();
    ui.painter().hline(
        rect.x_range(),
        rect.top(),
        ui.visuals().widgets.noninteractive.bg_stroke,
    );
    ui.add_space(12.0);
    chosen
}
pub fn section_heading(ui: &mut egui::Ui, label: &str) {
    ui.add_space(4.0);
    studio_ui_foundation::theme::strong(ui, label);
    ui.add_space(4.0);
}
/// The suite shares one presentation system across independent applications.
pub fn configure(ctx: &egui::Context) {
    studio_ui_foundation::theme::configure(ctx);
}
/// Identical padding and left text origin in resting, selected and focused rows.
/// `add_sized` centers its child layout, so use an explicitly aligned allocation.
pub fn list_button(
    ui: &mut egui::Ui,
    text: impl Into<egui::WidgetText>,
    selected: bool,
    size: egui::Vec2,
) -> egui::Response {
    let text = text.into();
    let label = crate::diagnostics::short(text.text());
    let accent = ui.visuals().hyperlink_color;
    let text = if selected { text.color(accent) } else { text };
    let mut button = egui::Button::new(text)
        .frame(true)
        .min_size(size)
        .fill(studio_ui_foundation::theme::palette(ui.visuals()).selected)
        .stroke(Stroke::new(1.0, accent))
        .corner_radius(studio_ui_foundation::theme::RADIUS)
        .wrap_mode(egui::TextWrapMode::Wrap);
    if !selected {
        button = button.fill(Color32::TRANSPARENT).stroke(Stroke::NONE);
    }
    let response = ui
        .allocate_ui_with_layout(
            size,
            egui::Layout::top_down(egui::Align::Min).with_main_align(egui::Align::Center),
            |ui| {
                ui.set_width(size.x);
                ui.add(button)
            },
        )
        .inner;
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect.shrink(1.0),
            4,
            Stroke::new(2.0, accent),
            egui::StrokeKind::Inside,
        );
    }
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::WidgetType::SelectableLabel,
            ui.is_enabled(),
            selected,
            &label,
        )
    });
    crate::diagnostics::response(ui.ctx(), &label, &response);
    response
}
pub fn muted(ui: &mut egui::Ui, text: impl Into<String>) {
    let color = studio_ui_foundation::theme::muted(ui.visuals());
    ui.label(RichText::new(text).small().color(color));
}
pub fn sample_summary(ui: &mut egui::Ui, samples: u64, backend_drops: u64, queue_drops: u64) {
    ui.horizontal_wrapped(|ui| {
        muted(ui, format!("{samples} samples"));
        if backend_drops > 0 || queue_drops > 0 {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!("Dropped: {backend_drops} backend · {queue_drops} app"),
            );
        }
    });
}
pub fn section(typ: &str) -> &'static str {
    if typ == "node" {
        "Nodes"
    } else if typ == "parameters" {
        "Parameters"
    } else if typ.contains("/srv/") {
        "Services"
    } else if typ.contains("/action/") {
        "Actions"
    } else {
        "Topics"
    }
}
pub fn short_name(name: &str) -> &str {
    name.rsplit('/').find(|s| !s.is_empty()).unwrap_or(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tabs_keep_a_single_row_and_selection_does_not_resize_controls() {
        fn render(selected: &str) -> Vec<egui::Rect> {
            let ctx = egui::Context::default();
            let mut rects = Vec::new();
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(220.0, 180.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        egui::ScrollArea::horizontal().show(ui, |ui| {
                            ui.horizontal(|ui| {
                                for label in
                                    ["Messages", "Plot", "Publish", "Endpoints", "Definition"]
                                {
                                    rects.push(tab(ui, label, label == selected).rect);
                                }
                            });
                        });
                    });
                },
            );
            rects
        }
        let first = render("Messages");
        assert_eq!(first, render("Definition"));
        assert!(first
            .windows(2)
            .all(|pair| pair[0].top() == pair[1].top() && pair[0].right() < pair[1].left()));
        assert!(
            first.last().unwrap().right() > 220.0,
            "overflow remains scrollable, not wrapped"
        );
    }
    #[test]
    fn row_selection_does_not_change_text_origin_or_hit_area() {
        fn render(selected: bool) -> (egui::Rect, egui::Pos2) {
            let ctx = egui::Context::default();
            let mut rect = egui::Rect::NOTHING;
            let output = ctx.run(egui::RawInput::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    rect = list_button(ui, "Topics", selected, egui::vec2(180.0, 36.0)).rect;
                });
            });
            let text_origin = output
                .shapes
                .iter()
                .find_map(|shape| {
                    if let egui::Shape::Text(text) = &shape.shape {
                        (text.galley.text() == "Topics").then_some(text.pos)
                    } else {
                        None
                    }
                })
                .expect("painted row label");
            (rect, text_origin)
        }
        let (rest_rect, rest_text) = render(false);
        let (selected_rect, selected_text) = render(true);
        assert_eq!(rest_rect, selected_rect);
        assert_eq!(rest_text, selected_text);
        assert!(
            rest_text.x < rest_rect.left() + 16.0,
            "row text must be left aligned"
        );
    }
}
