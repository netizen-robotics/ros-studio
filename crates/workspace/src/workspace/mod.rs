//! Configuration-only workspace files. This module has no session/backend dependency.
//! Integration must stop and await active views before applying an `InactiveRestore`.
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fmt;

mod io_worker;
mod layout;
mod migration;
mod presets;
mod renderers;
mod restore;
mod robot;
pub use restore::{
    ApplyApproval, HandleIdentity, PreviewId, RestoreCoordinator, RestoreError, RestoreState,
    StopFailure, StopIntent, SupportedRenderers,
};
pub use robot::{LinkAppearance, PackageRoot, RobotAppearance, RobotAssets, RobotTopicSource};
mod storage;
pub use io_worker::{
    IoCompletion, IoKind, IoOperation, IoOutput, IoWorker, OperationId, SubmitError,
};
pub use layout::Layout;
pub use presets::Preset;
pub use renderers::{
    ClockExpectation, ImageMode, LayerKind, PathSegment, SpatialLayerConfig, SpatialTransforms,
    TransformInput, ViewKind,
};
pub use storage::{load, save_atomic};

pub const FORMAT_VERSION: u32 = 7;
pub const MAX_FILE_BYTES: usize = 64 * 1024;
pub const MAX_JSON_DEPTH: usize = 12;
pub const MAX_VIEWS: usize = 8;
pub const MAX_VISUALIZER_VIEWS: usize = 32;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Workspace {
    #[serde(skip)]
    pub tools: Option<crate::tools::Config>,
    /// Application-envelope metadata; never accepted or emitted by frozen v7.
    #[serde(skip)]
    pub presentation: Option<crate::presentation::VisualizerPresentation>,
    pub version: u32,
    pub name: String,
    pub panels: Panels,
    pub layout: Layout,
    pub views: Vec<ViewConfig>,
    pub connection_suggestion: Option<ConnectionSuggestion>,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Panels {
    pub data_open: bool,
    pub inspector_open: bool,
    pub diagnostics_open: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionSuggestion {
    pub domain: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub middleware: Option<Middleware>,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Middleware {
    FastDds,
    CycloneDds,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ViewConfig {
    /// Stable configuration identity; never a wire subscription/request/session ID.
    pub id: String,
    pub kind: ViewKind,
    pub topic: String,
    pub message_type: String,
    pub qos: Qos,
}
impl ViewConfig {
    /// User-facing description source, distinct from Robot's live TF input.
    pub fn source_label(&self) -> &str {
        match &self.kind {
            ViewKind::RobotTopic { description, .. } => &description.topic,
            ViewKind::Robot { assets, .. } => &assets.urdf_path,
            _ => &self.topic,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Qos {
    pub reliability: Reliability,
    pub durability: Durability,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reliability {
    Reliable,
    BestEffort,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Durability {
    Volatile,
    TransientLocal,
}

#[derive(Debug)]
pub enum Error {
    Invalid(String),
    Io(std::io::Error),
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Invalid(message) => write!(f, "Invalid workspace: {message}"),
            Self::Io(error) => write!(f, "Workspace file error: {error}"),
        }
    }
}
impl std::error::Error for Error {}
impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}
fn invalid(message: impl Into<String>) -> Error {
    Error::Invalid(message.into())
}

impl Workspace {
    pub fn validate(&self) -> Result<(), Error> {
        if let Some(tools) = &self.tools {
            tools.validate()?;
        }
        if let Some(presentation) = &self.presentation {
            presentation.validate()?;
            presentation.shell.validate(&self.views)?;
            for sensor in &presentation.sensors {
                if !self.views.iter().any(|v| {
                    v.id == sensor.view
                        && matches!(
                            v.kind,
                            ViewKind::Spatial { .. } | ViewKind::PointCloud { .. }
                        )
                }) {
                    return Err(invalid(
                        "Sensor appearance must reference a configured scan/cloud display",
                    ));
                }
            }
            for robot in &presentation.robots {
                if !self.views.iter().any(|v| {
                    v.id == robot.view
                        && matches!(v.kind, ViewKind::Robot { .. } | ViewKind::RobotTopic { .. })
                }) {
                    return Err(invalid(
                        "Robot appearance must reference a configured Robot display",
                    ));
                }
            }
        }
        if self.version != FORMAT_VERSION {
            return Err(invalid(format!(
                "unsupported format version {} (expected {FORMAT_VERSION})",
                self.version
            )));
        }
        if self.name.is_empty() || self.name.len() > 128 || self.name.chars().any(char::is_control)
        {
            return Err(invalid(
                "name must contain 1–128 UTF-8 bytes without control characters",
            ));
        }
        let max_views = if self.presentation.is_some() {
            MAX_VISUALIZER_VIEWS
        } else {
            MAX_VIEWS
        };
        if self.views.len() > max_views {
            return Err(invalid(format!(
                "this format supports at most {max_views} displays"
            )));
        }
        if self
            .connection_suggestion
            .as_ref()
            .is_some_and(|c| c.domain > 232)
        {
            return Err(invalid("suggested domain must be 0–232"));
        }
        let mut ids = HashSet::new();
        for view in &self.views {
            if !valid_id(&view.id) {
                return Err(invalid("invalid view configuration ID"));
            }
            if !ids.insert(&view.id) {
                return Err(invalid("duplicate view configuration ID"));
            }
            if !topic_valid(&view.topic) {
                return Err(invalid(
                    "topic must be a fully qualified ROS name, at most 256 bytes",
                ));
            }
            if !message_type_valid(&view.message_type) {
                return Err(invalid("message type must be package/msg/Type"));
            }
            view.kind.validate(&view.message_type)?;
            if let ViewKind::Camera { info, .. } = &view.kind {
                if info.topic == view.topic {
                    return Err(invalid("Image and CameraInfo sources must be distinct"));
                }
            }
            if let ViewKind::Camera { transforms, .. }
            | ViewKind::Uncertainty { transforms, .. }
            | ViewKind::Map { transforms, .. }
            | ViewKind::Markers { transforms, .. } = &view.kind
            {
                if view.topic == transforms.dynamic_tf.topic
                    || view.topic == transforms.static_tf.topic
                {
                    return Err(invalid("Display source must differ from TF inputs"));
                }
            }
            if let ViewKind::Map {
                updates: Some(updates),
                ..
            } = &view.kind
            {
                if updates.topic == view.topic {
                    return Err(invalid("Map full and update topics must be distinct"));
                }
            }
            if let ViewKind::Robot { transforms, .. }
            | ViewKind::RobotTopic { transforms, .. }
            | ViewKind::TfGraph { transforms, .. } = &view.kind
            {
                if view.topic != transforms.dynamic_tf.topic
                    || view.qos != transforms.dynamic_tf.qos
                {
                    return Err(invalid(
                        "TF-owning panel source and QoS must match its dynamic TF input",
                    ));
                }
            }
        }
        self.layout.validate(&self.views)?;
        Ok(())
    }
    pub fn parse(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() > MAX_FILE_BYTES {
            return Err(invalid("file exceeds 65536 bytes"));
        }
        check_depth(bytes)?;
        // Typed deserialization rejects duplicate members, unknown fields and enum values.
        let workspace = migration::parse(bytes)?;
        workspace.validate()?;
        Ok(workspace)
    }
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.tools.is_some() {
            return Err(invalid(
                "Tools tasks require the Tools9 application envelope",
            ));
        }
        if self.views.iter().any(|v| {
            matches!(
                v.kind,
                ViewKind::TfGraph { .. }
                    | ViewKind::RobotTopic { .. }
                    | ViewKind::Map { .. }
                    | ViewKind::Markers { .. }
                    | ViewKind::Uncertainty { .. }
                    | ViewKind::Camera { .. }
            )
        }) {
            return Err(invalid(
                "This display requires the current Visualizer application envelope",
            ));
        }
        if self.presentation.is_some() {
            return Err(invalid(
                "Visualizer presentation requires the current application envelope",
            ));
        }
        self.validate()?;
        let bytes = serde_json::to_vec_pretty(self).map_err(|e| invalid(e.to_string()))?;
        if bytes.len() > MAX_FILE_BYTES {
            return Err(invalid("encoded file exceeds 65536 bytes"));
        }
        Ok(bytes)
    }
    /// This token carries only validated configuration. Applying it never implies activation.
    pub fn inactive_restore(self) -> Result<InactiveRestore, Error> {
        self.validate()?;
        Ok(InactiveRestore {
            configuration: self,
        })
    }
}
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
}
fn message_type_valid(message_type: &str) -> bool {
    let parts: Vec<_> = message_type.split('/').collect();
    message_type.len() <= 256
        && parts.len() == 3
        && parts[1] == "msg"
        && parts.iter().all(|p| token_valid(p))
}
fn token_valid(token: &str) -> bool {
    !token.is_empty()
        && token
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
fn topic_valid(topic: &str) -> bool {
    topic.len() <= 256 && topic.starts_with('/') && topic[1..].split('/').all(token_valid)
}
pub(crate) fn check_depth(bytes: &[u8]) -> Result<(), Error> {
    let (mut depth, mut quoted, mut escaped) = (0usize, false, false);
    for &byte in bytes {
        if quoted {
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                quoted = false;
            }
        } else {
            match byte {
                b'"' => quoted = true,
                b'{' | b'[' => {
                    depth += 1;
                    if depth > MAX_JSON_DEPTH {
                        return Err(invalid("JSON nesting exceeds 12 levels"));
                    }
                }
                b'}' | b']' => depth = depth.saturating_sub(1),
                _ => (),
            }
        }
    }
    Ok(())
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InactiveRestore {
    configuration: Workspace,
}
impl InactiveRestore {
    pub fn configuration(&self) -> &Workspace {
        &self.configuration
    }
    pub fn into_configuration(self) -> Workspace {
        self.configuration
    }
}

/// Pure editor state. Loading failure/cancellation never changes current or saved state.
#[derive(Debug)]
pub struct Document {
    current: Workspace,
    saved: Option<Workspace>,
}
impl Document {
    pub fn new(current: Workspace) -> Result<Self, Error> {
        current.validate()?;
        Ok(Self {
            current,
            saved: None,
        })
    }
    pub fn current(&self) -> &Workspace {
        &self.current
    }
    pub fn is_dirty(&self) -> bool {
        self.saved.as_ref() != Some(&self.current)
    }
    pub fn edit(&mut self, candidate: Workspace) -> Result<(), Error> {
        candidate.validate()?;
        self.current = candidate;
        Ok(())
    }
    /// Call only after the lifecycle owner has acknowledged all prior views stopped.
    pub fn apply_inactive(&mut self, restore: InactiveRestore) {
        self.current = restore.into_configuration();
        self.saved = Some(self.current.clone());
    }
    /// Record a worker's successful exact snapshot; newer edits remain dirty.
    pub fn mark_saved(&mut self, snapshot: Workspace) -> Result<(), Error> {
        snapshot.validate()?;
        self.saved = Some(snapshot);
        Ok(())
    }
    /// Only a successful durable save marks the exact snapshot saved.
    pub fn save(&mut self, path: &std::path::Path) -> Result<(), Error> {
        save_atomic(path, &self.current)?;
        self.saved = Some(self.current.clone());
        Ok(())
    }
}
#[cfg(test)]
mod tests;
#[cfg(test)]
mod v3_tests;
#[cfg(test)]
fn storage_test_id() -> u64 {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

pub use storage::{read_bytes_limit as read_local_bytes, save_bytes as save_local_bytes};
