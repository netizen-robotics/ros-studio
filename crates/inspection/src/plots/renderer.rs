use super::axis::{time_ticks, value_ticks};
use super::{FieldPath, Trace, MAX_POINTS, WINDOW};
use eframe::egui::{self, Align2, Color32, FontId, Pos2, Rect, Sense, Stroke, StrokeKind, Vec2};

#[derive(Clone, Copy, Debug)]
pub enum PlotStatus<'a> {
    Unconfigured,
    Starting,
    Waiting,
    Receiving,
    Stopped,
    Error(&'a str),
}
impl PlotStatus<'_> {
    fn label(self) -> String {
        match self {
            Self::Unconfigured => "Choose a numeric field, then explicitly Subscribe.".into(),
            Self::Starting => "Starting subscription…".into(),
            Self::Waiting => "Waiting for numeric samples…".into(),
            Self::Receiving => "Receiving numeric samples".into(),
            Self::Stopped => "Stopped. Subscribe to receive new samples.".into(),
            Self::Error(error) => format!("Subscription error: {error}"),
        }
    }
}

#[derive(Default)]
pub struct PlotRenderer;
impl PlotRenderer {
    /// Draft picker only. Caller owns Apply/Subscribe and metadata limits.
    /// Returns true when draft changed; never activates a subscription.
    pub fn field_picker(
        ui: &mut egui::Ui,
        fields: &[FieldPath],
        selected: &mut Option<FieldPath>,
    ) -> bool {
        let mut changed = false;
        let selected_label = selected
            .as_ref()
            .map_or_else(|| "Choose numeric field".into(), FieldPath::label);
        let response = egui::ComboBox::from_id_salt("numeric-field")
            .width(ui.available_width())
            .truncate()
            .selected_text(&selected_label)
            .show_ui(ui, |ui| {
                for field in fields {
                    if ui
                        .add(
                            egui::Button::new(field.label())
                                .selected(selected.as_ref() == Some(field))
                                .wrap(),
                        )
                        .on_hover_text(field.label())
                        .clicked()
                    {
                        *selected = Some(field.clone());
                        changed = true;
                        ui.memory_mut(|memory| memory.close_popup());
                    }
                }
            })
            .response
            .on_hover_text(&selected_label);
        response.widget_info(|| {
            let mut info = egui::WidgetInfo::labeled(
                egui::WidgetType::ComboBox,
                response.enabled(),
                "Numeric field (draft)",
            );
            info.current_text_value = Some(selected_label.clone());
            info
        });
        if changed {
            response.request_focus();
        }
        if fields.is_empty() {
            ui.label("No supported numeric fields are available.");
        }
        changed
    }
    /// No ROS, transport, parsing or subscription side effects.
    pub fn show(
        &mut self,
        ui: &mut egui::Ui,
        trace: &mut Trace,
        status: PlotStatus<'_>,
        field: Option<&FieldPath>,
    ) {
        self.show_content(ui, trace, status, field);
        Self::show_details(ui, trace);
    }
    /// Plot and local tools only; workspace inspector owns detailed history metadata.
    pub fn show_content(
        &mut self,
        ui: &mut egui::Ui,
        trace: &mut Trace,
        status: PlotStatus<'_>,
        field: Option<&FieldPath>,
    ) {
        self.show_content_with_guidance(ui, trace, status, field, true);
    }
    /// Compact hosts keep background explanations in tooltips.
    pub fn show_content_with_guidance(
        &mut self,
        ui: &mut egui::Ui,
        trace: &mut Trace,
        status: PlotStatus<'_>,
        field: Option<&FieldPath>,
        guidance: bool,
    ) {
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            Self::content(ui, trace, status, field, guidance);
        });
    }
    fn content(
        ui: &mut egui::Ui,
        trace: &mut Trace,
        status: PlotStatus<'_>,
        field: Option<&FieldPath>,
        guidance: bool,
    ) {
        ui.horizontal_wrapped(|ui| {
            let (pause, pause_name) = if trace.paused() {
                ("Resume", "Resume plot")
            } else {
                ("Pause", "Pause plot")
            };
            if toolbar_action(ui, pause, pause_name).clicked() {
                trace.set_paused(!trace.paused());
            }
            if toolbar_action(ui, "Clear", "Clear history").clicked() {
                trace.clear();
            }
            if let Some(value) = trace.points().back().and_then(|point| point.exact) {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(format!(
                            "Latest source value: {}",
                            value.exact_label()
                        ))
                        .monospace(),
                    )
                    .selectable(true)
                    .wrap(),
                );
            }
            if let Some(field) = field {
                ui.add(egui::Label::new(egui::RichText::new(field.label()).monospace()).truncate())
                    .on_hover_text(field.label());
            }
        });
        if !matches!(status, PlotStatus::Receiving) {
            let label = if guidance {
                status.label()
            } else {
                match status {
                    PlotStatus::Unconfigured => "Choose a numeric field.".into(),
                    PlotStatus::Stopped => "Stopped".into(),
                    _ => status.label(),
                }
            };
            ui.label(label);
        }
        if trace.paused() {
            ui.label(if guidance {
                "Locally paused. Subscription remains active; incoming samples are skipped."
            } else {
                "Paused"
            })
            .on_hover_text("Subscription remains active; incoming samples are skipped.");
        }
        if let Some(point) = trace.points().back() {
            if let Some(gap) = point.gap {
                ui.label(gap.label());
            }
        }
        Self::chart(ui, trace);
    }
    fn chart(ui: &mut egui::Ui, trace: &Trace) {
        let Some((y_min, y_max)) = trace.extents() else {
            ui.label("No plottable values in the retained history.");
            return;
        };
        let Some(first) = trace.points().front() else {
            return;
        };
        let start = first.receipt;
        let end = trace.points().back().unwrap().receipt;
        let span = end.saturating_sub(start).as_secs_f64();
        let y_ticks = value_ticks(y_min, y_max);
        let x_ticks = time_ticks(span);
        let font = FontId::monospace(11.0);
        let text_color = ui.visuals().text_color();
        let painter = ui.painter();
        let gutter = y_ticks
            .iter()
            .map(|tick| {
                painter
                    .layout_no_wrap(tick.label.clone(), font.clone(), text_color)
                    .size()
                    .x
            })
            .fold(0.0_f32, f32::max)
            + 12.0;
        let points = trace.display_points((ui.available_width() - gutter).max(1.0) as usize);
        let (outer, response) = ui.allocate_exact_size(
            Vec2::new(
                ui.available_width().max(1.0),
                // Keep at least 156 points for the graph after its axes. Small
                // panels scroll rather than reducing the graph to a thin strip.
                (ui.available_height() - 24.0).clamp(210.0, 500.0),
            ),
            Sense::focusable_noninteractive(),
        );
        response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Other, true, format!("Numeric trace. Local receipt time, seconds since first retained sample: 0 to {span}. Value range {y_min} to {y_max}. Units unknown. Drawing {} representatives. Latest exact value and gap reasons appear above the chart.", points.len())));
        response.clone().on_hover_text(format!("Drawing {} representatives; downsampling preserves extrema except buckets containing gaps.", points.len()));
        let painter = ui.painter_at(outer);
        if response.has_focus() {
            painter.rect_stroke(
                outer.shrink(1.0),
                0.0,
                ui.visuals().selection.stroke,
                StrokeKind::Inside,
            );
        }
        let plot = Rect::from_min_max(
            Pos2::new(
                (outer.left() + gutter).min(outer.right() - 1.0),
                outer.top() + 10.0,
            ),
            Pos2::new(
                (outer.right() - 14.0).max(outer.left() + gutter + 1.0),
                outer.bottom() - 44.0,
            ),
        );
        if plot.width() < 180.0 || plot.height() < 40.0 || plot.right() > outer.right() {
            painter.text(
                outer.center(),
                Align2::CENTER_CENTER,
                "Widen the workspace to show axes",
                FontId::proportional(12.0),
                text_color,
            );
            return;
        }
        let grid = Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color);
        painter.rect_stroke(plot, 0.0, grid, StrokeKind::Inside);
        for tick in &y_ticks {
            let y = plot.bottom() - tick.fraction * plot.height();
            painter.line_segment(
                [Pos2::new(plot.left(), y), Pos2::new(plot.right(), y)],
                grid,
            );
            painter.text(
                Pos2::new(plot.left() - 8.0, y),
                Align2::RIGHT_CENTER,
                &tick.label,
                font.clone(),
                text_color,
            );
        }
        let max_x_label = x_ticks
            .iter()
            .map(|tick| {
                painter
                    .layout_no_wrap(tick.label.clone(), font.clone(), text_color)
                    .size()
                    .x
            })
            .fold(0.0_f32, f32::max);
        for (index, tick) in x_ticks.iter().enumerate() {
            if x_ticks.len() > 1
                && plot.width() / ((x_ticks.len() - 1) as f32) < max_x_label + 12.0
                && index != 0
                && index != x_ticks.len() - 1
            {
                continue;
            }
            let x = plot.left() + tick.fraction * plot.width();
            painter.line_segment(
                [Pos2::new(x, plot.top()), Pos2::new(x, plot.bottom())],
                grid,
            );
            let align = if tick.fraction == 0.0 {
                Align2::LEFT_TOP
            } else if tick.fraction == 1.0 {
                Align2::RIGHT_TOP
            } else {
                Align2::CENTER_TOP
            };
            painter.text(
                Pos2::new(x, plot.bottom() + 6.0),
                align,
                &tick.label,
                font.clone(),
                text_color,
            );
        }
        painter.text(
            Pos2::new(plot.center().x, outer.bottom() - 4.0),
            Align2::CENTER_BOTTOM,
            "Seconds since first retained sample",
            FontId::proportional(11.0),
            text_color,
        );
        // Scale first so f64::MAX to -f64::MAX cannot overflow the range.
        let scale = y_min.abs().max(y_max.abs()).max(f64::MIN_POSITIVE);
        let lo = y_min / scale;
        let hi = y_max / scale;
        let map = |time: std::time::Duration, value: f64| {
            let x = if span == 0.0 {
                0.5
            } else {
                time.saturating_sub(start).as_secs_f64() / span
            };
            let y = if hi == lo {
                0.5
            } else {
                (value / scale - lo) / (hi - lo)
            };
            Pos2::new(
                plot.left() + x.clamp(0.0, 1.0) as f32 * plot.width(),
                plot.bottom() - y.clamp(0.0, 1.0) as f32 * plot.height(),
            )
        };
        let trace_painter = painter.with_clip_rect(plot);
        let mut previous = None;
        for point in points {
            if let Some((time, value)) = point {
                let pos = map(time, value);
                if let Some(before) = previous {
                    trace_painter.line_segment(
                        [before, pos],
                        Stroke::new(1.5, Color32::from_rgb(28, 140, 130)),
                    );
                }
                trace_painter.circle_filled(pos, 1.5, Color32::from_rgb(28, 140, 130));
                previous = Some(pos);
            } else {
                previous = None;
            }
        }
    }
    pub fn show_details(ui: &mut egui::Ui, trace: &Trace) {
        ui.collapsing("Plot details", |ui| {
            ui.small(format!(
                "Local monotonic receipt time · units unknown · history {}/{} · {} s maximum",
                trace.points().len(), MAX_POINTS, WINDOW.as_secs()
            ));
            ui.small(format!("History evictions: {} · paused skips: {} · stale session rejects: {} · backward receipt rejects: {}",
                trace.counters.evicted, trace.counters.skipped_paused, trace.counters.stale, trace.counters.backward));
            if let Some((min, max)) = trace.extents() {
                ui.small(format!("Value range: {min} … {max}"));
            }
            if let (Some(first), Some(last)) = (trace.points().front(), trace.points().back()) {
                ui.small(format!("Time range: {:.3} … {:.3} s since local clock origin", first.receipt.as_secs_f64(), last.receipt.as_secs_f64()));
            }
            ui.small("Downsampling preserves extrema except buckets containing gaps. Transport drops are reported separately by the workspace.");
        });
    }
}

fn toolbar_action(ui: &mut egui::Ui, text: &str, accessible_name: &str) -> egui::Response {
    let response = ui.add(
        egui::Button::new(text)
            .small()
            .fill(ui.visuals().panel_fill)
            .stroke(Stroke::NONE)
            .min_size(egui::vec2(0.0, 24.0)),
    );
    response.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            response.enabled(),
            accessible_name,
        )
    });
    // Match the content surface at rest while retaining finite button paint
    // geometry. This egui version reports Rect::NOTHING for transparent,
    // unstroked button rectangles. Quiet actions need an explicit focus outline.
    if response.has_focus() {
        ui.painter().rect_stroke(
            response.rect,
            2.0,
            Stroke::new(1.5, ui.visuals().hyperlink_color),
            StrokeKind::Inside,
        );
    }
    response.on_hover_text(accessible_name)
}

#[cfg(test)]
mod compact_tests {
    use super::*;

    #[test]
    fn compact_plot_keeps_useful_graph_and_full_keyboard_action_names() {
        let context = egui::Context::default();
        context.enable_accesskit();
        let mut trace = Trace::default();
        let field = FieldPath(vec![crate::plots::PathSegment::Field("data".into())]);
        trace.push(
            0,
            crate::plots::Sample {
                receipt: std::time::Duration::ZERO,
                value: Ok(crate::plots::Scalar::Float(1.25)),
            },
        );
        let draw = |trace: &mut Trace, event: Option<egui::Event>| {
            context.run(
                egui::RawInput {
                    screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(472.0, 260.0))),
                    events: event.into_iter().collect(),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        PlotRenderer.show(ui, trace, PlotStatus::Receiving, Some(&field))
                    });
                },
            )
        };
        let output = draw(&mut trace, None);
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Rect(rect) if rect.stroke.width == 1.0 && rect.rect.width() > 180.0 && rect.rect.height() >= 140.0 && rect.rect.top() < 50.0
        )), "Short field/exact value must share the toolbar and retain an early, useful graph");
        let tree = output.platform_output.accesskit_update.unwrap();
        let pause = tree
            .nodes
            .iter()
            .find(|(_, node)| node.label() == Some("Pause plot"))
            .unwrap()
            .0;
        let clear = tree
            .nodes
            .iter()
            .find(|(_, node)| node.label() == Some("Clear history"))
            .unwrap()
            .0;
        let action = |target, action| {
            egui::Event::AccessKitActionRequest(egui::accesskit::ActionRequest {
                target,
                action,
                data: None,
            })
        };
        draw(
            &mut trace,
            Some(action(pause, egui::accesskit::Action::Focus)),
        );
        let output = draw(
            &mut trace,
            Some(egui::Event::Key {
                key: egui::Key::Space,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }),
        );
        assert!(trace.paused());
        assert_eq!(trace.points().len(), 1);
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape,
            egui::Shape::Rect(rect) if rect.stroke.width == 1.5 && rect.stroke.color == context.style().visuals.hyperlink_color
        )), "Quiet action must preserve a visible keyboard focus ring");
        let output = draw(&mut trace, None);
        let resume = output
            .platform_output
            .accesskit_update
            .unwrap()
            .nodes
            .into_iter()
            .find(|(_, node)| node.label() == Some("Resume plot"))
            .unwrap()
            .0;
        draw(
            &mut trace,
            Some(action(clear, egui::accesskit::Action::Click)),
        );
        assert!(trace.points().is_empty());
        assert!(trace.paused());
        draw(
            &mut trace,
            Some(action(resume, egui::accesskit::Action::Click)),
        );
        assert!(!trace.paused());
    }
}
