use super::{Generation, SubscriptionId, MAX_SUBSCRIPTIONS};
use std::collections::VecDeque;

pub const PER_STREAM_SAMPLES: usize = 16;
pub const PER_STREAM_BYTES: usize = 2 * 1024 * 1024;
pub const AGGREGATE_SAMPLE_BYTES: usize = MAX_SUBSCRIPTIONS * RESERVED_STREAM_BYTES;
pub const MAX_SAMPLE_BYTES: usize = crate::structured::MAX_SAMPLE_BYTES;
/// Reserve one maximum sample for every other stream, including future slots.
pub const RESERVED_STREAM_BYTES: usize = MAX_SAMPLE_BYTES;
pub const RESERVED_ITEMS: usize = MAX_SUBSCRIPTIONS * 2;
pub const MAX_RESERVED_BYTES: usize = 4096;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counters {
    pub evicted_samples: u64,
    pub rejected_samples: u64,
    pub rejected_sample_bytes: u64,
    pub cleared_samples: u64,
    pub rejected_controls: u64,
    pub rejected_diagnostics: u64,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueError {
    StaleGeneration,
    InvalidId,
    DuplicateStream,
    UnknownStream,
    Capacity,
    Oversized,
    Full,
}
/// Accounted payload bytes; decoded node/allocator overhead is separately bounded
/// by the protocol validator. Implementations must charge their actual retained
/// content. Byte transports use boxed slices, never spare Vec capacity.
pub trait Payload {
    fn payload_bytes(&self) -> usize;
    fn is_image(&self) -> bool {
        false
    }
}
impl Payload for Box<[u8]> {
    fn payload_bytes(&self) -> usize {
        self.len()
    }
}
#[derive(Debug, PartialEq, Eq)]
pub enum Packet<P = Box<[u8]>> {
    Control(P),
    Diagnostic(P),
    Sample {
        generation: Generation,
        subscription: SubscriptionId,
        sequence: u64,
        bytes: P,
    },
}
struct Stream<P> {
    id: SubscriptionId,
    samples: VecDeque<(u64, P)>,
    bytes: usize,
    counters: Counters,
}
/// Payload-byte budgets exclude bounded metadata/allocator overhead. Owns boxed
/// buffers so arbitrary retained Vec capacity cannot bypass byte accounting.
/// Caller bounds decode allocation separately. This is not a transport codec.
/// All event classes and registered streams progress through deterministic round robin.
pub struct FairQueue<P = Box<[u8]>> {
    generation: Generation,
    streams: Vec<Stream<P>>,
    controls: VecDeque<P>,
    diagnostics: VecDeque<P>,
    sample_bytes: usize,
    lane: usize,
    stream_cursor: usize,
    counters: Counters,
    large_images: bool,
}
impl FairQueue<Box<[u8]>> {
    pub fn new(generation: Generation) -> Self {
        Self::with_payload(generation)
    }
}
impl<P: Payload> FairQueue<P> {
    pub fn with_payload(generation: Generation) -> Self {
        Self {
            generation,
            streams: Vec::new(),
            controls: VecDeque::new(),
            diagnostics: VecDeque::new(),
            sample_bytes: 0,
            lane: 0,
            stream_cursor: 0,
            counters: Counters::default(),
            large_images: false,
        }
    }
    /// Session21 only: two large image slots in addition to existing peer
    /// reservations. Per-owner eviction and explicit byte accounting remain.
    pub fn with_image_budget(mut self) -> Self {
        self.large_images = true;
        self
    }
    pub fn counters(&self) -> Counters {
        self.counters
    }
    pub fn stream_counters(&self, subscription: SubscriptionId) -> Option<Counters> {
        self.streams
            .iter()
            .find(|s| s.id == subscription)
            .map(|s| s.counters)
    }
    pub fn sample_bytes(&self) -> usize {
        self.sample_bytes
    }
    /// None denotes an owner already retired, distinct from an empty live slot.
    pub fn stream_queued_samples(&self, subscription: SubscriptionId) -> Option<usize> {
        self.streams
            .iter()
            .find(|s| s.id == subscription)
            .map(|s| s.samples.len())
    }
    pub fn queued_samples(&self) -> usize {
        self.streams.iter().map(|s| s.samples.len()).sum()
    }
    /// Cancel obsolete ordered data for one live owner without retiring it or
    /// touching peer/control lanes. The replacement must advance its epoch.
    pub fn clear_samples(
        &mut self,
        generation: Generation,
        subscription: SubscriptionId,
    ) -> Result<(), QueueError> {
        if generation != self.generation {
            return Err(QueueError::StaleGeneration);
        }
        let stream = self
            .streams
            .iter_mut()
            .find(|s| s.id == subscription)
            .ok_or(QueueError::UnknownStream)?;
        let count = stream.samples.len() as u64;
        self.sample_bytes -= stream.bytes;
        stream.bytes = 0;
        stream.samples.clear();
        stream.counters.cleared_samples = stream.counters.cleared_samples.saturating_add(count);
        self.counters.cleared_samples = self.counters.cleared_samples.saturating_add(count);
        Ok(())
    }
    pub fn register(
        &mut self,
        generation: Generation,
        subscription: SubscriptionId,
    ) -> Result<(), QueueError> {
        if generation != self.generation {
            return Err(QueueError::StaleGeneration);
        }
        if subscription.0 == 0 {
            return Err(QueueError::InvalidId);
        }
        if self.streams.iter().any(|s| s.id == subscription) {
            return Err(QueueError::DuplicateStream);
        }
        if self.streams.len() >= MAX_SUBSCRIPTIONS {
            return Err(QueueError::Capacity);
        }
        self.streams.push(Stream {
            id: subscription,
            samples: VecDeque::new(),
            bytes: 0,
            counters: Counters::default(),
        });
        Ok(())
    }
    pub fn unregister(&mut self, subscription: SubscriptionId) -> Result<(), QueueError> {
        let index = self
            .streams
            .iter()
            .position(|s| s.id == subscription)
            .ok_or(QueueError::UnknownStream)?;
        let stream = self.streams.remove(index);
        self.sample_bytes -= stream.bytes;
        self.counters.cleared_samples = self
            .counters
            .cleared_samples
            .saturating_add(stream.samples.len() as u64);
        self.stream_cursor = 0;
        Ok(())
    }
    pub fn reset(&mut self, generation: Generation) -> Result<(), QueueError> {
        if generation.0 <= self.generation.0 {
            return Err(QueueError::StaleGeneration);
        }
        self.counters.cleared_samples = self
            .counters
            .cleared_samples
            .saturating_add(self.queued_samples() as u64);
        self.streams.clear();
        self.controls.clear();
        self.diagnostics.clear();
        self.sample_bytes = 0;
        self.generation = generation;
        self.lane = 0;
        self.stream_cursor = 0;
        Ok(())
    }
    pub fn enqueue_sample(
        &mut self,
        generation: Generation,
        subscription: SubscriptionId,
        sequence: u64,
        bytes: P,
    ) -> Result<(), QueueError> {
        let large_image = self.large_images && bytes.is_image();
        let sample_limit = if large_image {
            crate::session::MAX_EVENT_BYTES
        } else {
            MAX_SAMPLE_BYTES
        };
        let stream_limit = PER_STREAM_BYTES.max(if large_image {
            bytes.payload_bytes()
        } else {
            0
        });
        let aggregate_limit = AGGREGATE_SAMPLE_BYTES
            + if self.large_images {
                2 * crate::session::MAX_EVENT_BYTES
            } else {
                0
            };
        let reason = if generation != self.generation {
            Some(QueueError::StaleGeneration)
        } else if bytes.payload_bytes() > sample_limit {
            Some(QueueError::Oversized)
        } else if !self.streams.iter().any(|s| s.id == subscription) {
            Some(QueueError::UnknownStream)
        } else {
            None
        };
        if let Some(reason) = reason {
            self.reject_sample(subscription, bytes.payload_bytes());
            return Err(reason);
        }
        let stream = self
            .streams
            .iter_mut()
            .find(|s| s.id == subscription)
            .expect("validated stream");
        // Evict only this stream's oldest data. A flood cannot evict another stream.
        while stream.samples.len() >= PER_STREAM_SAMPLES
            || stream.bytes + bytes.payload_bytes() > stream_limit
        {
            let (_, old) = stream.samples.pop_front().expect("nonempty budget");
            stream.bytes -= old.payload_bytes();
            self.sample_bytes -= old.payload_bytes();
            stream.counters.evicted_samples = stream.counters.evicted_samples.saturating_add(1);
            self.counters.evicted_samples = self.counters.evicted_samples.saturating_add(1);
        }
        let protected_bytes = (MAX_SUBSCRIPTIONS - self.streams.len()) * RESERVED_STREAM_BYTES
            + self
                .streams
                .iter()
                .filter(|s| s.id != subscription)
                .map(|s| RESERVED_STREAM_BYTES.saturating_sub(s.bytes))
                .sum::<usize>();
        // Future stream reservations can reduce this owner's effective quota
        // below its nominal 2 MiB. Retain its newest sample by evicting only
        // its own older payloads; never consume a peer's protected slot.
        while self.sample_bytes + bytes.payload_bytes() + protected_bytes > aggregate_limit {
            let stream = self
                .streams
                .iter_mut()
                .find(|s| s.id == subscription)
                .unwrap();
            let Some((_, old)) = stream.samples.pop_front() else {
                self.reject_sample(subscription, bytes.payload_bytes());
                return Err(QueueError::Full);
            };
            stream.bytes -= old.payload_bytes();
            self.sample_bytes -= old.payload_bytes();
            stream.counters.evicted_samples = stream.counters.evicted_samples.saturating_add(1);
            self.counters.evicted_samples = self.counters.evicted_samples.saturating_add(1);
        }
        let stream = self
            .streams
            .iter_mut()
            .find(|s| s.id == subscription)
            .expect("validated stream");
        stream.bytes += bytes.payload_bytes();
        self.sample_bytes += bytes.payload_bytes();
        stream.samples.push_back((sequence, bytes));
        Ok(())
    }
    fn reject_sample(&mut self, subscription: SubscriptionId, bytes: usize) {
        self.counters.rejected_samples = self.counters.rejected_samples.saturating_add(1);
        self.counters.rejected_sample_bytes = self
            .counters
            .rejected_sample_bytes
            .saturating_add(bytes as u64);
        if let Some(stream) = self.streams.iter_mut().find(|s| s.id == subscription) {
            stream.counters.rejected_samples = stream.counters.rejected_samples.saturating_add(1);
            stream.counters.rejected_sample_bytes = stream
                .counters
                .rejected_sample_bytes
                .saturating_add(bytes as u64);
        }
    }
    /// Reserved overflow returns the original payload for explicit retry/error handling.
    /// It is never silently discarded or displaced by sensor samples.
    pub fn enqueue_control(
        &mut self,
        generation: Generation,
        bytes: P,
    ) -> Result<(), (QueueError, P)> {
        if let Err(error) = enqueue_reserved(&mut self.controls, generation, self.generation, bytes)
        {
            self.counters.rejected_controls = self.counters.rejected_controls.saturating_add(1);
            Err(error)
        } else {
            Ok(())
        }
    }
    pub fn enqueue_diagnostic(
        &mut self,
        generation: Generation,
        bytes: P,
    ) -> Result<(), (QueueError, P)> {
        if let Err(error) =
            enqueue_reserved(&mut self.diagnostics, generation, self.generation, bytes)
        {
            self.counters.rejected_diagnostics =
                self.counters.rejected_diagnostics.saturating_add(1);
            Err(error)
        } else {
            Ok(())
        }
    }
    pub fn pop(&mut self) -> Option<Packet<P>> {
        for _ in 0..3 {
            let lane = self.lane;
            self.lane = (self.lane + 1) % 3;
            let packet = match lane {
                0 => self.controls.pop_front().map(Packet::Control),
                1 => self.diagnostics.pop_front().map(Packet::Diagnostic),
                _ => self.pop_sample(),
            };
            if packet.is_some() {
                return packet;
            }
        }
        None
    }
    fn pop_sample(&mut self) -> Option<Packet<P>> {
        for _ in 0..self.streams.len() {
            let index = self.stream_cursor % self.streams.len();
            self.stream_cursor = (index + 1) % self.streams.len();
            let stream = &mut self.streams[index];
            if let Some((sequence, bytes)) = stream.samples.pop_front() {
                stream.bytes -= bytes.payload_bytes();
                self.sample_bytes -= bytes.payload_bytes();
                return Some(Packet::Sample {
                    generation: self.generation,
                    subscription: stream.id,
                    sequence,
                    bytes,
                });
            }
        }
        None
    }
}
fn enqueue_reserved<P: Payload>(
    queue: &mut VecDeque<P>,
    generation: Generation,
    current: Generation,
    bytes: P,
) -> Result<(), (QueueError, P)> {
    if generation != current {
        return Err((QueueError::StaleGeneration, bytes));
    }
    if bytes.payload_bytes() > MAX_RESERVED_BYTES {
        return Err((QueueError::Oversized, bytes));
    }
    if queue.len() >= RESERVED_ITEMS {
        return Err((QueueError::Full, bytes));
    }
    queue.push_back(bytes);
    Ok(())
}
