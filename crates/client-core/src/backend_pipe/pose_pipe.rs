//! Complete decoding stays off Shared/UI locks. UI Clear/Stop use revocation.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use studio_protocol::pose_snapshot::{
    self as wire,
    transport::{Applied, Configuration, Invalidation, Message, Permission, Receiver, Revocation},
    Owner, Snapshot,
};
#[derive(Clone, Debug)]
pub enum PoseUpdate {
    Snapshot {
        snapshot: Arc<Snapshot>,
        permission: Permission,
        #[cfg(feature = "spatial")]
        prepared: Arc<studio_visualization::spatial::uncertainty::Prepared>,
    },
    Invalidated(Invalidation),
}
pub type PoseResult = Result<PoseUpdate, String>;
#[derive(Default)]
struct Mailbox {
    result: Option<PoseResult>,
    faulted: bool,
}
struct Stream {
    owner: Owner,
    configuration: Configuration,
    cancelled: AtomicBool,
    pending_clear: AtomicU64,
    receiver: Mutex<Receiver>,
    revocation: Revocation,
    mailbox: Mutex<Mailbox>,
}
#[derive(Clone)]
pub struct PoseStream(Arc<Stream>);
impl PoseStream {
    pub fn new(owner: Owner, configuration: Configuration) -> Result<Self, String> {
        let receiver = Receiver::new(owner.clone(), configuration)?;
        let revocation = receiver.revocation();
        Ok(Self(Arc::new(Stream {
            owner,
            configuration,
            cancelled: AtomicBool::new(false),
            pending_clear: AtomicU64::new(0),
            receiver: Mutex::new(receiver),
            revocation,
            mailbox: Mutex::new(Mailbox::default()),
        })))
    }
    pub fn is_stopped(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }
    pub fn is_waiting_for_clear(&self) -> bool {
        self.0.pending_clear.load(Ordering::Acquire) != 0
    }
    pub fn stop(&self) {
        self.0.cancelled.store(true, Ordering::Release);
        self.0.revocation.revoke();
        self.0.mailbox.lock().unwrap().result = None;
        if let Ok(mut receiver) = self.0.receiver.try_lock() {
            receiver.stop();
        }
    }
    pub fn take(&self) -> Option<PoseResult> {
        let mut mailbox = self.0.mailbox.lock().unwrap();
        if self.is_stopped() || self.is_waiting_for_clear() {
            mailbox.result = None;
            return None;
        }
        mailbox.result.take()
    }
    fn pause(&self, request: RequestId) {
        self.0.pending_clear.store(request.0, Ordering::Release);
        self.0.revocation.revoke();
        self.0.mailbox.lock().unwrap().result = None;
    }
    fn synchronize_clear(&self, receiver: &mut Receiver) -> Result<(), String> {
        let request = self.0.pending_clear.load(Ordering::Acquire);
        if request != 0 && receiver.pending_clear() != Some(RequestId(request)) {
            receiver.begin_clear(RequestId(request))?;
        }
        Ok(())
    }
    fn decode(&self, message: &Message, drops: u64) -> Option<PoseResult> {
        if self.0.mailbox.lock().unwrap().faulted {
            return None;
        }
        let mut receiver = self.0.receiver.lock().unwrap();
        if self.is_stopped() {
            receiver.stop();
            return None;
        }
        let mut result: Result<Option<PoseUpdate>, String> = (|| {
            self.synchronize_clear(&mut receiver)?;
            if message.owner() != &self.0.owner || drops != 0 {
                return Err("Pose owner or ordered delivery contradicts binding".into());
            }
            let _ = message.encode()?;
            match message {
                Message::Snapshot(snapshot) => match receiver.apply((**snapshot).clone())? {
                    Applied::Updated => {
                        let snapshot = receiver.snapshot().unwrap().clone();
                        #[cfg(feature = "spatial")]
                        let prepared =
                            Arc::new(studio_visualization::spatial::uncertainty::Prepared::new(
                                snapshot.clone(),
                            )?);
                        Ok(Some(PoseUpdate::Snapshot {
                            snapshot,
                            permission: receiver.permission().unwrap(),
                            #[cfg(feature = "spatial")]
                            prepared,
                        }))
                    }
                    _ => Ok(None),
                },
                Message::Invalidated(barrier) => match receiver.invalidate(barrier)? {
                    Applied::Updated => Ok(Some(PoseUpdate::Invalidated(barrier.clone()))),
                    _ => Ok(None),
                },
            }
        })();
        if self.is_stopped() {
            receiver.stop();
            return None;
        }
        if self.is_waiting_for_clear() {
            self.0.revocation.revoke();
            if result.is_ok() {
                return None;
            }
        }
        if result.is_err() {
            receiver.stop();
            self.0.revocation.revoke();
        }
        match result.as_mut() {
            Ok(update) => update.take().map(Ok),
            Err(error) => Some(Err(error.clone())),
        }
    }
    fn publish(&self, result: PoseResult) {
        let mut mailbox = self.0.mailbox.lock().unwrap();
        if self.is_stopped() || self.is_waiting_for_clear() || mailbox.faulted {
            return;
        }
        mailbox.faulted = result.is_err();
        mailbox.result = Some(result);
    }
    fn acknowledge(&self, request: RequestId, barrier: &Invalidation) -> Result<bool, String> {
        if self.is_stopped() || self.0.pending_clear.load(Ordering::Acquire) != request.0 {
            return Ok(false);
        }
        let mut receiver = self.0.receiver.lock().unwrap();
        self.synchronize_clear(&mut receiver)?;
        let accepted = receiver.acknowledge_clear(request, barrier)?;
        if accepted {
            self.0
                .pending_clear
                .compare_exchange(request.0, 0, Ordering::AcqRel, Ordering::Acquire)
                .map_err(|_| "Pose Clear retired during acknowledgment")?;
            self.0.mailbox.lock().unwrap().result = None;
        }
        Ok(accepted)
    }
    fn clear_failed(&self) {
        self.0.revocation.revoke();
        self.0.pending_clear.store(0, Ordering::Release);
        let mut mailbox = self.0.mailbox.lock().unwrap();
        mailbox.faulted = true;
        mailbox.result = Some(Err(
            "Pose Clear failed; explicitly Stop and restart the display".into(),
        ));
    }
}
#[derive(Clone)]
pub struct PoseTarget {
    pub owner: Owner,
    pub configuration: Configuration,
    pub stream: PoseStream,
}
impl Shared {
    pub fn bind_pose(&mut self, target: PoseTarget) -> Result<(), String> {
        if self.poses.len() >= wire::MAX_OWNERS
            || target.owner != target.stream.0.owner
            || target.configuration != target.stream.0.configuration
            || self.poses.iter().any(|t| {
                t.owner.subscription == target.owner.subscription
                    || t.owner.kind == target.owner.kind
            })
            || !self.registry.entries().iter().any(|entry| {
                entry.generation == target.owner.generation
                    && entry.subscription == target.owner.subscription
                    && entry.configuration.output == Output::PoseSnapshot(target.configuration)
                    && entry.configuration.topic == target.owner.topic
                    && entry
                        .configuration
                        .spatial
                        .as_ref()
                        .is_some_and(|s| s.fixed_frame == target.owner.fixed_frame)
            })
        {
            return Err("Pose binding needs an exact allocated owner".into());
        }
        self.poses.push(target);
        Ok(())
    }
    pub fn clear_pose_explicit(
        &mut self,
        subscription: SubscriptionId,
    ) -> Result<studio_protocol::session::Command, String> {
        let target = self
            .poses
            .iter()
            .find(|t| {
                t.owner.generation == self.registry.generation()
                    && t.owner.subscription == subscription
                    && !t.stream.is_stopped()
                    && !t.stream.0.mailbox.lock().unwrap().faulted
            })
            .ok_or("No active pose binding")?
            .clone();
        let intent = self
            .registry
            .clear_pose_explicit(subscription)
            .map_err(|e| format!("Pose Clear: {e:?}"))?;
        target.stream.pause(intent.request);
        Ok(studio_protocol::session::Command::ClearPose {
            generation: intent.generation,
            request: intent.request,
            subscription: intent.subscription,
        })
    }
    pub fn stop_pose_explicit(
        &mut self,
        subscription: SubscriptionId,
    ) -> Result<studio_protocol::session::Command, String> {
        if !self.poses.iter().any(|t| {
            t.owner.generation == self.registry.generation() && t.owner.subscription == subscription
        }) {
            return Err("No allocated pose binding".into());
        }
        let intent = self
            .registry
            .stop_explicit(subscription)
            .map_err(|e| format!("Pose Stop: {e:?}"))?;
        for target in &self.poses {
            if target.owner.subscription == subscription {
                target.stream.stop();
            }
        }
        Ok(studio_protocol::session::Command::Stop {
            generation: intent.generation,
            request: intent.request,
            subscription: intent.subscription,
        })
    }
    pub fn pose_clear_send_failed(
        &mut self,
        generation: Generation,
        subscription: SubscriptionId,
        request: RequestId,
    ) {
        if self
            .registry
            .clear_pose_ack(generation, subscription, request)
            .is_ok()
        {
            if let Some(target) = self
                .poses
                .iter()
                .find(|t| t.owner.generation == generation && t.owner.subscription == subscription)
            {
                target.stream.clear_failed();
            }
        }
    }
}
pub(super) fn prepare(
    shared: &mut Shared,
    generation: Generation,
    subscription: SubscriptionId,
    sequence: u64,
) -> Option<PoseTarget> {
    let target = shared
        .poses
        .iter()
        .find(|t| t.owner.generation == generation && t.owner.subscription == subscription)?
        .clone();
    if shared
        .registry
        .sample(generation, subscription, sequence)
        .is_err()
    {
        shared.ingress_rejections = shared.ingress_rejections.saturating_add(1);
        return None;
    }
    Some(target)
}
pub(super) fn decode(target: &PoseTarget, message: &Message, drops: u64) -> Option<PoseResult> {
    target.stream.decode(message, drops)
}
pub(super) fn publish(shared: &mut Shared, target: &PoseTarget, result: PoseResult) {
    if shared.registry.entries().iter().any(|e| {
        e.generation == target.owner.generation
            && e.subscription == target.owner.subscription
            && e.state == State::Active
    }) && shared
        .poses
        .iter()
        .any(|t| Arc::ptr_eq(&t.stream.0, &target.stream.0))
    {
        target.stream.publish(result);
    } else {
        target.stream.stop();
        shared.ingress_rejections = shared.ingress_rejections.saturating_add(1);
    }
}
pub(super) fn clear_ack(
    shared: &mut Shared,
    generation: Generation,
    subscription: SubscriptionId,
    request: RequestId,
    barrier: &Invalidation,
) -> bool {
    if !shared.registry.entries().iter().any(|e| {
        e.generation == generation
            && e.subscription == subscription
            && e.state == State::Active
            && e.clear_request == Some(request)
    }) {
        return false;
    }
    let Some(target) = shared
        .poses
        .iter()
        .find(|t| t.owner.generation == generation && t.owner.subscription == subscription)
    else {
        return false;
    };
    match target.stream.acknowledge(request, barrier) {
        Ok(true) => shared
            .registry
            .clear_pose_ack(generation, subscription, request)
            .is_ok(),
        Ok(false) => false,
        Err(_) => {
            target.stream.clear_failed();
            let _ = shared
                .registry
                .clear_pose_ack(generation, subscription, request);
            true
        }
    }
}
#[cfg(test)]
mod tests;
