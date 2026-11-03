//! One coalesced worker status per explicitly bound image subscription. Session
//! identity and sequence ingress are validated by the caller before publication.
use super::{mailbox::PublishOutcome, FrameMetadata, ImageSampleError, LatestFrames, TypedImage};
use std::sync::{Arc, Mutex};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageUpdate {
    pub sequence: u64,
    /// Unique current-generation observations, including paused/error frames.
    pub received_frames: u64,
    pub backend_drops: u64,
    pub error: Option<String>,
}
/// Owns no raw/base64 history or extra decoded frame. Clones share one status.
/// Construct only after ImageView::start(), retaining that local generation.
#[derive(Clone)]
pub struct ImageStream {
    frames: LatestFrames,
    local_generation: u64,
    latest: Arc<Mutex<Option<ImageUpdate>>>,
}
impl ImageStream {
    pub fn new(frames: LatestFrames, local_generation: u64) -> Self {
        Self {
            frames,
            local_generation,
            latest: Arc::new(Mutex::new(None)),
        }
    }
    /// Caller binds exact session generation/subscription/topic and rechecks
    /// Active under its registry lock after decoding outside that lock.
    /// True means current status accepted, including paused frames and errors;
    /// false means stale/local-stopped or zero/nonincreasing sequence rejected.
    /// A display error never establishes native cleanup: use correlated Stop.
    pub fn publish(
        &self,
        sequence: u64,
        backend_drops: u64,
        result: Result<TypedImage, ImageSampleError>,
    ) -> bool {
        let mut latest = self
            .latest
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if sequence == 0
            || latest
                .as_ref()
                .is_some_and(|update| sequence <= update.sequence)
        {
            return false;
        }
        let previous = latest.as_ref();
        let mut error = previous.and_then(|update| update.error.clone());
        let current = if error.is_some() {
            // Error remains latched and later successful frames cannot restore
            // pixels. The generation guard also protects replacement mailboxes.
            self.frames.discard_if_current(self.local_generation)
        } else {
            match result {
                Ok(image) => {
                    let metadata = FrameMetadata {
                        header_stamp: image.header_stamp,
                        frame_id: image.frame_id,
                        source_time: image.source_time,
                    };
                    !matches!(
                        self.frames.publish_tagged(
                            self.local_generation,
                            image.frame,
                            Some(metadata)
                        ),
                        PublishOutcome::Stale
                    )
                }
                Err(failure) => {
                    error = Some(bounded_error(&failure));
                    self.frames.discard_if_current(self.local_generation)
                }
            }
        };
        if !current {
            return false;
        }
        *latest = Some(ImageUpdate {
            sequence,
            received_frames: previous.map_or(1, |update| update.received_frames.saturating_add(1)),
            backend_drops: previous.map_or(backend_drops, |update| {
                update.backend_drops.max(backend_drops)
            }),
            error,
        });
        true
    }
    pub fn snapshot(&self) -> Option<ImageUpdate> {
        self.latest
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }
}
fn bounded_error(error: &impl std::fmt::Display) -> String {
    use std::fmt::Write;
    const LIMIT: usize = 512;
    struct Bounded(String);
    impl Write for Bounded {
        fn write_str(&mut self, value: &str) -> std::fmt::Result {
            let available = LIMIT.saturating_sub(self.0.len());
            let mut end = available.min(value.len());
            while !value.is_char_boundary(end) {
                end -= 1;
            }
            self.0.push_str(&value[..end]);
            if end < value.len() {
                Err(std::fmt::Error)
            } else {
                Ok(())
            }
        }
    }
    let mut output = Bounded(String::with_capacity(LIMIT));
    if write!(&mut output, "{error}").is_err() {
        let marker = " … [truncated]";
        let mut end = LIMIT - marker.len();
        while !output.0.is_char_boundary(end) {
            end -= 1;
        }
        output.0.truncate(end);
        output.0.push_str(marker);
    }
    output.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::images::{decode, HeaderStamp, ImageView, RawFrame};
    use eframe::egui;

    fn image(sequence: u64) -> TypedImage {
        TypedImage {
            frame: decode(RawFrame {
                width: 1,
                height: 1,
                encoding: "mono8",
                step: 1,
                big_endian: false,
                data: &[sequence as u8],
            })
            .unwrap(),
            header_stamp: HeaderStamp {
                seconds: sequence as i32,
                nanoseconds: 123,
            },
            frame_id: format!("camera_{sequence}"),
            source_time: None,
        }
    }
    fn draw(view: &mut ImageView, context: &egui::Context) {
        let _ = context.run(egui::RawInput::default(), |context| {
            egui::CentralPanel::default().show(context, |ui| view.show(ui));
        });
    }

    #[test]
    fn worker_flood_keeps_only_latest_pixels_metadata_and_coalesced_status() {
        let frames = LatestFrames::default();
        let generation = frames.start();
        let stream = ImageStream::new(frames.clone(), generation);
        let worker = stream.clone();
        std::thread::spawn(move || {
            for sequence in 1..=1000 {
                assert!(worker.publish(sequence, sequence % 17, Ok(image(sequence))));
            }
        })
        .join()
        .unwrap();
        assert_eq!(
            stream.snapshot(),
            Some(ImageUpdate {
                sequence: 1000,
                received_frames: 1000,
                backend_drops: 16,
                error: None
            })
        );
        let (frame, metadata) = frames.take_with_metadata().unwrap();
        assert_eq!(frame.pixels()[0].r(), 1000u64 as u8);
        let metadata = metadata.unwrap();
        assert_eq!(metadata.header_stamp.seconds, 1000);
        assert_eq!(metadata.frame_id, "camera_1000");
        assert_eq!(metadata.source_time, None);
        assert!(frames.take_with_metadata().is_none());
        assert_eq!(frames.counters().superseded, 999);
    }

    #[test]
    fn paused_display_keeps_its_own_metadata_until_pixels_update_together() {
        let context = egui::Context::default();
        let mut view = ImageView::default();
        let generation = view.start();
        let stream = ImageStream::new(view.frames(), generation);
        assert!(stream.publish(1, 2, Ok(image(1))));
        draw(&mut view, &context);
        assert_eq!(view.displayed_metadata().unwrap().frame_id, "camera_1");
        view.set_paused(true);
        assert!(stream.publish(2, 1, Ok(image(2))));
        draw(&mut view, &context);
        assert_eq!(view.displayed_metadata().unwrap().frame_id, "camera_1");
        assert_eq!(view.frames().counters().paused_skips, 1);
        assert_eq!(stream.snapshot().unwrap().received_frames, 2);
        assert_eq!(stream.snapshot().unwrap().backend_drops, 2);
        view.set_paused(false);
        draw(&mut view, &context);
        assert_eq!(view.displayed_metadata().unwrap().frame_id, "camera_1");
        assert!(stream.publish(3, 4, Ok(image(3))));
        draw(&mut view, &context);
        assert_eq!(view.displayed_metadata().unwrap().frame_id, "camera_3");
        view.stop();
        assert!(view.displayed_metadata().is_none());
        assert!(!view.has_displayed_frame());
    }

    #[test]
    fn stale_worker_error_cannot_clear_or_stop_a_replacement_mailbox() {
        let mut view = ImageView::default();
        let first = view.start();
        let old = ImageStream::new(view.frames(), first);
        assert!(old.publish(1, 0, Ok(image(1))));
        let next = view.start();
        let current = ImageStream::new(view.frames(), next);
        assert!(current.publish(1, 0, Ok(image(9))));
        assert!(!old.publish(2, 99, Err(ImageSampleError::Compact("old failure".into()))));
        let (frame, metadata) = view.frames().take_with_metadata().unwrap();
        assert_eq!(frame.pixels()[0].r(), 9);
        assert_eq!(metadata.unwrap().frame_id, "camera_9");
        assert_eq!(old.snapshot().unwrap().error, None);
        assert_eq!(current.snapshot().unwrap().error, None);
        assert!(current.publish(2, 0, Ok(image(10))));
        view.stop();
        assert!(!current.publish(3, 0, Err(ImageSampleError::Incomplete)));
        assert_eq!(current.snapshot().unwrap().sequence, 2);
    }

    #[test]
    fn local_error_latches_bounded_diagnostic_without_claiming_native_cleanup() {
        let frames = LatestFrames::default();
        let generation = frames.start();
        let stream = ImageStream::new(frames.clone(), generation);
        assert!(stream.publish(1, 9, Ok(image(1))));
        assert!(stream.publish(2, 2, Err(ImageSampleError::Compact("🤖".repeat(2000)))));
        assert!(frames.take().is_none());
        let error = stream.snapshot().unwrap().error.unwrap();
        assert!(error.len() <= 512);
        assert!(error.ends_with("[truncated]"));
        assert!(stream.publish(3, 1, Ok(image(3))));
        assert!(frames.take().is_none());
        let update = stream.snapshot().unwrap();
        assert_eq!(update.error, Some(error));
        assert_eq!(update.received_frames, 3);
        assert_eq!(update.backend_drops, 9);
        // Helper never deactivates even its own mailbox; root must correlate
        // Stop with the backend before releasing the native subscription slot.
        assert!(frames.publish(generation, image(4).frame));
        let replacement = frames.start();
        let replacement = ImageStream::new(frames.clone(), replacement);
        assert!(replacement.snapshot().is_none());
        assert!(replacement.publish(1, 0, Ok(image(5))));
        assert_eq!(replacement.snapshot().unwrap().error, None);
    }

    #[test]
    fn zero_repeated_or_reordered_sequences_cannot_change_status_or_pixels() {
        let frames = LatestFrames::default();
        let generation = frames.start();
        let stream = ImageStream::new(frames.clone(), generation);
        assert!(stream.publish(2, 3, Ok(image(2))));
        for sequence in [0, 1, 2] {
            assert!(!stream.publish(sequence, 99, Err(ImageSampleError::Incomplete)));
        }
        assert_eq!(
            stream.snapshot(),
            Some(ImageUpdate {
                sequence: 2,
                received_frames: 1,
                backend_drops: 3,
                error: None
            })
        );
        assert_eq!(frames.take().unwrap().pixels()[0].r(), 2);
    }

    #[test]
    fn legacy_frames_never_inherit_a_previous_displayed_header() {
        let context = egui::Context::default();
        let mut view = ImageView::default();
        let generation = view.start();
        let stream = ImageStream::new(view.frames(), generation);
        assert!(stream.publish(1, 0, Ok(image(1))));
        draw(&mut view, &context);
        assert!(view.displayed_metadata().is_some());
        assert!(view.frames().publish(generation, image(2).frame));
        draw(&mut view, &context);
        assert!(view.has_displayed_frame());
        assert!(view.displayed_metadata().is_none());
        let image = image(3);
        assert!(view.frames().publish_with_metadata(
            generation,
            image.frame,
            FrameMetadata {
                header_stamp: image.header_stamp,
                frame_id: image.frame_id,
                source_time: image.source_time
            }
        ));
        assert_eq!(view.frames().take().unwrap().pixels()[0].r(), 3);
    }
}
