//! One capture per owner. A slow writer retains at most one queued chunk.
use crate::observation::{EncodedPayload, Reader};
use std::{
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};
use studio_protocol::{robot_description::Document, subscriptions::*};

pub(crate) fn run(
    reader: &mut dyn Reader,
    topic: &str,
    generation: Generation,
    subscription: SubscriptionId,
    queue: &Arc<Mutex<FairQueue<EncodedPayload>>>,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let mut document = None;
    let mut next = 0;
    while !cancel.load(Ordering::Acquire) {
        if document.is_none() {
            if let Some(text) = reader.poll_description()? {
                document = Some(Document::new(topic, text)?);
            } else {
                thread::sleep(Duration::from_millis(1));
                continue;
            }
        }
        // Check before encoding, keeping both queue pressure and work bounded.
        let available = queue.lock().unwrap().stream_queued_samples(subscription);
        match available {
            None => return Ok(()), // authoritative Stop retired this owner
            Some(0) => (),
            Some(_) => {
                thread::sleep(Duration::from_millis(1));
                continue;
            }
        }
        if let Some(chunk) = document.as_ref().unwrap().chunk(next) {
            let bytes = EncodedPayload::RobotDescription(chunk.encode()?.into_boxed_slice());
            if cancel.load(Ordering::Acquire) {
                break;
            }
            let result = queue.lock().unwrap().enqueue_sample(
                generation,
                subscription,
                next as u64 + 1,
                bytes,
            );
            match result {
                Ok(()) => next += 1,
                Err(QueueError::UnknownStream | QueueError::StaleGeneration) => return Ok(()),
                Err(error) => {
                    return Err(format!(
                        "Description transfer queue rejected chunk: {error:?}"
                    ))
                }
            }
        } else {
            // No second document, supersession or retry while awaiting exact Stop.
            thread::sleep(Duration::from_millis(1));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{sync::atomic::AtomicUsize, time::Instant};
    use studio_protocol::robot_description::{Chunk, Reassembly, MAX_CHUNKS, MAX_DOCUMENT_BYTES};
    struct One {
        text: Option<String>,
        polls: Arc<AtomicUsize>,
        drops: Arc<AtomicUsize>,
    }
    impl Drop for One {
        fn drop(&mut self) {
            self.drops.fetch_add(1, Ordering::SeqCst);
        }
    }
    impl Reader for One {
        fn poll(&mut self) -> Result<Option<studio_protocol::structured::Value>, String> {
            panic!("Description must bypass generic preview")
        }
        fn poll_description(&mut self) -> Result<Option<String>, String> {
            self.polls.fetch_add(1, Ordering::SeqCst);
            Ok(self.text.take())
        }
    }
    fn wait(mut predicate: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(3);
        while !predicate() {
            assert!(
                Instant::now() < deadline,
                "Description worker made no progress"
            );
            // Leave time for the producer to acquire the same queue mutex.
            thread::sleep(Duration::from_millis(1));
        }
    }
    #[test]
    fn slow_writer_retains_one_chunk_with_peer_fairness_then_waits_for_stop() {
        let queue = Arc::new(Mutex::new(FairQueue::with_payload(Generation(1))));
        queue
            .lock()
            .unwrap()
            .register(Generation(1), SubscriptionId(1))
            .unwrap();
        queue
            .lock()
            .unwrap()
            .register(Generation(1), SubscriptionId(2))
            .unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let polls = Arc::new(AtomicUsize::new(0));
        let drops = Arc::new(AtomicUsize::new(0));
        let text = "x".repeat(MAX_DOCUMENT_BYTES);
        let mut reader = One {
            text: Some(text.clone()),
            polls: polls.clone(),
            drops: drops.clone(),
        };
        let (q, c) = (queue.clone(), cancel.clone());
        let worker = thread::spawn(move || {
            run(
                &mut reader,
                "/robot_description",
                Generation(1),
                SubscriptionId(1),
                &q,
                &c,
            )
        });
        wait(|| {
            queue
                .lock()
                .unwrap()
                .stream_queued_samples(SubscriptionId(1))
                == Some(1)
        });
        // Fill a peer lane while deliberately holding the capture's first chunk.
        for sequence in 1..=100 {
            let mut q = queue.lock().unwrap();
            q.enqueue_sample(
                Generation(1),
                SubscriptionId(2),
                sequence,
                EncodedPayload::Typed(vec![0; 512 * 1024].into_boxed_slice()),
            )
            .unwrap();
            assert_eq!(q.stream_queued_samples(SubscriptionId(1)), Some(1));
            assert_eq!(
                q.stream_counters(SubscriptionId(1))
                    .unwrap()
                    .evicted_samples,
                0
            );
        }
        let mut receiver = Reassembly::new("/robot_description".into()).unwrap();
        let mut count = 0;
        let mut complete = None;
        wait(|| {
            let packet = queue.lock().unwrap().pop();
            if let Some(Packet::Sample {
                subscription: SubscriptionId(1),
                sequence,
                bytes,
                ..
            }) = packet
            {
                assert_eq!(sequence, count + 1);
                let EncodedPayload::RobotDescription(bytes) = bytes else {
                    panic!("Wrong output");
                };
                complete = receiver.push(&Chunk::decode(&bytes).unwrap()).unwrap();
                count += 1;
            }
            count == MAX_CHUNKS as u64
        });
        assert_eq!(complete.unwrap(), text);
        assert_eq!(polls.load(Ordering::SeqCst), 1);
        assert_eq!(drops.load(Ordering::SeqCst), 0);
        cancel.store(true, Ordering::Release);
        worker.join().unwrap().unwrap();
        assert_eq!(drops.load(Ordering::SeqCst), 1);
    }
    #[test]
    fn cancellation_before_poll_and_retirement_during_transfer_release_exact_reader() {
        for initially_cancelled in [true, false] {
            let queue = Arc::new(Mutex::new(FairQueue::with_payload(Generation(1))));
            queue
                .lock()
                .unwrap()
                .register(Generation(1), SubscriptionId(1))
                .unwrap();
            let polls = Arc::new(AtomicUsize::new(0));
            let drops = Arc::new(AtomicUsize::new(0));
            let mut reader = One {
                text: Some("x".repeat(MAX_DOCUMENT_BYTES)),
                polls: polls.clone(),
                drops: drops.clone(),
            };
            let (q, p) = (queue.clone(), polls.clone());
            let worker = thread::spawn(move || {
                run(
                    &mut reader,
                    "/robot_description",
                    Generation(1),
                    SubscriptionId(1),
                    &q,
                    &AtomicBool::new(initially_cancelled),
                )
            });
            if !initially_cancelled {
                wait(|| {
                    p.load(Ordering::SeqCst) == 1 && queue.lock().unwrap().queued_samples() == 1
                });
                queue.lock().unwrap().unregister(SubscriptionId(1)).unwrap();
            }
            worker.join().unwrap().unwrap();
            assert_eq!(
                polls.load(Ordering::SeqCst),
                usize::from(!initially_cancelled)
            );
            assert_eq!(drops.load(Ordering::SeqCst), 1);
        }
    }
}
