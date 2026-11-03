//! Hash/reassembly stays on the reader worker; only final mailbox publication
//! takes Shared. One partial map and one completed mailbox are retained.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use studio_protocol::{
    map_grid::Continuity,
    map_snapshot::{Clear, Complete, Message, Owner, Received, Receiver},
};

#[derive(Clone, Debug)]
pub enum MapUpdate {
    Complete(Arc<Complete>),
    Cleared(Arc<Clear>),
}
pub type MapResult = Result<MapUpdate, String>;
/// Spatial consumers take the worker-prepared map from the same one-shot
/// mailbox as protocol-only consumers. Taking either view consumes both.
#[cfg(feature = "spatial")]
#[derive(Clone, Debug)]
pub enum PreparedMapUpdate {
    Complete(Arc<studio_visualization::spatial::map_texture::TextureMap>),
    Cleared(Arc<Clear>),
}
pub(super) struct Delivery {
    result: MapResult,
    #[cfg(feature = "spatial")]
    prepared: Option<Arc<studio_visualization::spatial::map_texture::TextureMap>>,
}
impl Delivery {
    /// Called by decoding on the reader worker, before reacquiring Shared.
    fn new(result: MapResult) -> Self {
        #[cfg(feature = "spatial")]
        {
            let mut result = result;
            let prepared = if let Ok(MapUpdate::Complete(complete)) = &result {
                match studio_visualization::spatial::map_texture::TextureMap::prepare(Arc::clone(
                    complete,
                )) {
                    Ok(map) => Some(Arc::new(map)),
                    Err(error) => {
                        result = Err(error.to_string());
                        None
                    }
                }
            } else {
                None
            };
            Self { result, prepared }
        }
        #[cfg(not(feature = "spatial"))]
        Self { result }
    }
}
#[derive(Default)]
struct Mailbox {
    revision: u64,
    faulted: bool,
    result: Option<Delivery>,
}
struct Stream {
    owner: Owner,
    updates: bool,
    cancelled: AtomicBool,
    receiver: Mutex<Receiver>,
    mailbox: Mutex<Mailbox>,
}
#[derive(Clone)]
pub struct MapStream(Arc<Stream>);
impl MapStream {
    pub fn is_stopped(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }
    pub fn new(owner: Owner, updates: bool) -> Result<Self, String> {
        Ok(Self(Arc::new(Stream {
            receiver: Mutex::new(Receiver::new(owner.clone())?),
            owner,
            updates,
            cancelled: AtomicBool::new(false),
            mailbox: Mutex::new(Mailbox::default()),
        })))
    }
    pub fn stop(&self) {
        self.0.cancelled.store(true, Ordering::Release);
        self.0.mailbox.lock().unwrap().result = None;
        // Never wait on worker hashing from an active UI frame.
        if let Ok(mut receiver) = self.0.receiver.try_lock() {
            receiver.stop();
        }
    }
    pub fn take(&self) -> Option<MapResult> {
        let mut mailbox = self.0.mailbox.lock().unwrap();
        if self.0.cancelled.load(Ordering::Acquire) {
            mailbox.result = None;
            return None;
        }
        mailbox.result.take().map(|delivery| delivery.result)
    }
    #[cfg(feature = "spatial")]
    pub fn take_prepared(&self) -> Option<Result<PreparedMapUpdate, String>> {
        let mut mailbox = self.0.mailbox.lock().unwrap();
        if self.0.cancelled.load(Ordering::Acquire) {
            mailbox.result = None;
            return None;
        }
        mailbox.result.take().map(|delivery| match delivery.result {
            Ok(MapUpdate::Complete(_)) => Ok(PreparedMapUpdate::Complete(
                delivery.prepared.expect("worker prepares complete maps"),
            )),
            Ok(MapUpdate::Cleared(clear)) => Ok(PreparedMapUpdate::Cleared(clear)),
            Err(error) => Err(error),
        })
    }
    fn decode(&self, message: &Message, backend_drops: u64) -> Option<Delivery> {
        let mut receiver = self.0.receiver.lock().unwrap();
        if self.0.cancelled.load(Ordering::Acquire) {
            receiver.stop();
            return None;
        }
        let (owner, status) = match message {
            Message::Chunk(c) => (&c.header.owner, &c.header.source_status),
            Message::Clear(c) => (&c.owner, &c.source_status),
        };
        let result = if owner != &self.0.owner
            || (status.continuity != Continuity::Disabled) != self.0.updates
            || backend_drops != 0
        {
            Err(
                "Map identity, configured updates or ordered transport contradicts its owner"
                    .into(),
            )
        } else {
            receiver.receive(message)
        };
        if self.0.cancelled.load(Ordering::Acquire) {
            receiver.stop();
            return None;
        }
        let result = match result {
            Ok(Received::Stale | Received::Pending) => None,
            Ok(Received::Complete(c)) => Some(Ok(MapUpdate::Complete(Arc::from(c)))),
            Ok(Received::Cleared(c)) => Some(Ok(MapUpdate::Cleared(Arc::from(c)))),
            Err(error) => {
                receiver.stop();
                Some(Err(error))
            }
        };
        result.map(Delivery::new)
    }
    fn publish(&self, delivery: Delivery) -> bool {
        let mut mailbox = self.0.mailbox.lock().unwrap();
        if self.0.cancelled.load(Ordering::Acquire) || mailbox.faulted {
            return false;
        }
        match &delivery.result {
            Err(_) => mailbox.faulted = true,
            Ok(update) => {
                let revision = match update {
                    MapUpdate::Complete(c) => c.header.revision,
                    MapUpdate::Cleared(c) => c.revision,
                };
                if revision <= mailbox.revision {
                    return false;
                }
                mailbox.revision = revision;
            }
        }
        mailbox.result = Some(delivery);
        true
    }
}
#[derive(Clone)]
pub struct MapTarget {
    pub owner: Owner,
    pub stream: MapStream,
}
impl Shared {
    pub fn bind_map(&mut self, target: MapTarget) -> Result<(), String> {
        if self.maps.len() >= studio_protocol::mapping::MAX_MAP_LAYERS
            || self.maps.iter().any(|map| {
                map.owner.generation == target.owner.generation
                    && map.owner.subscription == target.owner.subscription
            })
            || target.owner != target.stream.0.owner
            || !self.registry.entries().iter().any(|entry| {
                entry.generation == target.owner.generation
                    && entry.subscription == target.owner.subscription
                    && entry.configuration.output == Output::MapSnapshot
                    && entry.configuration.topic == target.owner.topic
                    && entry.configuration.map_updates.is_some() == target.stream.0.updates
                    && entry
                        .configuration
                        .spatial
                        .as_ref()
                        .is_some_and(|s| s.fixed_frame == target.owner.fixed_frame)
            })
        {
            return Err(
                "Map binding requires a unique allocated owner within the three-map limit".into(),
            );
        }
        self.maps.push(target);
        Ok(())
    }
}
pub(super) fn prepare(
    shared: &mut Shared,
    generation: Generation,
    subscription: SubscriptionId,
    sequence: u64,
) -> Option<MapTarget> {
    let target = shared
        .maps
        .iter()
        .find(|target| {
            target.owner.generation == generation && target.owner.subscription == subscription
        })?
        .clone();
    if shared
        .map_faults
        .iter()
        .any(|f| f.generation == generation && f.subscription == subscription)
        || shared
            .registry
            .sample(generation, subscription, sequence)
            .is_err()
    {
        shared.ingress_rejections = shared.ingress_rejections.saturating_add(1);
        return None;
    }
    Some(target)
}
pub(super) fn decode(target: &MapTarget, message: &Message, drops: u64) -> Option<Delivery> {
    target.stream.decode(message, drops)
}
pub(super) fn publish(shared: &mut Shared, target: &MapTarget, result: Delivery) {
    let active = shared.registry.entries().iter().any(|e| {
        e.generation == target.owner.generation
            && e.subscription == target.owner.subscription
            && e.state == State::Active
    }) && shared
        .maps
        .iter()
        .any(|t| Arc::ptr_eq(&t.stream.0, &target.stream.0));
    if !active {
        shared.ingress_rejections = shared.ingress_rejections.saturating_add(1);
        return;
    }
    let fault = result.result.as_ref().err().cloned();
    if target.stream.publish(result) {
        if let Some(message) = fault {
            shared.map_faults.push(RobotIngressFault {
                generation: target.owner.generation,
                subscription: target.owner.subscription,
                message,
            });
        }
    }
}

#[cfg(test)]
mod tests;
