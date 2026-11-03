use super::{DecodedFrame, HeaderStamp};
use std::sync::{Arc, Mutex};
use studio_protocol::structured::SourceTime;

/// Metadata travels atomically with its pixels, never as a separate latest slot.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameMetadata {
    pub header_stamp: HeaderStamp,
    pub frame_id: String,
    pub source_time: Option<SourceTime>,
}
pub(super) enum PublishOutcome {
    Published,
    Paused,
    Stale,
}

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct Counters {
    pub superseded: u64,
    pub paused_skips: u64,
    pub stale_rejected: u64,
}
#[derive(Default)]
struct Slot {
    generation: u64,
    active: bool,
    paused: bool,
    pending: Option<(DecodedFrame, Option<FrameMetadata>)>,
    counters: Counters,
}
/// Exactly one pending decoded frame. Never queues a history. Generation guards
/// prevent an old decoding worker from repopulating a stopped/replaced view.
#[derive(Clone, Default)]
pub struct LatestFrames(Arc<Mutex<Slot>>);
impl LatestFrames {
    pub fn start(&self) -> u64 {
        let mut slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        slot.generation = slot
            .generation
            .checked_add(1)
            .expect("image generation exhausted");
        slot.pending = None;
        slot.active = true;
        slot.paused = false;
        slot.counters = Counters::default();
        slot.generation
    }
    pub fn stop(&self) {
        let mut slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        slot.active = false;
        slot.paused = false;
        slot.pending = None;
    }
    pub fn set_paused(&self, paused: bool) {
        let mut slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        slot.paused = paused;
        if paused && slot.pending.take().is_some() {
            slot.counters.paused_skips = slot.counters.paused_skips.saturating_add(1);
        }
    }
    pub fn publish(&self, generation: u64, frame: DecodedFrame) -> bool {
        matches!(
            self.publish_tagged(generation, frame, None),
            PublishOutcome::Published
        )
    }
    pub fn publish_with_metadata(
        &self,
        generation: u64,
        frame: DecodedFrame,
        metadata: FrameMetadata,
    ) -> bool {
        matches!(
            self.publish_tagged(generation, frame, Some(metadata)),
            PublishOutcome::Published
        )
    }
    pub(super) fn publish_tagged(
        &self,
        generation: u64,
        frame: DecodedFrame,
        metadata: Option<FrameMetadata>,
    ) -> PublishOutcome {
        let mut slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if !slot.active || generation != slot.generation {
            slot.counters.stale_rejected = slot.counters.stale_rejected.saturating_add(1);
            return PublishOutcome::Stale;
        }
        if slot.paused {
            slot.counters.paused_skips = slot.counters.paused_skips.saturating_add(1);
            return PublishOutcome::Paused;
        }
        if slot.pending.replace((frame, metadata)).is_some() {
            slot.counters.superseded = slot.counters.superseded.saturating_add(1);
        }
        PublishOutcome::Published
    }
    /// A local decode error may discard its own pending frame, but cannot stop
    /// native work, deactivate a view or touch a replacement generation.
    pub(super) fn discard_if_current(&self, generation: u64) -> bool {
        let mut slot = self.0.lock().unwrap_or_else(|e| e.into_inner());
        if !slot.active || slot.generation != generation {
            slot.counters.stale_rejected = slot.counters.stale_rejected.saturating_add(1);
            return false;
        }
        slot.pending = None;
        true
    }
    pub fn take(&self) -> Option<DecodedFrame> {
        self.take_with_metadata().map(|(frame, _)| frame)
    }
    pub fn take_with_metadata(&self) -> Option<(DecodedFrame, Option<FrameMetadata>)> {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .pending
            .take()
    }
    pub fn counters(&self) -> Counters {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).counters
    }
}
