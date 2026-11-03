//! Synthetic local asset preparation/heap/cancel probe; no GPU/DDS/UI claim.
use std::{
    alloc::{GlobalAlloc, Layout, System},
    path::PathBuf,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    time::{Duration, Instant},
};
use studio_visualization::{
    robot::{
        load_model, load_model_from_text, AssetWorker, LoadOptions, RobotErrorKind, RobotGeometry,
    },
    spatial::robot_geometry::prepare_robot,
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
    let folder = PathBuf::from(std::env::args().nth(1).expect("fixture folder"));
    let options = LoadOptions::default();
    let cancel = AtomicBool::new(false);
    let mut rows = Vec::new();
    for iteration in 0..4 {
        let baseline = LIVE.load(Ordering::Relaxed);
        PEAK.store(baseline, Ordering::Relaxed);
        let began = Instant::now();
        let model = load_model(&folder.join("maximum.urdf"), &options, &cancel).unwrap();
        let prepared = prepare_robot(&model, &cancel).unwrap();
        let images = model
            .links
            .iter()
            .flat_map(|l| &l.visuals)
            .filter_map(|v| match &v.geometry {
                RobotGeometry::MeshWithUv {
                    texture: Some(t), ..
                } => Some(t),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(images.len(), 2);
        assert_eq!(
            images.iter().map(|i| i.rgba.len()).sum::<usize>(),
            32 * 1024 * 1024
        );
        assert_eq!(prepared.triangle_count, 24);
        rows.push(serde_json::json!({"iteration":iteration,"file_prepare_seconds":began.elapsed().as_secs_f64(),"added_peak_rust_heap_bytes":PEAK.load(Ordering::Relaxed)-baseline,"decoded_rgba_bytes":32*1024*1024}));
        let text = std::fs::read_to_string(folder.join("maximum.urdf")).unwrap();
        let topic = load_model_from_text(&text, Some(&folder), &options, &cancel).unwrap();
        assert_eq!(
            prepare_robot(&topic, &cancel).unwrap().triangle_count,
            prepared.triangle_count
        );
    }
    assert_eq!(
        load_model(&folder.join("over.urdf"), &options, &cancel)
            .unwrap_err()
            .kind,
        RobotErrorKind::Limit
    );
    let mut worker = AssetWorker::new().unwrap();
    let id = worker.start(folder.join("maximum.urdf"), options).unwrap();
    std::thread::sleep(Duration::from_millis(2));
    let began = Instant::now();
    assert!(worker.cancel(id));
    let result = loop {
        if let Some(result) = worker.poll() {
            break result;
        }
        assert!(began.elapsed() < Duration::from_secs(10));
        std::thread::sleep(Duration::from_millis(1));
    };
    assert_eq!(result.id, id);
    assert_eq!(result.result.unwrap_err().kind, RobotErrorKind::Cancelled);
    let cancellation = began.elapsed().as_secs_f64();
    worker.finish_shutdown().unwrap();
    println!(
        "{}",
        serde_json::json!({"cases":rows,"file_topic_equivalent":true,"third_image_rejected":true,"cancel_completion_seconds":cancellation,"scope":"Rust heap excludes allocator overhead, codec/native/driver RSS. Cancellation correlated, cooperative; no hard latency guarantee or GPU/UI acceptance."})
    );
}
