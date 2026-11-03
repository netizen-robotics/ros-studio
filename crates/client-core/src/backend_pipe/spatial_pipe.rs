use super::*;
use studio_visualization::spatial;

#[derive(Clone)]
pub struct SpatialTarget {
    pub generation: Generation,
    pub subscription: SubscriptionId,
    pub topic: String,
    pub fixed_frame: String,
    pub local_generation: u64,
    pub stream: spatial::SpatialStream,
}
impl Shared {
    pub fn bind_spatial(&mut self, target: SpatialTarget) -> Result<(), String> {
        if self.spatials.len() >= MAX_SUBSCRIPTIONS
            || self
                .spatials
                .iter()
                .any(|t| t.subscription == target.subscription)
            || !self.registry.entries().iter().any(|entry| {
                entry.generation == target.generation
                    && entry.subscription == target.subscription
                    && entry.configuration.topic == target.topic
                    && entry.configuration.output == Output::SpatialScan
                    && entry
                        .configuration
                        .spatial
                        .as_ref()
                        .is_some_and(|c| c.fixed_frame == target.fixed_frame)
            })
        {
            return Err("Spatial binding does not match an allocated native group".into());
        }
        self.spatials.push(target);
        Ok(())
    }
}
pub(super) fn prepare(
    shared: &mut Shared,
    generation: Generation,
    subscription: SubscriptionId,
    sequence: u64,
    sample: &studio_protocol::spatial::Sample,
) -> Option<SpatialTarget> {
    let target = shared
        .spatials
        .iter()
        .find(|t| {
            t.generation == generation
                && t.subscription == subscription
                && t.topic == sample.scan.topic
                && t.fixed_frame == sample.fixed_frame
        })
        .cloned();
    let identity = shared.registry.entries().iter().any(|entry| {
        entry.generation == generation
            && entry.subscription == subscription
            && entry.configuration.output == Output::SpatialScan
            && entry.configuration.topic == sample.scan.topic
            && entry.configuration.message_type == sample.scan.message_type
            && entry
                .configuration
                .spatial
                .as_ref()
                .is_some_and(|c| c.fixed_frame == sample.fixed_frame)
    });
    if target.is_some()
        && identity
        && shared
            .registry
            .sample(generation, subscription, sequence)
            .is_ok()
    {
        return target;
    }
    shared.ingress_rejections = shared.ingress_rejections.saturating_add(1);
    None
}
pub(super) fn publish(
    shared: &mut Shared,
    target: &SpatialTarget,
    sequence: u64,
    backend_drops: u64,
    decoded: Result<spatial::SpatialFrame, spatial::SpatialLiveError>,
) -> bool {
    if shared.registry.entries().iter().any(|entry| {
        entry.generation == target.generation
            && entry.subscription == target.subscription
            && entry.state == State::Active
            && entry.configuration.output == Output::SpatialScan
    }) {
        return target
            .stream
            .publish(target.local_generation, sequence, backend_drops, decoded);
    }
    false
}
