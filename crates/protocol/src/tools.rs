//! Bounded explicit interaction contract. No operation is restored or replayed.
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub const MAX_PAYLOAD: usize = 64 * 1024;
pub const MAX_OPERATIONS: usize = 16;
pub const MAX_GRAPH: usize = 4096;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Operation {
    NodeInfo {
        node: String,
    },
    ParameterRead {
        node: String,
        names: Vec<String>,
        timeout_ms: u64,
    },
    ParameterWrite {
        node: String,
        changes: Vec<ParameterChange>,
        atomic: bool,
        timeout_ms: u64,
    },
    ParameterEvents {
        node: String,
    },
    Goal {
        action: String,
        message_type: String,
        bundle: String,
        value: Value,
        timeout_ms: u64,
    },
    Cancel {
        target: u64,
    },
    Call {
        service: String,
        message_type: String,
        bundle: String,
        value: Value,
        timeout_ms: u64,
    },
    ServiceInfo {
        service: String,
    },
    Graph {
        section: String,
        node: String,
    },
    Details {
        topic: String,
    },
    Schema {
        message_type: String,
        bundle: String,
    },
    Publish {
        topic: String,
        message_type: String,
        bundle: String,
        value: Value,
        interval_ms: u64,
        best_effort: bool,
        transient_local: bool,
    },
    Statistics {
        #[serde(default)]
        bundle: String,
        topic: String,
        message_type: String,
        best_effort: bool,
        transient_local: bool,
    },
    Stop {
        target: u64,
    },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Update {
    pub state: String,
    pub value: Value,
    pub terminal: bool,
}
impl Update {
    pub fn new(state: &str, value: Value, terminal: bool) -> Self {
        Self {
            state: state.into(),
            value,
            terminal,
        }
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.state.len() > 128
            || serde_json::to_vec(&self.value)
                .map_err(|e| e.to_string())?
                .len()
                > MAX_PAYLOAD
        {
            return Err("Tools update exceeds budget".into());
        }
        Ok(())
    }
}
impl Operation {
    pub fn validate(&self) -> Result<(), String> {
        let text = |s: &str| {
            if s.len() > 4096 || s.contains('\0') {
                Err("Invalid bounded Tools text".to_owned())
            } else {
                Ok(())
            }
        };
        match self {
            Self::NodeInfo { node } | Self::ParameterEvents { node } => text(node)?,
            Self::ParameterRead {
                node,
                names,
                timeout_ms,
            } => {
                text(node)?;
                if names.len() > 256 || !(1..=60_000).contains(timeout_ms) {
                    return Err("Parameter read exceeds bounds".into());
                }
                for name in names {
                    text(name)?;
                }
            }
            Self::ParameterWrite {
                node,
                changes,
                timeout_ms,
                ..
            } => {
                text(node)?;
                if changes.is_empty() || changes.len() > 256 || !(1..=60_000).contains(timeout_ms) {
                    return Err("Parameter write exceeds bounds".into());
                }
                for c in changes {
                    text(&c.name)?;
                    c.wire()?;
                }
            }
            Self::Cancel { target } => {
                if *target == 0 {
                    return Err("Zero goal target".into());
                }
            }
            Self::Goal {
                action: service,
                message_type,
                bundle,
                value,
                timeout_ms,
            }
            | Self::Call {
                service,
                message_type,
                bundle,
                value,
                timeout_ms,
            } => {
                text(service)?;
                text(message_type)?;
                text(bundle)?;
                if !service.starts_with('/')
                    || !(1..=60_000).contains(timeout_ms)
                    || serde_json::to_vec(value).map_err(|e| e.to_string())?.len() > MAX_PAYLOAD
                {
                    return Err("Invalid service request or timeout".into());
                }
            }
            Self::ServiceInfo { service } => text(service)?,
            Self::Graph { section, node } => {
                if !["Topics", "Services", "Actions", "Nodes", "Parameters"]
                    .contains(&section.as_str())
                {
                    return Err("Unknown section".into());
                }
                text(node)?;
            }
            Self::Details { topic } => text(topic)?,
            Self::Schema {
                message_type,
                bundle,
            } => {
                text(message_type)?;
                text(bundle)?;
            }
            Self::Publish {
                topic,
                message_type,
                bundle,
                value,
                interval_ms,
                ..
            } => {
                text(topic)?;
                text(message_type)?;
                text(bundle)?;
                if !topic.starts_with('/')
                    || *interval_ms > 3_600_000
                    || (*interval_ms > 0 && *interval_ms < 10)
                {
                    return Err("Invalid publisher name or period (10..3600000 ms)".into());
                }
                if serde_json::to_vec(value).map_err(|e| e.to_string())?.len() > MAX_PAYLOAD {
                    return Err("Payload exceeds 64 KiB".into());
                }
            }
            Self::Statistics {
                topic,
                message_type,
                ..
            } => {
                text(topic)?;
                text(message_type)?;
            }
            Self::Stop { target } => {
                if *target == 0 {
                    return Err("Zero target".into());
                }
            }
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_oversize_and_busy_loop_publishers() {
        let mut op = Operation::Publish {
            topic: "/test".into(),
            message_type: "std_msgs/msg/String".into(),
            bundle: String::new(),
            value: Value::Null,
            interval_ms: 1,
            best_effort: false,
            transient_local: false,
        };
        assert!(op.validate().is_err());
        if let Operation::Publish {
            interval_ms, value, ..
        } = &mut op
        {
            *interval_ms = 100;
            *value = Value::String("x".repeat(MAX_PAYLOAD));
        }
        assert!(op.validate().is_err());
    }
    #[test]
    fn frozen_session_rejects_new_operations() {
        let command = crate::session::Command::Tool {
            generation: crate::subscriptions::Generation(1),
            request: crate::subscriptions::RequestId(1),
            subscription: crate::subscriptions::SubscriptionId(1),
            operation: Operation::Graph {
                section: "Topics".into(),
                node: String::new(),
            },
        };
        let wire = command.encode().unwrap();
        assert!(crate::session_v17::Command::decode(&wire).is_err());
        let ready = serde_json::to_vec(&crate::session_v17::Event::ready(
            "jazzy".into(),
            "fixture".into(),
        ))
        .unwrap();
        assert!(crate::session::Event::decode(&ready).is_err());
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterChange {
    pub name: String,
    pub parameter_type: u8,
    pub value: Value,
}
impl ParameterChange {
    pub fn wire(&self) -> Result<Value, String> {
        if self.name.is_empty() || self.name.len() > 256 {
            return Err("Invalid parameter name".into());
        }
        let (field, valid) = match self.parameter_type {
            0 => ("", self.value.is_null()),
            1 => ("bool_value", self.value.is_boolean()),
            2 => ("integer_value", self.value.as_i64().is_some()),
            3 => (
                "double_value",
                self.value.as_f64().is_some_and(|v| v.is_finite()),
            ),
            4 => ("string_value", self.value.is_string()),
            5 => (
                "byte_array_value",
                self.value
                    .as_array()
                    .is_some_and(|a| a.iter().all(|v| v.as_u64().is_some_and(|v| v <= 255))),
            ),
            6 => (
                "bool_array_value",
                self.value
                    .as_array()
                    .is_some_and(|a| a.iter().all(Value::is_boolean)),
            ),
            7 => (
                "integer_array_value",
                self.value
                    .as_array()
                    .is_some_and(|a| a.iter().all(|v| v.as_i64().is_some())),
            ),
            8 => (
                "double_array_value",
                self.value
                    .as_array()
                    .is_some_and(|a| a.iter().all(|v| v.as_f64().is_some_and(|v| v.is_finite()))),
            ),
            9 => (
                "string_array_value",
                self.value
                    .as_array()
                    .is_some_and(|a| a.iter().all(Value::is_string)),
            ),
            _ => return Err("Unknown parameter type".into()),
        };
        if !valid
            || serde_json::to_vec(&self.value)
                .map_err(|e| e.to_string())?
                .len()
                > MAX_PAYLOAD
        {
            return Err(format!("{}: invalid parameter value", self.name));
        }
        let mut v = serde_json::json!({"type":self.parameter_type});
        if !field.is_empty() {
            v[field] = self.value.clone();
        }
        Ok(serde_json::json!({"name":self.name,"value":v}))
    }
}
