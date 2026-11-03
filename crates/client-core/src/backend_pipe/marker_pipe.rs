//! Complete Marker assembly stays on the reader worker. Clear epochs and Stop
//! atomics bracket publication; UI paths never wait for decode/hash work.
use super::*;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use studio_protocol::marker_snapshot::{self as wire, Clock, Message, Owner, Received, Receiver};
mod visibility;
pub use visibility::MarkerVisibility;
#[derive(Clone, Debug)]
pub enum MarkerUpdate {
    Complete {
        revision: u64,
        state: Arc<wire::Complete>,
        visibility: Arc<MarkerVisibility>,
    },
    Cleared(Arc<wire::Clear>),
}
pub type MarkerResult = Result<MarkerUpdate, String>;
struct Mailbox {
    result: Option<MarkerResult>,
    faulted: bool,
    visibility: Option<Arc<MarkerVisibility>>,
}
struct Stream {
    owner: Owner,
    began: Instant,
    cancelled: AtomicBool,
    pending_clear: AtomicU64,
    epoch: AtomicU64,
    receiver: Mutex<(u64, Receiver)>,
    mailbox: Mutex<Mailbox>,
}
#[derive(Clone)]
pub struct MarkerStream(Arc<Stream>);
pub(super) struct Delivery {
    epoch: u64,
    result: Result<DecodedUpdate, String>,
}
enum DecodedUpdate {
    Complete {
        revision: u64,
        state: Arc<wire::Complete>,
    },
    Cleared(Arc<wire::Clear>),
}
impl MarkerStream {
    pub fn new(owner: Owner) -> Result<Self, String> {
        Ok(Self(Arc::new(Stream {
            receiver: Mutex::new((0, Receiver::new(owner.clone())?)),
            owner,
            began: Instant::now(),
            cancelled: AtomicBool::new(false),
            pending_clear: AtomicU64::new(0),
            epoch: AtomicU64::new(0),
            mailbox: Mutex::new(Mailbox {
                result: None,
                faulted: false,
                visibility: None,
            }),
        })))
    }
    /// Origin for local receiver deadlines; this does not authorize ROS time.
    pub fn monotonic_origin(&self) -> Instant {
        self.0.began
    }
    pub fn monotonic_ns(&self) -> u64 {
        self.0
            .began
            .elapsed()
            .as_nanos()
            .try_into()
            .unwrap_or(u64::MAX)
    }
    pub fn is_stopped(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire)
    }
    pub fn is_waiting_for_clear(&self) -> bool {
        self.0.pending_clear.load(Ordering::Acquire) != 0
    }
    pub fn stop(&self) {
        self.0.cancelled.store(true, Ordering::Release);
        let mut mailbox = self.0.mailbox.lock().unwrap();
        mailbox.result = None;
        mailbox.visibility = None;
        drop(mailbox);
        if let Ok(mut receiver) = self.0.receiver.try_lock() {
            receiver.1.stop();
        }
    }
    fn pause_for_clear(&self, request: RequestId) {
        self.0.pending_clear.store(request.0, Ordering::Release);
        self.0.epoch.store(request.0, Ordering::Release);
        let mut mailbox = self.0.mailbox.lock().unwrap();
        mailbox.result = None;
        mailbox.visibility = None;
    }
    fn clear_ack(&self, request: RequestId) {
        let mut mailbox = self.0.mailbox.lock().unwrap();
        if self
            .0
            .pending_clear
            .compare_exchange(request.0, 0, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
        {
            mailbox.result = None;
            mailbox.faulted = false;
            mailbox.visibility = None;
        }
    }
    pub fn take(&self) -> Option<MarkerResult> {
        let mut mailbox = self.0.mailbox.lock().unwrap();
        if self.is_stopped() || self.is_waiting_for_clear() {
            mailbox.result = None;
            return None;
        }
        mailbox.result.take()
    }
    /// Fresh, bounded metadata for rendering cached worker output. This never
    /// locks the snapshot receiver or visits geometry/asset arrays. Retaining an
    /// old observation does not replace consulting this method each UI frame.
    pub fn visibility(&self) -> Option<Arc<MarkerVisibility>> {
        let mailbox = self.0.mailbox.lock().unwrap();
        if self.is_stopped() || self.is_waiting_for_clear() || mailbox.faulted {
            None
        } else {
            mailbox.visibility.clone()
        }
    }
    fn decode(&self, message: &Message, drops: u64) -> Option<Delivery> {
        let clock = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|e| e.to_string())
            .and_then(|d| {
                u64::try_from(d.as_nanos()).map_err(|_| "Marker System clock exhausted".into())
            })
            .map(|unix_ns| Clock {
                unix_ns,
                monotonic_ns: self.monotonic_ns(),
            });
        self.decode_at(message, drops, clock)
    }
    fn decode_at(
        &self,
        message: &Message,
        drops: u64,
        clock: Result<Clock, String>,
    ) -> Option<Delivery> {
        let epoch = self.0.epoch.load(Ordering::Acquire);
        if self.is_stopped()
            || self.is_waiting_for_clear()
            || self.0.mailbox.lock().unwrap().faulted
        {
            return None;
        }
        let mut receiver = self.0.receiver.lock().unwrap();
        if receiver.0 != epoch {
            *receiver = (
                epoch,
                Receiver::new(self.0.owner.clone()).expect("validated Marker owner"),
            );
        }
        let result = if message.owner_revision().0 != &self.0.owner || drops != 0 {
            Err("Marker identity or ordered transport contradicts its owner".into())
        } else {
            clock.and_then(|clock| receiver.1.receive(message, clock))
        };
        if self.is_stopped() {
            receiver.1.stop();
            return None;
        }
        if epoch != self.0.epoch.load(Ordering::Acquire) || self.is_waiting_for_clear() {
            return None;
        }
        let result = match result {
            Ok(Received::Stale | Received::Pending) => return None,
            Ok(Received::Complete { revision, state }) => {
                Ok(DecodedUpdate::Complete { revision, state })
            }
            Ok(Received::Cleared(clear)) => Ok(DecodedUpdate::Cleared(Arc::new(clear))),
            Err(error) => {
                receiver.1.stop();
                Err(error)
            }
        };
        Some(Delivery { epoch, result })
    }
    fn publish(&self, delivery: Delivery) {
        let mut mailbox = self.0.mailbox.lock().unwrap();
        if self.is_stopped()
            || self.is_waiting_for_clear()
            || self.0.epoch.load(Ordering::Acquire) != delivery.epoch
            || mailbox.faulted
        {
            return;
        }
        let result = match delivery.result {
            Ok(DecodedUpdate::Complete { revision, state }) => {
                let visibility = Arc::new(MarkerVisibility::next(
                    mailbox.visibility.as_deref(),
                    &self.0.owner,
                    delivery.epoch,
                    revision,
                    &state,
                    self.monotonic_ns(),
                ));
                mailbox.visibility = Some(visibility.clone());
                Ok(MarkerUpdate::Complete {
                    revision,
                    state,
                    visibility,
                })
            }
            Ok(DecodedUpdate::Cleared(clear)) => {
                mailbox.visibility = None;
                Ok(MarkerUpdate::Cleared(clear))
            }
            Err(error) => {
                mailbox.faulted = true;
                mailbox.visibility = None;
                Err(error)
            }
        };
        mailbox.result = Some(result);
    }
}
#[derive(Clone)]
pub struct MarkerTarget {
    pub owner: Owner,
    pub stream: MarkerStream,
}
impl Shared {
    pub fn bind_marker(&mut self, target: MarkerTarget) -> Result<(), String> {
        if self.markers.len() >= wire::MAX_OWNERS
            || target.owner != target.stream.0.owner
            || self
                .markers
                .iter()
                .any(|t| t.owner.subscription == target.owner.subscription)
            || !self.registry.entries().iter().any(|entry| {
                entry.generation == target.owner.generation
                    && entry.subscription == target.owner.subscription
                    && entry.configuration.output == Output::MarkerSnapshot
                    && entry.configuration.topic == target.owner.topic
                    && entry.configuration.message_type == target.owner.message_type
                    && entry
                        .configuration
                        .spatial
                        .as_ref()
                        .is_some_and(|s| s.fixed_frame == target.owner.fixed_frame)
            })
        {
            return Err("Marker binding requires an exact allocated owner".into());
        }
        self.markers.push(target);
        Ok(())
    }
    pub fn clear_markers_explicit(
        &mut self,
        subscription: SubscriptionId,
    ) -> Result<studio_protocol::session::Command, String> {
        let target = self
            .markers
            .iter()
            .find(|t| {
                t.owner.generation == self.registry.generation()
                    && t.owner.subscription == subscription
                    && !t.stream.is_stopped()
            })
            .cloned()
            .ok_or("No active Marker binding")?;
        let intent = self
            .registry
            .clear_markers_explicit(subscription)
            .map_err(|e| format!("Marker Clear: {e:?}"))?;
        target.stream.pause_for_clear(intent.request);
        Ok(studio_protocol::session::Command::ClearMarkers {
            generation: intent.generation,
            request: intent.request,
            subscription: intent.subscription,
        })
    }
    /// Failed command dispatch permits a fresh explicit Clear/Stop. Keep data
    /// paused rather than resuming halfway through an old transfer.
    pub fn marker_clear_send_failed(
        &mut self,
        generation: Generation,
        subscription: SubscriptionId,
        request: RequestId,
    ) {
        let _ = self
            .registry
            .clear_markers_ack(generation, subscription, request);
    }
}
pub(super) fn prepare(
    shared: &mut Shared,
    generation: Generation,
    subscription: SubscriptionId,
    sequence: u64,
) -> Option<MarkerTarget> {
    let target = shared
        .markers
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
pub(super) fn decode(target: &MarkerTarget, message: &Message, drops: u64) -> Option<Delivery> {
    target.stream.decode(message, drops)
}
pub(super) fn publish(shared: &mut Shared, target: &MarkerTarget, delivery: Delivery) {
    if shared.registry.entries().iter().any(|e| {
        e.generation == target.owner.generation
            && e.subscription == target.owner.subscription
            && e.state == State::Active
    }) && shared
        .markers
        .iter()
        .any(|t| Arc::ptr_eq(&t.stream.0, &target.stream.0))
    {
        target.stream.publish(delivery);
    } else {
        shared.ingress_rejections = shared.ingress_rejections.saturating_add(1);
    }
}
pub(super) fn clear_ack(
    shared: &mut Shared,
    generation: Generation,
    subscription: SubscriptionId,
    request: RequestId,
) -> bool {
    if shared
        .registry
        .clear_markers_ack(generation, subscription, request)
        .is_err()
    {
        return false;
    }
    if let Some(target) = shared
        .markers
        .iter()
        .find(|t| t.owner.generation == generation && t.owner.subscription == subscription)
    {
        target.stream.clear_ack(request);
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_protocol::{
        marker::{self, store::Store, Batch, Geometry, Key, Marker, Operation},
        session::Command,
        spatial,
        spatial_input::SpatialClockPolicy,
    };
    fn setup() -> (Shared, StartIntent, MarkerTarget) {
        let mut shared = Shared::new(0);
        shared
            .ingest_for_test(Event::ready("jazzy".into(), "synthetic".into()))
            .unwrap();
        let config = Configuration {
            topic: "/markers".into(),
            message_type: marker::ARRAY_MESSAGE_TYPE.into(),
            output: Output::MarkerSnapshot,
            best_effort: false,
            transient_local: true,
            spatial: Some(spatial::Configuration {
                fixed_frame: "world".into(),
                clock_policy: SpatialClockPolicy::VerifiedSystem,
                dynamic_tf_topic: "/tf".into(),
                dynamic_tf_best_effort: false,
                static_tf_topic: "/tf_static".into(),
            }),
            robot: None,
            map_updates: None,
        };
        let start = shared.registry.start_explicit(ViewId(1), config).unwrap();
        shared
            .queue
            .register(start.generation, start.subscription)
            .unwrap();
        let owner = Owner {
            generation: start.generation,
            subscription: start.subscription,
            topic: "/markers".into(),
            message_type: marker::ARRAY_MESSAGE_TYPE.into(),
            fixed_frame: "world".into(),
        };
        let target = MarkerTarget {
            stream: MarkerStream::new(owner.clone()).unwrap(),
            owner,
        };
        shared.bind_marker(target.clone()).unwrap();
        shared
            .ingest_for_test(Event::Started {
                generation: start.generation,
                request: start.request,
                subscription: start.subscription,
            })
            .unwrap();
        (shared, start, target)
    }
    fn snapshot(target: &MarkerTarget, revision: u64, id: i32, large: bool) -> wire::Snapshot {
        let mut store = Store::default();
        store
            .start(
                target.owner.generation.0,
                target.owner.subscription.0,
                "world",
            )
            .unwrap();
        for offset in 0..if large { 4 } else { 1 } {
            let m = Marker {
                key: Key {
                    namespace: "objects".into(),
                    id: id + offset,
                },
                frame_id: "world".into(),
                stamp_ns: 10,
                pose: [0., 0., 0., 0., 0., 0., 1.],
                scale: [1.; 3],
                color: [1.; 4],
                lifetime_ns: 0,
                frame_locked: false,
                geometry: if large {
                    Geometry::Points {
                        points: vec![[0.123456789; 3]; marker::MAX_POINTS],
                        colors: vec![[0.1; 4]; marker::MAX_POINTS],
                    }
                } else {
                    Geometry::Cube
                },
            };
            store
                .apply(
                    store.token().unwrap(),
                    offset as u64 + 1,
                    Batch {
                        contract_version: marker::CONTRACT_VERSION,
                        operations: vec![Operation::Add {
                            marker: Box::new(m),
                        }],
                    },
                    0,
                )
                .unwrap();
        }
        wire::Snapshot::from_store(
            target.owner.clone(),
            revision,
            &store,
            Clock {
                unix_ns: 1,
                monotonic_ns: 0,
            },
            wire::LossSupport::Unknown,
            spatial::Counters::default(),
        )
        .unwrap()
    }
    fn ingest(shared: &mut Shared, target: &MarkerTarget, sequence: u64, message: Message) {
        shared
            .ingest_for_test(Event::MarkerSnapshotObservation {
                generation: target.owner.generation,
                subscription: target.owner.subscription,
                sequence,
                sample: Box::new(message),
                backend_drops: 0,
            })
            .unwrap();
    }
    fn ack(shared: &mut Shared, target: &MarkerTarget, request: RequestId) {
        shared
            .ingest_for_test(Event::MarkersCleared {
                generation: target.owner.generation,
                subscription: target.owner.subscription,
                request,
            })
            .unwrap();
    }
    #[test]
    fn marker_default_client_assembles_off_shared_and_takes_one_immutable_complete_mailbox() {
        let (mut shared, _, target) = setup();
        let snap = snapshot(&target, 1, 0, true);
        assert!(snap.chunk_count() > 1);
        for i in 0..snap.chunk_count() - 1 {
            ingest(
                &mut shared,
                &target,
                i as u64 + 1,
                Message::Chunk(Box::new(snap.chunk(i).unwrap())),
            );
            assert!(target.stream.take().is_none());
        }
        ingest(
            &mut shared,
            &target,
            snap.chunk_count() as u64,
            Message::Chunk(Box::new(snap.chunk(snap.chunk_count() - 1).unwrap())),
        );
        let MarkerUpdate::Complete { state, .. } = target.stream.take().unwrap().unwrap() else {
            panic!("Missing Marker state")
        };
        assert_eq!(state.entries().len(), 4);
        assert_eq!(state.entries()[0].marker.geometry.points().len(), 4096);
        assert!(target.stream.take().is_none());
    }
    #[test]
    fn clear_epoch_rejects_late_decode_and_only_exact_ack_resumes_without_releasing_resources() {
        let (mut shared, _, target) = setup();
        let snap = snapshot(&target, 1, 0, false);
        let message = Message::Chunk(Box::new(snap.chunk(0).unwrap()));
        let late = decode(&target, &message, 0).unwrap();
        let handles = shared.registry.used_resources();
        let command = shared
            .clear_markers_explicit(target.owner.subscription)
            .unwrap();
        let Command::ClearMarkers { request, .. } = command else {
            unreachable!()
        };
        publish(&mut shared, &target, late);
        assert!(target.stream.take().is_none());
        assert!(target.stream.is_waiting_for_clear());
        ack(&mut shared, &target, RequestId(request.0 + 1));
        assert!(target.stream.is_waiting_for_clear());
        ack(&mut shared, &target, request);
        assert!(!target.stream.is_waiting_for_clear());
        assert_eq!(shared.registry.used_resources(), handles);
        assert_eq!(shared.registry.entries()[0].state, State::Active);
        let fresh = snapshot(&target, 2, 99, false);
        ingest(
            &mut shared,
            &target,
            1,
            Message::Chunk(Box::new(fresh.chunk(0).unwrap())),
        );
        let MarkerUpdate::Complete { state, .. } = target.stream.take().unwrap().unwrap() else {
            panic!("Missing fresh state")
        };
        assert_eq!(state.entries()[0].marker.key.id, 99);
    }
    #[test]
    fn rejected_clear_stays_hidden_allows_explicit_retry_and_old_ack_cannot_resume_retry() {
        let (mut shared, _, target) = setup();
        let Command::ClearMarkers { request, .. } = shared
            .clear_markers_explicit(target.owner.subscription)
            .unwrap()
        else {
            unreachable!()
        };
        shared
            .ingest_for_test(Event::RequestError {
                generation: target.owner.generation,
                subscription: target.owner.subscription,
                request,
                message: "Clear lane full".into(),
                no_handle: true,
            })
            .unwrap();
        assert_eq!(shared.registry.entries().len(), 1);
        assert_eq!(shared.registry.entries()[0].state, State::Active);
        assert!(target.stream.is_waiting_for_clear());
        let Command::ClearMarkers { request: new, .. } = shared
            .clear_markers_explicit(target.owner.subscription)
            .unwrap()
        else {
            unreachable!()
        };
        assert!(new.0 > request.0);
        ack(&mut shared, &target, request);
        assert!(target.stream.is_waiting_for_clear());
        ack(&mut shared, &target, new);
        assert!(!target.stream.is_waiting_for_clear());
    }
    #[test]
    fn stopping_or_reconnecting_cancels_clear_and_rejects_old_marker_publication() {
        let (mut shared, _, target) = setup();
        let snap = snapshot(&target, 1, 0, false);
        let late = decode(
            &target,
            &Message::Chunk(Box::new(snap.chunk(0).unwrap())),
            0,
        )
        .unwrap();
        let Command::ClearMarkers { request, .. } = shared
            .clear_markers_explicit(target.owner.subscription)
            .unwrap()
        else {
            unreachable!()
        };
        let stop = shared
            .registry
            .stop_explicit(target.owner.subscription)
            .unwrap();
        ack(&mut shared, &target, request);
        assert!(target.stream.is_waiting_for_clear());
        shared
            .ingest_for_test(Event::Stopped {
                generation: stop.generation,
                request: stop.request,
                subscription: stop.subscription,
            })
            .unwrap();
        publish(&mut shared, &target, late);
        assert!(target.stream.is_stopped());
        assert!(target.stream.take().is_none());
        assert!(shared.markers.is_empty());
        shared
            .ingest_for_test(Event::ready("jazzy".into(), "synthetic".into()))
            .unwrap();
        assert!(shared.registry.entries().is_empty());
        assert!(shared.markers.is_empty());
    }
    #[test]
    fn malformed_marker_and_transport_loss_are_owner_scoped_and_stop_does_not_wait_on_receiver() {
        for bad_identity in [false, true] {
            let (mut shared, _, target) = setup();
            let snap = snapshot(&target, 1, 0, false);
            let mut message = Message::Chunk(Box::new(snap.chunk(0).unwrap()));
            if bad_identity {
                let Message::Chunk(c) = &mut message else {
                    unreachable!()
                };
                c.header.owner.topic = "/wrong".into();
            }
            let delivery = decode(&target, &message, if bad_identity { 0 } else { 1 }).unwrap();
            publish(&mut shared, &target, delivery);
            assert!(target.stream.take().unwrap().is_err());
            assert_eq!(shared.registry.entries().len(), 1);
            let guard = target.stream.0.receiver.lock().unwrap();
            target.stream.stop();
            assert!(target.stream.is_stopped());
            drop(guard);
        }
    }

    fn key(id: i32) -> Key {
        Key {
            namespace: "objects".into(),
            id,
        }
    }
    fn visible_snapshot(
        target: &MarkerTarget,
        revision: u64,
        keys: &[(i32, u64, bool)],
    ) -> wire::Snapshot {
        use studio_protocol::latest_pose::LatestPose;
        let entries: Vec<_> = keys
            .iter()
            .map(|&(id, version, ready)| wire::Entry {
                marker: Marker {
                    key: key(id),
                    frame_id: "world".into(),
                    stamp_ns: 0,
                    pose: [0., 0., 0., 0., 0., 0., 1.],
                    scale: [1.; 3],
                    color: [1.; 4],
                    lifetime_ns: 0,
                    frame_locked: true,
                    geometry: Geometry::Cube,
                },
                version,
                transform: wire::Transform::Resolved {
                    pose: if ready {
                        LatestPose::Ready {
                            resolved_stamp_ns: 0,
                            components: [0., 0., 0., 0., 0., 0., 1.],
                        }
                    } else {
                        LatestPose::Missing
                    },
                },
            })
            .collect();
        wire::Snapshot::new(
            wire::Header {
                contract_version: wire::CONTRACT_VERSION,
                owner: target.owner.clone(),
                revision,
                source_revision: keys.iter().map(|k| k.1).max().unwrap_or(0),
                captured_at_unix_ns: 1,
                manifest: entries
                    .iter()
                    .map(|e| wire::ManifestEntry {
                        key: e.marker.key.clone(),
                        version: e.version,
                        lifetime: wire::Lifetime::Forever,
                    })
                    .collect(),
                document_bytes: 1,
                sha256: "0".repeat(64),
            },
            wire::State {
                entries,
                diagnostics: vec![],
                counters: Default::default(),
                source_counters: Default::default(),
                known_history_gap: false,
                loss_support: wire::LossSupport::Unknown,
            },
        )
        .unwrap()
    }
    fn deliver_snapshot(
        shared: &mut Shared,
        target: &MarkerTarget,
        revision: u64,
        keys: &[(i32, u64, bool)],
    ) {
        let snap = visible_snapshot(target, revision, keys);
        assert_eq!(snap.chunk_count(), 1);
        ingest(
            shared,
            target,
            revision,
            Message::Chunk(Box::new(snap.chunk(0).unwrap())),
        );
    }
    #[test]
    fn visibility_retains_peers_and_revokes_coalesced_delete_readd_without_revision_starvation() {
        let (mut shared, _, target) = setup();
        deliver_snapshot(&mut shared, &target, 1, &[(1, 1, true), (2, 1, true)]);
        let MarkerUpdate::Complete {
            visibility: prepared,
            ..
        } = target.stream.take().unwrap().unwrap()
        else {
            unreachable!()
        };
        ingest(
            &mut shared,
            &target,
            2,
            Message::Prune(Box::new(wire::Prune {
                contract_version: wire::CONTRACT_VERSION,
                owner: target.owner.clone(),
                revision: 2,
                keys: vec![key(1)],
            })),
        );
        // No UI take between deletion and re-add: the mailbox loses that
        // intermediate result, whereas the visibility floor must survive it.
        deliver_snapshot(&mut shared, &target, 3, &[(1, 2, true), (2, 1, true)]);
        let current = target.stream.visibility().unwrap();
        assert!(!current.allows(&prepared, &key(1), 1, 0));
        assert!(current.allows(&prepared, &key(2), 1, 0));
        let MarkerUpdate::Complete {
            visibility: readded,
            ..
        } = target.stream.take().unwrap().unwrap()
        else {
            unreachable!()
        };
        for revision in 4..104 {
            deliver_snapshot(
                &mut shared,
                &target,
                revision,
                &[(1, revision, true), (2, 1, true)],
            );
        }
        let current = target.stream.visibility().unwrap();
        assert!(current.allows(&readded, &key(1), 2, 0));
        assert!(current.allows(&prepared, &key(2), 1, 0));
        assert!(!current.allows(&prepared, &key(1), 1, 0));
        assert!(!current.allows(&readded, &key(1), 999, 0));
        assert!(!readded.allows(&current, &key(1), 103, 0));
    }
    #[test]
    fn visibility_missing_tf_and_recovery_revoke_old_pose_at_unchanged_content_version() {
        let (mut shared, _, target) = setup();
        deliver_snapshot(&mut shared, &target, 1, &[(1, 1, true), (2, 1, true)]);
        let prepared = target.stream.visibility().unwrap();
        deliver_snapshot(&mut shared, &target, 2, &[(1, 1, false), (2, 1, true)]);
        let missing = target.stream.visibility().unwrap();
        assert!(!missing.allows(&prepared, &key(1), 1, 0));
        assert!(missing.allows(&prepared, &key(2), 1, 0));
        deliver_snapshot(&mut shared, &target, 3, &[(1, 1, true), (2, 1, true)]);
        let recovered = target.stream.visibility().unwrap();
        assert!(!recovered.allows(&prepared, &key(1), 1, 0));
        assert!(recovered.allows(&recovered, &key(1), 1, 0));
        assert!(recovered.allows(&prepared, &key(2), 1, 0));
    }
    #[test]
    fn visibility_clear_fault_stop_and_owner_identity_deny_cached_work() {
        let (mut shared, _, target) = setup();
        deliver_snapshot(&mut shared, &target, 1, &[(1, 1, true)]);
        let prepared = target.stream.visibility().unwrap();
        let Command::ClearMarkers { request, .. } = shared
            .clear_markers_explicit(target.owner.subscription)
            .unwrap()
        else {
            unreachable!()
        };
        assert!(target.stream.visibility().is_none());
        ack(&mut shared, &target, request);
        assert!(target.stream.visibility().is_none());
        // Transfer numbering can restart after the correlated Clear ack.
        deliver_snapshot(&mut shared, &target, 2, &[(1, 1, true)]);
        let current = target.stream.visibility().unwrap();
        assert!(!current.allows(&prepared, &key(1), 1, 0));
        let MarkerUpdate::Complete { state, .. } = target.stream.take().unwrap().unwrap() else {
            unreachable!()
        };
        let mut foreign_owner = target.owner.clone();
        foreign_owner.topic = "/other".into();
        let foreign = MarkerVisibility::next(None, &foreign_owner, request.0, 2, &state, 0);
        assert!(!current.allows(&foreign, &key(1), 1, 0));
        let snap = visible_snapshot(&target, 3, &[(1, 1, true)]);
        let fault = decode(
            &target,
            &Message::Chunk(Box::new(snap.chunk(0).unwrap())),
            1,
        )
        .unwrap();
        publish(&mut shared, &target, fault);
        assert!(target.stream.visibility().is_none());
        target.stream.stop();
        assert!(target.stream.visibility().is_none());
    }
    #[test]
    fn visibility_remote_clear_and_bounded_key_churn_never_resurrect_old_keys() {
        let (mut shared, _, target) = setup();
        deliver_snapshot(&mut shared, &target, 1, &[(1, 1, true)]);
        let prepared = target.stream.visibility().unwrap();
        ingest(
            &mut shared,
            &target,
            2,
            Message::Clear(Box::new(wire::Clear {
                contract_version: wire::CONTRACT_VERSION,
                owner: target.owner.clone(),
                revision: 2,
                reason: wire::ClearReason::SourceFault,
                known_history_gap: true,
            })),
        );
        assert!(target.stream.visibility().is_none());
        deliver_snapshot(&mut shared, &target, 3, &[(1, 1, true)]);
        assert!(!target
            .stream
            .visibility()
            .unwrap()
            .allows(&prepared, &key(1), 1, 0));
        for revision in 4..12 {
            let keys: Vec<_> = (0..marker::MAX_MARKERS)
                .map(|id| ((revision * 1000) as i32 + id as i32, revision, true))
                .collect();
            deliver_snapshot(&mut shared, &target, revision, &keys);
            assert_eq!(
                target.stream.visibility().unwrap().key_count(),
                marker::MAX_MARKERS
            );
        }
        deliver_snapshot(&mut shared, &target, 12, &[]);
        assert_eq!(target.stream.visibility().unwrap().key_count(), 0);
    }
    #[test]
    fn visibility_checks_exact_idle_expiry_without_receiver_lock_or_fresh_samples() {
        let (mut shared, _, target) = setup();
        let snapshot = visible_snapshot(&target, 1, &[(1, 1, true), (2, 1, true)]);
        let chunk = snapshot.chunk(0).unwrap();
        let bytes = studio_protocol::binary::decode_exact(
            &chunk.data_base64,
            snapshot.document_bytes(),
            wire::MAX_CHUNK_BYTES,
        )
        .unwrap();
        let mut state: wire::State = serde_json::from_slice(&bytes).unwrap();
        let mut header = snapshot.header().clone();
        let deadline = 10_000_000_000;
        state.entries[0].marker.lifetime_ns = deadline;
        header.manifest[0].lifetime = wire::Lifetime::Expires {
            remaining_ns: deadline,
            at_unix_ns: 1 + deadline,
        };
        let snapshot = wire::Snapshot::new(header, state).unwrap();
        let delivery = target
            .stream
            .decode_at(
                &Message::Chunk(Box::new(snapshot.chunk(0).unwrap())),
                0,
                Ok(Clock {
                    unix_ns: 1,
                    monotonic_ns: 0,
                }),
            )
            .unwrap();
        publish(&mut shared, &target, delivery);
        let prepared = target.stream.visibility().unwrap();
        let _held_receiver = target.stream.0.receiver.lock().unwrap();
        let current = target.stream.visibility().unwrap();
        assert!(current.allows(&prepared, &key(1), 1, deadline - 1));
        assert!(!current.allows(&prepared, &key(1), 1, deadline));
        assert!(!current.allows(&prepared, &key(1), 1, u64::MAX));
        assert!(current.allows(&prepared, &key(2), 1, u64::MAX));
        target.stream.stop();
        assert!(target.stream.visibility().is_none());
    }
}
