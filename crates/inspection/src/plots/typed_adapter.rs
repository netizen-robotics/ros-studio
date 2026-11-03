//! Lossless boundary between the candidate typed contract and the plot model.
//! Used by tests/components only until paired live protocol support is enabled.
use super::{FieldPath, Gap, PathSegment, Scalar};
use studio_protocol::structured::{self, FloatValue, LookupError, Value};

pub fn typed_path(path: &FieldPath) -> structured::FieldPath {
    path.0
        .iter()
        .map(|part| match part {
            PathSegment::Field(name) => structured::PathSegment::Member(name.clone()),
            PathSegment::Index(index) => structured::PathSegment::Index(*index),
        })
        .collect()
}

/// Caller supplies a structurally validated typed value, never a Debug preview.
/// Integer parsing here reads an explicit integer field's exact decimal wire
/// representation; arbitrary strings and complete Text values are not numeric.
pub fn numeric_value(root: &Value, path: &FieldPath) -> Result<Scalar, Gap> {
    let value = root
        .lookup(&typed_path(path))
        .map_err(|error| match error {
            LookupError::Incomplete | LookupError::Unsupported => Gap::Incomplete,
            LookupError::Missing | LookupError::InvalidPath => Gap::Missing,
            LookupError::WrongKind => Gap::Nonnumeric,
        })?;
    scalar(value)
}

fn scalar(value: &Value) -> Result<Scalar, Gap> {
    match value {
        Value::Signed { value } => value
            .parse()
            .map(Scalar::Signed)
            .map_err(|_| Gap::Nonnumeric),
        Value::Unsigned { value } => value
            .parse()
            .map(Scalar::Unsigned)
            .map_err(|_| Gap::Nonnumeric),
        Value::Float {
            value: FloatValue::Finite(value),
        } => value
            .parse::<f64>()
            .ok()
            .filter(|n| n.is_finite())
            .map(Scalar::Float)
            .ok_or(Gap::NonFinite),
        Value::Float { .. } => Err(Gap::NonFinite),
        _ => Err(Gap::Nonnumeric),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_protocol::structured::Field;

    #[test]
    fn explicit_numeric_kinds_retain_fidelity_and_never_parse_text() {
        let path = FieldPath(vec![
            PathSegment::Field("a.b".into()),
            PathSegment::Index(0),
        ]);
        let data = Value::Object {
            fields: vec![Field {
                name: "a.b".into(),
                value: Value::Array {
                    items: vec![Value::Unsigned {
                        value: u64::MAX.to_string(),
                    }],
                    original_items: 1,
                },
            }],
            original_fields: 1,
        };
        let scalar = numeric_value(&data, &path).unwrap();
        assert_eq!(scalar, Scalar::Unsigned(u64::MAX));
        assert_eq!(scalar.coordinate(), Err(Gap::PrecisionLoss));
        let text = Value::Text {
            value: "123".into(),
            original_bytes: 3,
        };
        assert_eq!(
            numeric_value(&text, &FieldPath(vec![])),
            Err(Gap::Nonnumeric)
        );
        let incomplete = Value::Array {
            items: vec![],
            original_items: 1,
        };
        assert_eq!(
            numeric_value(&incomplete, &FieldPath(vec![PathSegment::Index(0)])),
            Err(Gap::Incomplete)
        );
        let nan = Value::Float {
            value: FloatValue::Nan,
        };
        assert_eq!(numeric_value(&nan, &FieldPath(vec![])), Err(Gap::NonFinite));
    }
}

pub const MAX_NUMERIC_FIELDS: usize = 128;
/// Aggregate retained path storage, including FieldPath/segment storage and
/// duplicated member-name UTF-8 bytes. The traversal scratch path is separate
/// and bounded by the validated protocol depth/member-name limits.
pub const MAX_NUMERIC_PATH_BYTES: usize = 64 * 1024;

/// Enumerate a deterministic prefix of received numeric leaves that can be drawn
/// without precision loss. Retained leaves inside truncated containers remain
/// eligible. NaN/infinity and inexact integers are not initial choices; if a
/// selected field later changes to one, `numeric_value`/Trace expose its gap.
/// Stop at 128 choices, 64 KiB of duplicated paths, or protocol traversal bounds.
/// No allocation for a retained path occurs before its storage is charged.
pub fn numeric_fields(root: &Value) -> Vec<FieldPath> {
    fn walk(
        value: &Value,
        path: &mut Vec<PathSegment>,
        left: &mut usize,
        path_bytes: &mut usize,
        out: &mut Vec<FieldPath>,
    ) {
        if *left == 0 || path.len() > structured::MAX_DEPTH {
            return;
        }
        *left -= 1;
        match value {
            Value::Object { fields, .. } if path.len() < structured::MAX_DEPTH => {
                for field in fields {
                    if *left == 0 {
                        break;
                    }
                    path.push(PathSegment::Field(field.name.clone()));
                    walk(&field.value, path, left, path_bytes, out);
                    path.pop();
                }
            }
            Value::Array { items, .. } if path.len() < structured::MAX_DEPTH => {
                for (index, value) in items.iter().enumerate() {
                    if *left == 0 {
                        break;
                    }
                    path.push(PathSegment::Index(index));
                    walk(value, path, left, path_bytes, out);
                    path.pop();
                }
            }
            Value::Signed { .. } | Value::Unsigned { .. } | Value::Float { .. } => {
                if scalar(value).and_then(Scalar::coordinate).is_ok() {
                    let cost = path.iter().fold(
                        std::mem::size_of::<FieldPath>().saturating_add(
                            path.len()
                                .saturating_mul(std::mem::size_of::<PathSegment>()),
                        ),
                        |bytes, segment| {
                            bytes.saturating_add(match segment {
                                PathSegment::Field(name) => name.len(),
                                PathSegment::Index(_) => 0,
                            })
                        },
                    );
                    if cost > *path_bytes {
                        *left = 0;
                        return;
                    }
                    *path_bytes -= cost;
                    out.push(FieldPath(path.clone()));
                    if out.len() == MAX_NUMERIC_FIELDS {
                        *left = 0;
                    }
                }
            }
            _ => (),
        }
    }
    // Reserve/charge the output vector's full capacity too, so its unused slots
    // cannot take actual path storage beyond the aggregate limit. Per-choice
    // FieldPath charging above is deliberately conservative in addition.
    let mut out = Vec::with_capacity(MAX_NUMERIC_FIELDS);
    let mut remaining = structured::MAX_NODES;
    let mut path_bytes =
        MAX_NUMERIC_PATH_BYTES - MAX_NUMERIC_FIELDS * std::mem::size_of::<FieldPath>();
    walk(
        root,
        &mut Vec::new(),
        &mut remaining,
        &mut path_bytes,
        &mut out,
    );
    out
}

#[cfg(test)]
mod field_tests {
    use super::*;
    use studio_protocol::structured::Field;
    #[test]
    fn numeric_choice_output_bounds_deep_path_duplication_and_wide_arrays() {
        let mut deep = Value::Array {
            items: vec![Value::Signed { value: "1".into() }; 1_900],
            original_items: 1_900,
        };
        for _ in 0..15 {
            deep = Value::Object {
                fields: vec![Field {
                    name: "a".repeat(1_024),
                    value: deep,
                }],
                original_fields: 1,
            };
        }
        let sample = structured::Sample {
            contract_version: structured::CONTRACT_VERSION,
            topic: "/fixture".into(),
            message_type: "fixture/msg/Deep".into(),
            source_time: None,
            value: deep.clone(),
        };
        sample
            .encode()
            .expect("Regression input is a valid bounded sample");
        let choices = numeric_fields(&deep);
        let allocated_path_bytes: usize = choices.capacity() * std::mem::size_of::<FieldPath>()
            + choices
                .iter()
                .map(|path| {
                    path.0.len() * std::mem::size_of::<PathSegment>()
                        + path
                            .0
                            .iter()
                            .map(|segment| match segment {
                                PathSegment::Field(name) => name.len(),
                                PathSegment::Index(_) => 0,
                            })
                            .sum::<usize>()
                })
                .sum::<usize>();
        assert!(
            allocated_path_bytes <= 64 * 1024,
            "Duplicated paths used {allocated_path_bytes} bytes"
        );
        assert!(!choices.is_empty());
        assert!(choices.len() <= 128);
        assert_eq!(
            choices,
            numeric_fields(&deep),
            "Choice prefix is deterministic"
        );
        for choice in choices {
            assert_eq!(numeric_value(&deep, &choice), Ok(Scalar::Signed(1)));
        }
        let wide = Value::Array {
            items: vec![Value::Signed { value: "1".into() }; 1_900],
            original_items: 1_900,
        };
        let choices = numeric_fields(&wide);
        assert_eq!(choices.len(), 128);
        assert_eq!(choices[127], FieldPath(vec![PathSegment::Index(127)]));
    }
    #[test]
    fn field_choices_are_complete_representable_leaves_with_unambiguous_paths() {
        let value = Value::Object {
            fields: vec![
                Field {
                    name: "a.b".into(),
                    value: Value::Array {
                        items: vec![
                            Value::Unsigned { value: "42".into() },
                            Value::Unsigned {
                                value: u64::MAX.to_string(),
                            },
                            Value::Float {
                                value: FloatValue::Nan,
                            },
                            Value::Text {
                                value: "123".into(),
                                original_bytes: 3,
                            },
                        ],
                        original_items: 8,
                    },
                },
                Field {
                    name: "a".into(),
                    value: Value::Object {
                        fields: vec![Field {
                            name: "b".into(),
                            value: Value::Signed { value: "-2".into() },
                        }],
                        original_fields: 1,
                    },
                },
            ],
            original_fields: 4,
        };
        let fields = numeric_fields(&value);
        assert_eq!(fields.len(), 2);
        assert_eq!(fields[0].label(), "$[\"a.b\"][0]");
        assert_eq!(fields[1].label(), "$[\"a\"][\"b\"]");
        assert_eq!(numeric_value(&value, &fields[0]), Ok(Scalar::Unsigned(42)));
        assert!(numeric_fields(&Value::Float {
            value: FloatValue::PositiveInfinity
        })
        .is_empty());
        assert_eq!(
            numeric_fields(&Value::Float {
                value: FloatValue::Finite("1e-300".into())
            }),
            vec![FieldPath(vec![])]
        );
    }
    #[test]
    fn enumerator_stops_at_node_and_depth_budgets() {
        let value = Value::Array {
            items: vec![Value::Signed { value: "1".into() }; 10_000],
            original_items: 10_000,
        };
        assert_eq!(numeric_fields(&value).len(), MAX_NUMERIC_FIELDS);
        let mut deep = Value::Signed { value: "1".into() };
        for _ in 0..structured::MAX_DEPTH + 1 {
            deep = Value::Array {
                items: vec![deep],
                original_items: 1,
            };
        }
        assert!(numeric_fields(&deep).is_empty());
    }
}
