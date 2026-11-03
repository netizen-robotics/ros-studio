//! Strict Marker extension; frozen v14 and Tools grammars remain unchanged.
use super::*;
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum View {
    Markers(MarkerView),
    Previous(v14::View),
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct MarkerView {
    id: String,
    topic: String,
    message_type: String,
    qos: Qos,
    kind: MarkerKind,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "renderer", rename_all = "snake_case", deny_unknown_fields)]
enum MarkerKind {
    Markers {
        fixed_frame: String,
        transforms: SpatialTransforms,
        clock_expectation: ClockExpectation,
    },
}
impl From<&ViewConfig> for View {
    fn from(view: &ViewConfig) -> Self {
        if let ViewKind::Markers {
            fixed_frame,
            transforms,
            clock_expectation,
        } = &view.kind
        {
            Self::Markers(MarkerView {
                id: view.id.clone(),
                topic: view.topic.clone(),
                message_type: view.message_type.clone(),
                qos: view.qos.clone(),
                kind: MarkerKind::Markers {
                    fixed_frame: fixed_frame.clone(),
                    transforms: transforms.clone(),
                    clock_expectation: *clock_expectation,
                },
            })
        } else {
            Self::Previous(v14::View::from(view))
        }
    }
}
impl From<View> for ViewConfig {
    fn from(view: View) -> Self {
        match view {
            View::Previous(old) => old.into(),
            View::Markers(view) => {
                let MarkerKind::Markers {
                    fixed_frame,
                    transforms,
                    clock_expectation,
                } = view.kind;
                Self {
                    id: view.id,
                    topic: view.topic,
                    message_type: view.message_type,
                    qos: view.qos,
                    kind: ViewKind::Markers {
                        fixed_frame,
                        transforms,
                        clock_expectation,
                    },
                }
            }
        }
    }
}
