//! Development-only native sheet using the product's real fonts and shell styling.
//! No backend, ROS connection, subscription, saved document, or lifecycle evidence.
//! Hover, hold, and Tab through real widgets; the state captions describe their
//! actual egui responses rather than painting simulated interaction states.
use eframe::egui;
use std::{collections::BTreeSet, io::Write, path::PathBuf};

#[path = "../src/fonts.rs"]
mod fonts;
#[path = "../src/shell.rs"]
mod shell;

// Fixture-only adapter required by shell::status. This enum supplies visual
// labels only and does not implement or exercise product subscription behavior.
#[derive(Clone, Copy)]
enum ViewStatus {
    Receiving,
    Waiting,
    Starting,
    Stopping,
    NeedsRecovery,
    Stopped,
}
impl ViewStatus {
    fn label(self) -> &'static str {
        match self {
            Self::Receiving => "Receiving fixture",
            Self::Waiting => "Waiting fixture",
            Self::Starting => "Starting fixture",
            Self::Stopping => "Stopping fixture",
            Self::NeedsRecovery => "Cleanup needed fixture",
            Self::Stopped => "Stopped fixture",
        }
    }
}

struct VisualStates {
    field: String,
    selected: bool,
    clicks: u64,
    capture: Option<PressedCapture>,
}
impl Default for VisualStates {
    fn default() -> Self {
        Self {
            field: "工作空间 · café 🤖".into(),
            selected: true,
            clicks: 0,
            capture: PressedCapture::from_environment(),
        }
    }
}

#[derive(Clone, Debug)]
struct PressedContext {
    control: &'static str,
    theme: &'static str,
    requested_unix_ms: u128,
    logical_size: [f32; 2],
    pixels_per_point: f32,
    response_rect: egui::Rect,
    hovered: bool,
    focused: bool,
}
struct PressedCapture {
    directory: PathBuf,
    requested: BTreeSet<(&'static str, &'static str)>,
    status: String,
}
impl PressedCapture {
    fn from_environment() -> Option<Self> {
        let directory = PathBuf::from(std::env::var_os("STUDIO_COMPONENT_CAPTURE_DIR")?);
        if directory.is_absolute()
            || !directory.starts_with("artifacts/feasibility/evidence")
            || directory
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            eprintln!("STUDIO_COMPONENT_CAPTURE_DIR must be a relative directory under artifacts/feasibility/evidence, without parent traversal; capture disabled");
            return None;
        }
        Some(Self {
            directory,
            requested: BTreeSet::new(),
            status: "Pressed capture enabled; waiting for actual pointer hold".into(),
        })
    }
    fn observe(&mut self, ctx: &egui::Context, control: &'static str, response: &egui::Response) {
        if !response.enabled() || !response.is_pointer_button_down_on() {
            return;
        }
        let theme = if ctx.style().visuals.dark_mode {
            "dark"
        } else {
            "light"
        };
        if !self.requested.insert((theme, control)) {
            return;
        }
        let context = PressedContext {
            control,
            theme,
            requested_unix_ms: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis(),
            logical_size: ctx.screen_rect().size().into(),
            pixels_per_point: ctx.pixels_per_point(),
            response_rect: response.rect,
            hovered: response.hovered(),
            focused: response.has_focus(),
        };
        self.status = format!("Requested actual pressed frame: {theme} / {control}");
        ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(egui::UserData::new(
            context,
        )));
    }
    fn receive(&mut self, ctx: &egui::Context) {
        let screenshots = ctx.input(|input| {
            input
                .events
                .iter()
                .filter_map(|event| {
                    if let egui::Event::Screenshot {
                        user_data, image, ..
                    } = event
                    {
                        let context = user_data
                            .data
                            .as_ref()?
                            .downcast_ref::<PressedContext>()?
                            .clone();
                        Some((context, image.clone()))
                    } else {
                        None
                    }
                })
                .collect::<Vec<_>>()
        });
        for (context, image) in screenshots {
            self.status = match self.save(&context, &image) {
                Ok(path) => format!("Saved actual pressed frame: {}", path.display()),
                Err(error) => {
                    eprintln!("Pressed capture failed: {error}");
                    format!("Pressed capture failed: {error}")
                }
            };
        }
    }
    fn save(&self, context: &PressedContext, image: &egui::ColorImage) -> std::io::Result<PathBuf> {
        std::fs::create_dir_all(&self.directory)?;
        let stem = format!(
            "{}-{}-pressed-{}",
            context.theme, context.control, context.requested_unix_ms
        );
        let path = self.directory.join(format!("{stem}.ppm"));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        let mut writer = std::io::BufWriter::new(file);
        write_ppm(&mut writer, image)?;
        writer.flush()?;
        let mut metadata = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(self.directory.join(format!("{stem}.txt")))?;
        writeln!(metadata, "Native egui screenshot returned for an actual enabled response.is_pointer_button_down_on() == true.\nControl: {}\nOS-derived theme: {}\nRequest Unix milliseconds: {}\nRequest logical size: {:?}\nRequest pixels per logical point: {}\nRequest control rect: {:?}\nRequest hovered: {}\nRequest focused: {}\nReturned physical image size: {:?}\nFormat: RGB PPM P6; native viewport framebuffer, no synthetic rendering or theme override.\nCapture requested during the pressed frame; the asynchronous screenshot event may arrive after pointer release.\nNo ROS/backend/lifecycle behavior is established.", context.control, context.theme, context.requested_unix_ms, context.logical_size, context.pixels_per_point, context.response_rect, context.hovered, context.focused, image.size)?;
        Ok(path)
    }
}
fn write_ppm(writer: &mut impl Write, image: &egui::ColorImage) -> std::io::Result<()> {
    write!(writer, "P6\n{} {}\n255\n", image.size[0], image.size[1])?;
    for pixel in &image.pixels {
        let [red, green, blue, _] = pixel.to_srgba_unmultiplied();
        writer.write_all(&[red, green, blue])?;
    }
    Ok(())
}
pub fn section(ui: &mut egui::Ui, title: &str) {
    ui.add_space(4.0);
    ui.separator();
    ui.strong(title);
}

fn state_caption(ui: &mut egui::Ui, response: &egui::Response) {
    let mut states = Vec::new();
    if !response.enabled() {
        states.push("Disabled");
    } else {
        if response.is_pointer_button_down_on() {
            states.push("Pressed");
        }
        if response.has_focus() {
            states.push("Focused");
        }
        if response.hovered() {
            states.push("Hovered");
        }
        if states.is_empty() {
            states.push("Normal");
        }
    }
    ui.small(states.join(" · "));
}
impl eframe::App for VisualStates {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        if let Some(capture) = &mut self.capture {
            capture.receive(ctx);
        }
        egui::TopBottomPanel::top("sheet-header")
            .frame(egui::Frame::new().fill(ctx.style().visuals.panel_fill).inner_margin(shell::PADDING))
            .show(ctx, |ui| {
                ui.heading("Balanced Studio · component states");
                ui.label("Development fixture · no ROS traffic · follows the operating-system theme");
                ui.small("Hover a control, hold the pointer down, or use Tab / Shift-Tab and Space. Captions report actual interaction state.");
                if let Some(capture) = &self.capture { ui.small(&capture.status); }
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(ctx.style().visuals.extreme_bg_color).inner_margin(shell::PADDING))
            .show(ctx, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    section(ui, "Actions");
                    let action_visuals = ui.visuals().clone();
                    ui.columns(3, |columns| {
                        columns[0].strong("Primary");
                        let response = columns[0].add(shell::primary(&action_visuals, "Subscribe fixture"));
                        if let Some(capture) = &mut self.capture { capture.observe(ctx, "primary", &response); }
                        if response.clicked() { self.clicks = self.clicks.saturating_add(1); }
                        state_caption(&mut columns[0], &response);
                        let disabled = columns[0].add_enabled(false, shell::primary(&action_visuals, "Subscribe unavailable"));
                        state_caption(&mut columns[0], &disabled);
                        columns[0].small("Unavailable: choose a data source first.");

                        columns[1].strong("Secondary");
                        let response = columns[1].button("Configure fixture");
                        if let Some(capture) = &mut self.capture { capture.observe(ctx, "secondary", &response); }
                        if response.clicked() { self.clicks = self.clicks.saturating_add(1); }
                        state_caption(&mut columns[1], &response);
                        let disabled = columns[1].add_enabled(false, egui::Button::new("Configure unavailable"));
                        state_caption(&mut columns[1], &disabled);
                        columns[1].small("Unavailable: select a view first.");

                        columns[2].strong("Quiet");
                        let response = columns[2].add(shell::quiet("Details fixture"));
                        if let Some(capture) = &mut self.capture { capture.observe(ctx, "quiet", &response); }
                        if response.clicked() { self.clicks = self.clicks.saturating_add(1); }
                        state_caption(&mut columns[2], &response);
                        let disabled = columns[2].add_enabled(false, shell::quiet("Details unavailable"));
                        state_caption(&mut columns[2], &disabled);
                        columns[2].small("Unavailable: no fixture details selected.");
                    });
                    ui.small(format!("Fixture activations: {} · no external actions", self.clicks));
                    section(ui, "Selection and view frames");
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.selected, true, "Selected fixture");
                        ui.selectable_value(&mut self.selected, false, "Unselected fixture");
                    });
                    shell::view_frame(ui, self.selected).show(ui, |ui| {
                        ui.strong(shell::view_title("/fixture/scan", "Spatial scan"));
                        shell::status(ui, ViewStatus::Receiving);
                        ui.label("Selected outline and normal content surface; sample content only.");
                    });
                    section(ui, "Semantic states");
                    ui.horizontal_wrapped(|ui| {
                        for status in [ViewStatus::Receiving, ViewStatus::Waiting, ViewStatus::Stopped, ViewStatus::Starting, ViewStatus::Stopping, ViewStatus::NeedsRecovery] {
                            shell::status(ui, status);
                        }
                    });
                    ui.horizontal(|ui| { ui.spinner(); ui.label("Busy fixture · waiting for a synthetic operation"); });
                    ui.colored_label(ui.visuals().warn_fg_color, "Warning fixture · application sample queue dropped data");
                    ui.colored_label(ui.visuals().error_fg_color, "Error fixture · choose a source, then retry explicitly");
                    ui.label(egui::RichText::new("Muted supplemental fixture information").color(shell::muted(ui.visuals())));
                    section(ui, "Editable field and focus");
                    let label = ui.label("Workspace name fixture");
                    let response = ui.add(egui::TextEdit::singleline(&mut self.field).desired_width(400.0)).labelled_by(label.id);
                    state_caption(ui, &response);
                    ui.small("Latin / CJK / emoji are rendered with the same bundled fonts as the product.");
                    ui.collapsing("Bundled font notices", |ui| { ui.label(fonts::NOTICE); });
                });
            });
    }
}
fn main() -> eframe::Result {
    eframe::run_native(
        "ROS Studio · visual states fixture",
        eframe::NativeOptions {
            viewport: egui::ViewportBuilder::default()
                .with_inner_size([1100.0, 820.0])
                .with_min_inner_size([800.0, 600.0]),
            ..Default::default()
        },
        Box::new(|creation| {
            fonts::configure(&creation.egui_ctx);
            shell::configure(&creation.egui_ctx);
            Ok(Box::<VisualStates>::default())
        }),
    )
}

#[cfg(test)]
mod capture_tests {
    use super::*;

    #[test]
    fn ppm_keeps_native_dimensions_and_rgb_pixel_order() {
        let mut image = egui::ColorImage::new([2, 1], egui::Color32::RED);
        image.pixels[1] = egui::Color32::from_rgb(0, 0, 255);
        let mut bytes = Vec::new();
        write_ppm(&mut bytes, &image).unwrap();
        assert_eq!(&bytes[..11], b"P6\n2 1\n255\n");
        assert_eq!(&bytes[11..], &[255, 0, 0, 0, 0, 255]);
    }
}
