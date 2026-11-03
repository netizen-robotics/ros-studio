//! Configuration-only restore gate. It describes Stop intents, never activation.
//! The lifecycle owner must call cleanup_confirmed only after validated native cleanup.
use super::{Durability, InactiveRestore, Reliability, ViewKind, Workspace};
use studio_protocol::subscriptions::{Generation, SubscriptionId};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HandleIdentity {
    pub generation: Generation,
    pub subscription: SubscriptionId,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct StopIntent {
    pub owner: HandleIdentity,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PreviewId(pub u64);
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SupportedRenderers {
    pub messages: bool,
    pub numeric_plot: bool,
    pub image: bool,
    pub spatial: bool,
    pub robot: bool,
    pub point_cloud: bool,
    pub occupancy_grid: bool,
    pub map: bool,
    pub markers: bool,
    pub uncertainty: bool,
    pub camera: bool,
    pub pose: bool,
    pub path: bool,
    pub tf_graph: bool,
    pub robot_topic: bool,
}
impl SupportedRenderers {
    pub fn all() -> Self {
        Self {
            messages: true,
            numeric_plot: true,
            image: true,
            spatial: true,
            robot: true,
            point_cloud: true,
            occupancy_grid: true,
            map: true,
            markers: true,
            uncertainty: true,
            camera: true,
            pose: true,
            path: true,
            tf_graph: true,
            robot_topic: true,
        }
    }
    fn supports(self, kind: &ViewKind) -> bool {
        match kind {
            ViewKind::Uncertainty { .. } => self.uncertainty,
            ViewKind::Camera { .. } => self.camera,
            ViewKind::Map { .. } => self.map,
            ViewKind::Markers { .. } => self.markers,
            ViewKind::RobotTopic { .. } => self.robot_topic,
            ViewKind::TfGraph { .. } => self.tf_graph,
            ViewKind::Messages => self.messages,
            ViewKind::NumericPlot { .. } => self.numeric_plot,
            ViewKind::Image { .. } => self.image,
            ViewKind::Spatial { .. } => self.spatial,
            ViewKind::Robot { .. } => self.robot,
            ViewKind::PointCloud { .. } => self.point_cloud,
            ViewKind::OccupancyGrid { .. } => self.occupancy_grid,
            ViewKind::Pose { .. } => self.pose,
            ViewKind::Path { .. } => self.path,
        }
    }
    /// Checks current live configuration capabilities, not clock authorization
    /// or physical resource availability. Stored inactive drafts remain intact.
    pub fn validate_workspace(self, workspace: &Workspace) -> Result<(), RestoreError> {
        workspace
            .validate()
            .map_err(|error| RestoreError::Invalid(error.to_string()))?;
        for view in &workspace.views {
            if !self.supports(&view.kind) {
                return Err(RestoreError::UnsupportedRenderer(view.id.clone()));
            }
            let ViewKind::Spatial {
                fixed_frame,
                layers,
                transforms,
                ..
            } = &view.kind
            else {
                continue;
            };
            let blocked = |reason: &str| RestoreError::UnsupportedConfiguration {
                panel: view.id.clone(),
                reason: reason.into(),
            };
            if layers.len() != 1 {
                return Err(blocked("Current Spatial views support exactly one LaserScan layer; all saved layers were retained"));
            }
            if !studio_protocol::spatial::valid_frame(fixed_frame) {
                return Err(blocked("Choose a fixed frame without a leading slash, whitespace or control characters"));
            }
            let scan = &layers[0];
            if scan.topic != view.topic
                || scan.message_type != view.message_type
                || scan.qos != view.qos
            {
                return Err(blocked(
                    "Panel scan topic, type and QoS must match its single LaserScan layer",
                ));
            }
            let transforms = transforms.as_ref().ok_or_else(|| blocked("Configure explicit dynamic and static TF inputs before applying this workspace"))?;
            if transforms.dynamic_tf.topic == transforms.static_tf.topic {
                return Err(blocked("Dynamic and static TF topics must be distinct"));
            }
            for input in [&transforms.dynamic_tf, &transforms.static_tf] {
                if input.message_type != "tf2_msgs/msg/TFMessage" {
                    return Err(blocked("TF inputs require tf2_msgs/msg/TFMessage"));
                }
            }
            if transforms.dynamic_tf.qos.durability != Durability::Volatile {
                return Err(blocked("Dynamic TF requires Volatile durability"));
            }
            if transforms.static_tf.qos.reliability != Reliability::Reliable
                || transforms.static_tf.qos.durability != Durability::TransientLocal
            {
                return Err(blocked(
                    "Static TF requires Reliable reliability and TransientLocal durability",
                ));
            }
        }
        Ok(())
    }
}
/// CleanOrSaved requires the caller's authoritative current dirty flag to be false.
/// Save itself stays with the file worker/UI; a failed save cannot satisfy this gate.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ApplyApproval {
    CleanOrSaved,
    DiscardUnsaved,
    Cancel,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RestoreError {
    Invalid(String),
    UnsupportedRenderer(String),
    UnsupportedConfiguration { panel: String, reason: String },
    NoPreview,
    StalePreview,
    ApplyInProgress,
    NotApplying,
    UnsavedChanges,
    InvalidOwners,
    OwnersChanged,
    IdExhausted,
    RevisionChanged { expected: u64, actual: u64 },
}
impl std::fmt::Display for RestoreError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Invalid(message) => f.write_str(message),
            Self::UnsupportedRenderer(id) => write!(
                f,
                "Workspace panel {id} uses an unavailable renderer; no configuration was discarded"
            ),
            Self::UnsupportedConfiguration { panel, reason } => write!(
                f,
                "Workspace panel {panel}: {reason}; no configuration was discarded"
            ),
            Self::NoPreview => f.write_str("Open and review a workspace before applying it"),
            Self::StalePreview => {
                f.write_str("Workspace preview was replaced; review the current preview")
            }
            Self::ApplyInProgress => {
                f.write_str("Workspace replacement is already waiting for cleanup")
            }
            Self::NotApplying => f.write_str("Workspace replacement is not waiting for cleanup"),
            Self::UnsavedChanges => f.write_str(
                "Save or explicitly discard unsaved changes before replacing the workspace",
            ),
            Self::InvalidOwners => {
                f.write_str("Expected at most eight unique nonzero native stream identities")
            }
            Self::OwnersChanged => {
                f.write_str("Native streams changed during restore; review and apply again")
            }
            Self::IdExhausted => f.write_str("Workspace preview IDs exhausted"),
            Self::RevisionChanged { .. } => {
                f.write_str("Workspace changed after preview; review again before replacing it")
            }
        }
    }
}
impl std::error::Error for RestoreError {}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StopFailure {
    pub owner: HandleIdentity,
    pub message: String,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub enum RestoreState {
    #[default]
    Idle,
    Preview {
        id: PreviewId,
    },
    Stopping {
        id: PreviewId,
        pending: Vec<HandleIdentity>,
        failures: Vec<StopFailure>,
    },
    Ready {
        id: PreviewId,
    },
    ReviewRequired {
        id: PreviewId,
        expected_revision: u64,
        actual_revision: u64,
    },
}
struct Candidate {
    id: PreviewId,
    revision: u64,
    workspace: Workspace,
}
#[derive(Default)]
pub struct RestoreCoordinator {
    candidate: Option<Candidate>,
    state: RestoreState,
}
impl RestoreCoordinator {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn state(&self) -> &RestoreState {
        &self.state
    }
    pub fn configuration(&self) -> Option<&Workspace> {
        self.candidate.as_ref().map(|c| &c.workspace)
    }
    /// Validates before replacing the previous preview. A successful new preview
    /// cancels any prior apply; already requested native stops are not undone.
    pub fn preview(
        &mut self,
        workspace: Workspace,
        revision: u64,
        supported: SupportedRenderers,
    ) -> Result<PreviewId, RestoreError> {
        supported.validate_workspace(&workspace)?;
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        let id = PreviewId(
            NEXT.fetch_update(
                std::sync::atomic::Ordering::Relaxed,
                std::sync::atomic::Ordering::Relaxed,
                |id| id.checked_add(1),
            )
            .map_err(|_| RestoreError::IdExhausted)?,
        );
        self.candidate = Some(Candidate {
            id,
            revision,
            workspace,
        });
        self.state = RestoreState::Preview { id };
        Ok(id)
    }
    fn check_preview(&self, id: PreviewId) -> Result<(), RestoreError> {
        let candidate = self.candidate.as_ref().ok_or(RestoreError::NoPreview)?;
        if candidate.id != id {
            return Err(RestoreError::StalePreview);
        }
        Ok(())
    }
    fn check_revision(&mut self, revision: u64) -> Result<(), RestoreError> {
        let candidate = self.candidate.as_ref().ok_or(RestoreError::NoPreview)?;
        if candidate.revision != revision {
            self.state = RestoreState::ReviewRequired {
                id: candidate.id,
                expected_revision: candidate.revision,
                actual_revision: revision,
            };
            return Err(RestoreError::RevisionChanged {
                expected: candidate.revision,
                actual: revision,
            });
        }
        // Once a mismatch was observed, editing back to an old revision cannot
        // revive a canceled apply. A new explicit preview is required.
        if let RestoreState::ReviewRequired {
            expected_revision,
            actual_revision,
            ..
        } = self.state
        {
            return Err(RestoreError::RevisionChanged {
                expected: expected_revision,
                actual: actual_revision,
            });
        }
        Ok(())
    }
    /// Emits each initial Stop exactly once. The UI owns Save/Discard/Cancel
    /// prompts and must freeze further activation/config edits while this waits.
    pub fn apply(
        &mut self,
        id: PreviewId,
        revision: u64,
        dirty: bool,
        approval: ApplyApproval,
        owners: &[HandleIdentity],
    ) -> Result<Vec<StopIntent>, RestoreError> {
        self.check_preview(id)?;
        if approval == ApplyApproval::Cancel {
            self.cancel_apply();
            return Ok(Vec::new());
        }
        self.check_revision(revision)?;
        if matches!(
            self.state,
            RestoreState::Stopping { .. } | RestoreState::Ready { .. }
        ) {
            return Err(RestoreError::ApplyInProgress);
        }
        if dirty && approval != ApplyApproval::DiscardUnsaved {
            return Err(RestoreError::UnsavedChanges);
        }
        if owners.len() > super::MAX_VISUALIZER_VIEWS
            || owners
                .iter()
                .any(|owner| owner.generation.0 == 0 || owner.subscription.0 == 0)
            || owners
                .iter()
                .enumerate()
                .any(|(index, owner)| owners[..index].contains(owner))
        {
            return Err(RestoreError::InvalidOwners);
        }
        let mut pending = owners.to_vec();
        pending.sort_by_key(|owner| (owner.generation.0, owner.subscription.0));
        let intents = pending
            .iter()
            .map(|owner| StopIntent { owner: *owner })
            .collect();
        self.state = if pending.is_empty() {
            RestoreState::Ready { id }
        } else {
            RestoreState::Stopping {
                id,
                pending,
                failures: Vec::new(),
            }
        };
        Ok(intents)
    }
    /// Only a verified, matching cleanup acknowledgement is evidence. Samples,
    /// timeouts, queued Stop requests and unverified no-handle claims are not proof.
    /// Unknown/stale owners are ignored. Root must block new activations while applying.
    pub fn cleanup_confirmed(
        &mut self,
        owner: HandleIdentity,
        revision: u64,
    ) -> Result<bool, RestoreError> {
        if !matches!(self.state, RestoreState::Stopping { .. }) {
            return Ok(false);
        }
        self.check_revision(revision)?;
        let RestoreState::Stopping {
            id,
            pending,
            failures,
        } = &mut self.state
        else {
            unreachable!()
        };
        let Some(index) = pending.iter().position(|pending| *pending == owner) else {
            return Ok(false);
        };
        pending.remove(index);
        failures.retain(|failure| failure.owner != owner);
        if pending.is_empty() {
            self.state = RestoreState::Ready { id: *id };
        }
        Ok(true)
    }
    /// A failed Stop stays pending and cannot release its owner or finish restore.
    pub fn stop_failed(
        &mut self,
        owner: HandleIdentity,
        revision: u64,
        message: &str,
    ) -> Result<bool, RestoreError> {
        if !matches!(self.state, RestoreState::Stopping { .. }) {
            return Ok(false);
        }
        self.check_revision(revision)?;
        let RestoreState::Stopping {
            pending, failures, ..
        } = &mut self.state
        else {
            unreachable!()
        };
        if !pending.contains(&owner) {
            return Ok(false);
        }
        let suffix = " … [truncated]";
        let mut end = message.len().min(if message.len() > 512 {
            512 - suffix.len()
        } else {
            512
        });
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        let message = if end == 0 {
            "Stop failed without cleanup proof".to_owned()
        } else if end < message.len() {
            format!("{}{suffix}", &message[..end])
        } else {
            message.to_owned()
        };
        failures.retain(|failure| failure.owner != owner);
        failures.push(StopFailure { owner, message });
        Ok(true)
    }
    /// Explicit user retry only. No automatic replay is emitted by polling/events.
    pub fn retry_failed(
        &mut self,
        id: PreviewId,
        revision: u64,
    ) -> Result<Vec<StopIntent>, RestoreError> {
        self.check_preview(id)?;
        self.check_revision(revision)?;
        let RestoreState::Stopping { failures, .. } = &mut self.state else {
            return Err(RestoreError::NotApplying);
        };
        Ok(std::mem::take(failures)
            .into_iter()
            .map(|failure| StopIntent {
                owner: failure.owner,
            })
            .collect())
    }
    /// Yield the exact reviewed snapshot once, always as inactive configuration.
    /// Root must confirm there were no new owners since apply and commit this
    /// token synchronously with its configuration/layout replacement.
    pub fn take_ready(
        &mut self,
        revision: u64,
        current_owners: &[HandleIdentity],
    ) -> Result<Option<InactiveRestore>, RestoreError> {
        if !matches!(self.state, RestoreState::Ready { .. }) {
            return Ok(None);
        }
        self.check_revision(revision)?;
        if !current_owners.is_empty() {
            self.cancel_apply();
            return Err(RestoreError::OwnersChanged);
        }
        let candidate = self.candidate.take().ok_or(RestoreError::NoPreview)?;
        self.state = RestoreState::Idle;
        candidate
            .workspace
            .inactive_restore()
            .map(Some)
            .map_err(|error| RestoreError::Invalid(error.to_string()))
    }
    /// Cancel automatic replacement, retaining the preview for an explicit retry.
    /// Already emitted native Stop requests continue under the lifecycle owner.
    pub fn cancel_apply(&mut self) {
        if matches!(self.state, RestoreState::ReviewRequired { .. }) {
            return;
        }
        self.state = self
            .candidate
            .as_ref()
            .map_or(RestoreState::Idle, |c| RestoreState::Preview { id: c.id });
    }
    /// Disconnect invalidates preview and all pending restore intent. Reconnect
    /// cannot complete or replay a prior replacement.
    pub fn disconnected(&mut self) {
        self.candidate = None;
        self.state = RestoreState::Idle;
    }
}
#[cfg(test)]
mod tests;
