use std::fmt;

/// Conservative component budget, independent of any future wire envelope.
pub const MAX_DIMENSION: u32 = 2048;
pub const MAX_FRAME_BYTES: usize = studio_protocol::image::MAX_RAW_BYTES;

pub struct RawFrame<'a> {
    pub width: u32,
    pub height: u32,
    pub encoding: &'a str,
    pub step: u32,
    /// Applies to multi-byte depth pixels; 8-bit channels have no byte order.
    pub big_endian: bool,
    pub data: &'a [u8],
}

#[derive(Debug)]
pub struct DecodedFrame {
    pub(crate) size: [usize; 2],
    pub(crate) encoding: String,
    pub(crate) image: std::sync::Arc<eframe::egui::ColorImage>,
}
impl DecodedFrame {
    /// Share worker-converted pixels with presentation without a UI-frame copy.
    pub fn color_image(&self) -> std::sync::Arc<eframe::egui::ColorImage> {
        self.image.clone()
    }
    pub fn size(&self) -> [usize; 2] {
        self.size
    }
    pub fn encoding(&self) -> &str {
        &self.encoding
    }
    pub fn pixels(&self) -> &[eframe::egui::Color32] {
        &self.image.pixels
    }
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    UnsupportedEncoding,
    InvalidDimensions,
    TooLarge,
    InvalidStride,
    PayloadLength { expected: usize, actual: usize },
}
impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedEncoding => write!(f, "Unsupported encoding. Choose mono8, rgb8, bgr8, rgba8, bgra8, 16UC1 or 32FC1."),
            Self::InvalidDimensions => write!(f, "Image dimensions must be nonzero and at most {MAX_DIMENSION} per axis."),
            Self::TooLarge => write!(f, "Image exceeds the {MAX_FRAME_BYTES}-byte component budget. Publish a smaller image."),
            Self::InvalidStride => write!(f, "Image row stride is smaller than its declared pixel width."),
            Self::PayloadLength { expected, actual } => write!(f, "Incomplete or extra image payload: expected {expected} bytes, received {actual}."),
        }
    }
}
impl std::error::Error for DecodeError {}

/// Exact stride*height length required, including padding on the final row.
/// No diagnostic string parsing, byte-depth guessing or partial-frame display.
pub fn decode(raw: RawFrame<'_>) -> Result<DecodedFrame, DecodeError> {
    let channels = match raw.encoding {
        "mono8" => 1,
        "16UC1" => 2,
        "rgb8" | "bgr8" => 3,
        "rgba8" | "bgra8" | "32FC1" => 4,
        _ => return Err(DecodeError::UnsupportedEncoding),
    };
    if raw.width == 0 || raw.height == 0 || raw.width > MAX_DIMENSION || raw.height > MAX_DIMENSION
    {
        return Err(DecodeError::InvalidDimensions);
    }
    let width = raw.width as usize;
    let height = raw.height as usize;
    let row = width.checked_mul(channels).ok_or(DecodeError::TooLarge)?;
    let step = raw.step as usize;
    if step < row {
        return Err(DecodeError::InvalidStride);
    }
    let expected = step.checked_mul(height).ok_or(DecodeError::TooLarge)?;
    let output_len = width
        .checked_mul(height)
        .and_then(|n| n.checked_mul(4))
        .ok_or(DecodeError::TooLarge)?;
    if expected > MAX_FRAME_BYTES || output_len > MAX_FRAME_BYTES {
        return Err(DecodeError::TooLarge);
    }
    if expected != raw.data.len() {
        return Err(DecodeError::PayloadLength {
            expected,
            actual: raw.data.len(),
        });
    }
    let mut pixels = Vec::with_capacity(output_len / 4);
    for scanline in raw.data.chunks_exact(step) {
        for pixel in scanline[..row].chunks_exact(channels) {
            let mapped = match raw.encoding {
                "mono8" => [pixel[0], pixel[0], pixel[0], 255],
                "rgb8" => [pixel[0], pixel[1], pixel[2], 255],
                "bgr8" => [pixel[2], pixel[1], pixel[0], 255],
                "rgba8" => [pixel[0], pixel[1], pixel[2], pixel[3]],
                "bgra8" => [pixel[2], pixel[1], pixel[0], pixel[3]],
                "16UC1" => {
                    let bytes = [pixel[0], pixel[1]];
                    let millimeters = if raw.big_endian {
                        u16::from_be_bytes(bytes)
                    } else {
                        u16::from_le_bytes(bytes)
                    };
                    depth_color(f32::from(millimeters) * 0.001)
                }
                "32FC1" => {
                    let bytes = [pixel[0], pixel[1], pixel[2], pixel[3]];
                    depth_color(if raw.big_endian {
                        f32::from_be_bytes(bytes)
                    } else {
                        f32::from_le_bytes(bytes)
                    })
                }
                _ => unreachable!(),
            };
            pixels.push(eframe::egui::Color32::from_rgba_unmultiplied(
                mapped[0], mapped[1], mapped[2], mapped[3],
            ));
        }
    }
    let image = eframe::egui::ColorImage {
        size: [width, height],
        pixels,
    };
    Ok(DecodedFrame {
        size: [width, height],
        encoding: raw.encoding.to_owned(),
        image: std::sync::Arc::new(image),
    })
}

/// Stable metric range across frames. Invalid depth is transparent, never a
/// fabricated surface; no per-frame normalization that changes scene meaning.
fn depth_color(meters: f32) -> [u8; 4] {
    if !meters.is_finite() || meters <= 0. {
        return [0; 4];
    }
    let value = ((1. - (meters / 10.).clamp(0., 1.)) * 255.).round() as u8;
    [value, value, value, 255]
}
