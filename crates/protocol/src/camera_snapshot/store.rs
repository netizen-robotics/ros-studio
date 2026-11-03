//! One source owner's bounded pairing state. No ROS, I/O or UI authority.
use super::*;
use std::sync::atomic::{AtomicU64, Ordering};
static ACTIVATION: AtomicU64 = AtomicU64::new(0);

#[derive(Clone, Debug)]
pub struct TransformRequest {
    activation: u64,
    epoch: u64,
    sequence: u64,
    source_frame: String,
    fixed_frame: String,
    time: LookupTime,
}
impl TransformRequest {
    pub fn source_frame(&self) -> &str {
        &self.source_frame
    }
    pub fn fixed_frame(&self) -> &str {
        &self.fixed_frame
    }
    pub fn time(&self) -> LookupTime {
        self.time
    }
}
struct Pending {
    sequence: u64,
    image: ImageSample,
    calibration: Calibration,
    calibration_revision: u64,
    deadline: u64,
    retry_at: u64,
}
pub struct Store {
    activation: u64,
    state: Snapshot,
    calibration: Option<Calibration>,
    calibration_revision: u64,
    pending: Option<Pending>,
    ready_at: Option<u64>,
    sequence: u64,
    now: u64,
    stopped: bool,
}
impl Store {
    pub fn new(owner: Owner) -> Result<Self, String> {
        owner.validate()?;
        let activation = ACTIVATION
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |n| n.checked_add(1))
            .map_err(|_| "Camera activation identities exhausted")?
            + 1;
        Ok(Self {
            activation,
            state: Snapshot {
                contract_version: CONTRACT_VERSION,
                owner,
                revision: 1,
                continuity_epoch: 1,
                status: Status::WaitingInfo,
                diagnostic: None,
                counters: Counters::default(),
                frame: None,
            },
            calibration: None,
            calibration_revision: 0,
            pending: None,
            ready_at: None,
            sequence: 0,
            now: 0,
            stopped: false,
        })
    }
    pub fn snapshot(&self) -> Option<Snapshot> {
        (!self.stopped).then(|| self.state.clone())
    }
    pub fn revision(&self) -> u64 {
        self.state.revision
    }
    pub fn epoch(&self) -> u64 {
        self.state.continuity_epoch
    }
    pub fn invalidation(&self) -> Option<transport::Invalidation> {
        (!self.stopped).then(|| transport::Invalidation::from(&self.state))
    }
    pub fn pending_count(&self) -> usize {
        usize::from(self.pending.is_some())
    }
    pub fn stop(&mut self) {
        self.stopped = true;
        self.pending = None;
        self.calibration = None;
        self.state.frame = None;
        self.ready_at = None;
    }
    fn check(&self) -> Result<(), String> {
        if self.stopped {
            Err("Camera source owner is retired".into())
        } else {
            Ok(())
        }
    }
    fn clock(&mut self, now: u64) -> Result<(), String> {
        self.check()?;
        if now < self.now {
            self.stop();
            return Err("Camera monotonic clock regressed".into());
        }
        self.now = now;
        Ok(())
    }
    fn bump(&mut self) -> Result<(), String> {
        self.state.revision = self
            .state
            .revision
            .checked_add(1)
            .ok_or("Camera revision exhausted")?;
        Ok(())
    }
    fn status(&mut self, status: Status, diagnostic: Option<String>) -> Result<(), String> {
        self.state.status = status;
        self.state.diagnostic = diagnostic.map(|s| {
            let text: String = s.chars().filter(|&c| c != '\0').take(120).collect();
            if text.is_empty() {
                "Camera source unavailable".into()
            } else {
                text
            }
        });
        self.bump()
    }
    fn invalidate(&mut self, status: Status, diagnostic: Option<String>) -> Result<(), String> {
        self.state.continuity_epoch = self
            .state
            .continuity_epoch
            .checked_add(1)
            .ok_or("Camera continuity exhausted")?;
        self.state.frame = None;
        self.ready_at = None;
        self.status(status, diagnostic)
    }
    fn discard_pending(&mut self) {
        if self.pending.take().is_some() {
            self.state.counters.pending_dropped =
                self.state.counters.pending_dropped.saturating_add(1);
        }
    }
    pub fn invalid_info(&mut self, error: String) -> Result<(), String> {
        self.check()?;
        self.calibration = None;
        self.discard_pending();
        self.state.counters.invalid_calibrations =
            self.state.counters.invalid_calibrations.saturating_add(1);
        self.invalidate(Status::InvalidInfo, Some(error))
    }
    pub fn invalid_image(&mut self, error: String) -> Result<(), String> {
        self.check()?;
        self.discard_pending();
        self.state.counters.invalid_images = self.state.counters.invalid_images.saturating_add(1);
        self.invalidate(Status::InvalidImage, Some(error))
    }
    pub fn info(&mut self, calibration: Calibration) -> Result<(), String> {
        self.check()?;
        if let Err(error) = calibration.validate() {
            return self.invalid_info(error);
        }
        self.state.counters.calibrations = self.state.counters.calibrations.saturating_add(1);
        let same = self
            .calibration
            .as_ref()
            .is_some_and(|old| old.same_parameters(&calibration));
        if same {
            self.calibration = Some(calibration);
            return self.bump();
        }
        self.calibration_revision = self
            .calibration_revision
            .checked_add(1)
            .ok_or("Camera calibration identity exhausted")?;
        let (status, diagnostic) = if calibration.k[0] == 0. {
            (
                Status::Uncalibrated,
                Some("CameraInfo is uncalibrated (K[0] is zero)".into()),
            )
        } else if let Err(error) = calibration.effective() {
            (Status::Unsupported, Some(error))
        } else {
            (Status::WaitingImage, None)
        };
        self.calibration = Some(calibration);
        self.discard_pending();
        self.invalidate(status, diagnostic)
    }
    pub fn image(&mut self, image: ImageSample, now: u64) -> Result<(), String> {
        self.clock(now)?;
        let deadline = match &self.pending {
            Some(pending) => pending.deadline,
            None => now
                .checked_add(TIMEOUT_NS)
                .ok_or("Camera TF deadline exhausted")?,
        };
        self.state.counters.images = self.state.counters.images.saturating_add(1);
        if let Err(error) = image
            .validate()
            .and_then(|_| image_stamp(&image.metadata).map(|_| ()))
        {
            return self.invalid_image(error);
        }
        if image.topic != self.state.owner.topic {
            return self.invalid_image("Camera Image topic contradicts owner".into());
        }
        self.discard_pending();
        if !matches!(
            image.metadata.encoding.as_str(),
            "mono8" | "rgb8" | "bgr8" | "rgba8" | "bgra8"
        ) {
            self.state.counters.invalid_images =
                self.state.counters.invalid_images.saturating_add(1);
            return self.invalidate(
                Status::Unsupported,
                Some("Choose rectified mono8, rgb8, bgr8, rgba8 or bgra8 Image".into()),
            );
        }
        let Some(calibration) = &self.calibration else {
            return self.status(Status::WaitingInfo, None);
        };
        if calibration.k[0] == 0. {
            return self.status(
                Status::Uncalibrated,
                Some("CameraInfo is uncalibrated".into()),
            );
        }
        if let Err(error) = calibration.effective() {
            return self.status(Status::Unsupported, Some(error));
        }
        if let Err(error) = calibration.matches_image(&image.metadata) {
            return self.invalidate(Status::Incompatible, Some(error));
        }
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or("Camera Image identity exhausted")?;
        self.pending = Some(Pending {
            sequence: self.sequence,
            image,
            calibration: calibration.clone(),
            calibration_revision: self.calibration_revision,
            deadline,
            retry_at: now,
        });
        if self.state.frame.is_none() {
            self.status(Status::WaitingTf, None)
        } else {
            self.bump()
        }
    }
    fn expire(&mut self, now: u64) -> Result<bool, String> {
        if self.pending.as_ref().is_some_and(|p| now >= p.deadline) {
            self.pending = None;
            self.state.counters.lookup_timeouts =
                self.state.counters.lookup_timeouts.saturating_add(1);
            self.invalidate(
                Status::TfTimeout,
                Some("Camera Image acquisition TF timed out; waiting for a fresh Image".into()),
            )?;
            return Ok(true);
        }
        Ok(false)
    }
    pub fn request(&mut self, now: u64) -> Result<Option<TransformRequest>, String> {
        self.clock(now)?;
        if self.expire(now)? {
            return Ok(None);
        }
        let Some(pending) = self.pending.as_mut() else {
            return Ok(None);
        };
        if now < pending.retry_at {
            return Ok(None);
        }
        pending.retry_at = now
            .checked_add(RETRY_NS)
            .ok_or("Camera TF retry clock exhausted")?;
        Ok(Some(TransformRequest {
            activation: self.activation,
            epoch: self.state.continuity_epoch,
            sequence: pending.sequence,
            source_frame: pending.image.metadata.frame_id.clone(),
            fixed_frame: self.state.owner.fixed_frame.clone(),
            time: LookupTime::from_stamp(image_stamp(&pending.image.metadata)?)?,
        }))
    }
    pub fn complete(
        &mut self,
        request: &TransformRequest,
        transform: LatestPose,
        now: u64,
    ) -> Result<bool, String> {
        if self.stopped || request.activation != self.activation {
            return Ok(false);
        }
        self.clock(now)?;
        if self.expire(now)? {
            return Ok(false);
        }
        let Some(pending) = self.pending.as_ref() else {
            return Ok(false);
        };
        if request.sequence != pending.sequence || request.epoch != self.state.continuity_epoch {
            return Ok(false);
        }
        request.time.validate_result(&transform)?;
        if !matches!(transform, LatestPose::Ready { .. }) {
            // The pending image must never borrow the previous image's TF.
            // Keep the already complete pair until its bounded deadline.
            if self.state.frame.is_some()
                && self
                    .ready_at
                    .is_some_and(|at| now.saturating_sub(at) >= TIMEOUT_NS)
            {
                self.invalidate(
                    Status::TfTimeout,
                    Some("Camera Image acquisition TF timed out; waiting for a fresh Image".into()),
                )?;
            }
            return Ok(false);
        }
        let pending = self.pending.take().unwrap();
        let frame = Frame {
            image_sequence: pending.sequence,
            calibration_revision: pending.calibration_revision,
            image: pending.image,
            calibration: pending.calibration,
            transform,
        };
        frame.validate(&self.state.owner)?;
        self.state.frame = Some(Box::new(frame));
        self.ready_at = Some(now);
        self.status(Status::Receiving, None)?;
        Ok(true)
    }
}
