use super::*;
use studio_visualization::spatial::mapping_live::{MappingFrame, MappingLiveError, MappingStream};

#[derive(Clone)]
pub struct MappingTarget {
    pub generation: Generation,
    pub subscription: SubscriptionId,
    pub topic: String,
    pub message_type: String,
    pub fixed_frame: String,
    pub local_generation: u64,
    pub stream: MappingStream,
}
impl Shared {
    pub fn bind_mapping(&mut self, target: MappingTarget) -> Result<(), String> {
        if self.mappings.len() >= MAX_SUBSCRIPTIONS
            || self
                .mappings
                .iter()
                .any(|t| t.subscription == target.subscription)
            || !self.registry.entries().iter().any(|entry| {
                entry.generation == target.generation
                    && entry.subscription == target.subscription
                    && entry.configuration.output.mapping_message_type()
                        == Some(target.message_type.as_str())
                    && entry.configuration.topic == target.topic
                    && entry.configuration.message_type == target.message_type
                    && entry
                        .configuration
                        .spatial
                        .as_ref()
                        .is_some_and(|c| c.fixed_frame == target.fixed_frame)
            })
        {
            return Err("Mapping binding does not match an allocated native owner".into());
        }
        self.mappings.push(target);
        Ok(())
    }
}
fn fault(shared: &mut Shared, target: &MappingTarget, message: String) {
    // At most one bounded diagnostic per still-allocated owner. Keep its native
    // registry slot until Stopped/no-handle cleanup proves resource retirement.
    if !shared
        .mapping_faults
        .iter()
        .any(|f| f.generation == target.generation && f.subscription == target.subscription)
    {
        shared.mapping_faults.push(MappingIngressFault {
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
    sample: &studio_protocol::mapping::Sample,
) -> Option<MappingTarget> {
    let target = shared
        .mappings
        .iter()
        .find(|t| t.generation == generation && t.subscription == subscription)
        .cloned();
    let fresh = shared.registry.entries().iter().any(|entry| {
        entry.generation == generation
            && entry.subscription == subscription
            && entry.configuration.output.is_mapping()
            && entry.state == State::Active
            && sequence > 0
            && entry.last_sequence.is_none_or(|last| sequence > last)
    });
    if !fresh
        || target.is_none()
        || shared
            .mapping_faults
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
        || sample.message_type != target.message_type
        || sample.fixed_frame != target.fixed_frame
    {
        let message = "Mapping sample identity contradicts its active display; enable again explicitly after cleanup".to_string();
        if target.stream.publish(
            target.local_generation,
            sequence,
            backend_drops,
            Err(MappingLiveError(message.clone())),
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
    target: &MappingTarget,
    sequence: u64,
    backend_drops: u64,
    decoded: Result<MappingFrame, MappingLiveError>,
) -> bool {
    let active = shared.registry.entries().iter().any(|entry| {
        entry.generation == target.generation
            && entry.subscription == target.subscription
            && entry.state == State::Active
            && entry.configuration.output.mapping_message_type()
                == Some(target.message_type.as_str())
    }) && shared.mappings.iter().any(|current| {
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
