//! Strict application envelopes: Visualizer19 and Tools9, with frozen legacy imports.
use crate::workspace::*;
#[path = "application_v10.rs"]
mod v10;
#[path = "application_v11.rs"]
mod v11;
#[cfg(test)]
#[path = "application_v12_tests.rs"]
mod v12_tests;
#[cfg(test)]
#[path = "application_v13_tests.rs"]
mod v13_tests;
#[path = "application_v14.rs"]
mod v14;
#[cfg(test)]
#[path = "application_v14_tests.rs"]
mod v14_tests;
#[path = "application_v15.rs"]
mod v15;
#[cfg(test)]
#[path = "application_v15_tests.rs"]
mod v15_tests;
#[path = "application_v16.rs"]
mod v16;
#[cfg(test)]
#[path = "application_v16_tests.rs"]
mod v16_tests;
#[path = "application_v17.rs"]
mod v17;
#[cfg(test)]
#[path = "application_v17_tests.rs"]
mod v17_tests;
#[path = "application_v18.rs"]
mod v18;
#[cfg(test)]
#[path = "application_v18_tests.rs"]
mod v18_tests;
use serde::{Deserialize, Serialize};

/// Current Visualizer writer envelope. Legacy migration grammars retain their
/// explicit versions; generic integration consumers use this advertised value.
pub const VISUALIZER_VERSION: u32 = 19;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Application {
    Visualizer,
    Tools,
}
impl Application {
    pub fn name(self) -> &'static str {
        match self {
            Self::Visualizer => "ROS Studio Visualizer",
            Self::Tools => "ROS Studio Tools",
        }
    }
    pub fn accepts(self, kind: &ViewKind) -> bool {
        match kind {
            ViewKind::Image { .. } => true,
            ViewKind::Messages | ViewKind::NumericPlot { .. } => self == Self::Tools,
            _ => self == Self::Visualizer,
        }
    }
}
use crate::presentation::VisualizerPresentation;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireV8 {
    version: u32,
    application: Application,
    name: String,
    panels: Panels,
    layout: Layout,
    views: Vec<ViewConfig>,
    connection_suggestion: Option<ConnectionSuggestion>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireTools9 {
    version: u32,
    application: Application,
    name: String,
    panels: Panels,
    layout: Layout,
    views: Vec<ViewConfig>,
    connection_suggestion: Option<ConnectionSuggestion>,
    tools: crate::tools::Config,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireV9 {
    version: u32,
    application: Application,
    name: String,
    panels: Panels,
    layout: Layout,
    views: Vec<ViewConfig>,
    connection_suggestion: Option<ConnectionSuggestion>,
    presentation: crate::presentation::LegacyVisualizerPresentation,
}
pub struct Import {
    pub workspace: Workspace,
    pub excluded: Vec<String>,
    pub legacy: bool,
}
fn check_owner(owner: Application, expected: Application) -> Result<(), Error> {
    if owner != expected {
        return Err(Error::Invalid(format!(
            "This workspace belongs to {}; open it with that application",
            owner.name()
        )));
    }
    Ok(())
}
pub fn encode(workspace: &Workspace, application: Application) -> Result<Vec<u8>, Error> {
    workspace.validate()?;
    if application == Application::Visualizer && workspace.tools.is_some() {
        return Err(Error::Invalid(
            "Visualizer cannot contain Tools task configuration".into(),
        ));
    }
    if workspace
        .views
        .iter()
        .any(|v| !application.accepts(&v.kind))
    {
        return Err(Error::Invalid(
            "Workspace contains a display belonging to the other application".into(),
        ));
    }
    let bytes = match application {
        Application::Visualizer => {
            let presentation = workspace
                .presentation
                .clone()
                .unwrap_or_else(|| VisualizerPresentation::legacy(workspace));
            presentation.validate()?;
            serde_json::to_vec_pretty(&v18::Wire {
                shell: presentation.shell.clone(),
                version: VISUALIZER_VERSION,
                application,
                name: workspace.name.clone(),
                panels: workspace.panels.clone(),
                layout: workspace.layout.clone(),
                views: workspace.views.iter().map(v17::View::from).collect(),
                connection_suggestion: workspace.connection_suggestion.clone(),
                presentation,
            })
        }
        Application::Tools => {
            if workspace.presentation.is_some() {
                return Err(Error::Invalid(
                    "Tools documents cannot contain Visualizer presentation".into(),
                ));
            }
            serde_json::to_vec_pretty(&WireTools9 {
                tools: workspace.tools.clone().unwrap_or_default(),
                version: crate::tools::VERSION,
                application,
                name: workspace.name.clone(),
                panels: workspace.panels.clone(),
                layout: workspace.layout.clone(),
                views: workspace.views.clone(),
                connection_suggestion: workspace.connection_suggestion.clone(),
            })
        }
    }
    .map_err(|e| Error::Invalid(e.to_string()))?;
    if bytes.len()
        > if application == Application::Tools {
            crate::tools::MAX_FILE_BYTES
        } else {
            MAX_FILE_BYTES
        }
    {
        return Err(Error::Invalid(format!(
            "Workspace exceeds {} bytes",
            if application == Application::Tools {
                crate::tools::MAX_FILE_BYTES
            } else {
                MAX_FILE_BYTES
            }
        )));
    }
    Ok(bytes)
}
pub fn parse(bytes: &[u8], application: Application) -> Result<Import, Error> {
    if bytes.len()
        > if application == Application::Tools {
            crate::tools::MAX_FILE_BYTES
        } else {
            MAX_FILE_BYTES
        }
    {
        return Err(Error::Invalid("Workspace exceeds 65536 bytes".into()));
    }
    // Tools drafts can contain nested ROS messages; frozen Visualizer/legacy depths stay unchanged.
    if application == Application::Visualizer {
        crate::workspace::check_depth(bytes)?;
    }
    #[derive(Deserialize)]
    struct Version {
        version: u32,
        application: Option<Application>,
    }
    let version: Version =
        serde_json::from_slice(bytes).map_err(|e| Error::Invalid(e.to_string()))?;
    if let Some(owner) = version.application {
        check_owner(owner, application)?;
    }
    if !(version.version == 9 && version.application == Some(Application::Tools)) {
        if bytes.len() > MAX_FILE_BYTES {
            return Err(Error::Invalid("Legacy workspace exceeds65536 bytes".into()));
        }
        crate::workspace::check_depth(bytes)?;
    }
    let mut workspace = match version.version {
        9 if version.application == Some(Application::Tools) => {
            let wire: WireTools9 =
                serde_json::from_slice(bytes).map_err(|e| Error::Invalid(e.to_string()))?;
            wire.tools.validate()?;
            Workspace {
                tools: Some(wire.tools),
                presentation: None,
                version: FORMAT_VERSION,
                name: wire.name,
                panels: wire.panels,
                layout: wire.layout,
                views: wire.views,
                connection_suggestion: wire.connection_suggestion,
            }
        }
        8 => {
            let wire: WireV8 =
                serde_json::from_slice(bytes).map_err(|e| Error::Invalid(e.to_string()))?;
            check_owner(wire.application, application)?;
            Workspace {
                tools: None,
                presentation: None,
                version: FORMAT_VERSION,
                name: wire.name,
                panels: wire.panels,
                layout: wire.layout,
                views: wire.views,
                connection_suggestion: wire.connection_suggestion,
            }
        }
        18 | 19 => {
            let wire: v18::Wire =
                serde_json::from_slice(bytes).map_err(|e| Error::Invalid(e.to_string()))?;
            check_owner(wire.application, application)?;
            if application != Application::Visualizer {
                return Err(Error::Invalid(
                    "Version18/19 is a Visualizer document".into(),
                ));
            }
            let mut presentation = wire.presentation;
            presentation.shell = wire.shell;
            Workspace {
                tools: None,
                version: FORMAT_VERSION,
                name: wire.name,
                panels: wire.panels,
                layout: wire.layout,
                views: wire.views.into_iter().map(Into::into).collect(),
                connection_suggestion: wire.connection_suggestion,
                presentation: Some(presentation),
            }
        }
        17 => {
            let wire: v10::Wire<v17::View, VisualizerPresentation> =
                serde_json::from_slice(bytes).map_err(|e| Error::Invalid(e.to_string()))?;
            check_owner(wire.application, application)?;
            if application != Application::Visualizer {
                return Err(Error::Invalid("Version17 is a Visualizer document".into()));
            }
            Workspace {
                tools: None,
                version: FORMAT_VERSION,
                name: wire.name,
                panels: wire.panels,
                layout: wire.layout,
                views: wire.views.into_iter().map(Into::into).collect(),
                connection_suggestion: wire.connection_suggestion,
                presentation: Some(wire.presentation),
            }
        }
        16 => {
            let wire: v10::Wire<v16::View, VisualizerPresentation> =
                serde_json::from_slice(bytes).map_err(|e| Error::Invalid(e.to_string()))?;
            check_owner(wire.application, application)?;
            if application != Application::Visualizer {
                return Err(Error::Invalid("Version16 is a Visualizer document".into()));
            }
            Workspace {
                tools: None,
                version: FORMAT_VERSION,
                name: wire.name,
                panels: wire.panels,
                layout: wire.layout,
                views: wire.views.into_iter().map(Into::into).collect(),
                connection_suggestion: wire.connection_suggestion,
                presentation: Some(wire.presentation),
            }
        }
        15 => {
            let wire: v10::Wire<v15::View, VisualizerPresentation> =
                serde_json::from_slice(bytes).map_err(|e| Error::Invalid(e.to_string()))?;
            check_owner(wire.application, application)?;
            if application != Application::Visualizer {
                return Err(Error::Invalid("Version 15 is a Visualizer document".into()));
            }
            Workspace {
                tools: None,
                version: FORMAT_VERSION,
                name: wire.name,
                panels: wire.panels,
                layout: wire.layout,
                views: wire.views.into_iter().map(Into::into).collect(),
                connection_suggestion: wire.connection_suggestion,
                presentation: Some(wire.presentation),
            }
        }
        14 => {
            let wire: v10::Wire<v14::View, VisualizerPresentation> =
                serde_json::from_slice(bytes).map_err(|e| Error::Invalid(e.to_string()))?;
            check_owner(wire.application, application)?;
            if application != Application::Visualizer {
                return Err(Error::Invalid("Version 14 is a Visualizer document".into()));
            }
            Workspace {
                tools: None,
                version: FORMAT_VERSION,
                name: wire.name,
                panels: wire.panels,
                layout: wire.layout,
                views: wire.views.into_iter().map(Into::into).collect(),
                connection_suggestion: wire.connection_suggestion,
                presentation: Some(wire.presentation),
            }
        }
        13 => {
            let wire: v10::Wire<v11::View, VisualizerPresentation> =
                serde_json::from_slice(bytes).map_err(|e| Error::Invalid(e.to_string()))?;
            check_owner(wire.application, application)?;
            if application != Application::Visualizer {
                return Err(Error::Invalid("Version 13 is a Visualizer document".into()));
            }
            Workspace {
                tools: None,
                version: FORMAT_VERSION,
                name: wire.name,
                panels: wire.panels,
                layout: wire.layout,
                views: wire.views.into_iter().map(Into::into).collect(),
                connection_suggestion: wire.connection_suggestion,
                presentation: Some(wire.presentation),
            }
        }
        12 => {
            let wire: v10::Wire<v11::View, crate::presentation::LegacyV12Presentation> =
                serde_json::from_slice(bytes).map_err(|e| Error::Invalid(e.to_string()))?;
            check_owner(wire.application, application)?;
            if application != Application::Visualizer {
                return Err(Error::Invalid("Version 12 is a Visualizer document".into()));
            }
            Workspace {
                tools: None,
                version: FORMAT_VERSION,
                name: wire.name,
                panels: wire.panels,
                layout: wire.layout,
                views: wire.views.into_iter().map(Into::into).collect(),
                connection_suggestion: wire.connection_suggestion,
                presentation: Some(wire.presentation.into()),
            }
        }
        11 => {
            let wire: v10::Wire<v11::View> =
                serde_json::from_slice(bytes).map_err(|e| Error::Invalid(e.to_string()))?;
            check_owner(wire.application, application)?;
            if application != Application::Visualizer {
                return Err(Error::Invalid("Version 11 is a Visualizer document".into()));
            }
            Workspace {
                tools: None,
                version: FORMAT_VERSION,
                name: wire.name,
                panels: wire.panels,
                layout: wire.layout,
                views: wire.views.into_iter().map(Into::into).collect(),
                connection_suggestion: wire.connection_suggestion,
                presentation: Some(wire.presentation.into()),
            }
        }
        10 => {
            let wire: v10::Wire =
                serde_json::from_slice(bytes).map_err(|e| Error::Invalid(e.to_string()))?;
            check_owner(wire.application, application)?;
            if application != Application::Visualizer {
                return Err(Error::Invalid("Version 10 is a Visualizer document".into()));
            }
            Workspace {
                tools: None,
                version: FORMAT_VERSION,
                name: wire.name,
                panels: wire.panels,
                layout: wire.layout,
                views: wire.views.into_iter().map(Into::into).collect(),
                connection_suggestion: wire.connection_suggestion,
                presentation: Some(wire.presentation.into()),
            }
        }
        9 => {
            let wire: WireV9 =
                serde_json::from_slice(bytes).map_err(|e| Error::Invalid(e.to_string()))?;
            check_owner(wire.application, application)?;
            if application != Application::Visualizer {
                return Err(Error::Invalid("Version 9 is a Visualizer document".into()));
            }
            Workspace {
                tools: None,
                presentation: Some(wire.presentation.into()),
                version: FORMAT_VERSION,
                name: wire.name,
                panels: wire.panels,
                layout: wire.layout,
                views: wire.views,
                connection_suggestion: wire.connection_suggestion,
            }
        }
        _ => Workspace::parse(bytes)?,
    };
    // Only workspace19 grants expanded capacity. Validate before migrations or
    // filtering so older documents cannot smuggle extra views/appearance rows.
    if version.version != VISUALIZER_VERSION
        && (workspace.views.len() > MAX_VIEWS
            || workspace.presentation.as_ref().is_some_and(|p| {
                p.sensors.len() > MAX_VIEWS
                    || p.robots.len() > MAX_VIEWS
                    || p.shell.expanded_displays.len() > MAX_VIEWS
            }))
    {
        return Err(Error::Invalid(
            "This older workspace format supports at most eight displays".into(),
        ));
    }
    workspace.validate()?;
    // App-specific envelopes must never filter out a forbidden display silently.
    if version.version >= 8
        && workspace
            .views
            .iter()
            .any(|v| !application.accepts(&v.kind))
    {
        return Err(Error::Invalid(
            "Application workspace contains unsupported displays".into(),
        ));
    }
    let excluded = workspace
        .views
        .iter()
        .filter(|v| !application.accepts(&v.kind))
        .map(|v| format!("{} ({})", v.id, v.topic))
        .collect();
    workspace.views.retain(|v| application.accepts(&v.kind));
    workspace
        .layout
        .order
        .retain(|id| workspace.views.iter().any(|v| &v.id == id));
    if application == Application::Visualizer && workspace.presentation.is_none() {
        workspace.presentation = Some(VisualizerPresentation::legacy(&workspace));
    }
    let mut legacy_map = false;
    if application == Application::Visualizer {
        for view in &mut workspace.views {
            legacy_map |= v14::migrate_map(view);
        }
    }
    workspace.validate()?;
    let legacy = legacy_map
        || version.version
            != if application == Application::Visualizer {
                VISUALIZER_VERSION
            } else {
                crate::tools::VERSION
            };
    Ok(Import {
        workspace,
        excluded,
        legacy,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mixed_legacy_import_preserves_order_and_lists_exclusions() {
        let old = Preset::Visualize.workspace();
        let bytes = old.encode().unwrap();
        let imported = parse(&bytes, Application::Tools).unwrap();
        assert!(imported.legacy);
        assert!(!imported.excluded.is_empty());
        assert!(imported
            .workspace
            .views
            .iter()
            .all(|v| matches!(v.kind, ViewKind::Image { .. })));
        assert_eq!(old.encode().unwrap(), bytes);
    }
    #[test]
    fn scoped_roundtrip_and_wrong_application_rejection() {
        for app in [Application::Tools, Application::Visualizer] {
            let imported = parse(&Preset::Visualize.workspace().encode().unwrap(), app).unwrap();
            let bytes = encode(&imported.workspace, app).unwrap();
            let parsed = parse(&bytes, app).unwrap();
            assert!(!parsed.legacy);
            let mut expected = imported.workspace.clone();
            if app == Application::Tools {
                expected.tools = Some(crate::tools::Config::default());
            }
            assert_eq!(parsed.workspace, expected);
            let other = if app == Application::Tools {
                Application::Visualizer
            } else {
                Application::Tools
            };
            assert!(parse(&bytes, other).is_err());
            let version = if app == Application::Visualizer {
                VISUALIZER_VERSION
            } else {
                crate::tools::VERSION
            };
            let duplicate = String::from_utf8(bytes).unwrap().replacen(
                &format!("\"version\": {version}"),
                &format!("\"version\": {version}, \"version\": {version}"),
                1,
            );
            assert!(parse(duplicate.as_bytes(), app).is_err());
        }
    }
}

#[cfg(test)]
#[path = "application_v19_tests.rs"]
mod v19_tests;
