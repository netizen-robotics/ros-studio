//! Identified rectified Camera frames. Not advertised by session15.
//! Image acquisition, calibration and captured TF form one immutable value.
use crate::{
    camera_info::Calibration,
    image_v1::{ImageMetadata, ImageSample},
    latest_pose::{LatestPose, LookupTime, MAX_STAMP_NS},
    spatial,
    subscriptions::{valid_topic, Generation, SubscriptionId},
};
use serde::{Deserialize, Serialize};
#[path = "camera_snapshot/transport_v1.rs"]
pub mod transport;
pub const CONTRACT_VERSION: u32 = 1;
pub const MAX_SAMPLE_BYTES: usize = 512 * 1024;
pub const MAX_CONTROL_BYTES: usize = 4096;
pub const MAX_HZ: u32 = 20;
pub const MAX_OWNERS: usize = 1;
pub const TIMEOUT_NS: u64 = 2_000_000_000;
pub const RETRY_NS: u64 = 50_000_000;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InfoSource {
    pub topic: String,
    pub best_effort: bool,
    pub transient_local: bool,
}
impl InfoSource {
    pub fn validate(&self) -> Result<(), String> {
        if !valid_topic(&self.topic) {
            return Err("Invalid CameraInfo topic".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Owner {
    pub generation: Generation,
    pub subscription: SubscriptionId,
    pub topic: String,
    pub info: InfoSource,
    pub fixed_frame: String,
}
impl Owner {
    pub fn validate(&self) -> Result<(), String> {
        self.info.validate()?;
        if self.generation.0 == 0
            || self.subscription.0 == 0
            || !valid_topic(&self.topic)
            || self.topic == self.info.topic
            || !spatial::valid_frame(&self.fixed_frame)
        {
            return Err("Invalid Camera owner".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    WaitingInfo,
    WaitingImage,
    Uncalibrated,
    Incompatible,
    Unsupported,
    InvalidInfo,
    InvalidImage,
    WaitingTf,
    TfTimeout,
    Receiving,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Counters {
    pub images: u64,
    pub calibrations: u64,
    pub pending_dropped: u64,
    pub invalid_images: u64,
    pub invalid_calibrations: u64,
    pub lookup_timeouts: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    pub image_sequence: u64,
    pub calibration_revision: u64,
    pub image: ImageSample,
    pub calibration: Calibration,
    pub transform: LatestPose,
}
impl Frame {
    pub fn validate(&self, owner: &Owner) -> Result<(), String> {
        self.image.validate()?;
        let effective = self.calibration.effective()?;
        if self.image.metadata.frame_id != self.calibration.frame_id
            || effective.size != [self.image.metadata.width, self.image.metadata.height]
        {
            return Err("Image and CameraInfo optical frame or effective dimensions differ".into());
        }
        LookupTime::from_stamp(image_stamp(&self.image.metadata)?)?
            .validate_result(&self.transform)?;
        if self.image_sequence == 0
            || self.calibration_revision == 0
            || self.image.topic != owner.topic
            || !matches!(self.transform, LatestPose::Ready { .. })
        {
            return Err("Invalid captured Camera frame".into());
        }
        if self.image.metadata.frame_id == owner.fixed_frame {
            if let LatestPose::Ready { components, .. } = &self.transform {
                if components[..6].iter().any(|&v| v != 0.)
                    || (components[6].abs() - 1.).abs() > 1e-6
                {
                    return Err("Same-frame Camera TF must be identity".into());
                }
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub contract_version: u32,
    pub owner: Owner,
    pub revision: u64,
    pub continuity_epoch: u64,
    pub status: Status,
    #[serde(deserialize_with = "required_option")]
    pub diagnostic: Option<String>,
    pub counters: Counters,
    #[serde(deserialize_with = "required_option")]
    pub frame: Option<Box<Frame>>,
}
fn required_option<'de, D: serde::Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    Option::<T>::deserialize(deserializer)
}
impl Snapshot {
    pub fn validate(&self) -> Result<(), String> {
        self.owner.validate()?;
        if self.contract_version != CONTRACT_VERSION
            || self.revision == 0
            || self.continuity_epoch == 0
            || (self.status == Status::Receiving) != self.frame.is_some()
            || self
                .diagnostic
                .as_ref()
                .is_some_and(|s| s.is_empty() || s.len() > 512 || s.contains('\0'))
        {
            return Err("Invalid Camera snapshot state or identity".into());
        }
        if let Some(frame) = &self.frame {
            frame.validate(&self.owner)?;
            if frame.image_sequence > self.counters.images
                || frame.calibration_revision > self.counters.calibrations
            {
                return Err("Camera frame contradicts source counters".into());
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

pub fn image_stamp(metadata: &ImageMetadata) -> Result<i64, String> {
    metadata.raw_length()?;
    let stamp = metadata.header_stamp;
    if stamp.seconds < 0 {
        return Err("Camera Image requires a nonnegative acquisition stamp".into());
    }
    let ns = i64::from(stamp.seconds) * 1_000_000_000 + i64::from(stamp.nanoseconds);
    if ns > MAX_STAMP_NS {
        return Err("Camera Image stamp is outside ROS time bounds".into());
    }
    Ok(ns)
}

fn encode(value: &impl Serialize, limit: usize) -> Result<Vec<u8>, String> {
    let bytes = serde_json::to_vec(value).map_err(|e| e.to_string())?;
    if bytes.len() > limit {
        return Err("Camera value exceeds its byte budget".into());
    }
    Ok(bytes)
}
fn decode<T: serde::de::DeserializeOwned>(bytes: &[u8], limit: usize) -> Result<T, String> {
    if bytes.len() > limit {
        return Err("Camera value exceeds its byte budget".into());
    }
    serde_json::from_slice(bytes).map_err(|e| e.to_string())
}
