use serde::{Deserialize, Serialize};
pub mod binary;
pub mod camera_info;
pub mod camera_snapshot;
pub mod camera_snapshot_v1;
pub mod image;
pub mod image_v1;
pub mod latest_pose;
pub mod map_grid;
pub mod map_snapshot;
pub mod marker;
pub mod marker_snapshot;
pub mod pose_snapshot;
pub mod robot;
pub mod robot_description;
mod serde_bounded;
pub mod session;
/// Frozen session16 grammar retained for compatibility checks.
pub mod session_v16;
pub mod session_v17;
pub mod session_v18;
pub mod session_v19;
pub mod session_v20;
pub mod spatial;
pub mod spatial_input;
/// Bounded typed payload shared by the paired observation runtimes.
pub mod structured;
pub mod subscriptions;
pub mod tf_graph;
pub mod tools;
pub mod uncertainty;
pub const VERSION: u32 = 2;
pub const MAX_LINE: usize = 1024 * 1024;
/// Bounded diagnostic preview, including for very large ROS sequences.
pub fn preview(value: &impl std::fmt::Debug) -> String {
    struct Limited(String);
    impl std::fmt::Write for Limited {
        fn write_str(&mut self, s: &str) -> std::fmt::Result {
            let remaining = 512usize.saturating_sub(self.0.len());
            let mut end = remaining.min(s.len());
            while !s.is_char_boundary(end) {
                end -= 1;
            }
            self.0.push_str(&s[..end]);
            if end < s.len() {
                Err(std::fmt::Error)
            } else {
                Ok(())
            }
        }
    }
    let mut output = Limited(String::new());
    if std::fmt::write(&mut output, format_args!("{value:?}")).is_err() {
        output.0.push_str(" … [preview truncated]");
    }
    output.0
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    Hello {
        version: u32,
        distro: String,
        rmw: String,
    },
    Graph {
        topics: Vec<(String, Vec<String>)>,
    },
    Sample {
        topic: String,
        fields: Vec<(String, String)>,
    },
    /// Typed values from the observation backend. Diagnostic Sample is retained
    /// for explicit preview fixtures; its strings are never renderer input.
    TypedSample {
        sample: Box<structured::Sample>,
        dropped_samples: u64,
    },
    Error {
        message: String,
    },
}
impl Event {
    pub fn validate_payload(&self) -> Result<(), String> {
        if let Self::TypedSample { sample, .. } = self {
            sample.encode()?;
        }
        Ok(())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn bounds_large_and_unicode_previews() {
        for value in ["é".repeat(10000), "x".repeat(10000)] {
            let result = preview(&value);
            assert!(result.len() < 550);
            assert!(result.ends_with("[preview truncated]"));
        }
    }
    #[test]
    fn rejects_unknown_events() {
        assert!(serde_json::from_str::<Event>(r#"{"event":"execute"}"#).is_err());
    }
    #[test]
    fn sample_roundtrip_preserves_newlines() {
        let e = Event::Sample {
            topic: "/test".into(),
            fields: vec![("data".into(), "one\ntwo".into())],
        };
        let line = serde_json::to_string(&e).unwrap();
        assert!(!line.contains('\n'));
        assert!(matches!(
            serde_json::from_str::<Event>(&line).unwrap(),
            Event::Sample { .. }
        ));
    }
}
pub mod cloud;
pub mod mapping;
