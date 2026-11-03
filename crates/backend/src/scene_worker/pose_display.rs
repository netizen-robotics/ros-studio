//! One complete source, the existing native TF pair, and ordered reset barriers.
use super::*;
use crate::pose_producer::PoseProducer;
use std::collections::BTreeMap;
use studio_protocol::pose_snapshot::{
    transport::{Invalidation, Message},
    Owner,
};
pub(super) struct PoseDisplay {
    reader: Box<dyn Reader>,
    producer: PoseProducer,
    began: Instant,
    sequence: u64,
    last_epoch: u64,
    lost: u64,
    writers: BTreeMap<String, u64>,
    clear_gate: Option<Arc<AtomicBool>>,
}
impl PoseDisplay {
    pub(super) fn new(a: &Attach, factory: &Factory) -> Result<Self, String> {
        let Output::PoseSnapshot(configuration) = a.configuration.output else {
            return Err("Missing pose configuration".into());
        };
        let mut reader = factory(&a.configuration)?;
        if a.cancel.load(Ordering::Acquire) {
            return Err("Pose startup canceled".into());
        }
        let _ = reader.enable_message_loss_events()?;
        let producer = PoseProducer::new(
            Owner {
                generation: a.generation,
                subscription: a.subscription,
                kind: configuration.kind,
                topic: a.configuration.topic.clone(),
                fixed_frame: a
                    .configuration
                    .spatial
                    .as_ref()
                    .ok_or("Missing pose TF configuration")?
                    .fixed_frame
                    .clone(),
            },
            configuration.settings()?,
        )?;
        Ok(Self {
            reader,
            producer,
            began: Instant::now(),
            sequence: 0,
            last_epoch: 1,
            lost: 0,
            writers: BTreeMap::new(),
            clear_gate: None,
        })
    }
    fn monotonic(&self) -> Result<u64, String> {
        self.began
            .elapsed()
            .as_nanos()
            .try_into()
            .map_err(|_| "Pose monotonic clock exhausted".into())
    }
    fn retire(a: &Attach) -> Result<(), String> {
        a.queue
            .lock()
            .unwrap()
            .clear_samples(a.generation, a.subscription)
            .map_err(|e| format!("Pose queue retirement: {e:?}"))
    }
    pub(super) fn local_clear(
        &mut self,
        a: &Attach,
        gate: Arc<AtomicBool>,
    ) -> Result<Invalidation, String> {
        if !self.producer.clear()? {
            return Err("Pose owner retired before Clear".into());
        }
        Self::retire(a)?;
        let barrier = self
            .producer
            .source()
            .invalidation()
            .ok_or("Missing pose Clear barrier")?;
        self.last_epoch = barrier.continuity_epoch;
        self.clear_gate = Some(gate);
        Ok(barrier)
    }
    fn publication(&mut self) -> Result<(), String> {
        let publication = self.reader.publication()?;
        if publication.publisher_id.is_empty()
            || publication.publisher_id.len() > 128
            || !publication.publisher_id.len().is_multiple_of(2)
            || !publication
                .publisher_id
                .bytes()
                .all(|b| b.is_ascii_hexdigit())
        {
            return Err("Invalid pose publisher identity".into());
        }
        if let Some(sequence) = publication.sequence {
            if sequence == 0 {
                return Err("Invalid pose publication sequence".into());
            }
            if !self.writers.contains_key(&publication.publisher_id) && self.writers.len() == 16 {
                self.producer.source_gap()?;
                self.writers.clear();
            }
            if let Some(old) = self.writers.get(&publication.publisher_id) {
                if sequence <= *old {
                    return Err("Pose publication sequence regressed; explicitly restart".into());
                }
                if old.checked_add(1) != Some(sequence) {
                    self.producer.source_gap()?;
                }
            }
            self.writers.insert(publication.publisher_id, sequence);
        }
        Ok(())
    }
    pub(super) fn poll(
        &mut self,
        a: &Attach,
        context: &mut Context,
    ) -> Result<Option<EncodedPayload>, String> {
        if let Some(lost) = self.reader.message_loss_count()? {
            if lost != self.lost {
                self.producer.source_gap()?;
                self.lost = lost;
            }
        }
        for _ in 0..2 {
            let Some(input) = self.reader.poll_pose_covariance()? else {
                break;
            };
            self.publication()?;
            let input = input
                .map_err(|error| format!("Invalid pose source: {error}; explicitly restart"))?;
            self.sequence = self
                .sequence
                .checked_add(1)
                .ok_or("Pose receipt sequence exhausted")?;
            self.producer
                .input(self.sequence, input, self.monotonic()?)?;
        }
        let now = self.monotonic()?;
        self.producer.poll_snapshot(&mut context.tf, now, false)?;
        let epoch = self.producer.source().continuity_epoch();
        if epoch != self.last_epoch {
            Self::retire(a)?;
        }
        if a.cancel.load(Ordering::Acquire)
            || self
                .clear_gate
                .as_ref()
                .is_some_and(|gate| !gate.load(Ordering::Acquire))
        {
            return Ok(None);
        }
        self.clear_gate = None;
        if a.queue
            .lock()
            .unwrap()
            .stream_queued_samples(a.subscription)
            != Some(0)
        {
            return Ok(None);
        }
        let message = if epoch != self.last_epoch {
            self.last_epoch = epoch;
            Some(Message::Invalidated(
                self.producer
                    .source()
                    .invalidation()
                    .ok_or("Missing pose continuity barrier")?,
            ))
        } else {
            self.producer
                .poll_snapshot(&mut context.tf, now, true)?
                .map(|s| Message::Snapshot(Box::new(s)))
        };
        message
            .map(|m| {
                m.encode()
                    .map(|bytes| EncodedPayload::PoseSnapshot(bytes.into_boxed_slice()))
            })
            .transpose()
    }
}
