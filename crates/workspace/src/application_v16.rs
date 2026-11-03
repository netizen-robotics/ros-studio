//! Strict uncertainty extension; v15 and Tools grammars remain frozen.
use super::*;
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum View {
    Uncertainty(UncertaintyView),
    Previous(v15::View),
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct UncertaintyView {
    id: String,
    topic: String,
    message_type: String,
    qos: Qos,
    kind: Kind,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "renderer", rename_all = "snake_case", deny_unknown_fields)]
enum Kind {
    Uncertainty {
        configuration: studio_protocol::pose_snapshot::transport::Configuration,
        appearance: crate::uncertainty::Appearance,
        fixed_frame: String,
        transforms: SpatialTransforms,
        clock_expectation: ClockExpectation,
    },
}
impl From<&ViewConfig> for View {
    fn from(view: &ViewConfig) -> Self {
        if let ViewKind::Uncertainty {
            configuration,
            appearance,
            fixed_frame,
            transforms,
            clock_expectation,
        } = &view.kind
        {
            Self::Uncertainty(UncertaintyView {
                id: view.id.clone(),
                topic: view.topic.clone(),
                message_type: view.message_type.clone(),
                qos: view.qos.clone(),
                kind: Kind::Uncertainty {
                    configuration: *configuration,
                    appearance: *appearance,
                    fixed_frame: fixed_frame.clone(),
                    transforms: transforms.clone(),
                    clock_expectation: *clock_expectation,
                },
            })
        } else {
            Self::Previous(v15::View::from(view))
        }
    }
}
impl From<View> for ViewConfig {
    fn from(view: View) -> Self {
        match view {
            View::Previous(previous) => previous.into(),
            View::Uncertainty(view) => {
                let Kind::Uncertainty {
                    configuration,
                    appearance,
                    fixed_frame,
                    transforms,
                    clock_expectation,
                } = view.kind;
                Self {
                    id: view.id,
                    topic: view.topic,
                    message_type: view.message_type,
                    qos: view.qos,
                    kind: ViewKind::Uncertainty {
                        configuration,
                        appearance,
                        fixed_frame,
                        transforms,
                        clock_expectation,
                    },
                }
            }
        }
    }
}
