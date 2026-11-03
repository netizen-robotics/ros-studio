//! Ordered input continues while a single outbound map slot is occupied.
use super::*;
use crate::{map_ingress::Reader as SourceReader, map_producer::MapProducer};
use studio_protocol::{
    map_grid::{self, Invalidation},
    map_snapshot,
};

pub(super) struct MapDisplay {
    full: Box<dyn Reader>,
    updates: Option<Box<dyn Reader>>,
    producer: MapProducer,
    input_sequence: u64,
    source: Option<(u64, map_grid::Status, Option<i64>)>,
}
impl MapDisplay {
    pub(super) fn new(a: &Attach, factory: &Factory) -> Result<Self, String> {
        let mut full = factory(&a.configuration)?;
        if a.cancel.load(Ordering::Acquire) {
            return Err("Map startup canceled".into());
        }
        let full_support = full.enable_message_loss_events()?;
        let mut updates = if let Some(source) = &a.configuration.map_updates {
            let mut config = a.configuration.clone();
            config.topic = source.topic.clone();
            config.message_type = map_grid::UPDATE_MESSAGE_TYPE.into();
            config.best_effort = source.best_effort;
            config.transient_local = source.transient_local;
            config.output = Output::Typed;
            config.spatial = None;
            config.robot = None;
            config.map_updates = None;
            Some(factory(&config)?)
        } else {
            None
        };
        if a.cancel.load(Ordering::Acquire) {
            return Err("Map startup canceled after update open".into());
        }
        let update_support = updates
            .as_mut()
            .map(|reader| reader.enable_message_loss_events())
            .transpose()?;
        let producer = MapProducer::new(
            map_snapshot::Owner {
                generation: a.generation,
                subscription: a.subscription,
                topic: a.configuration.topic.clone(),
                fixed_frame: a
                    .configuration
                    .spatial
                    .as_ref()
                    .ok_or("Missing map TF configuration")?
                    .fixed_frame
                    .clone(),
            },
            updates.is_some(),
            [Some(full_support), update_support],
            Instant::now(),
        )?;
        Ok(Self {
            full,
            updates,
            producer,
            input_sequence: 0,
            source: None,
        })
    }
    fn sequence(&mut self) -> Result<u64, String> {
        self.input_sequence = self
            .input_sequence
            .checked_add(1)
            .ok_or("Map input sequence exhausted")?;
        Ok(self.input_sequence)
    }
    pub(super) fn poll(
        &mut self,
        a: &Attach,
        context: &mut Context,
        now: Instant,
    ) -> Result<Option<EncodedPayload>, String> {
        let token = self.producer.token().ok_or("Map producer stopped")?;
        let lost = self.full.message_loss_count()?;
        let sequence = self.sequence()?;
        self.producer
            .loss(token, sequence, SourceReader::Full, lost, now);
        if let Some(reader) = &mut self.updates {
            let lost = reader.message_loss_count()?;
            let sequence = self.sequence()?;
            self.producer
                .loss(token, sequence, SourceReader::Updates, lost, now);
        }
        if let Some(input) = self.full.poll_grid()? {
            let sequence = self.sequence()?;
            match input {
                Ok(grid) => {
                    let _ = self.producer.full(token, sequence, grid, now);
                }
                Err(_) => self
                    .producer
                    .fault(token, sequence, Invalidation::InvalidInput, now),
            }
        }
        // Bounded by the native depth2 history. Never coalesce patch inputs;
        // each taken patch is applied before the next is taken.
        for _ in 0..2 {
            let Some(reader) = &mut self.updates else {
                break;
            };
            let Some(input) = reader.poll_grid_update()? else {
                break;
            };
            let publication = reader.publication()?;
            let sequence = self.sequence()?;
            match input {
                Ok(patch) => {
                    let _ = self
                        .producer
                        .patch(token, sequence, patch, publication, now);
                }
                Err(_) => self
                    .producer
                    .fault(token, sequence, Invalidation::InvalidInput, now),
            }
        }
        let assembly = self.producer.source().assembly();
        let source = (
            assembly.revision(),
            assembly.status(),
            self.producer.source().recovery_floor(),
        );
        let available = {
            let mut queue = a.queue.lock().unwrap();
            if self.source != Some(source) {
                // Ordinary updates cancel the producer copy but wait for the
                // queued slot to drain. Clearing it on every update would
                // repeatedly copy/hash a full map even with a blocked writer.
                if matches!(source.1, map_grid::Status::WaitingFull(Some(_))) {
                    match queue.clear_samples(a.generation, a.subscription) {
                        Ok(()) => (),
                        Err(QueueError::UnknownStream | QueueError::StaleGeneration) => {
                            return Ok(None)
                        }
                        Err(error) => return Err(format!("Map cancellation queue: {error:?}")),
                    }
                }
                self.source = Some(source);
            }
            queue.stream_queued_samples(a.subscription)
        };
        if available != Some(0) || a.cancel.load(Ordering::Acquire) {
            return Ok(None);
        }
        self.producer
            .poll(&mut context.tf, now, &context.counters)?
            .map(|message| {
                message
                    .encode()
                    .map(|bytes| EncodedPayload::MapSnapshot(bytes.into_boxed_slice()))
            })
            .transpose()
    }
}
