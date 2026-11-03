//! Session owner/sequence checks enclose off-Shared Camera preparation.
use super::*;
use crate::camera_stream::{CameraResult, CameraStream};
use studio_protocol::camera_snapshot::{transport::Message, Owner, MAX_OWNERS};
#[derive(Clone)]
pub struct CameraTarget {
    pub owner: Owner,
    pub stream: CameraStream,
}
impl Shared {
    pub fn bind_camera(&mut self, target: CameraTarget) -> Result<(), String> {
        if self.cameras.len()>=MAX_OWNERS || target.stream.owner()!=&target.owner
            || target.stream.is_stopped()
            || !self.registry.entries().iter().any(|entry|
                entry.generation==target.owner.generation && entry.subscription==target.owner.subscription
                && matches!(&entry.configuration.output,Output::CameraSnapshot(info) if **info==target.owner.info)
                && entry.configuration.topic==target.owner.topic
                && entry.configuration.spatial.as_ref().is_some_and(|s|s.fixed_frame==target.owner.fixed_frame)) {
            return Err("Camera binding requires the exact allocated paired-source owner".into());
        }
        self.cameras.push(target);
        Ok(())
    }
    pub fn stop_camera(&mut self, subscription: SubscriptionId) {
        for target in &self.cameras {
            if target.owner.generation == self.registry.generation()
                && target.owner.subscription == subscription
            {
                target.stream.stop();
            }
        }
    }
}
pub(super) fn prepare(
    shared: &mut Shared,
    generation: Generation,
    subscription: SubscriptionId,
    sequence: u64,
) -> Option<CameraTarget> {
    let target = shared
        .cameras
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
pub(super) fn decode(target: &CameraTarget, message: Message, drops: u64) -> Option<CameraResult> {
    target.stream.decode(message, drops)
}
pub(super) fn publish(shared: &mut Shared, target: &CameraTarget, result: CameraResult) {
    if shared.registry.entries().iter().any(|e| {
        e.generation == target.owner.generation
            && e.subscription == target.owner.subscription
            && e.state == State::Active
    }) && shared
        .cameras
        .iter()
        .any(|t| t.stream.same_stream(&target.stream))
    {
        target.stream.publish_result(result);
    } else {
        target.stream.stop();
        shared.ingress_rejections = shared.ingress_rejections.saturating_add(1);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use studio_protocol::{
        camera_snapshot::{Counters, InfoSource, Snapshot, Status},
        session::Event,
        spatial,
        spatial_input::SpatialClockPolicy,
    };
    fn setup() -> (Shared, CameraTarget) {
        let mut shared = Shared::new(0);
        shared
            .ingest_for_test(Event::ready("jazzy".into(), "fixture".into()))
            .unwrap();
        let info = InfoSource {
            topic: "/camera/camera_info".into(),
            best_effort: true,
            transient_local: false,
        };
        let start = shared
            .registry
            .start_explicit(
                ViewId(1),
                Configuration {
                    topic: "/camera/image_rect".into(),
                    message_type: studio_protocol::image::MESSAGE_TYPE.into(),
                    best_effort: false,
                    transient_local: false,
                    output: Output::CameraSnapshot(Box::new(info.clone())),
                    spatial: Some(spatial::Configuration {
                        fixed_frame: "world".into(),
                        clock_policy: SpatialClockPolicy::VerifiedSystem,
                        dynamic_tf_topic: "/tf".into(),
                        dynamic_tf_best_effort: false,
                        static_tf_topic: "/tf_static".into(),
                    }),
                    robot: None,
                    map_updates: None,
                },
            )
            .unwrap();
        shared
            .queue
            .register(start.generation, start.subscription)
            .unwrap();
        let owner = Owner {
            generation: start.generation,
            subscription: start.subscription,
            topic: start.configuration.topic,
            info,
            fixed_frame: "world".into(),
        };
        let target = CameraTarget {
            stream: CameraStream::new(owner.clone()).unwrap(),
            owner,
        };
        shared.bind_camera(target.clone()).unwrap();
        shared
            .ingest_for_test(Event::Started {
                generation: start.generation,
                subscription: start.subscription,
                request: start.request,
            })
            .unwrap();
        (shared, target)
    }
    fn message(target: &CameraTarget, revision: u64) -> Message {
        Message::Snapshot(Box::new(Snapshot {
            contract_version: studio_protocol::camera_snapshot::CONTRACT_VERSION,
            owner: target.owner.clone(),
            revision,
            continuity_epoch: 1,
            status: Status::WaitingInfo,
            diagnostic: None,
            counters: Counters::default(),
            frame: None,
        }))
    }
    fn event(target: &CameraTarget, sequence: u64) -> Event {
        Event::CameraSnapshotObservation {
            generation: target.owner.generation,
            subscription: target.owner.subscription,
            sequence,
            sample: Box::new(message(target, sequence)),
            backend_drops: 0,
        }
    }
    #[test]
    fn paired_binding_and_ingress_sequence_require_exact_allocated_configuration() {
        let (mut shared, target) = setup();
        assert!(shared.bind_camera(target.clone()).is_err());
        shared.ingest_for_test(event(&target, 1)).unwrap();
        assert!(target.stream.take().unwrap().is_ok());
        shared.ingest_for_test(event(&target, 1)).unwrap();
        assert!(target.stream.take().is_none());
        assert_eq!(shared.ingress_rejections, 1);
        shared.stop_camera(target.owner.subscription);
        shared.ingest_for_test(event(&target, 2)).unwrap();
        assert!(target.stream.take().is_none());
    }
    #[test]
    fn stopping_during_preparation_prevents_publication_and_ordered_loss_is_scoped() {
        let (mut shared, target) = setup();
        let prepared = prepare(
            &mut shared,
            target.owner.generation,
            target.owner.subscription,
            1,
        )
        .unwrap();
        let result = decode(&prepared, message(&target, 1), 0).unwrap();
        shared
            .registry
            .stop_explicit(target.owner.subscription)
            .unwrap();
        publish(&mut shared, &prepared, result);
        assert!(target.stream.is_stopped());
        assert!(target.stream.take().is_none());
        let (mut shared, target) = setup();
        let mut dropped = event(&target, 1);
        if let Event::CameraSnapshotObservation { backend_drops, .. } = &mut dropped {
            *backend_drops = 1;
        }
        shared.ingest_for_test(dropped).unwrap();
        assert!(target.stream.take().unwrap().is_err());
        assert!(shared
            .registry
            .entries()
            .iter()
            .any(|e| e.state == State::Active));
    }
    #[test]
    fn byte_reader_eof_retires_camera_before_terminal_notification_without_releasing_registry_early(
    ) {
        let (shared, target) = setup();
        target.stream.ingest(message(&target, 1));
        let shared = Arc::new(Mutex::new(shared));
        let (tx, rx) = mpsc::sync_channel(1);
        let reader =
            super::super::spawn(std::io::Cursor::new(Vec::<u8>::new()), shared.clone(), tx);
        let terminal = rx.recv_timeout(std::time::Duration::from_secs(2)).unwrap();
        reader.join().unwrap();
        assert!(terminal.contains("closed"));
        assert!(target.stream.is_stopped());
        assert!(target.stream.take().is_none());
        assert_eq!(shared.lock().unwrap().registry.used_resources(), 4);
        target.stream.ingest(message(&target, 2));
        assert!(target.stream.take().is_none());
    }
}
