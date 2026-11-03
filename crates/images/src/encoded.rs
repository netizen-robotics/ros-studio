//! Bounded static file-raster decoding. Run on a worker, never an active UI frame.
//! These output/owned-buffer bounds do not promise a bound on codec RSS or time.
use image::{ColorType, ImageDecoder, ImageFormat, ImageReader, Limits};
use std::io::{self, BufRead, Cursor, Read, Seek, SeekFrom};
use std::sync::atomic::{AtomicBool, Ordering};
pub const MAX_ENCODED_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_RASTER_DIMENSION: u32 = 2048;
pub const MAX_OWNED_DECODE_BYTES: usize = 64 * 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RasterError {
    Cancelled,
    Limit,
    Unsupported,
    Invalid,
}
impl std::fmt::Display for RasterError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Cancelled => "Image decoding cancelled",
            Self::Limit => "Encoded image, dimensions or decoded memory exceed the raster limit",
            Self::Unsupported => "Use a static PNG or JPEG image",
            Self::Invalid => "Invalid or incomplete encoded image",
        })
    }
}
impl std::error::Error for RasterError {}
#[derive(Debug, PartialEq, Eq)]
pub struct DecodedRaster {
    pub width: u32,
    pub height: u32,
    /// Top-left row first, straight (unpremultiplied) RGBA8, including JPEG alpha=255.
    pub rgba: Vec<u8>,
    pub has_transparency: bool,
}
fn check(cancel: &AtomicBool) -> Result<(), RasterError> {
    if cancel.load(Ordering::Relaxed) {
        Err(RasterError::Cancelled)
    } else {
        Ok(())
    }
}
struct CancelReader<'a> {
    cursor: Cursor<&'a [u8]>,
    cancel: &'a AtomicBool,
}
impl CancelReader<'_> {
    fn check(&self) -> io::Result<()> {
        if self.cancel.load(Ordering::Relaxed) {
            Err(io::Error::new(
                io::ErrorKind::ConnectionAborted,
                "raster cancelled",
            ))
        } else {
            Ok(())
        }
    }
}
impl Read for CancelReader<'_> {
    fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
        self.check()?;
        self.cursor.read(b)
    }
}
impl BufRead for CancelReader<'_> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.check()?;
        self.cursor.fill_buf()
    }
    fn consume(&mut self, n: usize) {
        self.cursor.consume(n)
    }
}
impl Seek for CancelReader<'_> {
    fn seek(&mut self, p: SeekFrom) -> io::Result<u64> {
        self.check()?;
        self.cursor.seek(p)
    }
}
fn map_error(error: image::ImageError, cancel: &AtomicBool) -> RasterError {
    if check(cancel).is_err() {
        RasterError::Cancelled
    } else {
        match error {
            image::ImageError::Limits(_) => RasterError::Limit,
            image::ImageError::Unsupported(_) => RasterError::Unsupported,
            _ => RasterError::Invalid,
        }
    }
}
fn static_png(bytes: &[u8], cancel: &AtomicBool) -> Result<(), RasterError> {
    let mut at = 8usize;
    let mut chunks = 0usize;
    let mut end = false;
    while at < bytes.len() {
        check(cancel)?;
        chunks += 1;
        if chunks > 10_000 {
            return Err(RasterError::Limit);
        }
        let header = bytes
            .get(at..at.checked_add(8).ok_or(RasterError::Limit)?)
            .ok_or(RasterError::Invalid)?;
        let len =
            u32::from_be_bytes(header[..4].try_into().map_err(|_| RasterError::Invalid)?) as usize;
        let next = at
            .checked_add(12)
            .and_then(|n| n.checked_add(len))
            .filter(|n| *n <= bytes.len())
            .ok_or(RasterError::Invalid)?;
        if chunks == 1 && (&header[4..] != b"IHDR" || len != 13) {
            return Err(RasterError::Invalid);
        }
        if matches!(&header[4..], b"acTL" | b"fcTL" | b"fdAT") {
            return Err(RasterError::Unsupported);
        }
        if &header[4..] == b"IEND" {
            if len != 0 || next != bytes.len() {
                return Err(RasterError::Invalid);
            }
            end = true;
            break;
        }
        at = next;
    }
    if end {
        Ok(())
    } else {
        Err(RasterError::Invalid)
    }
}
/// Decode one bounded static PNG/JPEG with a caller-supplied remaining RGBA budget.
/// Strict dimensions and owned raw+RGBA sizes are checked before pixel allocation.
/// Codec max_alloc is best-effort; cancellation is cooperative at codec IO and conversion.
/// A codec may do CPU work between reads; no hard cancellation-latency claim is made.
pub fn decode_raster(
    bytes: &[u8],
    remaining_rgba_bytes: usize,
    cancel: &AtomicBool,
) -> Result<DecodedRaster, RasterError> {
    check(cancel)?;
    if bytes.len() > MAX_ENCODED_BYTES {
        return Err(RasterError::Limit);
    }
    let format = image::guess_format(bytes).map_err(|_| RasterError::Unsupported)?;
    if !matches!(format, ImageFormat::Png | ImageFormat::Jpeg) {
        return Err(RasterError::Unsupported);
    }
    if format == ImageFormat::Png {
        static_png(bytes, cancel)?;
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_RASTER_DIMENSION);
    limits.max_image_height = Some(MAX_RASTER_DIMENSION);
    limits.max_alloc = Some(MAX_OWNED_DECODE_BYTES as u64);
    let mut reader = ImageReader::with_format(
        CancelReader {
            cursor: Cursor::new(bytes),
            cancel,
        },
        format,
    );
    reader.limits(limits);
    let decoder = reader.into_decoder().map_err(|e| map_error(e, cancel))?;
    check(cancel)?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 || width > MAX_RASTER_DIMENSION || height > MAX_RASTER_DIMENSION {
        return Err(RasterError::Limit);
    }
    let color = decoder.color_type();
    let sixteen = matches!(
        color,
        ColorType::L16 | ColorType::La16 | ColorType::Rgb16 | ColorType::Rgba16
    );
    if !matches!(
        color,
        ColorType::L8
            | ColorType::La8
            | ColorType::Rgb8
            | ColorType::Rgba8
            | ColorType::L16
            | ColorType::La16
            | ColorType::Rgb16
            | ColorType::Rgba16
    ) {
        return Err(RasterError::Unsupported);
    }
    let rgba_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|n| n.checked_mul(4))
        .filter(|n| *n <= remaining_rgba_bytes)
        .ok_or(RasterError::Limit)?;
    let raw_len = usize::try_from(decoder.total_bytes()).map_err(|_| RasterError::Limit)?;
    if raw_len
        .checked_add(rgba_len)
        .is_none_or(|n| n > MAX_OWNED_DECODE_BYTES)
    {
        return Err(RasterError::Limit);
    }
    check(cancel)?;
    let mut raw = vec![0; raw_len];
    decoder
        .read_image(&mut raw)
        .map_err(|e| map_error(e, cancel))?;
    check(cancel)?;
    let mut rgba = Vec::with_capacity(rgba_len);
    let channels = color.channel_count() as usize;
    let step = color.bytes_per_pixel() as usize;
    let mut has_transparency = false;
    if raw.len() != width as usize * height as usize * step {
        return Err(RasterError::Invalid);
    }
    for pixel in raw.chunks_exact(step) {
        check(cancel)?;
        let component = |i: usize| {
            if sixteen {
                (u16::from_ne_bytes([pixel[i * 2], pixel[i * 2 + 1]]) >> 8) as u8
            } else {
                pixel[i]
            }
        };
        let a = if channels == 2 || channels == 4 {
            component(channels - 1)
        } else {
            255
        };
        let rgb = if channels < 3 {
            [component(0); 3]
        } else {
            [component(0), component(1), component(2)]
        };
        has_transparency |= a != 255;
        rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], a]);
    }
    check(cancel)?;
    Ok(DecodedRaster {
        width,
        height,
        rgba,
        has_transparency,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageEncoder;
    fn png(width: u32, height: u32, pixels: &[u8], color: image::ExtendedColorType) -> Vec<u8> {
        let mut out = Vec::new();
        image::codecs::png::PngEncoder::new(&mut out)
            .write_image(pixels, width, height, color)
            .unwrap();
        out
    }
    #[test]
    fn png_preserves_row_order_straight_alpha_and_exact_remaining_budget() {
        let pixels = [
            255, 0, 0, 255, 0, 255, 0, 128, 0, 0, 255, 0, 255, 255, 255, 255,
        ];
        let bytes = png(2, 2, &pixels, image::ExtendedColorType::Rgba8);
        let cancel = AtomicBool::new(false);
        let decoded = decode_raster(&bytes, 16, &cancel).unwrap();
        assert_eq!((decoded.width, decoded.height), (2, 2));
        assert_eq!(decoded.rgba, pixels);
        assert!(decoded.has_transparency);
        assert_eq!(
            decode_raster(&bytes, 15, &cancel).unwrap_err(),
            RasterError::Limit
        );
    }
    #[test]
    fn jpeg_rgb_and_sixteen_bit_png_convert_to_rgba() {
        let cancel = AtomicBool::new(false);
        let mut jpg = Vec::new();
        image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpg, 100)
            .encode(&[128; 12], 2, 2, image::ExtendedColorType::Rgb8)
            .unwrap();
        let d = decode_raster(&jpg, 16, &cancel).unwrap();
        assert!(!d.has_transparency);
        assert!(d
            .rgba
            .chunks_exact(4)
            .all(|p| p[3] == 255 && p[..3].iter().all(|v| v.abs_diff(128) <= 1)));
        let bytes = png(
            2,
            1,
            bytemuck_u16(&[0, 65535]).as_slice(),
            image::ExtendedColorType::L16,
        );
        assert_eq!(
            decode_raster(&bytes, 8, &cancel).unwrap().rgba,
            [0, 0, 0, 255, 255, 255, 255, 255]
        );
    }
    fn bytemuck_u16(values: &[u16]) -> Vec<u8> {
        values.iter().flat_map(|v| v.to_ne_bytes()).collect()
    }
    #[test]
    fn exact_dimensions_invalid_data_animation_and_start_cancellation() {
        let cancel = AtomicBool::new(false);
        let bytes = png(2048, 1, &vec![0; 2048 * 4], image::ExtendedColorType::Rgba8);
        assert!(decode_raster(&bytes, 8192, &cancel).is_ok());
        let over = png(2049, 1, &vec![0; 2049 * 4], image::ExtendedColorType::Rgba8);
        assert_eq!(
            decode_raster(&over, usize::MAX, &cancel).unwrap_err(),
            RasterError::Limit
        );
        assert_eq!(
            decode_raster(&vec![0; MAX_ENCODED_BYTES + 1], usize::MAX, &cancel).unwrap_err(),
            RasterError::Limit
        );
        assert!(decode_raster(&bytes[..bytes.len() - 5], 8192, &cancel).is_err());
        let mut animated = bytes.clone();
        animated.splice(33..33, [0, 0, 0, 0, b'a', b'c', b'T', b'L', 0, 0, 0, 0]);
        assert_eq!(
            decode_raster(&animated, 8192, &cancel).unwrap_err(),
            RasterError::Unsupported
        );
        assert_eq!(
            decode_raster(b"GIF89a", 100, &cancel).unwrap_err(),
            RasterError::Unsupported
        );
        assert_eq!(
            decode_raster(&bytes, 8192, &AtomicBool::new(true)).unwrap_err(),
            RasterError::Cancelled
        );
    }
    #[test]
    fn cancelled_reader_does_not_return_retryable_interrupted_error() {
        let cancel = AtomicBool::new(true);
        let mut reader = CancelReader {
            cursor: Cursor::new(b"pixels"),
            cancel: &cancel,
        };
        let error = reader.read_exact(&mut [0; 4]).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::ConnectionAborted);
        assert_eq!(
            reader.seek(SeekFrom::Start(0)).unwrap_err().kind(),
            io::ErrorKind::ConnectionAborted
        );
        assert_eq!(
            reader.fill_buf().unwrap_err().kind(),
            io::ErrorKind::ConnectionAborted
        );
    }
}
