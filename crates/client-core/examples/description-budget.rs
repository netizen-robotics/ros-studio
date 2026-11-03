//! Repeatable live Rust-heap/latency probe of the bounded description transport.
//! No DDS, native UI or RSS claim. Run with --features test-support.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    sync::atomic::{AtomicUsize, Ordering},
    time::Instant,
};
use studio_client_core::backend_pipe::{DescriptionStream, DescriptionTarget, Shared};
use studio_protocol::{
    robot_description::{Document, MAX_CHUNKS, MAX_DOCUMENT_BYTES},
    session::Event,
    subscriptions::*,
};

struct Measured;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
fn add(bytes: usize) {
    let live = LIVE.fetch_add(bytes, Ordering::Relaxed) + bytes;
    PEAK.fetch_max(live, Ordering::Relaxed);
}
unsafe impl GlobalAlloc for Measured {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let pointer = System.alloc(layout);
        if !pointer.is_null() {
            add(layout.size());
        }
        pointer
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let pointer = System.alloc_zeroed(layout);
        if !pointer.is_null() {
            add(layout.size());
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        System.dealloc(pointer, layout);
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        let next = System.realloc(pointer, layout, size);
        if !next.is_null() {
            if size >= layout.size() {
                add(size - layout.size());
            } else {
                LIVE.fetch_sub(layout.size() - size, Ordering::Relaxed);
            }
        }
        next
    }
}
#[global_allocator]
static ALLOCATOR: Measured = Measured;

fn main() {
    let mut rows = Vec::with_capacity(12);
    for iteration in 0..12 {
        let mut shared = Shared::new(0);
        shared
            .ingest_for_test(Event::ready("jazzy".into(), "in-process-no-ROS".into()))
            .unwrap();
        let intent = shared
            .registry
            .start_explicit(
                ViewId(1),
                Configuration {
                    map_updates: None,
                    topic: "/robot_description".into(),
                    message_type: "std_msgs/msg/String".into(),
                    output: Output::RobotDescription,
                    best_effort: false,
                    transient_local: true,
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
        let baseline = LIVE.load(Ordering::Relaxed);
        PEAK.store(baseline, Ordering::Relaxed);
        let start = Instant::now();
        let document = Document::new("/robot_description", "a".repeat(MAX_DOCUMENT_BYTES)).unwrap();
        for index in 0..MAX_CHUNKS {
            // At most one producer chunk plus bounded JSON frame, exactly as
            // slow-writer flow control permits. Test ingress uses the production
            // frame decoder and owner/reassembly path; no OS pipes are measured.
            shared
                .ingest_for_test(Event::RobotDescriptionObservation {
                    generation: intent.generation,
                    subscription: intent.subscription,
                    sequence: index as u64 + 1,
                    sample: Box::new(document.chunk(index).unwrap()),
                    backend_drops: 0,
                })
                .unwrap();
        }
        let output = stream.take().unwrap().unwrap();
        assert_eq!(output.len(), MAX_DOCUMENT_BYTES);
        assert!(output.bytes().all(|b| b == b'a'));
        let elapsed = start.elapsed().as_secs_f64();
        let peak = PEAK.load(Ordering::Relaxed).saturating_sub(baseline);
        // Explicit guard on retained Rust heap, not a machine RSS budget.
        assert!(
            peak < 6 * MAX_DOCUMENT_BYTES,
            "Description transport heap regressed: {peak}"
        );
        rows.push(serde_json::json!({"iteration": iteration, "document_bytes": output.len(), "chunks": MAX_CHUNKS, "added_peak_rust_heap_bytes": peak, "elapsed_seconds": elapsed}));
    }
    println!(
        "{}",
        serde_json::json!({"scope":"In-process producer/chunk framing/client reassembly, debug profile; excludes DDS/native allocation, allocator overhead, RSS, asset parser, UI and OS pipe latency", "runs": rows})
    );
}
