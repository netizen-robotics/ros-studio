//! Camera pairing advances independently of outbound queue pressure. The scene
//! actor will own both native readers and queue retirement before admission.
use crate::spatial_session::{SpatialSession, Status};
use studio_protocol::{
    camera_info::Calibration,
    camera_snapshot::{self as wire, store::Store, transport::Message, Owner},
    image::ImageSample,
    latest_pose::{LatestPose, LookupTime},
};

pub struct CameraProducer {
    store: Store,
    last_epoch: u64,
    last_revision: Option<u64>,
    next_emit: u64,
}
impl CameraProducer {
    pub fn new(owner: Owner) -> Result<Self, String> {
        let store = Store::new(owner)?;
        Ok(Self {
            last_epoch: store.epoch(),
            store,
            last_revision: None,
            next_emit: 0,
        })
    }
    pub fn source(&self) -> &Store {
        &self.store
    }
    pub fn info(&mut self, input: Result<Calibration, String>) -> Result<(), String> {
        match input {
            Ok(info) => self.store.info(info),
            Err(error) => self.store.invalid_info(error),
        }
    }
    pub fn image(&mut self, input: Result<ImageSample, String>, now: u64) -> Result<(), String> {
        match input {
            Ok(image) => self.store.image(image, now),
            Err(error) => self.store.invalid_image(error),
        }
    }
    pub fn stop(&mut self) {
        self.store.stop();
    }
    pub fn poll(
        &mut self,
        tf: &mut SpatialSession,
        now: u64,
        can_emit: bool,
    ) -> Result<Option<Message>, String> {
        let result = self.poll_inner(tf, now, can_emit);
        if result.is_err() {
            self.stop();
        }
        result
    }
    fn poll_inner(
        &mut self,
        tf: &mut SpatialSession,
        now: u64,
        can_emit: bool,
    ) -> Result<Option<Message>, String> {
        if self.store.invalidation().is_none() {
            return Ok(None);
        }
        if !tf.is_active() {
            return Err("Camera requires an active VerifiedSystem TF context".into());
        }
        if let Some(request) = self.store.request(now)? {
            if request.fixed_frame() == request.source_frame() {
                let stamp = match request.time() {
                    LookupTime::At(stamp) => stamp,
                    LookupTime::Latest => 0,
                };
                self.store.complete(
                    &request,
                    LatestPose::Ready {
                        resolved_stamp_ns: stamp,
                        components: [0., 0., 0., 0., 0., 0., 1.],
                    },
                    now,
                )?;
            } else {
                let result = match request.time() {
                    LookupTime::At(stamp) => {
                        tf.lookup_at(request.fixed_frame(), request.source_frame(), stamp)
                    }
                    LookupTime::Latest => {
                        tf.lookup_latest(request.fixed_frame(), request.source_frame())
                    }
                };
                let transform = match result {
                    Ok(value) => LatestPose::Ready {
                        resolved_stamp_ns: value.stamp_ns,
                        components: value.components,
                    },
                    Err(error) => match error.status {
                        Status::Missing => LatestPose::Missing,
                        Status::Stale => LatestPose::Stale,
                        Status::Extrapolated => LatestPose::Extrapolated,
                        _ => {
                            return Err(format!("Camera TF {:?}: {}", error.status, error.message))
                        }
                    },
                };
                self.store.complete(&request, transform, now)?;
            }
        }
        if !can_emit {
            return Ok(None);
        }
        if self.store.epoch() != self.last_epoch {
            self.last_epoch = self.store.epoch();
            return Ok(Some(Message::Invalidated(
                self.store
                    .invalidation()
                    .ok_or("Camera retired before invalidation")?,
            )));
        }
        if now < self.next_emit || self.last_revision == Some(self.store.revision()) {
            return Ok(None);
        }
        let snapshot = self
            .store
            .snapshot()
            .ok_or("Camera retired before capture")?;
        snapshot.validate()?;
        self.last_revision = Some(snapshot.revision);
        self.next_emit = now
            .checked_add(1_000_000_000 / u64::from(wire::MAX_HZ))
            .ok_or("Camera output clock exhausted")?;
        Ok(Some(Message::Snapshot(Box::new(snapshot))))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::spatial_session::{ClockPolicy, StreamKind, Transform};
    use studio_protocol::{
        camera_snapshot::{transport::Receiver, InfoSource},
        image::{HeaderStamp, ImageMetadata},
        subscriptions::{Generation, SubscriptionId},
    };
    fn owner() -> Owner {
        Owner {
            generation: Generation(1),
            subscription: SubscriptionId(2),
            topic: "/camera/image_rect".into(),
            info: InfoSource {
                topic: "/camera/camera_info".into(),
                best_effort: true,
                transient_local: false,
            },
            fixed_frame: "world".into(),
        }
    }
    fn calibration(frame: &str) -> Calibration {
        serde_json::from_value(serde_json::json!({"stamp_ns":1,"frame_id":frame,"width":2,"height":2,
            "distortion_model":"plumb_bob","d":[],"k":[100.,0.,1.,0.,100.,1.,0.,0.,1.],
            "r":[1.,0.,0.,0.,1.,0.,0.,0.,1.],"p":[100.,0.,1.,0.,0.,100.,1.,0.,0.,0.,1.,0.],
            "binning_x":0,"binning_y":0,"roi":{"x_offset":0,"y_offset":0,"width":0,"height":0,"do_rectify":false}})).unwrap()
    }
    fn image(frame: &str, seconds: i32) -> ImageSample {
        ImageSample::from_raw(
            &owner().topic,
            ImageMetadata {
                header_stamp: HeaderStamp {
                    seconds,
                    nanoseconds: 0,
                },
                frame_id: frame.into(),
                width: 2,
                height: 2,
                encoding: "rgba8".into(),
                is_bigendian: false,
                step: 8,
            },
            &[255; 16],
        )
        .unwrap()
    }
    #[test]
    fn native_exact_tf_progresses_under_backpressure_and_small_invalidation_bypasses_throttle() {
        let mut p = CameraProducer::new(owner()).unwrap();
        let mut tf = SpatialSession::new().unwrap();
        tf.activate_explicit(ClockPolicy::VerifiedSystem).unwrap();
        p.info(Ok(calibration("optical"))).unwrap();
        p.image(Ok(image("optical", 2)), 0).unwrap();
        assert!(p.poll(&mut tf, 0, false).unwrap().is_none());
        assert_eq!(
            p.source().snapshot().unwrap().status,
            wire::Status::WaitingTf
        );
        for seconds in [1, 3] {
            tf.insert(
                StreamKind::Dynamic,
                &Transform {
                    parent: "world".into(),
                    child: "optical".into(),
                    stamp_ns: seconds * 1_000_000_000,
                    components: [seconds as f64, 0., 0., 0., 0., 0., 1.],
                },
            )
            .unwrap();
        }
        assert!(p.poll(&mut tf, wire::RETRY_NS, false).unwrap().is_none());
        let Message::Invalidated(_) = p.poll(&mut tf, wire::RETRY_NS, true).unwrap().unwrap()
        else {
            panic!("missing initial barrier")
        };
        let Message::Snapshot(first) = p.poll(&mut tf, wire::RETRY_NS, true).unwrap().unwrap()
        else {
            panic!("missing frame")
        };
        assert!(
            matches!(first.frame.as_ref().unwrap().transform,LatestPose::Ready {resolved_stamp_ns:2_000_000_000,components} if components[0]==2.)
        );
        let mut receiver = Receiver::new(owner()).unwrap();
        let accepted = receiver.apply(*first).unwrap().unwrap();
        let mut cal = calibration("optical");
        cal.p[2] = 0.5;
        p.info(Ok(cal)).unwrap();
        let Message::Invalidated(barrier) =
            p.poll(&mut tf, wire::RETRY_NS + 1, true).unwrap().unwrap()
        else {
            panic!("invalidation was throttled")
        };
        receiver.invalidate(&barrier).unwrap();
        assert!(!accepted.permission.is_valid());
        assert!(p.poll(&mut tf, wire::RETRY_NS + 1, true).unwrap().is_none());
        p.image(Ok(image("optical", 0)), wire::RETRY_NS + 2)
            .unwrap();
        assert!(p
            .poll(&mut tf, wire::RETRY_NS + 2, false)
            .unwrap()
            .is_none());
        let Message::Snapshot(latest) = p.poll(&mut tf, wire::RETRY_NS * 2, true).unwrap().unwrap()
        else {
            panic!("missing latest frame")
        };
        assert!(
            matches!(latest.frame.as_ref().unwrap().transform,LatestPose::Ready {resolved_stamp_ns:3_000_000_000,components} if components[0]==3.)
        );
        assert_eq!(latest.frame.as_ref().unwrap().calibration.p[2], 0.5);
        p.stop();
        assert!(p.poll(&mut tf, wire::RETRY_NS * 3, true).unwrap().is_none());
    }
    #[test]
    fn same_frame_identity_still_requires_explicit_active_system_context() {
        let mut p = CameraProducer::new(owner()).unwrap();
        let mut inactive = SpatialSession::new().unwrap();
        p.info(Ok(calibration("world"))).unwrap();
        p.image(Ok(image("world", 2)), 0).unwrap();
        assert!(p.poll(&mut inactive, 0, true).is_err());
        assert!(p.source().snapshot().is_none());
        let mut p = CameraProducer::new(owner()).unwrap();
        let mut tf = SpatialSession::new().unwrap();
        tf.activate_explicit(ClockPolicy::VerifiedSystem).unwrap();
        p.info(Ok(calibration("world"))).unwrap();
        p.image(Ok(image("world", 2)), 0).unwrap();
        p.poll(&mut tf, 0, false).unwrap();
        assert!(
            matches!(p.source().snapshot().unwrap().frame.unwrap().transform,
            LatestPose::Ready {resolved_stamp_ns:2_000_000_000,components} if components==[0.,0.,0.,0.,0.,0.,1.])
        );
    }
}
