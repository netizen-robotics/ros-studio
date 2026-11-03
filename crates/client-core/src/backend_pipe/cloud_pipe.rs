use super::*;
use studio_protocol::cloud;
use studio_visualization::spatial::cloud_live::{CloudFrame, CloudLiveError, CloudStream};

#[derive(Clone)]
pub struct CloudTarget {
    pub generation: Generation,
    pub subscription: SubscriptionId,
    pub topic: String,
    pub fixed_frame: String,
    pub local_generation: u64,
    pub stream: CloudStream,
}
impl Shared {
    pub fn bind_cloud(&mut self, target: CloudTarget) -> Result<(), String> {
        if self.clouds.len() >= MAX_SUBSCRIPTIONS
            || self
                .clouds
                .iter()
                .any(|t| t.subscription == target.subscription)
            || !self.registry.entries().iter().any(|entry| {
                entry.generation == target.generation
                    && entry.subscription == target.subscription
                    && entry.configuration.output == Output::PointCloud
                    && entry.configuration.topic == target.topic
                    && entry.configuration.message_type == cloud::MESSAGE_TYPE
                    && entry
                        .configuration
                        .spatial
                        .as_ref()
                        .is_some_and(|c| c.fixed_frame == target.fixed_frame)
            })
        {
            return Err("Cloud binding does not match an allocated native owner".into());
        }
        self.clouds.push(target);
        Ok(())
    }
}
fn fault(shared: &mut Shared, target: &CloudTarget, message: String) {
    // At most one bounded diagnostic per still-allocated owner. Keep its native
    // registry slot until Stopped/no-handle cleanup proves resource retirement.
    if !shared
        .cloud_faults
        .iter()
        .any(|f| f.generation == target.generation && f.subscription == target.subscription)
    {
        shared.cloud_faults.push(CloudIngressFault {
            generation: target.generation,
            subscription: target.subscription,
            message,
        });
    }
}
pub(super) fn prepare(
    shared: &mut Shared,
    generation: Generation,
    subscription: SubscriptionId,
    sequence: u64,
    backend_drops: u64,
    sample: &cloud::Sample,
) -> Option<CloudTarget> {
    let target = shared
        .clouds
        .iter()
        .find(|t| t.generation == generation && t.subscription == subscription)
        .cloned();
    let fresh = shared.registry.entries().iter().any(|entry| {
        entry.generation == generation
            && entry.subscription == subscription
            && entry.configuration.output == Output::PointCloud
            && entry.state == State::Active
            && sequence > 0
            && entry.last_sequence.is_none_or(|last| sequence > last)
    });
    if !fresh
        || target.is_none()
        || shared
            .cloud_faults
            .iter()
            .any(|f| f.generation == generation && f.subscription == subscription)
    {
        shared.ingress_rejections = shared.ingress_rejections.saturating_add(1);
        return None;
    }
    let target = target.unwrap();
    // Advance only this fresh allocated owner; malformed identity cannot escape
    // the latch by retrying the same sequence while cleanup is pending.
    if shared
        .registry
        .sample(generation, subscription, sequence)
        .is_err()
    {
        shared.ingress_rejections = shared.ingress_rejections.saturating_add(1);
        return None;
    }
    if sample.topic != target.topic
        || sample.message_type != cloud::MESSAGE_TYPE
        || sample.fixed_frame != target.fixed_frame
    {
        let message = "PointCloud2 sample identity contradicts its active display; enable again explicitly after cleanup".to_string();
        if target.stream.publish(
            target.local_generation,
            sequence,
            backend_drops,
            Err(CloudLiveError(message.clone())),
        ) {
            fault(shared, &target, message);
        }
        shared.ingress_rejections = shared.ingress_rejections.saturating_add(1);
        return None;
    }
    Some(target)
}
pub(super) fn publish(
    shared: &mut Shared,
    target: &CloudTarget,
    sequence: u64,
    backend_drops: u64,
    decoded: Result<CloudFrame, CloudLiveError>,
) -> bool {
    let active = shared.registry.entries().iter().any(|entry| {
        entry.generation == target.generation
            && entry.subscription == target.subscription
            && entry.state == State::Active
            && entry.configuration.output == Output::PointCloud
    }) && shared.clouds.iter().any(|current| {
        current.generation == target.generation
            && current.subscription == target.subscription
            && current.local_generation == target.local_generation
    });
    if !active {
        shared.ingress_rejections = shared.ingress_rejections.saturating_add(1);
        return false;
    }
    let published =
        target
            .stream
            .publish(target.local_generation, sequence, backend_drops, decoded);
    if published {
        if let Some(message) = target.stream.snapshot().and_then(|u| u.error) {
            fault(shared, target, message);
        }
    }
    published
}
