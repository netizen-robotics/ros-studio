use eframe::egui;
use studio_launcher::suite::{Job, Snapshot, Worker};
mod presentation;
#[cfg(debug_assertions)]
mod preview;

struct Launcher {
    worker: Worker,
    snapshot: Snapshot,
}
impl Launcher {
    fn new() -> Self {
        let root = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(std::path::Path::to_path_buf))
            .unwrap_or_default();
        let mut worker = Worker::new(root);
        worker.submit(Job::Inspect);
        Self {
            worker,
            snapshot: Snapshot {
                available: [false; 2],
                error: None,
            },
        }
    }
}
impl eframe::App for Launcher {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        if let Some(snapshot) = self.worker.poll() {
            self.snapshot = snapshot;
        }
        let intent = presentation::Page {
            snapshot: &self.snapshot,
            busy: self.worker.busy,
        }
        .show(ctx);
        if let Some(job) = intent {
            self.worker.submit(job);
        }
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
    }
    fn on_exit(&mut self) {
        self.worker.shutdown();
    }
}
fn main() -> eframe::Result {
    if std::env::args().any(|arg| arg == "--suite-configuration") {
        println!(
            "{}",
            serde_json::json!({"version":env!("CARGO_PKG_VERSION"),"distribution":if cfg!(feature="store-distribution"){"microsoft-store"}else{"bundled"}})
        );
        return Ok(());
    }
    let state = studio_app_lifecycle::state_directory()
        .map_err(|e| eframe::Error::AppCreation(std::io::Error::other(e).into()))?;
    let _lease = studio_app_lifecycle::AppLease::acquire(&state, "launcher")
        .map_err(|e| eframe::Error::AppCreation(std::io::Error::other(e).into()))?;
    let compact = cfg!(debug_assertions)
        && std::env::var("STUDIO_LAUNCHER_UI_FIXTURE").is_ok()
        && std::env::var("STUDIO_LAUNCHER_COMPACT_FIXTURE").as_deref() == Ok("1");
    eframe::run_native(
        "ROS Studio",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size(if compact {
                    [560.0, 560.0]
                } else {
                    [860.0, 560.0]
                })
                .with_min_inner_size([560.0, 500.0])
                .with_icon(studio_ui_foundation::app_icons::AppIcon::Launcher.window_icon()),
            ..Default::default()
        },
        Box::new(|cc| {
            studio_ui_foundation::fonts::configure(&cc.egui_ctx);
            studio_ui_foundation::theme::configure(&cc.egui_ctx);
            #[cfg(debug_assertions)]
            if let Ok(mode) = std::env::var("STUDIO_LAUNCHER_UI_FIXTURE") {
                return Ok(Box::new(preview::Preview::new(&mode)));
            }
            Ok(Box::new(Launcher::new()))
        }),
    )
}
