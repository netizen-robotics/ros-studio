//! Derived colors and sizes shared by renderer and sensor inspector.
use super::{sensor_history::SensorPoint, CameraPose, Point3};
pub use studio_workspace::sensor::{ColorMode, SensorAppearance, SizeMode};

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ColorRange {
    pub minimum: f64,
    pub maximum: f64,
}
impl ColorRange {
    pub fn normalized(self, value: f64) -> f32 {
        if self.minimum == self.maximum {
            return 0.5;
        }
        let span = self.maximum - self.minimum;
        let normalized = if span.is_finite() {
            (value - self.minimum) / span
        } else {
            (value / 2. - self.minimum / 2.) / (self.maximum / 2. - self.minimum / 2.)
        };
        normalized.clamp(0., 1.) as f32
    }
}
pub fn range(
    settings: &SensorAppearance,
    points: impl Iterator<Item = SensorPoint>,
) -> Option<ColorRange> {
    if !matches!(settings.color, ColorMode::Height | ColorMode::Intensity) {
        return None;
    }
    if !settings.automatic_range {
        let values = if settings.color == ColorMode::Height {
            settings.height_mm
        } else {
            settings.intensity_milli
        };
        return Some(ColorRange {
            minimum: values[0] as f64 / 1000.,
            maximum: values[1] as f64 / 1000.,
        });
    }
    let mut bounds: Option<ColorRange> = None;
    for p in points {
        let value = if settings.color == ColorMode::Height {
            Some(p.position.0[2])
        } else {
            p.intensity
        };
        if let Some(value) = value.filter(|n| n.is_finite()) {
            match &mut bounds {
                Some(b) => {
                    b.minimum = b.minimum.min(value);
                    b.maximum = b.maximum.max(value)
                }
                None => {
                    bounds = Some(ColorRange {
                        minimum: value,
                        maximum: value,
                    })
                }
            }
        }
    }
    bounds
}
fn linear(rgb: [u8; 3]) -> [f32; 3] {
    let rgba = eframe::egui::Rgba::from(eframe::egui::Color32::from_rgb(rgb[0], rgb[1], rgb[2]))
        .to_array();
    [rgba[0], rgba[1], rgba[2]]
}
pub fn color(
    settings: &SensorAppearance,
    point: SensorPoint,
    range: Option<ColorRange>,
) -> [f32; 4] {
    let mut source_alpha = 1.;
    let rgb = match settings.color {
        ColorMode::Flat => linear(settings.flat_rgb),
        ColorMode::Rgb => match point.rgba {
            Some([r, g, b, a]) => {
                source_alpha = f32::from(a) / 255.;
                linear([r, g, b])
            }
            None => linear([143, 148, 150]),
        },
        ColorMode::Height | ColorMode::Intensity => {
            let value = if settings.color == ColorMode::Height {
                Some(point.position.0[2])
            } else {
                point.intensity
            };
            match value.zip(range) {
                Some((value, bounds)) => {
                    let t = bounds.normalized(value);
                    let low = linear([39, 164, 190]);
                    let high = linear([241, 177, 61]);
                    std::array::from_fn(|i| low[i] * (1. - t) + high[i] * t)
                }
                None => linear([143, 148, 150]),
            }
        }
    };
    [
        rgb[0],
        rgb[1],
        rgb[2],
        source_alpha * f32::from(settings.opacity) / 255.,
    ]
}
/// Clip-space half extents. Pixel diameter stays constant across perspective
/// depth; world diameter is a camera-facing square in metres in both projections.
pub fn half_extents(
    settings: &SensorAppearance,
    camera: CameraPose,
    point: Point3,
    size: [u32; 2],
    clip: [f32; 4],
) -> [f32; 2] {
    match settings.size_mode {
        SizeMode::Pixels => [
            f32::from(settings.pixels) / size[0] as f32 * clip[3],
            f32::from(settings.pixels) / size[1] as f32 * clip[3],
        ],
        SizeMode::World => {
            let half = f64::from(settings.diameter_mm) / 2000.;
            let right = Point3(std::array::from_fn(|i| {
                point.0[i] + f64::from(camera.right[i]) * half
            }));
            let up = Point3(std::array::from_fn(|i| {
                point.0[i] + f64::from(camera.up[i]) * half
            }));
            let aspect = size[0] as f32 / size[1] as f32;
            [
                (camera.clip(right, aspect)[0] - clip[0]).abs(),
                (camera.clip(up, aspect)[1] - clip[1]).abs(),
            ]
        }
    }
}
