use super::*;

#[derive(Deserialize)]
struct Version {
    version: u32,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyWorkspace {
    version: u32,
    name: String,
    panels: Panels,
    views: Vec<LegacyView>,
    connection_suggestion: Option<LegacyConnection>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyConnection {
    domain: u16,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyView {
    id: String,
    kind: LegacyKind,
    topic: String,
    message_type: String,
    qos: Qos,
}
#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
enum LegacyKind {
    Messages,
}

// Keep the original v2 shape strict: adding defaults to v3 would otherwise
// silently accept new fields in old files or invent absent TF configuration.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V2Workspace {
    version: u32,
    name: String,
    panels: Panels,
    layout: LegacyLayout,
    views: Vec<V2View>,
    connection_suggestion: Option<ConnectionSuggestion>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V2View {
    id: String,
    kind: V2Kind,
    topic: String,
    message_type: String,
    qos: Qos,
}
#[derive(Deserialize)]
#[serde(tag = "renderer", rename_all = "snake_case", deny_unknown_fields)]
enum V2Kind {
    Messages {},
    NumericPlot {
        field_path: Vec<PathSegment>,
    },
    Image {
        display_mode: ImageMode,
    },
    Spatial {
        fixed_frame: String,
        layers: Vec<LegacyLayer>,
    },
}
impl From<V2Kind> for ViewKind {
    fn from(kind: V2Kind) -> Self {
        match kind {
            V2Kind::Messages {} => Self::Messages,
            V2Kind::NumericPlot { field_path } => Self::NumericPlot { field_path },
            V2Kind::Image { display_mode } => Self::Image { display_mode },
            V2Kind::Spatial {
                fixed_frame,
                layers,
            } => Self::Spatial {
                fixed_frame,
                layers: layers.into_iter().map(Into::into).collect(),
                transforms: None,
                clock_expectation: ClockExpectation::Unknown,
            },
        }
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyLayout {
    order: Vec<String>,
    columns: u8,
    panel_weights: Vec<u16>,
    collapsed: Vec<String>,
}
impl LegacyLayout {
    fn convert(self, ids: &[String]) -> Result<Layout, Error> {
        let known: HashSet<_> = ids.iter().collect();
        let ordered: HashSet<_> = self.order.iter().collect();
        let collapsed: HashSet<_> = self.collapsed.iter().collect();
        if ids.len() > MAX_VIEWS
            || self.order.len() != ids.len()
            || ordered.len() != ids.len()
            || known != ordered
            || !(1..=4).contains(&self.columns)
            || self.panel_weights.len() != ids.len()
            || self.panel_weights.iter().any(|w| !(10..=1000).contains(w))
            || self.collapsed.len() != collapsed.len()
            || !collapsed.is_subset(&known)
        {
            return Err(invalid("invalid legacy panel layout"));
        }
        Ok(Layout {
            order: self.order,
            scene_percent: 65,
        })
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyLayer {
    id: String,
    kind: LayerKind,
    topic: String,
    message_type: String,
    qos: Qos,
    visible: bool,
}
impl From<LegacyLayer> for SpatialLayerConfig {
    fn from(layer: LegacyLayer) -> Self {
        let _legacy_visibility = layer.visible;
        Self {
            id: layer.id,
            kind: layer.kind,
            topic: layer.topic,
            message_type: layer.message_type,
            qos: layer.qos,
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V3Workspace {
    version: u32,
    name: String,
    panels: Panels,
    layout: LegacyLayout,
    views: Vec<V3View>,
    connection_suggestion: Option<ConnectionSuggestion>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V3View {
    id: String,
    kind: V3Kind,
    topic: String,
    message_type: String,
    qos: Qos,
}
#[derive(Deserialize)]
#[serde(tag = "renderer", rename_all = "snake_case", deny_unknown_fields)]
enum V3Kind {
    Messages {},
    NumericPlot {
        field_path: Vec<PathSegment>,
    },
    Image {
        display_mode: ImageMode,
    },
    Spatial {
        fixed_frame: String,
        layers: Vec<LegacyLayer>,
        transforms: Option<SpatialTransforms>,
        clock_expectation: ClockExpectation,
    },
}
impl From<V3Kind> for ViewKind {
    fn from(kind: V3Kind) -> Self {
        match kind {
            V3Kind::Messages {} => Self::Messages,
            V3Kind::NumericPlot { field_path } => Self::NumericPlot { field_path },
            V3Kind::Image { display_mode } => Self::Image { display_mode },
            V3Kind::Spatial {
                fixed_frame,
                layers,
                transforms,
                clock_expectation,
            } => Self::Spatial {
                fixed_frame,
                layers: layers.into_iter().map(Into::into).collect(),
                transforms,
                clock_expectation,
            },
        }
    }
}

// Freeze v4's pre-Robot renderer grammar. Reusing the current ViewKind would
// silently accept new renderer fields inside an older declared version.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V4Workspace {
    version: u32,
    name: String,
    panels: Panels,
    layout: Layout,
    views: Vec<V4View>,
    connection_suggestion: Option<ConnectionSuggestion>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V4View {
    id: String,
    kind: V4Kind,
    topic: String,
    message_type: String,
    qos: Qos,
}
#[derive(Deserialize)]
#[serde(tag = "renderer", rename_all = "snake_case", deny_unknown_fields)]
enum V4Kind {
    Messages {},
    NumericPlot {
        field_path: Vec<PathSegment>,
    },
    Image {
        display_mode: ImageMode,
    },
    Spatial {
        fixed_frame: String,
        layers: Vec<SpatialLayerConfig>,
        transforms: Option<SpatialTransforms>,
        clock_expectation: ClockExpectation,
    },
}
impl From<V4Kind> for ViewKind {
    fn from(kind: V4Kind) -> Self {
        match kind {
            V4Kind::Messages {} => Self::Messages,
            V4Kind::NumericPlot { field_path } => Self::NumericPlot { field_path },
            V4Kind::Image { display_mode } => Self::Image { display_mode },
            V4Kind::Spatial {
                fixed_frame,
                layers,
                transforms,
                clock_expectation,
            } => Self::Spatial {
                fixed_frame,
                layers,
                transforms,
                clock_expectation,
            },
        }
    }
}
// Freeze v5 before PointCloud2 and the eight-display v6 capacity.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V5Workspace {
    version: u32,
    name: String,
    panels: Panels,
    layout: Layout,
    views: Vec<V5View>,
    connection_suggestion: Option<ConnectionSuggestion>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V5View {
    id: String,
    kind: V5Kind,
    topic: String,
    message_type: String,
    qos: Qos,
}
#[derive(Deserialize)]
#[serde(tag = "renderer", rename_all = "snake_case", deny_unknown_fields)]
enum V5Kind {
    Messages {},
    NumericPlot {
        field_path: Vec<PathSegment>,
    },
    Image {
        display_mode: ImageMode,
    },
    Spatial {
        fixed_frame: String,
        layers: Vec<SpatialLayerConfig>,
        transforms: Option<SpatialTransforms>,
        clock_expectation: ClockExpectation,
    },
    Robot {
        assets: super::RobotAssets,
        fixed_frame: String,
        transforms: SpatialTransforms,
        clock_expectation: ClockExpectation,
    },
}
impl From<V5Kind> for ViewKind {
    fn from(value: V5Kind) -> Self {
        match value {
            V5Kind::Messages {} => Self::Messages,
            V5Kind::NumericPlot { field_path } => Self::NumericPlot { field_path },
            V5Kind::Image { display_mode } => Self::Image { display_mode },
            V5Kind::Robot {
                assets,
                fixed_frame,
                transforms,
                clock_expectation,
            } => Self::Robot {
                assets,
                fixed_frame,
                transforms,
                clock_expectation,
            },
            V5Kind::Spatial {
                fixed_frame,
                layers,
                transforms,
                clock_expectation,
            } => Self::Spatial {
                fixed_frame,
                layers,
                transforms,
                clock_expectation,
            },
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V6Workspace {
    version: u32,
    name: String,
    panels: Panels,
    layout: Layout,
    views: Vec<V6View>,
    connection_suggestion: Option<ConnectionSuggestion>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct V6View {
    id: String,
    kind: V6Kind,
    topic: String,
    message_type: String,
    qos: Qos,
}
#[derive(Deserialize)]
#[serde(tag = "renderer", rename_all = "snake_case", deny_unknown_fields)]
enum V6Kind {
    Messages {},
    NumericPlot {
        field_path: Vec<PathSegment>,
    },
    Image {
        display_mode: ImageMode,
    },
    Spatial {
        fixed_frame: String,
        layers: Vec<SpatialLayerConfig>,
        transforms: Option<SpatialTransforms>,
        clock_expectation: ClockExpectation,
    },
    PointCloud {
        fixed_frame: String,
        transforms: SpatialTransforms,
        clock_expectation: ClockExpectation,
    },
    Robot {
        assets: super::RobotAssets,
        fixed_frame: String,
        transforms: SpatialTransforms,
        clock_expectation: ClockExpectation,
    },
}
impl From<V6Kind> for ViewKind {
    fn from(value: V6Kind) -> Self {
        match value {
            V6Kind::PointCloud {
                fixed_frame,
                transforms,
                clock_expectation,
            } => Self::PointCloud {
                fixed_frame,
                transforms,
                clock_expectation,
            },
            V6Kind::Messages {} => Self::Messages,
            V6Kind::NumericPlot { field_path } => Self::NumericPlot { field_path },
            V6Kind::Image { display_mode } => Self::Image { display_mode },
            V6Kind::Robot {
                assets,
                fixed_frame,
                transforms,
                clock_expectation,
            } => Self::Robot {
                assets,
                fixed_frame,
                transforms,
                clock_expectation,
            },
            V6Kind::Spatial {
                fixed_frame,
                layers,
                transforms,
                clock_expectation,
            } => Self::Spatial {
                fixed_frame,
                layers,
                transforms,
                clock_expectation,
            },
        }
    }
}
pub(super) fn parse(bytes: &[u8]) -> Result<Workspace, Error> {
    let version: Version = serde_json::from_slice(bytes).map_err(|e| invalid(e.to_string()))?;
    let workspace = match version.version {
        FORMAT_VERSION => serde_json::from_slice(bytes).map_err(|e| invalid(e.to_string())),
        6 => {
            let legacy: V6Workspace =
                serde_json::from_slice(bytes).map_err(|e| invalid(e.to_string()))?;
            if legacy.version != 6 {
                return Err(invalid("expected v6 workspace"));
            }
            Ok(Workspace {
                tools: None,
                presentation: None,
                version: FORMAT_VERSION,
                name: legacy.name,
                panels: legacy.panels,
                layout: legacy.layout,
                connection_suggestion: legacy.connection_suggestion,
                views: legacy
                    .views
                    .into_iter()
                    .map(|v| ViewConfig {
                        id: v.id,
                        kind: v.kind.into(),
                        topic: v.topic,
                        message_type: v.message_type,
                        qos: v.qos,
                    })
                    .collect(),
            })
        }
        5 => {
            let legacy: V5Workspace =
                serde_json::from_slice(bytes).map_err(|e| invalid(e.to_string()))?;
            if legacy.version != 5 {
                return Err(invalid("expected v5 workspace"));
            }
            Ok(Workspace {
                tools: None,
                presentation: None,
                version: FORMAT_VERSION,
                name: legacy.name,
                panels: legacy.panels,
                layout: legacy.layout,
                connection_suggestion: legacy.connection_suggestion,
                views: legacy
                    .views
                    .into_iter()
                    .map(|v| ViewConfig {
                        id: v.id,
                        kind: v.kind.into(),
                        topic: v.topic,
                        message_type: v.message_type,
                        qos: v.qos,
                    })
                    .collect(),
            })
        }
        4 => {
            let legacy: V4Workspace =
                serde_json::from_slice(bytes).map_err(|e| invalid(e.to_string()))?;
            if legacy.version != 4 {
                return Err(invalid("expected v4 workspace"));
            }
            Ok(Workspace {
                tools: None,
                presentation: None,
                version: FORMAT_VERSION,
                name: legacy.name,
                panels: legacy.panels,
                layout: legacy.layout,
                connection_suggestion: legacy.connection_suggestion,
                views: legacy
                    .views
                    .into_iter()
                    .map(|v| ViewConfig {
                        id: v.id,
                        kind: v.kind.into(),
                        topic: v.topic,
                        message_type: v.message_type,
                        qos: v.qos,
                    })
                    .collect(),
            })
        }
        3 => {
            let legacy: V3Workspace =
                serde_json::from_slice(bytes).map_err(|e| invalid(e.to_string()))?;
            if legacy.version != 3 {
                return Err(invalid("expected v3 workspace"));
            }
            let layout = legacy.layout.convert(
                &legacy
                    .views
                    .iter()
                    .map(|v| v.id.clone())
                    .collect::<Vec<_>>(),
            )?;
            Ok(Workspace {
                tools: None,
                presentation: None,
                version: FORMAT_VERSION,
                name: legacy.name,
                panels: legacy.panels,
                layout,
                views: legacy
                    .views
                    .into_iter()
                    .map(|v| ViewConfig {
                        id: v.id,
                        kind: v.kind.into(),
                        topic: v.topic,
                        message_type: v.message_type,
                        qos: v.qos,
                    })
                    .collect(),
                connection_suggestion: legacy.connection_suggestion,
            })
        }
        2 => {
            let legacy: V2Workspace =
                serde_json::from_slice(bytes).map_err(|e| invalid(e.to_string()))?;
            if legacy.version != 2 {
                return Err(invalid("expected v2 workspace"));
            }
            Ok(Workspace {
                tools: None,
                presentation: None,
                version: FORMAT_VERSION,
                name: legacy.name,
                panels: legacy.panels,
                layout: legacy.layout.convert(
                    &legacy
                        .views
                        .iter()
                        .map(|v| v.id.clone())
                        .collect::<Vec<_>>(),
                )?,
                views: legacy
                    .views
                    .into_iter()
                    .map(|view| ViewConfig {
                        id: view.id,
                        kind: view.kind.into(),
                        topic: view.topic,
                        message_type: view.message_type,
                        qos: view.qos,
                    })
                    .collect(),
                connection_suggestion: legacy.connection_suggestion,
            })
        }
        1 => {
            // Parse the original typed DTO again; no Value map may erase duplicate members.
            let legacy: LegacyWorkspace =
                serde_json::from_slice(bytes).map_err(|e| invalid(e.to_string()))?;
            if legacy.version != 1 || legacy.views.len() > 1 {
                return Err(invalid("v1 supports at most one Messages view"));
            }
            let views = legacy
                .views
                .into_iter()
                .map(|view| {
                    let LegacyKind::Messages = view.kind;
                    ViewConfig {
                        id: view.id,
                        kind: ViewKind::Messages,
                        topic: view.topic,
                        message_type: view.message_type,
                        qos: view.qos,
                    }
                })
                .collect::<Vec<_>>();
            Ok(Workspace {
                tools: None,
                presentation: None,
                version: FORMAT_VERSION,
                name: legacy.name,
                panels: legacy.panels,
                layout: Layout::for_views(&views),
                views,
                connection_suggestion: legacy.connection_suggestion.map(|c| ConnectionSuggestion {
                    domain: c.domain,
                    middleware: None,
                }),
            })
        }
        version => Err(invalid(format!("unsupported format version {version}"))),
    }?;
    if version.version <= 5 && workspace.views.len() > 4 {
        return Err(invalid("legacy workspace supports at most four displays"));
    }
    if version.version == 6 && workspace.views.len() > 8 {
        return Err(invalid("v6 supports at most eight displays"));
    }
    Ok(workspace)
}
