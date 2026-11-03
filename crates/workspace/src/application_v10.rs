//! V10 view adapter extends the frozen v7/v8/v9 grammar without widening it.
use super::*;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Wire<V = View, P = crate::presentation::LegacyVisualizerPresentation> {
    pub version: u32,
    pub application: Application,
    pub name: String,
    pub panels: Panels,
    pub layout: Layout,
    pub views: Vec<V>,
    pub connection_suggestion: Option<ConnectionSuggestion>,
    pub presentation: P,
}
#[derive(Serialize, Deserialize)]
#[serde(untagged)]
pub(super) enum View {
    Tf(TfView),
    Legacy(ViewConfig),
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct TfView {
    id: String,
    kind: TfKind,
    topic: String,
    message_type: String,
    qos: Qos,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "renderer", rename_all = "snake_case", deny_unknown_fields)]
enum TfKind {
    TfGraph {
        fixed_frame: String,
        transforms: SpatialTransforms,
        clock_expectation: ClockExpectation,
    },
}
impl From<&ViewConfig> for View {
    fn from(view: &ViewConfig) -> Self {
        if let ViewKind::TfGraph {
            fixed_frame,
            transforms,
            clock_expectation,
        } = &view.kind
        {
            Self::Tf(TfView {
                id: view.id.clone(),
                kind: TfKind::TfGraph {
                    fixed_frame: fixed_frame.clone(),
                    transforms: transforms.clone(),
                    clock_expectation: *clock_expectation,
                },
                topic: view.topic.clone(),
                message_type: view.message_type.clone(),
                qos: view.qos.clone(),
            })
        } else {
            Self::Legacy(view.clone())
        }
    }
}
impl From<View> for ViewConfig {
    fn from(view: View) -> Self {
        match view {
            View::Legacy(view) => view,
            View::Tf(view) => {
                let TfKind::TfGraph {
                    fixed_frame,
                    transforms,
                    clock_expectation,
                } = view.kind;
                Self {
                    id: view.id,
                    kind: ViewKind::TfGraph {
                        fixed_frame,
                        transforms,
                        clock_expectation,
                    },
                    topic: view.topic,
                    message_type: view.message_type,
                    qos: view.qos,
                }
            }
        }
    }
}
