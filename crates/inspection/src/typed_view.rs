//! Progressive received-value inspection. No schema inference or preview parsing.
use eframe::egui;
use studio_protocol::structured::{self, FloatValue, Value};

/// Only the latest validated received value is supplied by the caller. Expanded
/// nodes share one hard budget; closed branches are not recursively inspected.
pub fn show(ui: &mut egui::Ui, value: &Value) {
    let mut remaining = structured::MAX_NODES;
    node(ui, "$", value, 0, &mut remaining);
    if remaining == 0 {
        ui.label("Inspection node limit reached; remaining values are hidden.");
    }
    ui.collapsing("Message details", |ui| {
        ui.small("Received typed values · source time unknown unless independently qualified");
        ui.small("Select values to copy them, or use their context menu. Only retained fields are shown.");
    });
}

fn node(ui: &mut egui::Ui, name: &str, value: &Value, depth: usize, remaining: &mut usize) {
    if *remaining == 0 {
        return;
    }
    *remaining -= 1;
    if depth > structured::MAX_DEPTH {
        ui.label("Inspection depth limit reached.");
        return;
    }
    match value {
        Value::Object {
            fields,
            original_fields,
        } => {
            egui::CollapsingHeader::new(format!(
                "{name} · {}",
                retained("fields", fields.len(), *original_fields)
            ))
            .id_salt((name, "object"))
            .default_open(depth == 0)
            .show(ui, |ui| {
                for field in fields {
                    if *remaining == 0 {
                        break;
                    }
                    ui.push_id(&field.name, |ui| {
                        node(ui, &field.name, &field.value, depth + 1, remaining)
                    });
                }
            });
        }
        Value::Array {
            items,
            original_items,
        } => {
            egui::CollapsingHeader::new(format!(
                "{name} · {}",
                retained("items", items.len(), *original_items)
            ))
            .id_salt((name, "array"))
            .default_open(depth == 0)
            .show(ui, |ui| {
                for (index, item) in items.iter().enumerate() {
                    if *remaining == 0 {
                        break;
                    }
                    ui.push_id(index, |ui| {
                        node(ui, &format!("[{index}]"), item, depth + 1, remaining)
                    });
                }
            });
        }
        Value::Text {
            value,
            original_bytes,
        } => {
            scalar_row(
                ui,
                name,
                if value.is_empty() {
                    "(empty string)"
                } else {
                    value
                },
                value,
            );
            if value.len() < *original_bytes {
                ui.small(format!(
                    "Text truncated: {} of {original_bytes} bytes retained",
                    value.len()
                ));
            }
        }
        Value::Unsupported { type_name, reason } => {
            ui.label(format!("{name}: Unsupported {type_name}: {reason}"));
        }
        _ => {
            scalar_row(ui, name, scalar_label(value), scalar_label(value));
        }
    }
}
fn scalar_row(ui: &mut egui::Ui, name: &str, display: &str, exact: &str) {
    if ui.available_width() < 260.0 {
        ui.label(format!("{name}:"));
        selectable_value(ui, display, exact);
    } else {
        ui.horizontal_top(|ui| {
            let width = (ui.available_width() * 0.32).clamp(80.0, 160.0);
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.allocate_ui_with_layout(
                egui::vec2(width, 0.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    ui.set_min_width(width);
                    ui.set_max_width(width);
                    ui.add(egui::Label::new(format!("{name}:")).wrap());
                },
            );
            ui.add_space(12.0);
            selectable_value(ui, display, exact);
        });
    }
}
fn selectable_value(ui: &mut egui::Ui, display: &str, exact: &str) {
    ui.add(
        egui::Label::new(egui::RichText::new(display).monospace())
            .selectable(true)
            .wrap(),
    )
    .context_menu(|ui| {
        if ui.button("Copy retained value").clicked() {
            ui.ctx().copy_text(exact.to_owned());
            ui.close_menu();
        }
    });
}
fn retained(kind: &str, retained: usize, original: usize) -> String {
    if retained == original {
        format!("{retained} {kind} retained")
    } else {
        format!("{retained} of {original} {kind} retained (truncated)")
    }
}
fn scalar_label(value: &Value) -> &str {
    match value {
        Value::Bool { value: true } => "true",
        Value::Bool { value: false } => "false",
        Value::Signed { value } | Value::Unsigned { value } => value,
        Value::Float {
            value: FloatValue::Finite(value),
        } => value,
        Value::Float {
            value: FloatValue::Nan,
        } => "NaN (not finite)",
        Value::Float {
            value: FloatValue::PositiveInfinity,
        } => "+infinity (not finite)",
        Value::Float {
            value: FloatValue::NegativeInfinity,
        } => "-infinity (not finite)",
        _ => "Unavailable scalar",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn scalar_name_columns_are_left_aligned_with_a_real_value_gap() {
        let context = egui::Context::default();
        let mut left = 0.0;
        let mut column_width = 0.0;
        let output = context.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(480.0, 400.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    left = ui.cursor().left();
                    column_width = (ui.available_width() * 0.32).clamp(80.0, 160.0);
                    scalar_row(ui, "data", "Synthetic", "Synthetic");
                    scalar_row(ui, "longer_field", "Other value", "Other value");
                });
            },
        );
        let rect = |expected: &str| {
            output
                .shapes
                .iter()
                .find_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) if text.galley.text() == expected => {
                        Some(egui::Rect::from_min_size(text.pos, text.galley.size()))
                    }
                    _ => None,
                })
                .expect("Expected field/value text")
        };
        assert!((rect("data:").left() - left).abs() < 1.0);
        assert!((rect("longer_field:").left() - left).abs() < 1.0);
        assert!(rect("Synthetic").left() >= left + column_width + 11.0);
        assert!((rect("Synthetic").left() - rect("Other value").left()).abs() < 1.0);
    }
    #[test]
    fn long_exact_values_wrap_within_wide_and_stacked_rows() {
        let exact = "0123456789".repeat(40);
        let context = egui::Context::default();
        for width in [220.0, 480.0] {
            let mut rendered_width = 0.0;
            let output = context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(width, 900.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        scalar_row(ui, "a_very_long_field_name_for_inspection", &exact, &exact);
                        rendered_width = ui.min_rect().width();
                    });
                },
            );
            assert!(rendered_width <= width, "Value must wrap inside its panel");
            assert!(
                output.shapes.iter().any(|shape| matches!(&shape.shape,
                    egui::Shape::Text(text) if text.galley.text() == exact
                )),
                "Wrapping must retain every source byte"
            );
        }
    }
    #[test]
    fn scalar_and_retention_labels_preserve_wire_fidelity() {
        assert_eq!(
            scalar_label(&Value::Unsigned {
                value: u64::MAX.to_string()
            }),
            "18446744073709551615"
        );
        assert_eq!(
            scalar_label(&Value::Float {
                value: FloatValue::Finite("1.2300e-200".into())
            }),
            "1.2300e-200"
        );
        assert_eq!(retained("items", 2, 8), "2 of 8 items retained (truncated)");
        assert!(!retained("fields", 2, 2).contains("truncated"));
    }
    #[test]
    fn actual_walk_is_bounded_even_for_unvalidated_wide_trees() {
        let value = Value::Array {
            items: vec![Value::Bool { value: true }; 10_000],
            original_items: 10_000,
        };
        let ctx = egui::Context::default();
        let mut remaining = 8;
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| node(ui, "$", &value, 0, &mut remaining));
        });
        assert_eq!(remaining, 0);
    }
    #[test]
    fn expanded_received_array_stays_open_after_count_and_member_order_change() {
        use studio_protocol::structured::Field;
        let context = egui::Context::default();
        context.enable_accesskit();
        context.style_mut(|style| style.animation_time = 0.0);
        let make_value = |count: usize, reverse: bool| {
            let mut fields = vec![
                Field {
                    name: "status".into(),
                    value: Value::Bool { value: true },
                },
                Field {
                    name: "values".into(),
                    value: Value::Array {
                        items: vec![
                            Value::Signed {
                                value: "314159".into()
                            };
                            count
                        ],
                        original_items: count,
                    },
                },
            ];
            if reverse {
                fields.reverse();
            }
            Value::Object {
                fields,
                original_fields: 2,
            }
        };
        let frame = |value: &Value, event: Option<egui::Event>| {
            context.run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(640.0, 600.0),
                    )),
                    events: event.into_iter().collect(),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| show(ui, value));
                },
            )
        };
        fn contains_scalar(output: &egui::FullOutput) -> bool {
            fn has(shape: &egui::Shape) -> bool {
                match shape {
                    egui::Shape::Text(text) => text.galley.text() == "314159",
                    egui::Shape::Vec(shapes) => shapes.iter().any(has),
                    _ => false,
                }
            }
            output.shapes.iter().any(|shape| has(&shape.shape))
        }
        let initial = make_value(1, false);
        let output = frame(&initial, None);
        assert!(
            !contains_scalar(&output),
            "Nested array initially stays collapsed"
        );
        let array = output
            .platform_output
            .accesskit_update
            .unwrap()
            .nodes
            .into_iter()
            .find(|(_, node)| {
                node.label()
                    .is_some_and(|label| label.starts_with("values ·"))
            })
            .expect("Array expander must be exposed to accessibility")
            .0;
        frame(
            &initial,
            Some(egui::Event::AccessKitActionRequest(
                egui::accesskit::ActionRequest {
                    action: egui::accesskit::Action::Click,
                    target: array,
                    data: None,
                },
            )),
        );
        assert!(
            contains_scalar(&frame(&initial, None)),
            "Explicitly expanded array must reveal its scalar"
        );
        assert!(
            contains_scalar(&frame(&make_value(2, false), None)),
            "New array length must preserve expansion"
        );
        assert!(
            contains_scalar(&frame(&make_value(2, true), None)),
            "Reordered object members must preserve expansion by field name"
        );
    }
}
