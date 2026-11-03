//! Tools9 configuration only. Native handles, permissions and samples are absent.
use crate::workspace::Error;
use serde::{Deserialize, Serialize};
use serde_json::Value;
pub const VERSION: u32 = 9;
pub const MAX_FILE_BYTES: usize = 4 * 1024 * 1024;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub section: String,
    pub open: bool,
    pub selected: usize,
    pub tasks: Vec<Task>,
}
impl Default for Config {
    fn default() -> Self {
        Self {
            section: "Topics".into(),
            open: false,
            selected: 0,
            tasks: Vec::new(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Task {
    pub name: String,
    pub message_type: String,
    pub bundle: String,
    pub draft: Value,
    pub yaml: String,
    pub yaml_mode: bool,
    pub period_ms: u64,
    pub timeout_ms: u64,
    pub best_effort: bool,
    pub transient_local: bool,
    pub parameters: Parameters,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Parameters {
    pub names: String,
    pub atomic: bool,
    pub yaml: String,
    pub drafts: Vec<ParameterDraft>,
}
impl Default for Parameters {
    fn default() -> Self {
        Self {
            names: String::new(),
            atomic: true,
            yaml: String::new(),
            drafts: Vec::new(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterDraft {
    pub name: String,
    pub kind: u8,
    pub text: String,
}
impl Config {
    pub fn validate(&self) -> Result<(), Error> {
        let bad = |text: &str| Error::Invalid(text.into());
        if !["Topics", "Services", "Actions", "Nodes", "Parameters"]
            .contains(&self.section.as_str())
            || self.tasks.len() > 16
            || (!self.tasks.is_empty() && self.selected >= self.tasks.len())
        {
            return Err(bad("Invalid Tools task layout"));
        }
        fn bounded(v: &Value, depth: usize) -> bool {
            depth <= 16
                && match v {
                    Value::Array(a) => a.len() <= 65536 && a.iter().all(|v| bounded(v, depth + 1)),
                    Value::Object(m) => m.len() <= 256 && m.values().all(|v| bounded(v, depth + 1)),
                    _ => true,
                }
        }
        for t in &self.tasks {
            if !bounded(&t.draft, 0) {
                return Err(bad("Tools draft exceeds nesting or item bounds"));
            }
            if t.name.len() > 256
                || t.message_type.len() > 256
                || t.bundle.len() > 4096
                || t.yaml.len() > 65536
                || serde_json::to_vec(&t.draft)
                    .map_err(|e| bad(&e.to_string()))?
                    .len()
                    > 65536
                || !(10..=3_600_000).contains(&t.period_ms)
                || !(1..=60_000).contains(&t.timeout_ms)
            {
                return Err(bad("Tools task exceeds its configuration budget"));
            }
            if !t.name.starts_with('/') || t.name.contains('\0') || t.bundle.contains('\0') {
                return Err(bad("Invalid Tools endpoint or bundle path"));
            }
            if !["node", "parameters"].contains(&t.message_type.as_str()) {
                let parts: Vec<_> = t.message_type.split('/').collect();
                if parts.len() != 3
                    || !["msg", "srv", "action"].contains(&parts[1])
                    || parts.iter().any(|p| {
                        p.is_empty() || !p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                    })
                {
                    return Err(bad("Invalid Tools interface type"));
                }
            }
            let p = &t.parameters;
            if p.names.len() > 4096
                || p.yaml.len() > 65536
                || p.drafts.len() > 256
                || p.drafts
                    .iter()
                    .any(|d| d.name.len() > 256 || d.text.len() > 65536 || d.kind > 9)
            {
                return Err(bad("Parameter drafts exceed budget"));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        application::{self, Application},
        workspace::Preset,
    };
    fn config() -> Config {
        Config {
            tasks: vec![Task {
                name: "/text".into(),
                message_type: "std_msgs/msg/String".into(),
                bundle: String::new(),
                draft: serde_json::json!({"data":"draft"}),
                yaml: "data: draft\n".into(),
                yaml_mode: true,
                period_ms: 1000,
                timeout_ms: 10000,
                best_effort: false,
                transient_local: false,
                parameters: Parameters::default(),
            }],
            ..Default::default()
        }
    }
    #[test]
    fn tools9_roundtrips_intent_and_tools8_import_is_legacy() {
        let mut w = Preset::Inspect.workspace();
        w.tools = Some(config());
        let bytes = application::encode(&w, Application::Tools).unwrap();
        let parsed = application::parse(&bytes, Application::Tools).unwrap();
        assert!(!parsed.legacy);
        assert_eq!(parsed.workspace.tools, w.tools);
        assert!(application::parse(&bytes, Application::Visualizer).is_err());
        assert!(w.encode().is_err());
        let mut old: Value = serde_json::from_slice(&bytes).unwrap();
        old["version"] = 8.into();
        old.as_object_mut().unwrap().remove("tools");
        let old = serde_json::to_vec(&old).unwrap();
        let unchanged = old.clone();
        let import = application::parse(&old, Application::Tools).unwrap();
        assert!(import.legacy);
        assert!(import.workspace.tools.is_none());
        assert_eq!(unchanged, old);
    }
    #[test]
    fn strict_tools9_rejects_saved_permissions_and_accepts_a_bounded_large_draft() {
        let mut w = Preset::Inspect.workspace();
        w.tools = Some(config());
        let mut wire: Value =
            serde_json::from_slice(&application::encode(&w, Application::Tools).unwrap()).unwrap();
        wire["tools"]["interaction"] = true.into();
        assert!(
            application::parse(&serde_json::to_vec(&wire).unwrap(), Application::Tools).is_err()
        );
        let c = w.tools.as_mut().unwrap();
        c.tasks[0].draft = serde_json::json!({"data":"x".repeat(50000)});
        c.tasks[0].yaml = format!("data: {}\n", "x".repeat(50000));
        let bytes = application::encode(&w, Application::Tools).unwrap();
        assert!(bytes.len() > crate::workspace::MAX_FILE_BYTES);
        assert!(application::parse(&bytes, Application::Tools).is_ok());
    }
}
