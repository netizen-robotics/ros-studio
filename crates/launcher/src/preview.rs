//! Side-effect-free debug preview of the bundled suite.
use eframe::egui;
use studio_launcher::suite::Snapshot;
pub struct Preview {
    snapshot: Snapshot,
    busy: bool,
}
impl Preview {
    pub fn new(mode: &str) -> Self {
        Self {
            snapshot: Snapshot {
                available: if mode == "missing" {
                    [true, false]
                } else {
                    [true, true]
                },
                error: (mode == "error")
                    .then(|| "Could not open Tools. Retry, or reinstall ROS Studio.".into()),
            },
            busy: mode == "busy",
        }
    }
}
impl eframe::App for Preview {
    fn update(&mut self, ctx: &egui::Context, _: &mut eframe::Frame) {
        if (crate::presentation::Page {
            snapshot: &self.snapshot,
            busy: self.busy,
        })
        .show(ctx)
        .is_some()
        {
            self.snapshot = Snapshot {
                available: [true, true],
                error: None,
            };
            self.busy = false;
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rendering_is_passive_and_open_slots_stay_fixed_across_states() {
        for width in [560.0, 920.0] {
            let mut expected = None;
            for dark in [false, true] {
                for mode in ["ready", "busy", "missing", "error"] {
                    let ctx = egui::Context::default();
                    ctx.enable_accesskit();
                    studio_ui_foundation::fonts::configure(&ctx);
                    studio_ui_foundation::theme::configure(&ctx);
                    ctx.set_theme(if dark {
                        egui::ThemePreference::Dark
                    } else {
                        egui::ThemePreference::Light
                    });
                    let app = Preview::new(mode);
                    let mut bounds = Vec::new();
                    for _ in 0..4 {
                        let output = ctx.run(
                            egui::RawInput {
                                screen_rect: Some(egui::Rect::from_min_size(
                                    egui::Pos2::ZERO,
                                    egui::vec2(width, 1200.0),
                                )),
                                ..Default::default()
                            },
                            |ctx| {
                                assert!(crate::presentation::Page {
                                    snapshot: &app.snapshot,
                                    busy: app.busy,
                                }
                                .show(ctx)
                                .is_none());
                            },
                        );
                        bounds = output
                            .platform_output
                            .accesskit_update
                            .unwrap()
                            .nodes
                            .into_iter()
                            .filter_map(|(_, n)| {
                                (n.label() == Some("Open")).then(|| n.bounds().unwrap())
                            })
                            .collect::<Vec<_>>();
                        bounds.sort_by(|a, b| a.y0.total_cmp(&b.y0).then(a.x0.total_cmp(&b.x0)));
                    }
                    assert_eq!(bounds.len(), 2);
                    if width < 720.0 {
                        assert_eq!(
                            bounds[1].y0 - bounds[0].y0,
                            246.0,
                            "230pt cards have a deliberate 16pt gap"
                        );
                    }
                    let b = bounds
                        .iter()
                        .map(|r| (r.x0, r.y0, r.x1, r.y1))
                        .collect::<Vec<_>>();
                    assert_eq!(*expected.get_or_insert(b.clone()), b, "{mode}");
                }
            }
        }
    }
}
