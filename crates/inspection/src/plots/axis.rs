//! Small fixed-size axes. Normalized interpolation avoids overflow at ±f64::MAX.
#[derive(Debug)]
pub(super) struct Tick {
    pub fraction: f32,
    pub label: String,
}

pub(super) fn value_ticks(min: f64, max: f64) -> Vec<Tick> {
    if min == max {
        return vec![Tick {
            fraction: 0.5,
            label: label(min, 3),
        }];
    }
    let scale = min.abs().max(max.abs()).max(f64::MIN_POSITIVE);
    let lo = min / scale;
    let hi = max / scale;
    let values: Vec<_> = (0..5)
        .map(|index| {
            let t = index as f64 / 4.0;
            // Return exact endpoints; clamping prevents a rounded 1+epsilon
            // coefficient from producing an infinite extreme coordinate.
            match index {
                0 => min,
                4 => max,
                _ => ((1.0 - t) * lo + t * hi).clamp(-1.0, 1.0) * scale,
            }
        })
        .collect();
    let mut precision = 3;
    loop {
        let labels: Vec<_> = values
            .iter()
            .map(|&value| label(value, precision))
            .collect();
        let collision = values
            .windows(2)
            .zip(labels.windows(2))
            .any(|(values, labels)| values[0] != values[1] && labels[0] == labels[1]);
        if !collision || precision == 16 {
            let mut ticks = Vec::new();
            for (index, text) in labels.into_iter().enumerate() {
                if ticks.last().is_some_and(|tick: &Tick| tick.label == text) {
                    continue;
                }
                ticks.push(Tick {
                    fraction: index as f32 / 4.0,
                    label: text,
                });
            }
            return ticks;
        }
        precision += 1;
    }
}

fn label(value: f64, precision: usize) -> String {
    if value == 0.0 {
        return "0".into();
    }
    if value.abs() >= 1e6 || value.abs() < 0.001 || precision > 6 {
        let text = format!("{value:.precision$e}");
        // Rounding MAX upward at short precision must not label a finite
        // endpoint as an out-of-range infinity.
        if text.parse::<f64>().is_ok_and(f64::is_finite) {
            return text;
        }
        return format!("{value:.16e}");
    }
    let text = format!("{value:.precision$}");
    let text = text.trim_end_matches('0').trim_end_matches('.');
    if text == "-0" {
        "0".into()
    } else {
        text.into()
    }
}

pub(super) fn time_ticks(span: f64) -> Vec<Tick> {
    if span == 0.0 {
        return vec![Tick {
            fraction: 0.5,
            label: "0".into(),
        }];
    }
    (0..5)
        .map(|index| Tick {
            fraction: index as f32 / 4.0,
            label: label(span * index as f64 / 4.0, 3),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn labels_stay_bounded_and_distinguish_narrow_and_extreme_ranges() {
        for (min, max) in [
            (-f64::MAX, f64::MAX),
            (0.0, f64::from_bits(1)),
            (1e200, f64::from_bits(1e200_f64.to_bits() + 4)),
            (-0.0, 0.0),
            (3.0, 3.0),
        ] {
            let ticks = value_ticks(min, max);
            assert!(!ticks.is_empty() && ticks.len() <= 5);
            assert!(ticks
                .iter()
                .all(|t| t.label.len() <= 25 && t.fraction.is_finite()));
            assert!(ticks
                .iter()
                .all(|t| t.label.parse::<f64>().unwrap().is_finite()));
            assert!(ticks.windows(2).all(|pair| pair[0].label != pair[1].label));
            if min != max {
                assert!(ticks.len() >= 2);
            }
        }
        assert_eq!(value_ticks(4.0, 4.0)[0].fraction, 0.5);
        assert_eq!(time_ticks(60.0)[4].label, "60");
        assert_eq!(time_ticks(0.0)[0].label, "0");
    }
}
