//! Workspace18 shell extension; earlier root/presentation grammars stay frozen.
use super::*;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Wire {
    pub version: u32,
    pub application: Application,
    pub name: String,
    pub panels: Panels,
    pub layout: Layout,
    pub views: Vec<v17::View>,
    pub connection_suggestion: Option<ConnectionSuggestion>,
    pub presentation: VisualizerPresentation,
    pub shell: crate::visualizer_shell::Layout,
}
