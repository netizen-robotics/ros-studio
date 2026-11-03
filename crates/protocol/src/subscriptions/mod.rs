//! Shared pure observation lifecycle and fair queues. No ROS, egui or I/O.
use serde::{Deserialize, Serialize};
mod queue;
mod registry;
pub use queue::*;
pub use registry::*;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Generation(pub u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct RequestId(pub u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SubscriptionId(pub u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ViewId(pub u64);
pub const MAX_SUBSCRIPTIONS: usize = 32;
pub const MAX_NATIVE_HANDLES: usize = 64;
/// One simultaneous Start/Stop burst for each logical owner.
pub const COMMAND_QUEUE_CAPACITY: usize = MAX_SUBSCRIPTIONS * 2;
pub const MAX_SENSOR_LAYERS: usize = 4;
#[cfg(test)]
mod tests;
