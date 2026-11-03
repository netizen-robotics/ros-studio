//! Shared presentation only; subscription and document state stay with App.
use eframe::egui::{self, Color32, Stroke};

pub const SPACE: f32 = 8.0;
pub const PADDING: i8 = 12;
pub const VIEW_PADDING: i8 = 8;
pub const COMPACT_HEIGHT: f32 = 28.0;
pub const CONTROL_HEIGHT: f32 = 34.0;
pub const DOMAIN_WIDTH: f32 = 64.0;
pub const MIDDLEWARE_WIDTH: f32 = 144.0;
pub const CONNECTION_ACTION_WIDTH: f32 = 144.0;
/// Semantic sRGB equivalents of robot-harness's light/dark OKLCH tokens.
/// Keep measurement/data colors separate from application chrome.
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub surface: Color32,
    pub subtle: Color32,
    pub raised: Color32,
    pub ink: Color32,
    pub muted: Color32,
    pub line: Color32,
    pub control_border: Color32,
    pub accent: Color32,
    pub on_accent: Color32,
    pub selected: Color32,
    pub warning: Color32,
    pub warning_surface: Color32,
    pub success: Color32,
    pub danger: Color32,
}

pub const LIGHT: Palette = Palette {
    surface: Color32::from_rgb(250, 251, 253),
    subtle: Color32::from_rgb(240, 245, 250),
    raised: Color32::from_rgb(253, 254, 255),
    ink: Color32::from_rgb(24, 32, 49),
    muted: Color32::from_rgb(79, 89, 103),
    line: Color32::from_rgb(212, 218, 225),
    control_border: Color32::from_rgb(130, 144, 163),
    accent: Color32::from_rgb(26, 82, 233),
    on_accent: Color32::from_rgb(251, 252, 253),
    selected: Color32::from_rgb(221, 234, 255),
    warning: Color32::from_rgb(133, 80, 0),
    warning_surface: Color32::from_rgb(255, 244, 223),
    success: Color32::from_rgb(15, 106, 49),
    danger: Color32::from_rgb(176, 13, 16),
};
pub const DARK: Palette = Palette {
    surface: Color32::from_rgb(16, 20, 25),
    subtle: Color32::from_rgb(11, 15, 20),
    raised: Color32::from_rgb(23, 28, 34),
    ink: Color32::from_rgb(236, 239, 242),
    muted: Color32::from_rgb(170, 178, 189),
    line: Color32::from_rgb(54, 62, 72),
    control_border: Color32::from_rgb(103, 116, 133),
    accent: Color32::from_rgb(124, 180, 252),
    on_accent: Color32::from_rgb(12, 18, 26),
    selected: Color32::from_rgb(27, 44, 68),
    warning: Color32::from_rgb(234, 188, 110),
    warning_surface: Color32::from_rgb(45, 37, 24),
    success: Color32::from_rgb(121, 202, 139),
    danger: Color32::from_rgb(251, 153, 140),
};
pub const RADIUS: u8 = 6;
pub const PAGE_HEADING: &str = "PageHeading";

pub fn palette(visuals: &egui::Visuals) -> &'static Palette {
    if visuals.dark_mode {
        &DARK
    } else {
        &LIGHT
    }
}

pub fn configure(ctx: &egui::Context) {
    crate::fonts::configure(ctx);
    ctx.all_styles_mut(|style| {
        style.spacing.item_spacing = egui::vec2(SPACE, SPACE);
        style.spacing.button_padding = egui::vec2(12.0, 5.0);
        style.spacing.interact_size.y = CONTROL_HEIGHT;
        for (role, size) in [
            (egui::TextStyle::Body, 14.0),
            (egui::TextStyle::Button, 13.0),
            (egui::TextStyle::Small, 12.0),
            (egui::TextStyle::Heading, 18.0),
            (egui::TextStyle::Name(PAGE_HEADING.into()), 22.0),
        ] {
            let family = if matches!(role, egui::TextStyle::Heading | egui::TextStyle::Name(_)) {
                egui::FontFamily::Name(crate::fonts::SEMIBOLD.into())
            } else {
                egui::FontFamily::Proportional
            };
            style
                .text_styles
                .insert(role, egui::FontId::new(size, family));
        }
        style
            .text_styles
            .insert(egui::TextStyle::Monospace, egui::FontId::monospace(12.0));
    });
    for (theme, mut v, p) in [
        (egui::Theme::Light, egui::Visuals::light(), LIGHT),
        (egui::Theme::Dark, egui::Visuals::dark(), DARK),
    ] {
        v.panel_fill = p.surface;
        v.window_fill = p.raised;
        v.extreme_bg_color = p.subtle;
        v.faint_bg_color = p.subtle;
        v.code_bg_color = p.subtle;
        for widget in [
            &mut v.widgets.noninteractive,
            &mut v.widgets.inactive,
            &mut v.widgets.hovered,
            &mut v.widgets.active,
            &mut v.widgets.open,
        ] {
            widget.fg_stroke = Stroke::new(1.65, p.ink);
            widget.bg_fill = p.raised;
            widget.weak_bg_fill = p.raised;
            widget.bg_stroke = Stroke::new(1.0, p.control_border);
            widget.corner_radius = egui::CornerRadius::same(RADIUS);
            widget.expansion = 0.0;
        }
        v.widgets.noninteractive.bg_fill = p.surface;
        v.widgets.noninteractive.bg_stroke = Stroke::new(1.0, p.line);
        v.widgets.hovered.bg_fill = p.subtle;
        v.widgets.hovered.weak_bg_fill = p.subtle;
        v.widgets.hovered.bg_stroke = Stroke::new(2.0, p.accent);
        v.widgets.active.bg_fill = p.selected;
        v.widgets.active.weak_bg_fill = p.selected;
        v.widgets.active.bg_stroke = Stroke::new(2.0, p.accent);
        v.widgets.open.bg_fill = p.selected;
        v.widgets.open.weak_bg_fill = p.selected;
        v.selection.bg_fill = p.selected;
        v.selection.stroke = Stroke::new(1.65, p.accent);
        v.hyperlink_color = p.accent;
        v.window_stroke = Stroke::new(1.0, p.line);
        v.window_corner_radius = egui::CornerRadius::same(RADIUS);
        v.menu_corner_radius = egui::CornerRadius::same(RADIUS);
        v.window_shadow = egui::epaint::Shadow::NONE;
        v.popup_shadow = egui::epaint::Shadow::NONE;
        v.text_cursor.stroke = Stroke::new(2.0, p.accent);
        v.error_fg_color = p.danger;
        v.warn_fg_color = p.warning;
        ctx.set_visuals_of(theme, v);
    }
}

pub fn primary<'a>(visuals: &egui::Visuals, label: &'a str) -> egui::Button<'a> {
    let p = palette(visuals);
    egui::Button::new(egui::RichText::new(label).color(p.on_accent))
        .fill(p.accent)
        .stroke(Stroke::new(1.0, p.accent))
}

pub fn disconnect<'a>(visuals: &egui::Visuals, label: &'a str) -> egui::Button<'a> {
    let p = palette(visuals);
    egui::Button::new(egui::RichText::new(label).color(p.on_accent))
        .fill(p.danger)
        .stroke(Stroke::new(1.0, p.danger))
}

/// egui's selected Button changes the fill, but does not change its text color.
pub fn selectable(
    visuals: &egui::Visuals,
    text: impl Into<egui::WidgetText>,
    selected: bool,
) -> egui::Button<'static> {
    let text = text.into();
    egui::Button::new(if selected {
        text.color(visuals.selection.stroke.color)
    } else {
        text
    })
    .selected(selected)
}

/// Emphasized text uses the platform semibold face, preserving its body size.
pub fn strong(ui: &mut egui::Ui, text: impl Into<String>) -> egui::Response {
    ui.label(egui::RichText::new(text).family(crate::fonts::semibold_family(ui.ctx())))
}

/// Pane titles are distinct from body-sized section labels.
pub fn pane_heading(ui: &mut egui::Ui, text: impl Into<String>) -> egui::Response {
    ui.label(
        egui::RichText::new(text)
            .size(18.0)
            .family(crate::fonts::semibold_family(ui.ctx())),
    )
}

/// Shared explicit appearance choices; changing theme never changes a session.
pub fn appearance(ui: &mut egui::Ui) {
    let current = ui.ctx().options(|options| options.theme_preference);
    ui.horizontal(|ui| {
        for (label, preference) in [
            ("System", egui::ThemePreference::System),
            ("Light", egui::ThemePreference::Light),
            ("Dark", egui::ThemePreference::Dark),
        ] {
            if ui.selectable_label(current == preference, label).clicked() {
                ui.ctx().set_theme(preference);
            }
        }
    });
}

#[derive(Clone, Copy)]
pub enum UtilityIcon {
    System,
    Light,
    Dark,
    Information,
}

/// A vector icon with a full keyboard and accessibility action target.
pub fn icon_button(ui: &mut egui::Ui, icon: UtilityIcon, label: &str) -> egui::Response {
    let response = ui.add_sized([CONTROL_HEIGHT, CONTROL_HEIGHT], egui::Button::new(""));
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, response.enabled(), label)
    });
    let painter = ui.painter();
    let center = response.rect.center();
    let stroke = Stroke::new(
        1.65,
        if response.enabled() {
            palette(ui.visuals()).accent
        } else {
            muted(ui.visuals())
        },
    );
    match icon {
        UtilityIcon::Information => {
            painter.circle_stroke(center, 8.0, stroke);
            painter.circle_filled(center + egui::vec2(0.0, -4.0), 1.0, stroke.color);
            painter.line_segment(
                [
                    center + egui::vec2(0.0, -0.5),
                    center + egui::vec2(0.0, 5.0),
                ],
                stroke,
            );
        }
        UtilityIcon::System => {
            painter.rect_stroke(
                egui::Rect::from_center_size(center - egui::vec2(0.0, 2.0), egui::vec2(18.0, 12.0)),
                2.0,
                stroke,
                egui::StrokeKind::Inside,
            );
            painter.line_segment(
                [center + egui::vec2(0.0, 4.0), center + egui::vec2(0.0, 8.0)],
                stroke,
            );
            painter.line_segment(
                [
                    center + egui::vec2(-5.0, 8.0),
                    center + egui::vec2(5.0, 8.0),
                ],
                stroke,
            );
        }
        UtilityIcon::Light => {
            painter.circle_stroke(center, 4.0, stroke);
            for index in 0..8 {
                let angle = index as f32 * std::f32::consts::TAU / 8.0;
                let direction = egui::vec2(angle.cos(), angle.sin());
                painter.line_segment([center + direction * 6.5, center + direction * 9.0], stroke);
            }
        }
        UtilityIcon::Dark => {
            let points = (0..=20)
                .map(|i| {
                    let angle = (45.0 + i as f32 * 270.0 / 20.0).to_radians();
                    center + egui::vec2(angle.cos(), angle.sin()) * 8.0
                })
                .chain((0..=16).map(|i| {
                    let angle = (270.0 - i as f32 * 180.0 / 16.0).to_radians();
                    center + egui::vec2(5.7, 0.0) + egui::vec2(angle.cos(), angle.sin()) * 5.7
                }))
                .collect();
            painter.add(egui::Shape::line(points, stroke));
        }
    }
    response
}

/// Cycle an explicit preference; System follows later OS appearance changes.
/// Keep appearance at the toolbar's right edge while connection controls wrap.
pub fn appearance_toolbar(ui: &mut egui::Ui, controls: impl FnOnce(&mut egui::Ui)) {
    ui.horizontal_top(|ui| {
        let width = (ui.available_width() - CONTROL_HEIGHT - ui.spacing().item_spacing.x).max(1.0);
        ui.allocate_ui_with_layout(
            egui::vec2(width, CONTROL_HEIGHT),
            egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true),
            |ui| {
                ui.set_min_width(width);
                controls(ui);
            },
        );
        appearance_button(ui);
    });
}

pub fn appearance_button(ui: &mut egui::Ui) -> egui::Response {
    let current = ui.ctx().options(|options| options.theme_preference);
    let (name, next_name, icon, next) = match current {
        egui::ThemePreference::System => (
            "System",
            "Light",
            UtilityIcon::System,
            egui::ThemePreference::Light,
        ),
        egui::ThemePreference::Light => (
            "Light",
            "Dark",
            UtilityIcon::Light,
            egui::ThemePreference::Dark,
        ),
        egui::ThemePreference::Dark => (
            "Dark",
            "System",
            UtilityIcon::Dark,
            egui::ThemePreference::System,
        ),
    };
    let response = icon_button(
        ui,
        icon,
        &format!("Appearance: {name}; switch to {next_name}"),
    )
    .on_hover_text(format!("Appearance: {name}. Switch to {next_name}"));
    if response.clicked() {
        ui.ctx().set_theme(next);
    }
    response
}

#[cfg(test)]
mod appearance_cycle_tests {
    use super::*;
    #[test]
    fn appearance_button_cycles_preferences_without_opening_a_window() {
        let ctx = egui::Context::default();
        ctx.set_theme(egui::ThemePreference::System);
        let mut rect = egui::Rect::NOTHING;
        for expected in [
            egui::ThemePreference::Light,
            egui::ThemePreference::Dark,
            egui::ThemePreference::System,
        ] {
            let _ = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    rect = appearance_button(ui).rect;
                });
            });
            for pressed in [true, false] {
                let pos = rect.center();
                let output = ctx.run(
                    egui::RawInput {
                        events: vec![
                            egui::Event::PointerMoved(pos),
                            egui::Event::PointerButton {
                                pos,
                                button: egui::PointerButton::Primary,
                                pressed,
                                modifiers: Default::default(),
                            },
                        ],
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            appearance_button(ui);
                        });
                    },
                );
                assert_eq!(output.viewport_output.len(), 1);
            }
            assert_eq!(ctx.options(|o| o.theme_preference), expected);
        }
    }
}

/// Single-line inputs share action height, text inset and a visible outline.
pub fn input<'a>(ui: &egui::Ui, text: &'a mut dyn egui::TextBuffer) -> egui::TextEdit<'a> {
    egui::TextEdit::singleline(text)
        .min_size(egui::vec2(0.0, CONTROL_HEIGHT))
        .margin(egui::Margin::symmetric(12, 5))
        .vertical_align(egui::Align::Center)
        .background_color(palette(ui.visuals()).raised)
}

pub fn hint(ui: &egui::Ui, text: impl Into<String>) -> egui::RichText {
    egui::RichText::new(text).color(muted(ui.visuals()))
}

/// Slider geometry is independent of standard button/input hit-area height.
pub fn slider(slider: egui::Slider<'_>) -> impl egui::Widget + '_ {
    move |ui: &mut egui::Ui| {
        ui.scope(|ui| {
            ui.set_min_height(CONTROL_HEIGHT);
            ui.spacing_mut().interact_size.y = 20.0;
            let p = *palette(ui.visuals());
            ui.visuals_mut().widgets.inactive.bg_fill = p.control_border;
            ui.visuals_mut().widgets.inactive.fg_stroke = Stroke::new(1.65, p.accent);
            ui.add(slider)
        })
        .inner
    }
}

/// A fixed action strip includes its vertical padding in the height.
pub fn strip_frame(visuals: &egui::Visuals) -> egui::Frame {
    egui::Frame::new()
        .fill(palette(visuals).surface)
        .inner_margin(egui::Margin::symmetric(PADDING, 4))
}

/// Text-labelled status: color supplements, rather than replaces, the label.
pub fn badge(ui: &mut egui::Ui, label: &str, color: Color32) -> egui::Response {
    let p = palette(ui.visuals());
    egui::Frame::new()
        .fill(p.raised)
        .stroke(Stroke::new(1.0, p.line))
        .corner_radius(5)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.label(egui::RichText::new(label).small().color(color))
        })
        .inner
}

/// Exact, clipped action slot: labels and enabled states cannot resize it.
pub fn action_slot(
    ui: &mut egui::Ui,
    enabled: bool,
    width: f32,
    button: egui::Button<'_>,
) -> egui::Response {
    let response = ui
        .add_enabled_ui(enabled, |ui| {
            ui.add_sized([width, CONTROL_HEIGHT], button.truncate())
        })
        .inner;
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect.expand(3.0),
            RADIUS + 3,
            Stroke::new(2.0, palette(ui.visuals()).accent),
            egui::StrokeKind::Outside,
        );
    }
    response
}

/// Reserve a line without the centered child layout imposed by `add_sized`.
pub fn label_slot(
    ui: &mut egui::Ui,
    width: f32,
    text: impl Into<egui::WidgetText>,
) -> egui::Response {
    let size = egui::vec2(width, COMPACT_HEIGHT);
    ui.allocate_ui_with_layout(
        size,
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.set_min_size(size);
            ui.add(egui::Label::new(text).truncate())
        },
    )
    .inner
}

/// Presentation only. Editing these drafts never connects or changes a session.
pub fn connection_fields(
    ui: &mut egui::Ui,
    id: &str,
    domain: &mut String,
    cyclone: &mut bool,
    enabled: bool,
) {
    ui.add_enabled_ui(enabled, |ui| {
        let label = ui.label("Domain");
        ui.add_sized(
            [DOMAIN_WIDTH, CONTROL_HEIGHT],
            input(ui, domain)
                .vertical_align(egui::Align::Center)
                .hint_text(hint(ui, "0"))
                .desired_width(DOMAIN_WIDTH)
                .clip_text(true),
        )
        .labelled_by(label.id);
        ui.label("Middleware");
        let selected = if *cyclone { "Cyclone DDS" } else { "Fast DDS" };
        let choice = ui
            .allocate_ui_with_layout(
                egui::vec2(MIDDLEWARE_WIDTH, CONTROL_HEIGHT),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    egui::ComboBox::from_id_salt(id)
                        .width(MIDDLEWARE_WIDTH)
                        .truncate()
                        .selected_text(selected)
                        .show_ui(ui, |ui| {
                            ui.selectable_value(cyclone, false, "Fast DDS");
                            ui.selectable_value(cyclone, true, "Cyclone DDS");
                        })
                },
            )
            .inner;
        choice.response.widget_info(|| {
            let mut info = egui::WidgetInfo::labeled(
                egui::WidgetType::ComboBox,
                choice.response.enabled(),
                "Middleware",
            );
            info.current_text_value = Some(selected.into());
            info
        });
    });
}

/// Transparent resting surface, retaining egui's hover and keyboard-focus stroke.
pub fn quiet(label: &str) -> egui::Button<'_> {
    egui::Button::new(label)
        .small()
        .min_size(egui::vec2(0.0, COMPACT_HEIGHT))
        .fill(Color32::TRANSPARENT)
}

pub fn muted(visuals: &egui::Visuals) -> Color32 {
    palette(visuals).muted
}

pub fn view_frame(ui: &egui::Ui, selected: bool) -> egui::Frame {
    let stroke = if selected {
        Stroke::new(1.5, ui.visuals().hyperlink_color)
    } else {
        ui.visuals().widgets.noninteractive.bg_stroke
    };
    egui::Frame::new()
        .fill(palette(ui.visuals()).raised)
        .stroke(stroke)
        .corner_radius(RADIUS)
        .inner_margin(VIEW_PADDING)
}

pub fn view_title(topic: &str, renderer: &str) -> String {
    format!(
        "{} · {renderer}",
        topic.rsplit('/').find(|s| !s.is_empty()).unwrap_or(topic)
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contrast(a: Color32, b: Color32) -> f32 {
        let luminance = |color: Color32| {
            let linear = |byte: u8| {
                let v = f32::from(byte) / 255.0;
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            };
            0.2126 * linear(color.r()) + 0.7152 * linear(color.g()) + 0.0722 * linear(color.b())
        };
        let (a, b) = (luminance(a), luminance(b));
        (a.max(b) + 0.05) / (a.min(b) + 0.05)
    }

    #[test]
    fn semantic_text_and_action_colors_meet_contrast_in_both_themes() {
        let ctx = egui::Context::default();
        configure(&ctx);
        for theme in [egui::Theme::Light, egui::Theme::Dark] {
            let style = ctx.style_of(theme);
            let v = &style.visuals;
            for background in [v.panel_fill, v.extreme_bg_color] {
                for foreground in [
                    v.text_color(),
                    muted(v),
                    v.hyperlink_color,
                    v.error_fg_color,
                    v.warn_fg_color,
                ] {
                    assert!(
                        contrast(foreground, background) >= 4.5,
                        "{theme:?}: {foreground:?} on {background:?}"
                    );
                }
            }
            assert!(contrast(v.selection.stroke.color, v.selection.bg_fill) >= 4.5);
        }
        for p in [LIGHT, DARK] {
            for fill in [p.accent, p.danger] {
                assert!(contrast(p.on_accent, fill) >= 4.5);
            }
            for background in [p.surface, p.subtle, p.raised, p.selected] {
                for foreground in [p.ink, p.muted, p.accent, p.danger, p.warning, p.success] {
                    assert!(
                        contrast(foreground, background) >= 4.5,
                        "{foreground:?} on {background:?}"
                    );
                }
            }
        }
    }

    #[test]
    fn painted_input_hint_border_and_slider_match_control_rules() {
        for theme in [egui::ThemePreference::Light, egui::ThemePreference::Dark] {
            let ctx = egui::Context::default();
            configure(&ctx);
            ctx.set_theme(theme);
            let mut text = String::new();
            let mut value = 0.5;
            let output = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let input = ui.add(input(ui, &mut text).hint_text(hint(ui, "Find a source")));
                    let action = ui.button("Start receiving");
                    // TextEdit::show returns its inset text rect; interact_rect retains the painted outer bounds.
                    assert_eq!(input.interact_rect.height(), CONTROL_HEIGHT);
                    assert_eq!(action.rect.height(), CONTROL_HEIGHT);
                    ui.add(slider(egui::Slider::new(&mut value, 0.0..=1.0)));
                });
            });
            let p = *palette(&ctx.style().visuals);
            let painted_hint = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(t) if t.galley.text() == "Find a source" => Some(t),
                    _ => None,
                })
                .expect("placeholder is actually painted");
            let color = painted_hint.galley.job.sections[0].format.color;
            assert_eq!(color, p.muted);
            assert!(contrast(color, p.raised) >= 4.5);
            assert!(output.shapes.iter().any(|shape| match &shape.shape {
                egui::Shape::Rect(rect) =>
                    rect.stroke.color == p.control_border
                        && contrast(rect.stroke.color, p.raised) >= 3.0,
                _ => false,
            }));
            let handle = output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Circle(circle) => Some(circle),
                    _ => None,
                })
                .expect("slider handle is painted");
            assert!(
                handle.radius * 2.0 <= 20.0,
                "slider must not inherit 34pt button geometry"
            );
        }
    }

    #[test]
    fn connection_actions_keep_exact_bounds_across_labels_themes_and_enabled_states() {
        let ctx = egui::Context::default();
        crate::fonts::configure(&ctx);
        configure(&ctx);
        let mut expected = None;
        for theme in [egui::ThemePreference::Light, egui::ThemePreference::Dark] {
            ctx.set_theme(theme);
            for enabled in [true, false] {
                for label in [
                    "Connect",
                    "Cancel startup",
                    "Disconnect",
                    "Cleaning up",
                    "Unexpected very long status that must remain contained",
                ] {
                    let mut rect = egui::Rect::NOTHING;
                    let _ = ctx.run(egui::RawInput::default(), |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            rect = action_slot(
                                ui,
                                enabled,
                                CONNECTION_ACTION_WIDTH,
                                primary(ui.visuals(), label),
                            )
                            .rect;
                        });
                    });
                    assert_eq!(
                        rect.size(),
                        egui::vec2(CONNECTION_ACTION_WIDTH, CONTROL_HEIGHT)
                    );
                    assert_eq!(*expected.get_or_insert(rect), rect);
                }
            }
        }
    }
}

#[cfg(test)]
mod selected_tests {
    use super::*;
    #[test]
    fn selected_button_paints_readable_text_without_moving_its_label() {
        for theme in [egui::ThemePreference::Light, egui::ThemePreference::Dark] {
            let ctx = egui::Context::default();
            configure(&ctx);
            ctx.set_theme(theme);
            let mut expected = None;
            for selected in [false, true] {
                let output = ctx.run(Default::default(), |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        ui.add_sized(
                            [144.0, CONTROL_HEIGHT],
                            selectable(ui.visuals(), "Topics", selected),
                        );
                    });
                });
                let text = output
                    .shapes
                    .iter()
                    .find_map(|s| match &s.shape {
                        egui::Shape::Text(t) if t.galley.text() == "Topics" => Some(t),
                        _ => None,
                    })
                    .unwrap();
                assert_eq!(*expected.get_or_insert(text.pos), text.pos);
                if selected {
                    assert_eq!(
                        text.galley.job.sections[0].format.color,
                        ctx.style().visuals.selection.stroke.color
                    );
                }
            }
        }
    }
}

#[cfg(test)]
mod label_tests {
    use super::*;
    #[test]
    fn reserved_labels_start_at_the_left_and_keep_following_controls_stationary() {
        let ctx = egui::Context::default();
        configure(&ctx);
        let mut next = None;
        for text in [
            "Installed: 1.0.0".to_owned(),
            format!("Installed: {}", "long-version".repeat(50)),
        ] {
            let mut label = egui::Rect::NOTHING;
            let mut action = egui::Rect::NOTHING;
            let output = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        label = label_slot(ui, 200.0, &text).rect;
                        action = action_slot(ui, true, 112.0, egui::Button::new("Open")).rect;
                    });
                });
            });
            let painted = output
                .shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::Shape::Text(t) if t.galley.text().starts_with("Installed:") => Some(t),
                    _ => None,
                })
                .unwrap();
            assert_eq!(painted.pos.x, 8.0);
            assert!(label.width() <= 200.0);
            assert_eq!(*next.get_or_insert(action), action);
        }
    }
}

/// Close this application through eframe's native shutdown callback.
pub fn exit_menu_item(ui: &mut egui::Ui) -> egui::Response {
    let response = ui.button("Exit");
    if response.clicked() {
        ui.close_menu();
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
    }
    response
}

#[cfg(test)]
mod exit_tests {
    use super::*;
    #[test]
    fn exit_requests_native_close_only_when_clicked() {
        let ctx = egui::Context::default();
        let mut rect = egui::Rect::NOTHING;
        let initial = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                rect = exit_menu_item(ui).rect;
            });
        });
        assert!(!initial.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .contains(&egui::ViewportCommand::Close));
        let pos = rect.center();
        let frame = |pressed| {
            ctx.run(
                egui::RawInput {
                    events: vec![
                        egui::Event::PointerMoved(pos),
                        egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: Default::default(),
                        },
                    ],
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        exit_menu_item(ui);
                    });
                },
            )
        };
        frame(true);
        let output = frame(false);
        assert!(output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .contains(&egui::ViewportCommand::Close));
    }
}
