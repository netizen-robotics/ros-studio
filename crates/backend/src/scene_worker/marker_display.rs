//! One explicit Marker source shares this actor's native TF pair. Inputs and
//! expiry continue before checking the fair outbound slot.
use super::*;
use crate::marker_producer::MarkerProducer;
use std::collections::BTreeMap;
use studio_protocol::{
    latest_pose::LatestPose,
    marker::LookupTime,
    marker_snapshot::{Clock, LossSupport, Owner},
};
pub(super) struct MarkerDisplay {
    reader: Box<dyn Reader>,
    producer: MarkerProducer,
    began: Instant,
    input_sequence: u64,
    lost: u64,
    writers: BTreeMap<String, u64>,
    faults: u64,
}
impl MarkerDisplay {
    pub(super) fn new(a: &Attach, factory: &Factory) -> Result<Self, String> {
        let mut reader = factory(&a.configuration)?;
        if a.cancel.load(Ordering::Acquire) {
            return Err("Marker startup canceled".into());
        }
        let supported = reader.enable_message_loss_events()?;
        let producer = MarkerProducer::new(
            Owner {
                generation: a.generation,
                subscription: a.subscription,
                topic: a.configuration.topic.clone(),
                message_type: a.configuration.message_type.clone(),
                fixed_frame: a
                    .configuration
                    .spatial
                    .as_ref()
                    .ok_or("Missing Marker scene configuration")?
                    .fixed_frame
                    .clone(),
            },
            if supported {
                LossSupport::Supported
            } else {
                LossSupport::Unsupported
            },
        )?;
        Ok(Self {
            reader,
            producer,
            began: Instant::now(),
            input_sequence: 0,
            lost: 0,
            writers: BTreeMap::new(),
            faults: 0,
        })
    }
    fn monotonic(&self) -> Result<u64, String> {
        self.began
            .elapsed()
            .as_nanos()
            .try_into()
            .map_err(|_| "Marker monotonic clock exhausted".into())
    }
    fn clock(&self) -> Result<Clock, String> {
        Ok(Clock {
            unix_ns: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_err(|e| e.to_string())?
                .as_nanos()
                .try_into()
                .map_err(|_| "Marker System clock exhausted")?,
            monotonic_ns: self.monotonic()?,
        })
    }
    fn retire_old_data(a: &Attach) -> Result<(), String> {
        match a
            .queue
            .lock()
            .unwrap()
            .clear_samples(a.generation, a.subscription)
        {
            Ok(()) => Ok(()),
            Err(error) => Err(format!("Marker clear queue: {error:?}")),
        }
    }
    pub(super) fn local_clear(&mut self, a: &Attach) -> Result<(), String> {
        self.producer.local_clear()?;
        Self::retire_old_data(a)
    }
    fn publication(&mut self) -> Result<bool, String> {
        let publication = self.reader.publication()?;
        if publication.publisher_id.is_empty()
            || publication.publisher_id.len() > 128
            || !publication.publisher_id.len().is_multiple_of(2)
            || !publication
                .publisher_id
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        {
            return Err("Invalid Marker native publisher identity".into());
        }
        if let Some(sequence) = publication.sequence {
            if !self.writers.contains_key(&publication.publisher_id) && self.writers.len() == 16 {
                self.producer.source_fault()?;
                self.writers.clear();
            }
            if let Some(previous) = self.writers.get(&publication.publisher_id).copied() {
                if sequence <= previous {
                    self.producer.source_fault()?;
                    return Ok(false);
                }
                if previous.checked_add(1) != Some(sequence) {
                    self.producer.source_fault()?;
                }
            }
            self.writers.insert(publication.publisher_id, sequence);
        }
        Ok(true)
    }
    pub(super) fn poll(
        &mut self,
        a: &Attach,
        context: &mut Context,
    ) -> Result<Option<EncodedPayload>, String> {
        if let Some(lost) = self.reader.message_loss_count()? {
            if lost != self.lost {
                self.producer.source_fault()?;
                self.lost = lost;
            }
        }
        for _ in 0..2 {
            let Some(input) = self.reader.poll_markers()? else {
                break;
            };
            if !self.publication()? {
                continue;
            }
            match input {
                Ok(batch) => {
                    self.input_sequence = self
                        .input_sequence
                        .checked_add(1)
                        .ok_or("Marker source sequence exhausted")?;
                    let _ = self
                        .producer
                        .input(self.input_sequence, batch, self.monotonic()?);
                    if self.producer.source().token().is_none() {
                        return Err("Marker source clock/state failed".into());
                    }
                }
                Err(_) => self.producer.source_fault()?,
            }
        }
        let requests = self.producer.requests(self.monotonic()?)?;
        for request in requests {
            if a.cancel.load(Ordering::Acquire) {
                return Ok(None);
            }
            let result = match request.time {
                LookupTime::At(stamp) => {
                    context
                        .tf
                        .lookup_at(&request.fixed_frame, &request.source_frame, stamp)
                }
                LookupTime::Latest => context
                    .tf
                    .lookup_latest(&request.fixed_frame, &request.source_frame),
            };
            let pose = match result {
                Ok(value) => LatestPose::Ready {
                    resolved_stamp_ns: value.stamp_ns,
                    components: value.components,
                },
                Err(error) => match error.status {
                    crate::spatial_session::Status::Extrapolated => LatestPose::Extrapolated,
                    crate::spatial_session::Status::Stale => LatestPose::Stale,
                    _ => LatestPose::Missing,
                },
            };
            let _ = self.producer.complete(&request, pose, self.monotonic()?);
        }
        let faults = self.producer.source().counters().source_faults;
        if faults != self.faults {
            Self::retire_old_data(a)?;
            self.faults = faults;
        }
        let available = a
            .queue
            .lock()
            .unwrap()
            .stream_queued_samples(a.subscription);
        if available != Some(0) || a.cancel.load(Ordering::Acquire) {
            return Ok(None);
        }
        self.producer
            .poll(self.clock()?, context.counters)?
            .map(|message| {
                message
                    .encode()
                    .map(|bytes| EncodedPayload::MarkerSnapshot(bytes.into_boxed_slice()))
            })
            .transpose()
    }
}
