//! Parameter drafts, constraints and local YAML transfer, separate from live handles.
use eframe::egui;
use serde_json::{json, Value};
use std::{collections::BTreeMap, sync::mpsc};
use studio_protocol::tools::{Operation, ParameterChange, MAX_PAYLOAD};
const FIELDS: [&str; 10] = [
    "",
    "bool_value",
    "integer_value",
    "double_value",
    "string_value",
    "byte_array_value",
    "bool_array_value",
    "integer_array_value",
    "double_array_value",
    "string_array_value",
];
const TYPES: [&str; 10] = [
    "Unset",
    "Boolean",
    "Integer",
    "Double",
    "String",
    "Bytes",
    "Bool array",
    "Int array",
    "Double array",
    "String array",
];
#[derive(Clone)]
struct Row {
    current: Option<Value>,
    name: String,
    kind: u8,
    text: String,
    descriptor: Value,
    dirty: bool,
    selected: bool,
    error: String,
}
pub struct Panel {
    rows: Vec<Row>,
    names: String,
    atomic: bool,
    yaml: String,
    path: String,
    pub error: String,
    pub status: String,
    pub result: Value,
    pub event: Value,
    live: bool,
    io: Option<mpsc::Receiver<Result<String, String>>>,
    io_writing: bool,
    transfer_status: String,
    filter: String,
}
impl Default for Panel {
    fn default() -> Self {
        Self {
            rows: Vec::new(),
            names: String::new(),
            atomic: true,
            yaml: String::new(),
            path: String::new(),
            error: String::new(),
            status: "Not read".into(),
            result: Value::Null,
            event: Value::Null,
            live: false,
            io: None,
            io_writing: false,
            transfer_status: String::new(),
            filter: String::new(),
        }
    }
}
impl Panel {
    pub fn disconnected(&mut self) {
        self.live = false;
        self.event = Value::Null;
    }
    pub fn receive(&mut self, value: &Value) {
        self.result = value["write_results"].clone();
        let Some(names) = value["names"].as_array() else {
            self.error = "Invalid parameter response".into();
            return;
        };
        let mut rows = Vec::new();
        for (i, name) in names.iter().enumerate() {
            let Some(name) = name.as_str() else { continue };
            let raw = &value["values"][i];
            let kind = raw["type"].as_u64().unwrap_or(0).min(9) as u8;
            let actual = if kind == 0 {
                Value::Null
            } else {
                raw[FIELDS[kind as usize]].clone()
            };
            let text = if kind == 4 {
                actual.as_str().unwrap_or("").into()
            } else {
                serde_json::to_string(&actual).unwrap_or_default()
            };
            let mut row = Row {
                current: Some(actual.clone()),
                name: name.into(),
                kind,
                text,
                descriptor: value["descriptors"][i].clone(),
                dirty: false,
                selected: false,
                error: String::new(),
            };
            if let Some(draft) = self.rows.iter().find(|r| r.name == name && r.dirty) {
                row.kind = draft.kind;
                row.dirty =
                    draft.kind != kind || change(draft).map(|c| c.value != actual).unwrap_or(true);
                if row.dirty {
                    row.text = draft.text.clone();
                }
                row.selected = draft.selected;
            }
            rows.push(row);
        }
        if !value["write_results"].is_null() {
            let mut merged = self.rows.clone();
            for row in rows {
                if let Some(old) = merged.iter_mut().find(|r| r.name == row.name) {
                    *old = row;
                } else {
                    merged.push(row);
                }
            }
            self.rows = merged;
        } else {
            self.rows = rows;
        }
        self.status = if value["write_results"].is_null() {
            "Parameters read"
        } else {
            "Write completed; values read back"
        }
        .into();
        self.live = true;
        self.error.clear();
    }
    pub fn saved(&self) -> studio_workspace::tools::Parameters {
        studio_workspace::tools::Parameters {
            names: self.names.clone(),
            atomic: self.atomic,
            yaml: self.yaml.clone(),
            drafts: self
                .rows
                .iter()
                .filter(|r| r.dirty)
                .map(|r| studio_workspace::tools::ParameterDraft {
                    name: r.name.clone(),
                    kind: r.kind,
                    text: r.text.clone(),
                })
                .collect(),
        }
    }
    pub fn restore(&mut self, p: &studio_workspace::tools::Parameters) {
        self.names = p.names.clone();
        self.atomic = p.atomic;
        self.yaml = p.yaml.clone();
        self.rows = p
            .drafts
            .iter()
            .map(|r| Row {
                current: None,
                name: r.name.clone(),
                kind: r.kind,
                text: r.text.clone(),
                descriptor: Value::Null,
                dirty: true,
                selected: false,
                error: String::new(),
            })
            .collect();
        self.live = false;
    }
    pub fn ui(
        &mut self,
        ui: &mut egui::Ui,
        node: &str,
        connected: bool,
        busy: bool,
        events: Option<u64>,
    ) -> Option<Operation> {
        let mut op = None;
        if !self.status.is_empty() {
            crate::presentation::muted(ui, &self.status);
        }
        if let Some(result) = self.io.as_ref().and_then(|rx| rx.try_recv().ok()) {
            self.io = None;
            match result {
                Ok(content) => {
                    self.yaml = content;
                    self.error.clear();
                    self.transfer_status = if self.io_writing {
                        "YAML file saved"
                    } else {
                        "YAML file loaded; import as drafts to review"
                    }
                    .into();
                }
                Err(e) => self.error = e,
            }
        }
        ui.horizontal_wrapped(|ui| {
            if crate::presentation::action(ui, "Read parameters", 150.0, connected && !busy, false)
                .clicked()
            {
                op = Some(Operation::ParameterRead {
                    node: node.into(),
                    names: self
                        .names
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect(),
                    timeout_ms: 10000,
                });
            }
            if crate::presentation::action(
                ui,
                if events.is_some() {
                    "Stop events"
                } else {
                    "Observe events"
                },
                142.0,
                connected,
                false,
            )
            .clicked()
            {
                op = Some(if let Some(target) = events {
                    Operation::Stop { target }
                } else {
                    Operation::ParameterEvents { node: node.into() }
                });
            }
        });
        ui.add_space(6.0);
        ui.collapsing("Read selected names", |ui| {
            crate::presentation::muted(
                ui,
                "Names, separated by commas. Leave empty for all parameters.",
            );
            ui.add(
                studio_ui_foundation::theme::input(ui, &mut self.names)
                    .desired_width(f32::INFINITY),
            );
        });
        ui.add(
            studio_ui_foundation::theme::input(ui, &mut self.filter)
                .hint_text(studio_ui_foundation::theme::hint(
                    ui,
                    "Filter parameter names",
                ))
                .desired_width(f32::INFINITY),
        );
        if !self.live {
            crate::presentation::muted(ui, "Read parameters before applying drafts.");
        }
        let filter = self.filter.to_lowercase();
        let width = ui.available_width();
        let name_width = (width * 0.22).clamp(130.0, 170.0);
        let value_width = (width * 0.19).clamp(100.0, 145.0);
        let draft_width = (width * 0.25).clamp(140.0, 200.0);
        egui::ScrollArea::horizontal()
            .id_salt("parameter-table-scroll")
            .auto_shrink([false, true])
            .show(ui, |ui| {
                egui::Grid::new("parameter-rows")
                    .striped(true)
                    .min_col_width(24.0)
                    .spacing(egui::vec2(12.0, 10.0))
                    .show(ui, |ui| {
                        for heading in ["Apply", "Parameter", "Current", "Draft", "", ""] {
                            studio_ui_foundation::theme::strong(ui, heading);
                        }
                        ui.end_row();
                        for row in &mut self.rows {
                            let readonly = row.descriptor["read_only"] == true;
                            row.error = if readonly {
                                String::new()
                            } else {
                                change(row).err().unwrap_or_default()
                            };
                            if !row.name.to_lowercase().contains(&filter) {
                                continue;
                            }
                            ui.add_enabled(
                                !readonly,
                                egui::Checkbox::without_text(&mut row.selected),
                            );
                            ui.allocate_ui_with_layout(
                                egui::vec2(name_width, 42.0),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    ui.set_width(name_width);
                                    ui.add(egui::Label::new(&row.name).truncate())
                                        .on_hover_text(&row.name);
                                    crate::presentation::muted(
                                        ui,
                                        *TYPES.get(row.kind as usize).unwrap_or(&"Unsupported"),
                                    );
                                },
                            );
                            let current = row
                                .current
                                .as_ref()
                                .map(|v| {
                                    if v.is_string() {
                                        v.as_str().unwrap_or("").to_owned()
                                    } else {
                                        v.to_string()
                                    }
                                })
                                .unwrap_or_else(|| "Not read".into());
                            studio_ui_foundation::theme::label_slot(
                                ui,
                                value_width,
                                egui::RichText::new(&current).monospace(),
                            )
                            .on_hover_text(&current);
                            let response = ui
                                .add_enabled_ui(!readonly, |ui| {
                                    ui.add_sized(
                                        [draft_width, 30.0],
                                        studio_ui_foundation::theme::input(ui, &mut row.text)
                                            .desired_width(draft_width),
                                    )
                                })
                                .inner;
                            if response.changed() {
                                row.dirty = true;
                            }
                            let parsed = change(row);
                            row.error = if readonly {
                                String::new()
                            } else {
                                parsed.as_ref().err().cloned().unwrap_or_default()
                            };
                            if crate::presentation::action(
                                ui,
                                "Apply",
                                62.0,
                                connected
                                    && self.live
                                    && !busy
                                    && !readonly
                                    && row.dirty
                                    && parsed.is_ok(),
                                false,
                            )
                            .on_hover_text(if readonly {
                                "Read only"
                            } else if !row.error.is_empty() {
                                &row.error
                            } else {
                                "Apply only this parameter, then read it back"
                            })
                            .clicked()
                            {
                                op = Some(Operation::ParameterWrite {
                                    node: node.into(),
                                    changes: vec![parsed.unwrap()],
                                    atomic: true,
                                    timeout_ms: 10000,
                                });
                            }
                            ui.menu_button("Details", |ui| {
                                ui.label(&row.name);
                                if readonly {
                                    ui.label("Read only");
                                }
                                if let Some(description) = row.descriptor["description"].as_str() {
                                    ui.label(description);
                                }
                                if !row.error.is_empty() {
                                    ui.colored_label(ui.visuals().error_fg_color, &row.error);
                                }
                                ui.collapsing("Type and constraints", |ui| {
                                    egui::ComboBox::from_id_salt((&row.name, "type"))
                                        .width(110.0)
                                        .selected_text(
                                            *TYPES.get(row.kind as usize).unwrap_or(&"Unsupported"),
                                        )
                                        .show_ui(ui, |ui| {
                                            for (kind, name) in TYPES.iter().enumerate() {
                                                if ui
                                                    .add_enabled(
                                                        !readonly
                                                            && row.descriptor["dynamic_typing"]
                                                                == true,
                                                        egui::Button::new(*name),
                                                    )
                                                    .clicked()
                                                {
                                                    row.kind = kind as u8;
                                                    row.dirty = true;
                                                }
                                            }
                                        });
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(
                                                serde_json::to_string_pretty(&row.descriptor)
                                                    .unwrap_or_default(),
                                            )
                                            .monospace(),
                                        )
                                        .wrap(),
                                    );
                                });
                            });
                            ui.end_row();
                        }
                    });
            });
        let first_error = self
            .rows
            .iter()
            .find(|r| !r.error.is_empty())
            .map(|r| format!("{}: {}", r.name, r.error));
        if let Some(error) = first_error {
            ui.add_sized(
                [ui.available_width(), 24.0],
                egui::Label::new(
                    egui::RichText::new(&error)
                        .small()
                        .color(ui.visuals().error_fg_color),
                )
                .truncate(),
            )
            .on_hover_text(error);
        }
        let selected = self.rows.iter().filter(|r| r.selected).collect::<Vec<_>>();
        let changes = selected
            .iter()
            .map(|r| change(r))
            .collect::<Result<Vec<_>, _>>();
        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.atomic, "Apply selected atomically");
            if crate::presentation::action(
                ui,
                "Apply selected",
                142.0,
                connected && self.live && !busy && !selected.is_empty() && changes.is_ok(),
                true,
            )
            .clicked()
            {
                op = Some(Operation::ParameterWrite {
                    node: node.into(),
                    changes: changes.unwrap(),
                    atomic: self.atomic,
                    timeout_ms: 10000,
                });
            }
            crate::presentation::muted(ui, format!("{} selected", selected.len()));
        });
        ui.add_space(12.0);
        if !self.result.is_null() {
            ui.collapsing("Write result and rejection details", |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(
                            serde_json::to_string_pretty(&self.result).unwrap_or_default(),
                        )
                        .monospace(),
                    )
                    .selectable(true)
                    .wrap(),
                );
            });
        }
        if !self.event.is_null() {
            ui.collapsing("Latest parameter event", |ui| {
                ui.add(
                    egui::Label::new(
                        egui::RichText::new(
                            serde_json::to_string_pretty(&self.event).unwrap_or_default(),
                        )
                        .monospace(),
                    )
                    .selectable(true)
                    .wrap(),
                );
            });
        }
        ui.collapsing("Parameter YAML import / export", |ui| {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        self.live,
                        egui::Button::new("Prepare YAML from current values"),
                    )
                    .clicked()
                {
                    let values = self
                        .rows
                        .iter()
                        .filter_map(|r| {
                            r.current
                                .as_ref()
                                .filter(|v| !v.is_null())
                                .map(|v| (r.name.clone(), v.clone()))
                        })
                        .collect::<BTreeMap<_, _>>();
                    self.yaml = serde_yaml::to_string(&json!({node:{"ros__parameters":values}}))
                        .unwrap_or_default();
                }
                if ui.button("Import YAML as drafts").clicked() {
                    match import(&self.yaml, node, &self.rows) {
                        Ok(rows) => {
                            self.rows = rows;
                            self.live = false;
                            self.error.clear();
                        }
                        Err(e) => self.error = e,
                    }
                }
                if ui.button("Copy YAML").clicked() {
                    ui.ctx().copy_text(self.yaml.clone());
                }
            });
            ui.add(
                egui::TextEdit::multiline(&mut self.yaml)
                    .desired_rows(8)
                    .desired_width(f32::INFINITY)
                    .code_editor(),
            );
            ui.horizontal_wrapped(|ui| {
                ui.label("YAML file");
                ui.add(
                    studio_ui_foundation::theme::input(ui, &mut self.path)
                        .hint_text(studio_ui_foundation::theme::hint(ui, "Absolute path")),
                );
                if ui
                    .add_enabled(
                        self.io.is_none() && !self.path.is_empty(),
                        egui::Button::new("Load YAML file"),
                    )
                    .clicked()
                {
                    self.io_writing = false;
                    self.transfer_status = "Loading YAML file".into();
                    self.io = Some(crate::transfers::start(self.path.clone(), None));
                }
                if ui
                    .add_enabled(
                        self.io.is_none()
                            && !self.path.is_empty()
                            && self.yaml.len() <= MAX_PAYLOAD,
                        egui::Button::new("Save YAML file"),
                    )
                    .clicked()
                {
                    self.io_writing = true;
                    self.transfer_status = "Saving YAML file".into();
                    self.io = Some(crate::transfers::start(
                        self.path.clone(),
                        Some(self.yaml.clone()),
                    ));
                }
            });
            ui.label(&self.transfer_status);
        });
        if !self.error.is_empty() {
            ui.colored_label(ui.visuals().error_fg_color, &self.error);
        }
        op
    }
}
fn change(row: &Row) -> Result<ParameterChange, String> {
    let value = if row.kind == 4 {
        json!(row.text)
    } else if row.kind == 0 {
        Value::Null
    } else {
        super::workbench::parse_yaml(&row.text)?
    };
    let c = ParameterChange {
        name: row.name.clone(),
        parameter_type: row.kind,
        value,
    };
    c.wire()?;
    if row.descriptor["read_only"] == true {
        return Err("Read only".into());
    }
    if row.descriptor["dynamic_typing"] == false
        && row.descriptor["type"]
            .as_u64()
            .is_some_and(|t| t != row.kind as u64)
    {
        return Err("Parameter type is fixed".into());
    }
    let range = if row.kind == 2 {
        "integer_range"
    } else {
        "floating_point_range"
    };
    if let Some(ranges) = row.descriptor[range].as_array() {
        if let Some(range) = ranges.first() {
            if row.kind == 2 {
                let v = c.value.as_i64().ok_or("Expected integer")?;
                let lo = range["from_value"].as_i64().ok_or("Invalid range")?;
                let hi = range["to_value"].as_i64().ok_or("Invalid range")?;
                let step = range["step"].as_u64().unwrap_or(0);
                if v < lo
                    || v > hi
                    || (step > 0 && ((v as i128 - lo as i128) % (step as i128) != 0) && v != hi)
                {
                    return Err("Outside integer range or step".into());
                }
            } else if row.kind == 3 {
                let v = c.value.as_f64().ok_or("Expected double")?;
                let lo = range["from_value"].as_f64().ok_or("Invalid range")?;
                let hi = range["to_value"].as_f64().ok_or("Invalid range")?;
                let step = range["step"].as_f64().unwrap_or(0.0).abs();
                let q = if step > 0.0 { (v - lo) / step } else { 0.0 };
                if v < lo
                    || v > hi
                    || (step > 0.0
                        && v != lo
                        && v != hi
                        && (q - q.round()).abs() > 1e-8 * q.abs().max(1.0))
                {
                    return Err("Outside floating point range".into());
                }
            }
        }
    }
    Ok(c)
}
fn import(text: &str, node: &str, previous: &[Row]) -> Result<Vec<Row>, String> {
    let value = super::workbench::parse_yaml(text)?;
    let root = value[node]["ros__parameters"]
        .as_object()
        .ok_or("YAML must contain this exact node and ros__parameters mapping")?;
    fn flatten<'a>(
        values: &'a serde_json::Map<String, Value>,
        prefix: &str,
        depth: usize,
        out: &mut BTreeMap<String, &'a Value>,
    ) -> Result<(), String> {
        if depth > 16 {
            return Err("Parameter namespaces exceed depth limit".into());
        }
        for (key, value) in values {
            let name = if prefix.is_empty() {
                key.clone()
            } else {
                format!("{prefix}.{key}")
            };
            if let Some(nested) = value.as_object() {
                flatten(nested, &name, depth + 1, out)?;
            } else if out.insert(name, value).is_some() {
                return Err("Duplicate flattened parameter name".into());
            }
            if out.len() > 256 {
                return Err("At most256 parameters".into());
            }
        }
        Ok(())
    }
    let mut values = BTreeMap::new();
    flatten(root, "", 0, &mut values)?;
    if values.len() > 256 {
        return Err("At most256 parameters".into());
    }
    let mut rows = Vec::new();
    for (name, value) in values {
        let known_kind = previous
            .iter()
            .find(|r| r.name == name)
            .filter(|r| {
                ParameterChange {
                    name: name.clone(),
                    parameter_type: r.kind,
                    value: value.clone(),
                }
                .wire()
                .is_ok()
            })
            .map(|r| r.kind);
        let kind = if let Some(kind) = known_kind {
            kind
        } else if value.is_boolean() {
            1
        } else if value.as_i64().is_some() {
            2
        } else if value.is_f64() {
            3
        } else if value.is_string() {
            4
        } else if let Some(a) = value.as_array() {
            if a.is_empty() {
                previous
                    .iter()
                    .find(|r| r.name == name)
                    .map(|r| r.kind)
                    .ok_or("Read an empty array's type before import")?
            } else if a.iter().all(Value::is_boolean) {
                6
            } else if a.iter().all(|v| v.as_i64().is_some()) {
                7
            } else if a.iter().all(Value::is_number) {
                8
            } else if a.iter().all(Value::is_string) {
                9
            } else {
                return Err("Mixed parameter array types".into());
            }
        } else {
            return Err("Unsupported parameter YAML value".into());
        };
        let text = if kind == 4 {
            value.as_str().unwrap().into()
        } else {
            serde_json::to_string(value).map_err(|e| e.to_string())?
        };
        rows.push(Row {
            current: None,
            name: name.clone(),
            kind,
            text,
            descriptor: Value::Null,
            dirty: true,
            selected: false,
            error: String::new(),
        });
    }
    Ok(rows)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn imported_values_are_inactive_and_preserve_integer_precision() {
        let rows = import(
            "/test:\n  ros__parameters:\n    exact: 9223372036854775807\n",
            "/test",
            &[],
        )
        .unwrap();
        assert_eq!(change(&rows[0]).unwrap().value.as_i64(), Some(i64::MAX));
        assert!(import("/other: {}", "/test", &[]).is_err());
    }
    #[test]
    fn read_only_and_integer_step_are_checked() {
        let mut row = Row {
            current: None,
            name: "test".into(),
            kind: 2,
            text: "3".into(),
            descriptor: json!({"read_only":true}),
            dirty: true,
            selected: true,
            error: String::new(),
        };
        assert!(change(&row).is_err());
        row.descriptor = json!({"integer_range":[{"from_value":0,"to_value":10,"step":2}]});
        assert!(change(&row).is_err());
        row.text = "4".into();
        assert!(change(&row).is_ok());
    }
}

#[cfg(test)]
mod readback_tests {
    use super::*;
    #[test]
    fn targeted_readback_preserves_peer_rows_and_clears_accepted_draft() {
        let mut panel = Panel::default();
        panel.receive(&json!({"names":["a","b"],"values":[{"type":2,"integer_value":1},{"type":4,"string_value":"peer"}],"descriptors":[{},{}]}));
        panel.rows[0].text = "2".into();
        panel.rows[0].dirty = true;
        panel.receive(&json!({"names":["a"],"values":[{"type":2,"integer_value":2}],"descriptors":[{}],"write_results":{"result":{"successful":true}}}));
        assert_eq!(panel.rows.len(), 2);
        assert_eq!(panel.rows[0].text, "2");
        assert!(!panel.rows[0].dirty);
        assert_eq!(panel.rows[1].text, "peer");
    }
}

#[cfg(test)]
mod yaml_roundtrip_tests {
    use super::*;
    #[test]
    fn nested_namespaces_flatten_and_known_byte_types_survive_yaml() {
        let old = Row {
            current: Some(json!([1, 255])),
            name: "bytes".into(),
            kind: 5,
            text: "[1,255]".into(),
            descriptor: json!({"type":5}),
            dirty: false,
            selected: false,
            error: String::new(),
        };
        let rows = import(
            "/test:\n  ros__parameters:\n    bytes: [1, 255]\n    nested:\n      gain: 1.25\n",
            "/test",
            &[old],
        )
        .unwrap();
        assert_eq!(rows.iter().find(|r| r.name == "bytes").unwrap().kind, 5);
        assert_eq!(
            change(rows.iter().find(|r| r.name == "nested.gain").unwrap())
                .unwrap()
                .value,
            json!(1.25)
        );
        assert!(super::super::workbench::parse_yaml("data: [&x 1,*x]").is_err());
        assert!(super::super::workbench::parse_yaml("data: '*literal'").is_ok());
    }
}
