//! Bounded complete pose/covariance snapshots. Not negotiated by session14.
//! Source coordinates remain intact; TF captures do not grant clock authority.
use crate::{latest_pose::LatestPose, mapping, spatial, subscriptions::*, uncertainty::Covariance};
use serde::{Deserialize, Serialize};
pub mod store;
pub mod transport;
pub const CONTRACT_VERSION: u32 = 1;
pub const POSE_MESSAGE_TYPE: &str = "geometry_msgs/msg/PoseWithCovarianceStamped";
pub const ODOMETRY_MESSAGE_TYPE: &str = "nav_msgs/msg/Odometry";
pub const MAX_HISTORY: usize = 256;
pub const MAX_PENDING: usize = 16;
pub const MAX_OWNERS: usize = 2;
pub const MAX_HZ: u32 = 20;
pub const MAX_DECODED_BYTES: usize = 8 * 1024;
pub const MAX_SAMPLE_BYTES: usize = 512 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    PoseWithCovariance,
    Odometry,
}
impl Kind {
    pub fn message_type(self) -> &'static str {
        match self {
            Self::PoseWithCovariance => POSE_MESSAGE_TYPE,
            Self::Odometry => ODOMETRY_MESSAGE_TYPE,
        }
    }
    pub fn from_message_type(value: &str) -> Result<Self, String> {
        match value {
            POSE_MESSAGE_TYPE => Ok(Self::PoseWithCovariance),
            ODOMETRY_MESSAGE_TYPE => Ok(Self::Odometry),
            _ => Err("Unsupported exact pose/covariance message type".into()),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub frame_id: String,
    /// Odometry metadata only; never the frame for pose/covariance TF lookup.
    pub child_frame_id: Option<String>,
}
impl Source {
    pub fn validate(&self, kind: Kind) -> Result<(), String> {
        let child = match (kind, &self.child_frame_id) {
            (Kind::PoseWithCovariance, None) => true,
            (Kind::Odometry, Some(frame)) => frame.is_empty() || spatial::valid_frame(frame),
            _ => false,
        };
        if !spatial::valid_frame(&self.frame_id) || !child {
            return Err("Invalid pose/covariance source frame or child-frame metadata".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Decoded {
    pub kind: Kind,
    pub topic: String,
    pub source: Source,
    pub acquisition_stamp_ns: i64,
    pub components: [f64; 7],
    pub covariance: Covariance,
}
impl Decoded {
    pub fn validate(&self) -> Result<(), String> {
        self.source.validate(self.kind)?;
        if !crate::subscriptions::valid_topic(&self.topic)
            || !(0..=crate::latest_pose::MAX_STAMP_NS).contains(&self.acquisition_stamp_ns)
            || !mapping::valid_pose(&self.components)
        {
            return Err("Invalid bounded stamped pose/covariance source".into());
        }
        Ok(())
    }
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        encode(self, MAX_DECODED_BYTES)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let value: Self = decode(bytes, MAX_DECODED_BYTES)?;
        value.validate()?;
        Ok(value)
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistorySettings {
    pub keep: usize,
    pub position_tolerance_m: f64,
    pub angle_tolerance_rad: f64,
}
impl Default for HistorySettings {
    fn default() -> Self {
        Self {
            keep: 100,
            position_tolerance_m: 0.1,
            angle_tolerance_rad: 0.1,
        }
    }
}
impl HistorySettings {
    pub fn validate(&self) -> Result<(), String> {
        if !(1..=MAX_HISTORY).contains(&self.keep)
            || !self.position_tolerance_m.is_finite()
            || !(0.0..=1000.).contains(&self.position_tolerance_m)
            || !self.angle_tolerance_rad.is_finite()
            || !(0.0..=std::f64::consts::PI).contains(&self.angle_tolerance_rad)
        {
            return Err("Invalid bounded Odometry history settings".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub sequence: u64,
    /// Zero explicitly requests latest-available TF, not exact acquisition TF.
    pub acquisition_stamp_ns: i64,
    pub components: [f64; 7],
    pub covariance: Covariance,
    pub transform: LatestPose,
}
impl Entry {
    pub fn validate(&self) -> Result<(), String> {
        crate::latest_pose::LookupTime::from_stamp(self.acquisition_stamp_ns)?
            .validate_result(&self.transform)?;
        if self.sequence == 0
            || !mapping::valid_pose(&self.components)
            || !(0..=crate::latest_pose::MAX_STAMP_NS).contains(&self.acquisition_stamp_ns)
        {
            return Err("Invalid identified stamped pose/covariance entry".into());
        }
        Ok(())
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Owner {
    pub generation: Generation,
    pub subscription: SubscriptionId,
    pub kind: Kind,
    pub topic: String,
    pub fixed_frame: String,
}
impl Owner {
    pub fn validate(&self) -> Result<(), String> {
        if self.generation.0 == 0
            || self.subscription.0 == 0
            || !crate::subscriptions::valid_topic(&self.topic)
            || !spatial::valid_frame(&self.fixed_frame)
        {
            return Err("Invalid pose/covariance snapshot owner".into());
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResetReason {
    LocalClear,
    SourceGap,
    FrameChanged,
    TimePolicyChanged,
    SourceFault,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Counters {
    pub received: u64,
    pub duplicates: u64,
    pub filtered: u64,
    pub evicted: u64,
    pub pending_dropped: u64,
    pub lookup_timeouts: u64,
    pub known_source_gaps: u64,
    pub resets: u64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub contract_version: u32,
    pub owner: Owner,
    pub revision: u64,
    pub continuity_epoch: u64,
    pub source: Option<Source>,
    pub history: Option<HistorySettings>,
    pub latest: Option<Entry>,
    #[serde(deserialize_with = "crate::serde_bounded::pose_history")]
    pub trail: Vec<Entry>,
    pub last_reset: Option<ResetReason>,
    pub known_history_gap: bool,
    pub counters: Counters,
}
impl Snapshot {
    pub fn validate(&self) -> Result<(), String> {
        self.owner.validate()?;
        if self.contract_version != CONTRACT_VERSION
            || self.revision == 0
            || self.continuity_epoch == 0
        {
            return Err("Invalid pose/covariance snapshot version or revision".into());
        }
        if (self.counters.known_source_gaps > 0 || self.counters.pending_dropped > 0)
            && !self.known_history_gap
        {
            return Err("Known pose history loss must remain explicit".into());
        }
        if let Some(source) = &self.source {
            source.validate(self.owner.kind)?;
        }
        match (self.owner.kind, self.history) {
            (Kind::PoseWithCovariance, None) if self.trail.is_empty() => (),
            (Kind::Odometry, Some(settings)) if self.trail.len() <= settings.keep => {
                settings.validate()?
            }
            _ => return Err("Contradictory pose/covariance kind or history settings".into()),
        }
        if self.trail.len() > MAX_HISTORY {
            return Err("Pose history exceeds its entry budget".into());
        }
        let Some(latest) = &self.latest else {
            return if self.trail.is_empty() {
                Ok(())
            } else {
                Err("Pose history requires an identified latest head".into())
            };
        };
        let source = self
            .source
            .as_ref()
            .ok_or("Pose entries require source-frame context")?;
        latest.validate()?;
        let mut previous: Option<&Entry> = None;
        for entry in &self.trail {
            entry.validate()?;
            if !matches!(entry.transform, LatestPose::Ready { .. })
                || entry.sequence > latest.sequence
                || (entry.acquisition_stamp_ns == 0) != (latest.acquisition_stamp_ns == 0)
                || (entry.sequence == latest.sequence && entry != latest)
                || (entry.sequence < latest.sequence
                    && latest.acquisition_stamp_ns > 0
                    && entry.acquisition_stamp_ns >= latest.acquisition_stamp_ns)
                || previous.is_some_and(|p| {
                    p.sequence >= entry.sequence
                        || (entry.acquisition_stamp_ns > 0
                            && p.acquisition_stamp_ns >= entry.acquisition_stamp_ns)
                })
            {
                return Err("Contradictory ordered pose history or captured TF".into());
            }
            previous = Some(entry);
        }
        if source.frame_id == self.owner.fixed_frame {
            for entry in self.trail.iter().chain(std::iter::once(latest)) {
                if let LatestPose::Ready { components, .. } = &entry.transform {
                    if components[..6].iter().any(|v| *v != 0.)
                        || (components[6].abs() - 1.).abs() > 1e-6
                    {
                        return Err("Same-frame pose TF must be identity".into());
                    }
                }
            }
        }
        Ok(())
    }
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        encode(self, MAX_SAMPLE_BYTES)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        let value: Self = decode(bytes, MAX_SAMPLE_BYTES)?;
        value.validate()?;
        Ok(value)
    }
}
fn encode(value: &impl Serialize, maximum: usize) -> Result<Vec<u8>, String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() > maximum {
        Err("Pose/covariance value exceeds its encoded byte budget".into())
    } else {
        Ok(bytes)
    }
}
fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8], maximum: usize) -> Result<T, String> {
    if bytes.len() > maximum {
        return Err("Pose/covariance input exceeds its byte budget".into());
    }
    serde_json::from_slice(bytes).map_err(|e| e.to_string())
}
#[cfg(test)]
mod tests;
