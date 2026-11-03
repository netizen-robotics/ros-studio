//! Camera worker preparation and one-slot client delivery. Session15 does not
//! route this stream yet. No decoding, waiting or ROS work belongs on UI frames.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use studio_images::images::{self, TypedImage};
use studio_protocol::camera_snapshot::{
    transport::{Invalidation, Message, Permission, Receiver, Revocation},
    Owner, Snapshot, Status,
};

#[derive(Clone, Debug)]
pub enum CameraUpdate {
    Snapshot {
        snapshot: Arc<Snapshot>,
        permission: Permission,
        image: Option<Arc<TypedImage>>,
        #[cfg(feature = "spatial")]
        projection: Option<studio_visualization::spatial::calibrated_camera::RectifiedProjection>,
    },
    Invalidated(Invalidation),
}
impl CameraUpdate {
    pub fn is_receiving(&self) -> bool {
        match self {
            Self::Snapshot {
                snapshot,
                permission,
                image,
                ..
            } => {
                snapshot.status == Status::Receiving
                    && image.is_some()
                    && permission.authorizes(snapshot)
            }
            Self::Invalidated(_) => false,
        }
    }
}
pub type CameraResult = Result<CameraUpdate, String>;
#[derive(Default)]
struct Mailbox {
    pending: Option<CameraResult>,
    coalesced: u64,
    revision: u64,
    epoch: u64,
    snapshot: bool,
}
struct Stream {
    owner: Owner,
    cancelled: AtomicBool,
    faulted: AtomicBool,
    ingress: Mutex<()>,
    receiver: Mutex<Receiver>,
    revocation: Revocation,
    mailbox: Mutex<Mailbox>,
}
#[derive(Clone)]
pub struct CameraStream(Arc<Stream>);
impl CameraStream {
    pub fn new(owner: Owner) -> Result<Self, String> {
        let receiver = Receiver::new(owner.clone())?;
        let revocation = receiver.revocation();
        Ok(Self(Arc::new(Stream {
            owner,
            cancelled: AtomicBool::new(false),
            faulted: AtomicBool::new(false),
            ingress: Mutex::new(()),
            receiver: Mutex::new(receiver),
            revocation,
            mailbox: Mutex::new(Mailbox::default()),
        })))
    }
    pub fn is_stopped(&self) -> bool {
        self.0.cancelled.load(Ordering::Acquire) || self.0.faulted.load(Ordering::Acquire)
    }
    /// Retires even a frame whose conversion is currently running. Never waits
    /// for the ingress/preparation mutex or a backend process on the UI thread.
    pub fn stop(&self) {
        self.0.cancelled.store(true, Ordering::Release);
        self.0.revocation.retire();
        self.0.mailbox.lock().unwrap().pending = None;
        if let Ok(mut receiver) = self.0.receiver.try_lock() {
            receiver.stop();
        }
    }
    pub fn take(&self) -> Option<CameraResult> {
        let mut mailbox = self.0.mailbox.lock().unwrap();
        if self.0.cancelled.load(Ordering::Acquire) {
            mailbox.pending = None;
            return None;
        }
        mailbox.pending.take()
    }
    pub fn coalesced(&self) -> u64 {
        self.0.mailbox.lock().unwrap().coalesced
    }
    /// Exact generation/source binding for the ingress adapter.
    pub fn owner(&self) -> &Owner {
        &self.0.owner
    }
    pub(crate) fn same_stream(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.0, &other.0)
    }
    /// Call on a transport/preparation worker. Concurrent offers are serialized;
    /// only the bounded decoded result is published to the UI mailbox.
    pub fn ingest(&self, message: Message) {
        if let Some(result) = self.decode(message, 0) {
            self.publish_result(result);
        }
    }
    pub(crate) fn decode(&self, message: Message, drops: u64) -> Option<CameraResult> {
        let _ingress = self.0.ingress.lock().unwrap();
        if self.is_stopped() {
            return None;
        }
        let result = if drops != 0 {
            Err("Camera ordered delivery lost a frame or invalidation".into())
        } else {
            self.prepare(message)
        };
        match result {
            Ok(update) => update.map(Ok),
            Err(error) => {
                self.0.faulted.store(true, Ordering::Release);
                self.0.revocation.retire();
                self.0.receiver.lock().unwrap().stop();
                Some(Err(error))
            }
        }
    }
    pub(crate) fn publish_result(&self, result: CameraResult) {
        match result {
            Ok(update) => self.publish(update),
            Err(error) => {
                let mut mailbox = self.0.mailbox.lock().unwrap();
                if !self.0.cancelled.load(Ordering::Acquire) {
                    if mailbox.pending.is_some() {
                        mailbox.coalesced = mailbox.coalesced.saturating_add(1);
                    }
                    mailbox.pending = Some(Err(error));
                }
            }
        }
    }
    fn prepare(&self, message: Message) -> Result<Option<CameraUpdate>, String> {
        if self.is_stopped() {
            return Ok(None);
        }
        if message.owner() != &self.0.owner {
            return Err("Camera data contradicts client owner binding".into());
        }
        message.encode()?;
        match message {
            Message::Invalidated(barrier) => {
                if self.0.receiver.lock().unwrap().invalidate(&barrier)? {
                    Ok(Some(CameraUpdate::Invalidated(barrier)))
                } else {
                    Ok(None)
                }
            }
            Message::Snapshot(snapshot) => {
                let Some(accepted) = self.0.receiver.lock().unwrap().apply(*snapshot)? else {
                    return Ok(None);
                };
                let image = accepted
                    .snapshot
                    .frame
                    .as_ref()
                    .map(|frame| {
                        images::decode_compact_image(&frame.image)
                            .map(Arc::new)
                            .map_err(|e| e.to_string())
                    })
                    .transpose()?;
                #[cfg(feature = "spatial")]
                let projection = accepted
                    .snapshot
                    .frame
                    .as_ref()
                    .map(|frame| {
                        let effective = frame.calibration.effective()?;
                        studio_visualization::spatial::calibrated_camera::RectifiedProjection::new(
                            effective.size,
                            effective.p,
                            0.05,
                            10000.,
                        )
                    })
                    .transpose()?;
                if self.is_stopped() || !accepted.permission.authorizes(&accepted.snapshot) {
                    return Ok(None);
                }
                Ok(Some(CameraUpdate::Snapshot {
                    snapshot: accepted.snapshot,
                    permission: accepted.permission,
                    image,
                    #[cfg(feature = "spatial")]
                    projection,
                }))
            }
        }
    }
    fn publish(&self, update: CameraUpdate) {
        let mut mailbox = self.0.mailbox.lock().unwrap();
        if self.is_stopped() {
            return;
        }
        if let CameraUpdate::Snapshot {
            snapshot,
            permission,
            ..
        } = &update
        {
            if !permission.authorizes(snapshot) {
                return;
            }
        }
        let (revision, epoch, snapshot) = match &update {
            CameraUpdate::Snapshot { snapshot, .. } => {
                (snapshot.revision, snapshot.continuity_epoch, true)
            }
            CameraUpdate::Invalidated(barrier) => {
                (barrier.revision, barrier.continuity_epoch, false)
            }
        };
        if epoch < mailbox.epoch
            || revision < mailbox.revision
            || (revision == mailbox.revision && mailbox.snapshot)
        {
            return;
        }
        mailbox.revision = revision;
        mailbox.epoch = epoch;
        mailbox.snapshot = snapshot;
        if mailbox.pending.is_some() {
            mailbox.coalesced = mailbox.coalesced.saturating_add(1);
        }
        mailbox.pending = Some(Ok(update));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_protocol::{
        camera_info::Calibration,
        camera_snapshot::{store::Store, InfoSource},
        image::{HeaderStamp, ImageMetadata, ImageSample},
        latest_pose::LatestPose,
        subscriptions::{Generation, SubscriptionId},
    };
    fn owner(id: u64) -> Owner {
        Owner {
            generation: Generation(1),
            subscription: SubscriptionId(id),
            topic: "/camera/image_rect".into(),
            info: InfoSource {
                topic: "/camera/camera_info".into(),
                best_effort: true,
                transient_local: false,
            },
            fixed_frame: "world".into(),
        }
    }
    fn fixture(id: u64) -> Store {
        let cal:Calibration=serde_json::from_value(serde_json::json!({"stamp_ns":1,"frame_id":"optical","width":2,"height":2,
            "distortion_model":"plumb_bob","d":[],"k":[100.,0.,1.,0.,100.,1.,0.,0.,1.],
            "r":[1.,0.,0.,0.,1.,0.,0.,0.,1.],"p":[100.,0.,1.,0.,0.,100.,1.,0.,0.,0.,1.,0.],
            "binning_x":0,"binning_y":0,"roi":{"x_offset":0,"y_offset":0,"width":0,"height":0,"do_rectify":false}})).unwrap();
        let mut store = Store::new(owner(id)).unwrap();
        store.info(cal).unwrap();
        offer_image(&mut store, 0);
        store
    }
    fn offer_image(store: &mut Store, now: u64) {
        let image = ImageSample::from_raw(
            "/camera/image_rect",
            ImageMetadata {
                header_stamp: HeaderStamp {
                    seconds: 1,
                    nanoseconds: 0,
                },
                frame_id: "optical".into(),
                width: 2,
                height: 2,
                encoding: "rgba8".into(),
                is_bigendian: false,
                step: 8,
            },
            &[
                255, 0, 0, 255, 0, 255, 0, 192, 0, 0, 255, 128, 255, 255, 255, 64,
            ],
        )
        .unwrap();
        store.image(image, now).unwrap();
        let request = store.request(now).unwrap().unwrap();
        store
            .complete(
                &request,
                LatestPose::Ready {
                    resolved_stamp_ns: 1_000_000_000,
                    components: [0., 0., 0., 0., 0., 0., 1.],
                },
                now,
            )
            .unwrap();
    }
    fn message(store: &Store) -> Message {
        Message::Snapshot(Box::new(store.snapshot().unwrap()))
    }
    #[test]
    fn image_preparation_coalesces_latest_and_control_revokes_cached_output() {
        let mut store = fixture(1);
        let stream = CameraStream::new(owner(1)).unwrap();
        stream.ingest(message(&store));
        offer_image(&mut store, 1);
        stream.ingest(message(&store));
        assert_eq!(stream.coalesced(), 1);
        let current = stream.take().unwrap().unwrap();
        assert!(current.is_receiving());
        if let CameraUpdate::Snapshot {
            snapshot, image, ..
        } = &current
        {
            assert_eq!(snapshot.frame.as_ref().unwrap().image_sequence, 2);
            let pixels = image.as_ref().unwrap().frame.pixels();
            assert_eq!(pixels.len(), 4);
            assert_eq!(pixels[0].to_array(), [255, 0, 0, 255]);
            assert_eq!(pixels[3].a(), 64);
        } else {
            panic!("missing decoded frame");
        }
        #[cfg(feature = "spatial")]
        if let CameraUpdate::Snapshot { projection, .. } = &current {
            assert_eq!(
                projection.as_ref().unwrap().pixel([0., 0., 1.]),
                Some([1., 1.])
            );
        }
        store
            .invalid_info("calibration source invalid".into())
            .unwrap();
        stream.ingest(Message::Invalidated(store.invalidation().unwrap()));
        assert!(!current.is_receiving());
        assert!(matches!(
            stream.take().unwrap().unwrap(),
            CameraUpdate::Invalidated(_)
        ));
        assert!(stream.take().is_none());
    }
    #[test]
    fn camera_completed_frame_survives_next_preparation_but_not_invalidation() {
        let mut store = fixture(1);
        let stream = CameraStream::new(owner(1)).unwrap();
        stream.ingest(message(&store));
        let presented = stream.take().unwrap().unwrap();
        assert!(presented.is_receiving());
        offer_image(&mut store, 1);
        // The worker has accepted/decoded the next frame, but the UI has not
        // taken it. This gap must not replace a valid image with a placeholder.
        let prepared = stream.prepare(message(&store)).unwrap().unwrap();
        assert!(
            presented.is_receiving(),
            "ordinary frame preparation revoked the displayed image"
        );
        assert!(prepared.is_receiving());
        stream.publish(prepared);
        assert!(presented.is_receiving());
        let next = stream.take().unwrap().unwrap();
        assert!(next.is_receiving());
        store.invalid_info("calibration changed".into()).unwrap();
        stream.ingest(Message::Invalidated(store.invalidation().unwrap()));
        assert!(!presented.is_receiving());
        assert!(!next.is_receiving());
    }
    #[test]
    fn stop_blocks_prepared_late_publication_without_waiting_for_worker_lock() {
        let store = fixture(1);
        let stream = CameraStream::new(owner(1)).unwrap();
        let prepared = stream.prepare(message(&store)).unwrap().unwrap();
        assert!(prepared.is_receiving());
        let guard = stream.0.ingress.lock().unwrap();
        stream.stop();
        assert!(!prepared.is_receiving());
        stream.publish(prepared);
        assert!(stream.take().is_none());
        drop(guard);
        stream.ingest(message(&store));
        assert!(stream.take().is_none());
    }
    #[test]
    fn owner_fault_is_terminal_for_one_mailbox_and_preserves_peer_authority() {
        let store = fixture(1);
        let stream = CameraStream::new(owner(1)).unwrap();
        stream.ingest(message(&store));
        let current = stream.take().unwrap().unwrap();
        let peer = CameraStream::new(owner(2)).unwrap();
        let peer_store = fixture(2);
        peer.ingest(message(&peer_store));
        let peer_current = peer.take().unwrap().unwrap();
        let mut wrong = store.snapshot().unwrap();
        wrong.owner.generation = Generation(99);
        stream.ingest(Message::Snapshot(Box::new(wrong)));
        assert!(!current.is_receiving());
        assert!(stream.is_stopped());
        assert!(stream.take().unwrap().is_err());
        assert!(peer_current.is_receiving());
        stream.ingest(message(&store));
        assert!(stream.take().is_none());
    }
    #[test]
    fn delayed_control_publication_cannot_replace_a_newer_prepared_snapshot() {
        let mut store = fixture(1);
        let stream = CameraStream::new(owner(1)).unwrap();
        stream.ingest(message(&store));
        stream.take();
        store.invalid_info("changed source".into()).unwrap();
        let barrier = stream
            .prepare(Message::Invalidated(store.invalidation().unwrap()))
            .unwrap()
            .unwrap();
        let snapshot = stream.prepare(message(&store)).unwrap().unwrap();
        stream.publish(snapshot);
        stream.publish(barrier);
        assert!(matches!(
            stream.take().unwrap().unwrap(),
            CameraUpdate::Snapshot { .. }
        ));
    }
}
