//! Bounded typed value contract2, shared by paired observation protocols.
//! Never converts diagnostic previews into typed data.
use serde::{Deserialize, Serialize};
use std::{collections::HashSet, io::Write};

pub const CONTRACT_VERSION: u32 = 2;
pub const MAX_SAMPLE_BYTES: usize = 512 * 1024;
pub const MAX_NODES: usize = 4096;
pub const MAX_DEPTH: usize = 16;
pub const MAX_ITEMS: usize = 2048;
pub const MAX_FIELDS: usize = 256;
pub const MAX_TEXT_BYTES: usize = 32 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum PathSegment {
    Member(String),
    Index(usize),
}
pub type FieldPath = Vec<PathSegment>;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum FloatValue {
    /// Decimal JSON-number spelling, validated as finite and retained exactly.
    Finite(String),
    Nan,
    PositiveInfinity,
    NegativeInfinity,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Field {
    pub name: String,
    pub value: Value,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Value {
    Bool {
        value: bool,
    },
    /// Canonical base-10 i64 spelling; serialized as text to prevent rounding.
    Signed {
        value: String,
    },
    /// Canonical base-10 u64 spelling; serialized as text to prevent rounding.
    Unsigned {
        value: String,
    },
    Float {
        value: FloatValue,
    },
    Text {
        value: String,
        original_bytes: usize,
    },
    Object {
        fields: Vec<Field>,
        original_fields: usize,
    },
    Array {
        items: Vec<Value>,
        original_items: usize,
    },
    Unsupported {
        type_name: String,
        reason: String,
    },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClockDomain {
    System,
    Ros { epoch: u64 },
}

/// Only supplied when a real source/header timestamp and clock policy are known.
/// Local receipt `Instant` belongs to the receiver and is not serialized here.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceTime {
    pub seconds: i64,
    pub nanoseconds: u32,
    pub clock: ClockDomain,
    pub header_path: FieldPath,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    pub contract_version: u32,
    pub topic: String,
    pub message_type: String,
    pub source_time: Option<SourceTime>,
    pub value: Value,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LookupError {
    InvalidPath,
    Missing,
    WrongKind,
    Incomplete,
    Unsupported,
}

impl Value {
    /// Completeness is recursive. Unknown native types are never complete input
    /// to a renderer merely because their diagnostic text is present.
    pub fn is_complete(&self) -> bool {
        match self {
            Self::Text {
                value,
                original_bytes,
            } => value.len() == *original_bytes,
            Self::Object {
                fields,
                original_fields,
            } => fields.len() == *original_fields && fields.iter().all(|f| f.value.is_complete()),
            Self::Array {
                items,
                original_items,
            } => items.len() == *original_items && items.iter().all(Self::is_complete),
            Self::Unsupported { .. } => false,
            _ => true,
        }
    }

    /// Exact paths distinguish literal field names such as `a.b` from nesting.
    /// Complete retained children remain usable even if an unrelated sibling
    /// was truncated. Missing children inside truncated containers are explicit.
    pub fn lookup(&self, path: &[PathSegment]) -> Result<&Self, LookupError> {
        validate_path(path).map_err(|_| LookupError::InvalidPath)?;
        let mut current = self;
        for segment in path {
            current = match (current, segment) {
                (
                    Self::Object {
                        fields,
                        original_fields,
                    },
                    PathSegment::Member(name),
                ) => fields
                    .iter()
                    .find(|f| &f.name == name)
                    .map(|f| &f.value)
                    .ok_or(if fields.len() < *original_fields {
                        LookupError::Incomplete
                    } else {
                        LookupError::Missing
                    })?,
                (
                    Self::Array {
                        items,
                        original_items,
                    },
                    PathSegment::Index(index),
                ) => items.get(*index).ok_or(if *index < *original_items {
                    LookupError::Incomplete
                } else {
                    LookupError::Missing
                })?,
                (Self::Unsupported { .. }, _) => return Err(LookupError::Unsupported),
                _ => return Err(LookupError::WrongKind),
            };
        }
        if matches!(current, Self::Unsupported { .. }) {
            Err(LookupError::Unsupported)
        } else if !current.is_complete() {
            Err(LookupError::Incomplete)
        } else {
            Ok(current)
        }
    }
}

fn label(text: &str, max: usize) -> bool {
    !text.is_empty() && text.len() <= max && !text.contains('\0')
}

fn validate_path(path: &[PathSegment]) -> Result<(), String> {
    if path.len() > MAX_DEPTH
        || path
            .iter()
            .any(|segment| matches!(segment, PathSegment::Member(name) if !label(name, 1024)))
    {
        return Err("Invalid typed field path".into());
    }
    Ok(())
}

fn check_value(value: &Value, depth: usize, remaining: &mut usize) -> Result<(), String> {
    if depth > MAX_DEPTH || *remaining == 0 {
        return Err("Typed value depth/node budget exceeded".into());
    }
    *remaining -= 1;
    match value {
        Value::Signed { value } => {
            if value.len() > 20 || value.parse::<i64>().map(|n| n.to_string()).as_ref() != Ok(value)
            {
                return Err("Invalid canonical signed integer".into());
            }
        }
        Value::Unsigned { value } => {
            if value.len() > 20 || value.parse::<u64>().map(|n| n.to_string()).as_ref() != Ok(value)
            {
                return Err("Invalid canonical unsigned integer".into());
            }
        }
        Value::Float {
            value: FloatValue::Finite(value),
        } => {
            if value.len() > 128
                || serde_json::from_str::<serde_json::Number>(value)
                    .ok()
                    .and_then(|n| n.as_f64())
                    .is_none_or(|n| !n.is_finite())
            {
                return Err("Invalid finite numeric spelling".into());
            }
        }
        Value::Text {
            value,
            original_bytes,
        } => {
            if value.len() > MAX_TEXT_BYTES || *original_bytes < value.len() {
                return Err("Invalid bounded text length".into());
            }
        }
        Value::Object {
            fields,
            original_fields,
        } => {
            if fields.len() > MAX_FIELDS || *original_fields < fields.len() {
                return Err("Invalid bounded object length".into());
            }
            let mut names = HashSet::new();
            for field in fields {
                if !label(&field.name, 1024) || !names.insert(&field.name) {
                    return Err("Invalid or duplicate member name".into());
                }
                check_value(&field.value, depth + 1, remaining)?;
            }
        }
        Value::Array {
            items,
            original_items,
        } => {
            if items.len() > MAX_ITEMS || *original_items < items.len() {
                return Err("Invalid bounded array length".into());
            }
            for item in items {
                check_value(item, depth + 1, remaining)?;
            }
        }
        Value::Unsupported { type_name, reason } => {
            if !label(type_name, 1024) || !label(reason, 1024) {
                return Err("Invalid unsupported-value diagnostic".into());
            }
        }
        _ => (),
    }
    Ok(())
}

impl Sample {
    fn check_shape(&self) -> Result<(), String> {
        if self.contract_version != CONTRACT_VERSION {
            return Err("Incompatible typed component contract".into());
        }
        if !label(&self.topic, 4096) || !label(&self.message_type, 4096) {
            return Err("Invalid topic/type identity".into());
        }
        if let Some(time) = &self.source_time {
            if time.nanoseconds >= 1_000_000_000 || time.header_path.is_empty() {
                return Err("Invalid source timestamp/provenance".into());
            }
            validate_path(&time.header_path)?;
        }
        let mut remaining = MAX_NODES;
        check_value(&self.value, 0, &mut remaining)
    }

    /// Serialization growth is bounded during writing, including JSON escapes.
    /// Failure never exposes a partial sample as a valid payload.
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.check_shape()?;
        struct Bounded(Vec<u8>);
        impl Write for Bounded {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if bytes.len() > MAX_SAMPLE_BYTES.saturating_sub(self.0.len()) {
                    return Err(std::io::Error::other("Typed wire byte budget exceeded"));
                }
                self.0.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut out = Bounded(Vec::new());
        serde_json::to_writer(&mut out, self).map_err(|e| e.to_string())?;
        Ok(out.0)
    }

    /// Input bytes are bounded before allocating the deserialized tree. Shape
    /// budgets apply after decode; middleware allocations are outside this API.
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_SAMPLE_BYTES {
            return Err("Typed wire byte budget exceeded".into());
        }
        let value: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        value.check_shape()?;
        Ok(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample(value: Value) -> Sample {
        Sample {
            contract_version: CONTRACT_VERSION,
            topic: "/fixture".into(),
            message_type: "std_msgs/msg/String".into(),
            source_time: None,
            value,
        }
    }
    fn object(name: &str, value: Value) -> Value {
        Value::Object {
            fields: vec![Field {
                name: name.into(),
                value,
            }],
            original_fields: 1,
        }
    }

    #[test]
    fn out_of_original_array_range_is_missing_not_truncated() {
        let value = Value::Array {
            items: vec![Value::Bool { value: true }],
            original_items: 2,
        };
        assert_eq!(
            value.lookup(&[PathSegment::Index(1)]),
            Err(LookupError::Incomplete)
        );
        for index in [2, usize::MAX] {
            assert_eq!(
                value.lookup(&[PathSegment::Index(index)]),
                Err(LookupError::Missing)
            );
        }
    }

    #[test]
    fn exact_numeric_extremes_and_explicit_nonfinite_roundtrip() {
        for value in [
            Value::Signed {
                value: i64::MIN.to_string(),
            },
            Value::Unsigned {
                value: u64::MAX.to_string(),
            },
            Value::Float {
                value: FloatValue::Finite("1.2345678901234567e-200".into()),
            },
            Value::Float {
                value: FloatValue::Nan,
            },
            Value::Float {
                value: FloatValue::NegativeInfinity,
            },
        ] {
            let s = sample(value);
            assert_eq!(Sample::decode(&s.encode().unwrap()).unwrap(), s);
        }
        for value in [
            Value::Signed { value: "01".into() },
            Value::Unsigned {
                value: "18446744073709551616".into(),
            },
            Value::Float {
                value: FloatValue::Finite("NaN".into()),
            },
            Value::Float {
                value: FloatValue::Finite("1e999".into()),
            },
        ] {
            assert!(sample(value).encode().is_err());
        }
    }

    #[test]
    fn unambiguous_paths_and_completeness_do_not_fabricate_missing_values() {
        let mut value = object(
            "a.b",
            Value::Array {
                items: vec![Value::Bool { value: true }],
                original_items: 2,
            },
        );
        let path = vec![PathSegment::Member("a.b".into()), PathSegment::Index(0)];
        assert_eq!(value.lookup(&path), Ok(&Value::Bool { value: true }));
        assert_eq!(
            value.lookup(&[PathSegment::Member("a.b".into())]),
            Err(LookupError::Incomplete)
        );
        assert_eq!(
            value.lookup(&[PathSegment::Member("a.b".into()), PathSegment::Index(1)]),
            Err(LookupError::Incomplete)
        );
        assert_eq!(
            value.lookup(&[PathSegment::Member("a".into())]),
            Err(LookupError::Missing)
        );
        value = object(
            "text",
            Value::Text {
                value: "é".into(),
                original_bytes: 3,
            },
        );
        assert!(!value.is_complete());
        assert_eq!(
            value.lookup(&[PathSegment::Member("text".into())]),
            Err(LookupError::Incomplete)
        );
        assert!(!Value::Unsupported {
            type_name: "wstring".into(),
            reason: "not supported".into()
        }
        .is_complete());
    }

    #[test]
    fn shape_and_serialization_growth_budgets_are_enforced() {
        let mut deep = Value::Bool { value: false };
        for _ in 0..MAX_DEPTH + 1 {
            deep = object("nested", deep);
        }
        assert!(sample(deep).encode().is_err());
        assert!(sample(Value::Array {
            items: vec![Value::Bool { value: true }; MAX_ITEMS + 1],
            original_items: MAX_ITEMS + 1
        })
        .encode()
        .is_err());
        let escaping = Value::Text {
            value: "\n".repeat(MAX_TEXT_BYTES),
            original_bytes: MAX_TEXT_BYTES,
        };
        let fields = (0..16)
            .map(|i| Field {
                name: i.to_string(),
                value: escaping.clone(),
            })
            .collect();
        assert!(sample(Value::Object {
            fields,
            original_fields: 16
        })
        .encode()
        .is_err());
        assert!(Sample::decode(&vec![b' '; MAX_SAMPLE_BYTES + 1]).is_err());
        let repeated = Value::Array {
            items: vec![Value::Bool { value: true }; MAX_ITEMS],
            original_items: MAX_ITEMS,
        };
        assert!(sample(Value::Array {
            items: vec![repeated; 2],
            original_items: 2
        })
        .encode()
        .is_err());
    }

    #[test]
    fn malformed_contract_and_inconsistent_metadata_rejected() {
        let mut s = sample(Value::Text {
            value: "café 🤖".into(),
            original_bytes: "café 🤖".len(),
        });
        assert_eq!(Sample::decode(&s.encode().unwrap()).unwrap(), s);
        s.contract_version = 999;
        assert!(s.encode().is_err());
        let s = sample(Value::Object {
            fields: vec![
                Field {
                    name: "a".into(),
                    value: Value::Bool { value: true }
                };
                2
            ],
            original_fields: 2,
        });
        assert!(s.encode().is_err());
        let s = sample(Value::Text {
            value: "abc".into(),
            original_bytes: 2,
        });
        assert!(s.encode().is_err());
        let bytes = sample(Value::Bool { value: true }).encode().unwrap();
        let mut json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        json["unknown"] = true.into();
        assert!(Sample::decode(&serde_json::to_vec(&json).unwrap()).is_err());
    }
}
