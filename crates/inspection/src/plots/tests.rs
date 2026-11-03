use super::*;
use std::time::Duration;

fn sample(t: f64, value: Result<Scalar, Gap>) -> Sample {
    Sample {
        receipt: Duration::from_secs_f64(t),
        value,
    }
}
fn number(t: f64, value: f64) -> Sample {
    sample(t, Ok(Scalar::Float(value)))
}
#[test]
fn precision_is_explicit_and_source_label_is_exact() {
    for n in [0, 1, 1 << 53, 1 << 54] {
        assert!(Scalar::Unsigned(n).coordinate().is_ok());
    }
    for n in [(1 << 53) + 1, u64::MAX] {
        assert_eq!(Scalar::Unsigned(n).coordinate(), Err(Gap::PrecisionLoss));
    }
    assert_eq!(
        Scalar::Signed(i64::MAX).coordinate(),
        Err(Gap::PrecisionLoss)
    );
    assert!(Scalar::Signed(i64::MIN).coordinate().is_ok());
    assert_eq!(
        Scalar::Unsigned(u64::MAX).exact_label(),
        "18446744073709551615"
    );
    assert_eq!(Scalar::Float(f64::NAN).coordinate(), Err(Gap::NonFinite));
    assert_eq!(
        Scalar::Float(f64::INFINITY).coordinate(),
        Err(Gap::NonFinite)
    );
}
#[test]
fn flood_obeys_count_and_time_limits() {
    let mut trace = Trace::default();
    trace.begin(1);
    for n in 0..20_000 {
        trace.push(1, number(n as f64 / 1000.0, n as f64));
    }
    assert_eq!(trace.points().len(), MAX_POINTS);
    assert_eq!(trace.counters.evicted, 10_000);
    trace.push(1, number(100.0, 3.0));
    assert_eq!(trace.points().len(), 1);
    assert_eq!(trace.extents(), Some((3.0, 3.0)));
}
#[test]
fn pause_clear_resume_and_stale_sessions_are_bounded() {
    let mut trace = Trace::default();
    trace.begin(7);
    trace.push(7, number(1.0, 1.0));
    trace.set_paused(true);
    for n in 2..100_000 {
        trace.push(7, number(n as f64, 9.0));
    }
    assert_eq!(trace.points().len(), 1);
    trace.clear();
    assert!(trace.paused());
    assert!(trace.points().is_empty());
    trace.set_paused(false);
    trace.push(7, number(100_000.0, 5.0));
    assert_eq!(trace.points().len(), 2);
    assert_eq!(trace.points()[0].gap, Some(Gap::Paused));
    trace.begin(8);
    trace.push(7, number(100_001.0, 8.0));
    assert!(trace.points().is_empty());
    assert_eq!(trace.counters.stale, 1);
}
#[test]
fn missing_nonfinite_precision_and_backward_time_break_segments() {
    let mut trace = Trace::default();
    trace.push(0, number(1.0, 2.0));
    trace.push(0, sample(2.0, Err(Gap::Missing)));
    trace.push(0, number(3.0, f64::NAN));
    trace.push(0, sample(4.0, Ok(Scalar::Unsigned(u64::MAX))));
    trace.push(0, number(0.5, 999.0));
    trace.push(0, number(5.0, 4.0));
    assert_eq!(trace.extents(), Some((2.0, 4.0)));
    assert_eq!(trace.counters.backward, 1);
    assert_eq!(trace.points()[3].exact, Some(Scalar::Unsigned(u64::MAX)));
    assert!(trace.display_points(20).iter().any(Option::is_none));
}
#[test]
fn deterministic_sine_ramp_and_spike_survive_bounded_reduction() {
    let mut trace = Trace::default();
    for n in 0..10_000 {
        let x = n as f64 / 200.0;
        trace.push(0, number(x, x.sin()));
    }
    trace.push(0, number(50.0, 1000.0));
    let display = trace.display_points(100);
    assert!(display.len() <= 400);
    assert!(display.iter().flatten().any(|(_, y)| *y == 1000.0));
    assert_eq!(trace.extents().unwrap().1, 1000.0);
    assert!(display
        .iter()
        .flatten()
        .map(|(t, _)| *t)
        .collect::<Vec<_>>()
        .windows(2)
        .all(|w| w[0] <= w[1]));
}
#[test]
fn gap_bucket_cannot_bridge_and_path_labels_are_unambiguous() {
    let mut trace = Trace::default();
    trace.push(0, number(0.0, 1.0));
    trace.push(0, sample(0.1, Err(Gap::Incomplete)));
    trace.push(0, number(0.2, 3.0));
    assert_eq!(trace.display_points(1), vec![None]);
    assert_ne!(
        FieldPath(vec![PathSegment::Field("a.b".into())]).label(),
        FieldPath(vec![
            PathSegment::Field("a".into()),
            PathSegment::Field("b".into())
        ])
        .label()
    );
}
#[test]
fn renderer_handles_all_states_and_extreme_values_headlessly() {
    let context = eframe::egui::Context::default();
    let mut renderer = crate::plots::PlotRenderer;
    let mut trace = Trace::default();
    for state in [
        crate::plots::PlotStatus::Unconfigured,
        crate::plots::PlotStatus::Starting,
        crate::plots::PlotStatus::Waiting,
        crate::plots::PlotStatus::Receiving,
        crate::plots::PlotStatus::Stopped,
        crate::plots::PlotStatus::Error("fixture failure"),
    ] {
        let _ = context.run(Default::default(), |ctx| {
            eframe::egui::CentralPanel::default().show(ctx, |ui| {
                renderer.show(ui, &mut trace, state, None);
            });
        });
    }
    trace.push(0, number(0.0, -f64::MAX));
    trace.push(0, number(1.0, f64::MAX));
    let output = context.run(Default::default(), |ctx| {
        eframe::egui::CentralPanel::default().show(ctx, |ui| {
            renderer.show(ui, &mut trace, crate::plots::PlotStatus::Receiving, None);
        });
    });
    assert!(!output.shapes.is_empty());
}

#[test]
fn window_boundary_and_failed_values_are_explicit() {
    let mut trace = Trace::default();
    trace.push(0, number(0.0, 1.0));
    trace.push(0, number(60.0, 2.0));
    assert_eq!(trace.points().len(), 2);
    trace.push(0, sample(60.001, Err(Gap::Nonnumeric)));
    assert_eq!(trace.points().len(), 2);
    assert_eq!(trace.points().back().unwrap().gap, Some(Gap::Nonnumeric));
    trace.push(0, number(59.0, 3.0));
    trace.push(0, number(61.0, 4.0));
    assert_eq!(trace.points()[2].gap, Some(Gap::BackwardReceiptTime));
    trace.clear();
    trace.push(0, sample(62.0, Err(Gap::Incomplete)));
    assert_eq!(trace.extents(), None);
    assert_eq!(trace.points()[0].gap, Some(Gap::Incomplete));
}

#[test]
fn axis_caption_and_exact_integer_survive_laptop_render() {
    use eframe::egui;
    let context = egui::Context::default();
    let mut trace = Trace::default();
    trace.push(0, sample(0.0, Ok(Scalar::Unsigned(1 << 54))));
    trace.push(0, sample(60.0, Ok(Scalar::Unsigned(1 << 54))));
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::Pos2::ZERO,
            egui::vec2(640.0, 600.0),
        )),
        ..Default::default()
    };
    let output = context.run(input, |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            PlotRenderer.show(ui, &mut trace, PlotStatus::Receiving, None)
        });
    });
    fn texts(shape: &egui::Shape, out: &mut Vec<String>) {
        match shape {
            egui::Shape::Text(text) => out.push(text.galley.text().to_owned()),
            egui::Shape::Vec(shapes) => {
                for shape in shapes {
                    texts(shape, out);
                }
            }
            _ => (),
        }
    }
    let mut labels = Vec::new();
    for shape in &output.shapes {
        texts(&shape.shape, &mut labels);
    }
    assert!(labels
        .iter()
        .any(|text| text == "Seconds since first retained sample"));
    assert!(labels.iter().any(|text| text == "60"));
    assert!(labels
        .iter()
        .any(|text| text.contains("Latest source value: 18014398509481984")));
    assert!(output
        .shapes
        .iter()
        .all(|s| s.shape.visual_bounding_rect().is_finite()));
}

#[test]
fn long_selected_field_does_not_expand_inspector() {
    use eframe::egui;
    let context = egui::Context::default();
    let path = FieldPath(vec![PathSegment::Field("long_field_name".repeat(40))]);
    let mut selected = Some(path.clone());
    let mut width = 0.0;
    let _ = context.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.allocate_ui(egui::vec2(220.0, 100.0), |ui| {
                PlotRenderer::field_picker(ui, std::slice::from_ref(&path), &mut selected);
                width = ui.min_rect().width();
            });
        });
    });
    assert!(width <= 220.0, "field picker expanded to {width}");
    assert_eq!(selected, Some(path));
}
