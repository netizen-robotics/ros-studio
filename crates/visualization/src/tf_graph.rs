//! Shared, bounded TF presentation. No ROS, subscription actions or clock grants.
use crate::spatial::{
    camera_fit::Bounds, overlay_geometry, CameraPose, Point3, RenderVertex, RigidTransform,
};
use eframe::egui;
use studio_protocol::tf_graph::{LatestPose, Sample};
pub const MAX_VERTICES: usize = studio_protocol::tf_graph::MAX_FRAMES
    * overlay_geometry::POSE_VERTICES
    + studio_protocol::tf_graph::MAX_EDGES * 6;

pub struct TfView {
    sample: Option<Sample>,
    poses: Vec<(String, RigidTransform)>,
    selected: Option<String>,
    query: String,
    names: bool,
}
impl Default for TfView {
    fn default() -> Self {
        Self {
            sample: None,
            poses: Vec::new(),
            selected: None,
            query: String::new(),
            names: true,
        }
    }
}
impl TfView {
    pub fn apply(&mut self, sample: Sample) -> Result<(), String> {
        sample.validate()?;
        let mut poses = Vec::new();
        for frame in &sample.frames {
            if let LatestPose::Ready { components, .. } = frame.pose {
                let pose = RigidTransform::new(
                    components[..3].try_into().unwrap(),
                    components[3..].try_into().unwrap(),
                )
                .ok_or("Invalid TF pose")?;
                // A legal native origin may put an axis endpoint outside the
                // renderer envelope. Keep the frame/diagnostic, hide its axes.
                if overlay_geometry::pose_segments(pose).is_ok() {
                    poses.push((frame.name.clone(), pose));
                }
            }
        }
        if self
            .selected
            .as_ref()
            .is_some_and(|name| !sample.frames.iter().any(|f| &f.name == name))
        {
            self.selected = None;
        }
        self.poses = poses;
        self.sample = Some(sample);
        Ok(())
    }
    pub fn stop(&mut self) {
        self.sample = None;
        self.poses.clear();
        self.selected = None;
    }
    pub fn sample(&self) -> Option<&Sample> {
        self.sample.as_ref()
    }
    pub fn ready_count(&self) -> usize {
        self.poses.len()
    }
    pub fn selected(&self) -> Option<&str> {
        self.selected.as_deref()
    }
    pub fn select(&mut self, name: &str) -> bool {
        if self
            .sample
            .as_ref()
            .is_some_and(|s| s.frames.iter().any(|f| f.name == name))
        {
            self.selected = Some(name.into());
            true
        } else {
            false
        }
    }
    pub fn validate_frame(&self, fixed: &str) -> Result<(), String> {
        if self.sample.as_ref().is_some_and(|s| s.fixed_frame != fixed) {
            return Err("TF graph fixed frame contradicts the scene".into());
        }
        Ok(())
    }
    pub fn bounds(&self, selected_only: bool) -> Option<Bounds> {
        let mut bounds: Option<Bounds> = None;
        for (name, pose) in &self.poses {
            if selected_only && self.selected.as_ref().is_some_and(|s| s != name) {
                continue;
            }
            for (a, b, _) in overlay_geometry::pose_segments(*pose).ok()? {
                for p in [a, b] {
                    if let Some(bounds) = &mut bounds {
                        for i in 0..3 {
                            bounds.min[i] = bounds.min[i].min(p.0[i]);
                            bounds.max[i] = bounds.max[i].max(p.0[i]);
                        }
                    } else {
                        bounds = Some(Bounds { min: p.0, max: p.0 });
                    }
                }
            }
        }
        bounds
    }
    pub fn vertices(
        &self,
        camera: CameraPose,
        size: [u32; 2],
    ) -> Result<Vec<RenderVertex>, crate::spatial::combined::CombinedSceneError> {
        let mut out = Vec::with_capacity(self.poses.len() * 24);
        for (name, pose) in &self.poses {
            let selected = self.selected.as_ref() == Some(name);
            for (a, b, color) in overlay_geometry::pose_segments(*pose)? {
                overlay_geometry::ribbon(
                    &mut out,
                    camera,
                    size,
                    a,
                    b,
                    color,
                    if selected { 4. } else { 2. },
                )?;
            }
            if let Some(parent) = self
                .sample
                .as_ref()
                .and_then(|s| s.frames.iter().find(|f| &f.name == name))
                .and_then(|f| f.parent.as_ref())
            {
                if let Some((_, parent_pose)) = self.poses.iter().find(|(n, _)| n == &parent.frame)
                {
                    overlay_geometry::ribbon(
                        &mut out,
                        camera,
                        size,
                        pose.apply(Point3([0.; 3])),
                        parent_pose.apply(Point3([0.; 3])),
                        if selected {
                            [0.9, 0.6, 0.05, 1.]
                        } else {
                            [0.4, 0.55, 0.55, 1.]
                        },
                        1.,
                    )?;
                }
            }
        }
        Ok(out)
    }
    /// Screen annotations deliberately remain legible over the depth-tested axes.
    pub fn paint_names(&self, ui: &egui::Ui, rect: egui::Rect, camera: CameraPose) {
        if !self.names {
            return;
        }
        let painter = ui.painter_at(rect);
        let mut occupied = Vec::new();
        // Coincident XY origins are common in top-down view. Place the selected
        // frame first, then seek a bounded non-overlapping name for its peers.
        for (name, pose) in self
            .poses
            .iter()
            .filter(|(n, _)| self.selected.as_ref() == Some(n))
            .chain(
                self.poses
                    .iter()
                    .filter(|(n, _)| self.selected.as_ref() != Some(n)),
            )
        {
            let p = camera.clip(pose.apply(Point3([0.; 3])), rect.width() / rect.height());
            if p[3] <= 0. || p[2] < 0. || p[2] > p[3] || p[0].abs() > p[3] || p[1].abs() > p[3] {
                continue;
            }
            let anchor = egui::pos2(
                rect.left() + (p[0] / p[3] + 1.) * rect.width() / 2.,
                rect.top() + (1. - p[1] / p[3]) * rect.height() / 2.,
            );
            let selected = self.selected.as_ref() == Some(name);
            let color = if selected {
                ui.visuals().selection.stroke.color
            } else {
                ui.visuals().text_color()
            };
            let label: String = name.chars().take(28).collect();
            let galley = painter.layout_no_wrap(
                label,
                egui::FontId::proportional(if selected { 14. } else { 11. }),
                color,
            );
            let Some(label_rect) =
                place_label(rect, anchor, galley.size() + egui::vec2(6., 4.), &occupied)
            else {
                continue;
            };
            occupied.push(label_rect);
            let nearest = egui::pos2(
                anchor.x.clamp(label_rect.left(), label_rect.right()),
                anchor.y.clamp(label_rect.top(), label_rect.bottom()),
            );
            painter.line_segment([anchor, nearest], egui::Stroke::new(0.7, color));
            painter.rect_filled(
                label_rect,
                3.,
                if selected {
                    ui.visuals().selection.bg_fill
                } else {
                    ui.visuals().panel_fill
                },
            );
            painter.galley(label_rect.min + egui::vec2(3., 2.), galley, color);
        }
    }

    pub fn show_inspector(&mut self, ui: &mut egui::Ui, waiting: bool) {
        ui.separator();
        let Some(sample) = &self.sample else {
            ui.label(if waiting {
                "Waiting for the backend TF graph…"
            } else {
                "Enable TF to receive the shared transform graph."
            });
            return;
        };
        ui.strong(format!(
            "{} / {} frames drawable",
            self.poses.len(),
            sample.frames.len()
        )).on_hover_text("Latest available poses. Dynamic paths become stale after 2 s; each frame has its own native timestamp.");
        if sample.frames.is_empty() {
            ui.label(
                "Waiting for transforms. Check both TF topics and their QoS in Scene settings.",
            );
        } else if !sample.frames.iter().any(|f| f.name == sample.fixed_frame) {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                format!(
                    "Fixed frame {} is missing. Publish it or change Scene settings.",
                    sample.fixed_frame
                ),
            );
        }
        let label = ui.label("Find frame");
        ui.add(
            studio_ui_foundation::theme::input(ui, &mut self.query)
                .hint_text(studio_ui_foundation::theme::hint(ui, "Filter by name…"))
                .desired_width(f32::INFINITY)
                .char_limit(256),
        )
        .labelled_by(label.id);
        ui.checkbox(&mut self.names, "Show frame names").on_hover_text("Crowded names are spaced or hidden. Select a frame to prioritize its label; the full names remain in this list.");
        let query = self.query.to_lowercase();
        egui::ScrollArea::vertical()
            .id_salt("tf-frame-list")
            .max_height(230.)
            .show(ui, |ui| {
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                ui.spacing_mut().item_spacing.y = 2.0;
                let mut matches = 0;
                for frame in sample
                    .frames
                    .iter()
                    .filter(|f| query.is_empty() || f.name.to_lowercase().contains(&query))
                {
                    let status = pose_status(&frame.pose);
                    matches += 1;
                    if ui
                        .selectable_label(
                            self.selected.as_ref() == Some(&frame.name),
                            format!("{} · {status}", frame.name),
                        )
                        .on_hover_text(format!(
                            "{}\nParent: {}\n{status}",
                            frame.name,
                            frame.parent.as_ref().map_or("root", |p| p.frame.as_str())
                        ))
                        .clicked()
                    {
                        self.selected = Some(frame.name.clone());
                    }
                }
                if matches == 0 && !query.is_empty() {
                    ui.label("No matching frames. Clear the filter to show all.");
                }
            });
        if let Some(frame) = self
            .selected
            .as_ref()
            .and_then(|name| sample.frames.iter().find(|f| &f.name == name))
        {
            ui.separator();
            ui.label(format!("Selected frame: {}", frame.name));
            if let Some(parent) = &frame.parent {
                ui.label(format!("Parent: {} · {:?}", parent.frame, parent.kind));
                ui.small(format!(
                    "Last accepted source stamp: {}",
                    parent
                        .stamp_ns
                        .map(|stamp| format!("{stamp} ns"))
                        .unwrap_or_else(|| "Unavailable after reset".into())
                ));
            } else {
                ui.label("Root frame · no parent edge");
            }
            match frame.pose {
                LatestPose::Ready {
                    resolved_stamp_ns, ..
                } => {
                    ui.small(format!("Native resolved stamp: {resolved_stamp_ns} ns"));
                    if !self.poses.iter().any(|(name, _)| name == &frame.name) {
                        ui.label("Axes exceed the renderer coordinate bounds.");
                    } else {
                        ui.small("Use Focus selected or Shift+F in the viewport.");
                    }
                }
                LatestPose::Missing => {
                    ui.label("No path to the fixed frame. Check that the trees are connected.");
                }
                LatestPose::Stale => {
                    ui.label("Dynamic path is older than 2 s. Check its publisher and System clock agreement.");
                }
                LatestPose::Extrapolated => {
                    ui.label(
                        "Transform time is unavailable or in the future. Check clock agreement.",
                    );
                }
                LatestPose::LookupTimeout => {
                    ui.label("Transform lookup timed out. Check TF publication.");
                }
            }
        }
    }
}
fn place_label(
    viewport: egui::Rect,
    anchor: egui::Pos2,
    size: egui::Vec2,
    occupied: &[egui::Rect],
) -> Option<egui::Rect> {
    let viewport = viewport.shrink(2.);
    if size.x > viewport.width() || size.y > viewport.height() {
        return None;
    }
    for offset in [
        egui::vec2(6., -size.y - 6.),
        egui::vec2(6., 6.),
        egui::vec2(-size.x - 6., -size.y - 6.),
        egui::vec2(-size.x - 6., 6.),
        egui::vec2(-size.x / 2., -size.y * 2. - 10.),
        egui::vec2(-size.x / 2., size.y + 10.),
    ] {
        let proposed = anchor + offset;
        let min = egui::pos2(
            proposed.x.clamp(viewport.left(), viewport.right() - size.x),
            proposed.y.clamp(viewport.top(), viewport.bottom() - size.y),
        );
        let rect = egui::Rect::from_min_size(min, size);
        if !occupied
            .iter()
            .any(|other| other.expand(2.).intersects(rect))
        {
            return Some(rect);
        }
    }
    None
}

fn pose_status(pose: &LatestPose) -> &'static str {
    match pose {
        LatestPose::Ready { .. } => "Ready",
        LatestPose::Missing => "Missing",
        LatestPose::Stale => "Stale",
        LatestPose::Extrapolated => "Extrapolated",
        LatestPose::LookupTimeout => "Lookup timeout",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_protocol::tf_graph::{EdgeKind, Frame, Parent};
    fn sample() -> Sample {
        Sample {
            contract_version: 1,
            fixed_frame: "map".into(),
            frames: vec![
                Frame {
                    name: "base".into(),
                    parent: Some(Parent {
                        frame: "map".into(),
                        kind: EdgeKind::Dynamic,
                        stamp_ns: Some(100),
                    }),
                    pose: LatestPose::Ready {
                        resolved_stamp_ns: 100,
                        components: [1., 2., 3., 0., 0., 0., 1.],
                    },
                },
                Frame {
                    name: "map".into(),
                    parent: None,
                    pose: LatestPose::Ready {
                        resolved_stamp_ns: 0,
                        components: [0., 0., 0., 0., 0., 0., 1.],
                    },
                },
                Frame {
                    name: "remote".into(),
                    parent: None,
                    pose: LatestPose::Missing,
                },
            ],
            counters: Default::default(),
        }
    }
    #[test]
    fn coincident_frame_labels_do_not_overlap_and_stay_inside_the_viewport() {
        let viewport = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(300., 200.));
        let mut occupied = Vec::new();
        for size in [
            egui::vec2(60., 20.),
            egui::vec2(48., 16.),
            egui::vec2(55., 16.),
        ] {
            let rect = place_label(viewport, egui::pos2(150., 100.), size, &occupied).unwrap();
            assert!(viewport.contains_rect(rect));
            assert!(occupied
                .iter()
                .all(|other: &egui::Rect| !other.intersects(rect)));
            occupied.push(rect);
        }
        assert!(place_label(viewport, egui::pos2(1., 1.), egui::vec2(400., 20.), &[]).is_none());
        let edge =
            place_label(viewport, egui::pos2(299., 199.), egui::vec2(60., 20.), &[]).unwrap();
        assert!(viewport.contains_rect(edge));
    }
    #[test]
    fn tf_selection_bounds_unavailable_replacement_and_stop_never_keep_old_axes() {
        let mut view = TfView::default();
        view.apply(sample()).unwrap();
        assert_eq!(view.ready_count(), 2);
        assert!(view.select("base"));
        let bounds = view.bounds(true).unwrap();
        assert_eq!(bounds.min, [1., 2., 3.]);
        assert_eq!(bounds.max, [1.35, 2.35, 3.35]);
        let camera = crate::spatial::Camera::default().pose();
        assert_eq!(view.vertices(camera, [640, 480]).unwrap().len(), 42);
        let mut stale = sample();
        stale.frames[0].pose = LatestPose::Stale;
        view.apply(stale).unwrap();
        assert_eq!(view.ready_count(), 1);
        assert_eq!(view.selected(), Some("base"));
        assert!(view.bounds(true).is_none());
        assert!(view.validate_frame("other").is_err());
        assert!(view.select("remote"));
        assert!(view.bounds(true).is_none());
        assert!(!view.select("absent"));
        view.stop();
        assert!(view.sample().is_none());
        assert_eq!(view.ready_count(), 0);
        assert!(view.selected().is_none());
    }
    #[test]
    fn legal_origin_outside_axis_envelope_keeps_diagnostics_without_invalid_geometry() {
        let mut sample = sample();
        sample.frames[0].pose = LatestPose::Ready {
            resolved_stamp_ns: 100,
            components: [1_000_000., 0., 0., 0., 0., 0., 1.],
        };
        let mut view = TfView::default();
        view.apply(sample).unwrap();
        assert_eq!(view.sample().unwrap().frames.len(), 3);
        assert_eq!(view.ready_count(), 1);
        assert!(view.select("base"));
        assert!(view.bounds(true).is_none());
    }
}
