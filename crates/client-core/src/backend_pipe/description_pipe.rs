//! Generation-bound complete-document mailbox. Reassembly/hash work stays off UI.
use super::*;
use std::sync::atomic::{AtomicBool, Ordering};
use studio_protocol::robot_description::{Chunk, Reassembly};

#[derive(Clone, Debug)]
pub struct CapturedDescription {
    pub text: Arc<str>,
    pub sha256: String,
}
impl std::ops::Deref for CapturedDescription {
    type Target = str;
    fn deref(&self) -> &str {
        &self.text
    }
}
type CaptureResult = Result<CapturedDescription, String>;
#[derive(Clone)]
pub struct DescriptionStream(Arc<Stream>);
struct Stream {
    cancelled: AtomicBool,
    completed: AtomicBool,
    reassembly: Mutex<Reassembly>,
    result: Mutex<Option<CaptureResult>>,
}
impl DescriptionStream {
    pub fn new(topic: String) -> Result<Self, String> {
        Ok(Self(Arc::new(Stream {
            cancelled: AtomicBool::new(false),
            completed: AtomicBool::new(false),
            reassembly: Mutex::new(Reassembly::new(topic)?),
            result: Mutex::new(None),
        })))
    }
    /// Cancelling never waits for hashing/decoding. Late completions are discarded.
    pub fn stop(&self) {
        self.0.cancelled.store(true, Ordering::Release);
        self.0.result.lock().unwrap().take();
    }
    pub fn take(&self) -> Option<CaptureResult> {
        let mut result = self.0.result.lock().unwrap();
        if self.0.cancelled.load(Ordering::Acquire) {
            result.take();
            return None;
        }
        result.take()
    }
    fn accept(&self, chunk: &Chunk, sequence: u64) {
        if self.0.cancelled.load(Ordering::Acquire) || self.0.completed.load(Ordering::Acquire) {
            return;
        }
        let output = if sequence != chunk.index as u64 + 1 {
            Err("Description sequence does not match ordered chunk index".into())
        } else {
            self.0.reassembly.lock().unwrap().push(chunk)
        };
        let output = match output {
            Ok(None) => return,
            Ok(Some(text)) => Ok(CapturedDescription {
                text: Arc::from(text),
                sha256: chunk.sha256.clone(),
            }),
            Err(error) => Err(error),
        };
        self.0.completed.store(true, Ordering::Release);
        let mut result = self.0.result.lock().unwrap();
        if !self.0.cancelled.load(Ordering::Acquire) && result.is_none() {
            *result = Some(output);
        }
    }
}

#[derive(Clone)]
pub struct DescriptionTarget {
    pub generation: Generation,
    pub subscription: SubscriptionId,
    pub topic: String,
    pub stream: DescriptionStream,
}
pub(super) fn prepare(
    shared: &mut Shared,
    generation: Generation,
    subscription: SubscriptionId,
    sequence: u64,
) -> Option<DescriptionTarget> {
    let target = shared
        .descriptions
        .iter()
        .find(|t| t.generation == generation && t.subscription == subscription)?
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
pub(super) fn accept(target: &DescriptionTarget, sample: &Chunk, sequence: u64) {
    target.stream.accept(sample, sequence);
}

impl Shared {
    pub fn bind_description(&mut self, target: DescriptionTarget) -> Result<(), String> {
        if !self.descriptions.is_empty()
            || !self.registry.entries().iter().any(|e| {
                e.generation == target.generation
                    && e.subscription == target.subscription
                    && e.configuration.topic == target.topic
                    && e.configuration.output == Output::RobotDescription
            })
        {
            return Err("Description capture binding requires one allocated exact owner".into());
        }
        self.descriptions.push(target);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_protocol::robot_description::{Document, MAX_CHUNKS, MAX_DOCUMENT_BYTES};
    fn setup() -> (Shared, StartIntent, DescriptionStream) {
        let mut shared = Shared::new(0);
        shared
            .ingest_for_test(Event::ready("jazzy".into(), "fixture-no-ROS".into()))
            .unwrap();
        let intent = shared
            .registry
            .start_explicit(
                ViewId(1),
                Configuration {
                    map_updates: None,
                    topic: "/robot_description".into(),
                    message_type: "std_msgs/msg/String".into(),
                    best_effort: false,
                    transient_local: true,
                    output: Output::RobotDescription,
                    spatial: None,
                    robot: None,
                },
            )
            .unwrap();
        shared
            .queue
            .register(intent.generation, intent.subscription)
            .unwrap();
        let stream = DescriptionStream::new("/robot_description".into()).unwrap();
        shared
            .bind_description(DescriptionTarget {
                generation: intent.generation,
                subscription: intent.subscription,
                topic: "/robot_description".into(),
                stream: stream.clone(),
            })
            .unwrap();
        shared
            .ingest_for_test(Event::Started {
                generation: intent.generation,
                subscription: intent.subscription,
                request: intent.request,
            })
            .unwrap();
        (shared, intent, stream)
    }
    fn event(intent: &StartIntent, document: &Document, index: usize) -> Event {
        Event::RobotDescriptionObservation {
            generation: intent.generation,
            subscription: intent.subscription,
            sequence: index as u64 + 1,
            sample: Box::new(document.chunk(index).unwrap()),
            backend_drops: 0,
        }
    }
    #[test]
    fn maximum_capture_bypasses_fair_sample_coalescing_and_delivers_one_complete_document() {
        let (mut shared, intent, stream) = setup();
        let text = "x".repeat(MAX_DOCUMENT_BYTES);
        let document = Document::new("/robot_description", text.clone()).unwrap();
        for index in 0..MAX_CHUNKS {
            shared
                .ingest_for_test(event(&intent, &document, index))
                .unwrap();
            if index + 1 != MAX_CHUNKS {
                assert!(stream.take().is_none());
            }
        }
        assert_eq!(&*stream.take().unwrap().unwrap(), text);
        assert_eq!(shared.queue.queued_samples(), 0);
        // A second document cannot replace the completed capture, even after take.
        let mut late = event(&intent, &document, 0);
        if let Event::RobotDescriptionObservation { sequence, .. } = &mut late {
            *sequence = 7;
        }
        shared.ingest_for_test(late).unwrap();
        assert!(stream.take().is_none());
    }
    #[test]
    fn invalid_complete_chunk_is_scoped_and_cancellation_suppresses_queued_success() {
        for mode in 0..4 {
            let (mut shared, intent, stream) = setup();
            let document = Document::new("/robot_description", "a".into()).unwrap();
            let mut bad = event(&intent, &document, 0);
            if let Event::RobotDescriptionObservation {
                sample, sequence, ..
            } = &mut bad
            {
                match mode {
                    0 => sample.topic = "/other".into(),
                    1 => sample.sha256 = "0".repeat(64),
                    2 => sample.data_base64 = "YR==".into(),
                    _ => *sequence = 2,
                }
            }
            // Geometry errors are not protocol disconnects of an unrelated peer.
            shared.ingest_for_test(bad).unwrap();
            assert!(stream.take().unwrap().is_err());
            assert_eq!(shared.registry.entries().len(), 1);
        }
        let (mut shared, intent, stream) = setup();
        let document = Document::new("/robot_description", "a".into()).unwrap();
        shared
            .ingest_for_test(event(&intent, &document, 0))
            .unwrap();
        stream.stop();
        assert!(stream.take().is_none());
    }
    #[test]
    fn stopping_and_old_generation_cannot_complete_capture_or_bind_to_a_peer() {
        let (mut shared, intent, stream) = setup();
        let document = Document::new("/robot_description", "x".repeat(MAX_DOCUMENT_BYTES)).unwrap();
        shared
            .ingest_for_test(event(&intent, &document, 0))
            .unwrap();
        let stop = shared.registry.stop_explicit(intent.subscription).unwrap();
        for index in 1..MAX_CHUNKS {
            shared
                .ingest_for_test(event(&intent, &document, index))
                .unwrap();
        }
        assert!(stream.take().is_none());
        shared
            .ingest_for_test(Event::Stopped {
                generation: stop.generation,
                request: stop.request,
                subscription: stop.subscription,
            })
            .unwrap();
        assert!(shared.descriptions.is_empty());
        for index in 0..MAX_CHUNKS {
            shared
                .ingest_for_test(event(&intent, &document, index))
                .unwrap();
        }
        assert!(stream.take().is_none());
        shared.registry.disconnect();
        shared.registry.connect().unwrap();
        for index in 0..MAX_CHUNKS {
            shared
                .ingest_for_test(event(&intent, &document, index))
                .unwrap();
        }
        assert!(shared.registry.entries().is_empty());
        assert!(stream.take().is_none());
    }
}
