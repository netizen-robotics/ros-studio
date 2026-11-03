//! Strict calibrated Camera extension; all earlier wire grammars stay frozen.
use super::*;
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum View {
    Camera(CameraView),
    Previous(v16::View),
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct CameraView {
    id: String,
    topic: String,
    message_type: String,
    qos: Qos,
    kind: Kind,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "renderer", rename_all = "snake_case", deny_unknown_fields)]
enum Kind {
    Camera {
        info: studio_protocol::camera_snapshot::InfoSource,
        appearance: crate::camera::Appearance,
        fixed_frame: String,
        transforms: SpatialTransforms,
        clock_expectation: ClockExpectation,
    },
}
impl From<&ViewConfig> for View {
    fn from(view: &ViewConfig) -> Self {
        if let ViewKind::Camera {
            info,
            appearance,
            fixed_frame,
            transforms,
            clock_expectation,
        } = &view.kind
        {
            Self::Camera(CameraView {
                id: view.id.clone(),
                topic: view.topic.clone(),
                message_type: view.message_type.clone(),
                qos: view.qos.clone(),
                kind: Kind::Camera {
                    info: info.clone(),
                    appearance: *appearance,
                    fixed_frame: fixed_frame.clone(),
                    transforms: transforms.clone(),
                    clock_expectation: *clock_expectation,
                },
            })
        } else {
            Self::Previous(v16::View::from(view))
        }
    }
}
impl From<View> for ViewConfig {
    fn from(view: View) -> Self {
        match view {
            View::Previous(previous) => previous.into(),
            View::Camera(view) => {
                let Kind::Camera {
                    info,
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
                    kind: ViewKind::Camera {
                        info,
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
