//! Shared test-only native GPU/App frame and measurement helpers.
use crate::{fonts, shell, App, Instant};
use eframe::{egui, egui_wgpu, wgpu};
use std::sync::Arc;
fn block_on<F: std::future::Future>(future: F) -> F::Output {
    struct Wake(std::thread::Thread);
    impl std::task::Wake for Wake {
        fn wake(self: Arc<Self>) {
            self.0.unpark();
        }
    }
    let waker = std::task::Waker::from(Arc::new(Wake(std::thread::current())));
    let mut context = std::task::Context::from_waker(&waker);
    let mut future = std::pin::pin!(future);
    loop {
        match future.as_mut().poll(&mut context) {
            std::task::Poll::Ready(v) => return v,
            std::task::Poll::Pending => std::thread::park(),
        }
    }
}
pub(crate) fn gpu() -> egui_wgpu::RenderState {
    let instance = wgpu::Instance::new(&Default::default());
    let adapter =
        block_on(instance.request_adapter(&Default::default())).expect("Native GPU required");
    eprintln!("App load adapter: {:?}", adapter.get_info());
    let (device, queue) = block_on(adapter.request_device(
        &wgpu::DeviceDescriptor {
            label: Some("App load qualification"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
            memory_hints: Default::default(),
        },
        None,
    ))
    .unwrap();
    let renderer = Arc::new(egui::mutex::RwLock::new(egui_wgpu::Renderer::new(
        &device,
        wgpu::TextureFormat::Rgba8Unorm,
        None,
        1,
        false,
    )));
    egui_wgpu::RenderState {
        adapter,
        available_adapters: vec![],
        renderer,
        device,
        queue,
        target_format: wgpu::TextureFormat::Rgba8Unorm,
    }
}
pub(crate) struct Surface {
    state: egui_wgpu::RenderState,
    texture: wgpu::Texture,
    ctx: egui::Context,
    start: Instant,
}
impl Surface {
    pub(crate) fn new(state: egui_wgpu::RenderState) -> Self {
        let texture = state.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("App offscreen presentation"),
            size: wgpu::Extent3d {
                width: 1920,
                height: 1080,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: state.target_format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        });
        let ctx = egui::Context::default();
        fonts::configure(&ctx);
        shell::configure(&ctx);
        Self {
            state,
            texture,
            ctx,
            start: Instant::now(),
        }
    }
    #[cfg(unix)]
    pub(crate) fn frame(&self, app: &mut App) -> [f64; 3] {
        self.frame_with_size(app, [1920, 1080])
    }
    pub(crate) fn frame_with_size(&self, app: &mut App, size: [u32; 2]) -> [f64; 3] {
        assert!(size[0] > 0 && size[0] <= 1920 && size[1] > 0 && size[1] <= 1080);
        #[cfg(target_os = "macos")]
        {
            objc2::rc::autoreleasepool(|_| self.frame_inner(app, size))
        }
        #[cfg(not(target_os = "macos"))]
        {
            self.frame_inner(app, size)
        }
    }
    fn frame_inner(&self, app: &mut App, size: [u32; 2]) -> [f64; 3] {
        let start = Instant::now();
        let cpu = crate::frame_trace::thread_seconds();
        let output = self.ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(size[0] as f32, size[1] as f32),
                )),
                time: Some(self.start.elapsed().as_secs_f64()),
                predicted_dt: 1. / 30.,
                ..Default::default()
            },
            |ctx| app.update_frame(ctx),
        );
        let app_ms = start.elapsed().as_secs_f64() * 1000.;
        let app_cpu_ms = (crate::frame_trace::thread_seconds() - cpu) * 1000.;
        let jobs = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [1920, 1080],
            pixels_per_point: output.pixels_per_point,
        };
        let mut encoder = self
            .state
            .device
            .create_command_encoder(&Default::default());
        let mut renderer = self.state.renderer.write();
        for (id, delta) in &output.textures_delta.set {
            renderer.update_texture(&self.state.device, &self.state.queue, *id, delta);
        }
        let mut commands = renderer.update_buffers(
            &self.state.device,
            &self.state.queue,
            &mut encoder,
            &jobs,
            &screen,
        );
        {
            let view = self.texture.create_view(&Default::default());
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("Complete App egui pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                occlusion_query_set: None,
                timestamp_writes: None,
            });
            renderer.render(&mut pass.forget_lifetime(), &jobs, &screen);
        }
        commands.push(encoder.finish());
        self.state.queue.submit(commands);
        let _ = self.state.device.poll(wgpu::Maintain::Wait);
        for id in output.textures_delta.free {
            renderer.free_texture(&id);
        }
        [app_ms, start.elapsed().as_secs_f64() * 1000., app_cpu_ms]
    }
}
#[cfg(unix)]
pub(crate) fn rss() -> u64 {
    let result = std::process::Command::new("ps")
        .env("LC_ALL", "C")
        .args(["-p", &std::process::id().to_string(), "-o", "rss="])
        .output()
        .unwrap();
    assert!(result.status.success());
    String::from_utf8(result.stdout)
        .unwrap()
        .trim()
        .parse()
        .unwrap()
}
#[cfg(unix)]
pub(crate) fn percentile(values: &[f64], p: f64) -> f64 {
    let mut v = values.to_vec();
    v.sort_by(f64::total_cmp);
    v[((v.len() - 1) as f64 * p).ceil() as usize]
}
