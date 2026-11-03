//! Compact Image adaptation for a serialized transport worker. Drop the wire
//! sample after this returns; enqueue only decoded RGBA and bounded metadata.
use super::{decode, HeaderStamp, ImageSampleError, RawFrame, TypedImage};
use studio_protocol::image::ImageSample;

/// Decode exact compact raw bytes on a worker, never an active UI frame.
/// `source_time` stays None: a declared ROS header stamp does not identify its
/// clock domain, and this envelope intentionally carries no clock provenance.
pub fn decode_compact_image(sample: &ImageSample) -> Result<TypedImage, ImageSampleError> {
    let raw = sample.raw_bytes().map_err(ImageSampleError::Compact)?;
    let metadata = &sample.metadata;
    let frame = decode(RawFrame {
        width: metadata.width,
        height: metadata.height,
        encoding: &metadata.encoding,
        step: metadata.step,
        big_endian: metadata.is_bigendian,
        data: &raw,
    })
    .map_err(ImageSampleError::Decode)?;
    Ok(TypedImage {
        frame,
        header_stamp: HeaderStamp {
            seconds: metadata.header_stamp.seconds,
            nanoseconds: metadata.header_stamp.nanoseconds,
        },
        frame_id: metadata.frame_id.clone(),
        source_time: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::images::{DecodeError, MAX_FRAME_BYTES};
    use studio_protocol::image::{HeaderStamp as WireStamp, ImageMetadata};

    fn image(encoding: &str, width: u32, height: u32, step: u32, raw: &[u8]) -> ImageSample {
        ImageSample::from_raw(
            "/camera",
            ImageMetadata {
                header_stamp: WireStamp {
                    seconds: -2,
                    nanoseconds: 999_999_999,
                },
                frame_id: "camera_optical".into(),
                width,
                height,
                step,
                encoding: encoding.into(),
                is_bigendian: false,
            },
            raw,
        )
        .unwrap()
    }

    #[test]
    fn compact_five_encodings_preserve_channels_alpha_stride_and_top_down_orientation() {
        let colors = [
            [255, 0, 0, 255],
            [0, 255, 0, 192],
            [0, 0, 255, 128],
            [255, 255, 255, 64],
        ];
        for encoding in ["mono8", "rgb8", "bgr8", "rgba8", "bgra8"] {
            let channels = match encoding {
                "mono8" => 1,
                "rgb8" | "bgr8" => 3,
                _ => 4,
            };
            let step = 2 * channels + 2;
            let mut raw = Vec::new();
            let mut expected = Vec::new();
            for (index, &[r, g, b, a]) in colors.iter().enumerate() {
                match encoding {
                    "mono8" => {
                        let value = (index as u8 + 1) * 32;
                        raw.push(value);
                        expected.push([value, value, value, 255]);
                    }
                    "rgb8" => {
                        raw.extend([r, g, b]);
                        expected.push([r, g, b, 255]);
                    }
                    "bgr8" => {
                        raw.extend([b, g, r]);
                        expected.push([r, g, b, 255]);
                    }
                    "rgba8" => {
                        raw.extend([r, g, b, a]);
                        expected.push([r, g, b, a]);
                    }
                    _ => {
                        raw.extend([b, g, r, a]);
                        expected.push([r, g, b, a]);
                    }
                }
                if index % 2 == 1 {
                    raw.extend([99, 98]);
                }
            }
            for endian in [false, true] {
                let mut sample = image(encoding, 2, 2, step, &raw);
                sample.metadata.is_bigendian = endian;
                let decoded = decode_compact_image(&sample).unwrap();
                assert_eq!(decoded.frame.size(), [2, 2]);
                assert_eq!(decoded.frame.encoding(), encoding);
                assert_eq!(
                    decoded
                        .frame
                        .pixels()
                        .iter()
                        .map(|pixel| pixel.to_srgba_unmultiplied())
                        .collect::<Vec<_>>(),
                    expected
                );
            }
        }
    }

    #[test]
    fn compact_declared_header_stamp_never_becomes_clock_provenance() {
        let decoded = decode_compact_image(&image("mono8", 1, 1, 1, &[7])).unwrap();
        assert_eq!(
            decoded.header_stamp,
            HeaderStamp {
                seconds: -2,
                nanoseconds: 999_999_999
            }
        );
        assert_eq!(decoded.frame_id, "camera_optical");
        assert_eq!(decoded.source_time, None);
    }

    #[test]
    fn compact_maximum_frame_decodes_without_typed_array_limit() {
        let raw = [1, 2, 3, 255].repeat(MAX_FRAME_BYTES / 4);
        let decoded = decode_compact_image(&image("rgba8", 2048, 2048, 8192, &raw)).unwrap();
        assert_eq!(decoded.frame.size(), [2048, 2048]);
        assert_eq!(decoded.frame.pixels().len(), 4_194_304);
        assert_eq!(decoded.frame.pixels()[0].to_array(), [1, 2, 3, 255]);
        assert_eq!(decoded.frame.pixels()[4_194_303].to_array(), [1, 2, 3, 255]);
        let raw = vec![8; 64 * 48];
        assert_eq!(
            decode_compact_image(&image("mono8", 64, 48, 64, &raw))
                .unwrap()
                .frame
                .pixels()
                .len(),
            3072
        );
    }

    #[test]
    fn compact_unsupported_incomplete_oversized_and_invalid_metadata_fail_explicitly() {
        let unsupported = image("16SC1", 1, 1, 2, &[1, 2]);
        let error = decode_compact_image(&unsupported).unwrap_err();
        assert_eq!(
            error,
            ImageSampleError::Decode(DecodeError::UnsupportedEncoding)
        );
        assert!(error.to_string().contains("16UC1 or 32FC1"));
        let good = image("rgb8", 1, 1, 3, &[1, 2, 3]);
        let mut short = good.clone();
        short.data_base64.truncate(3);
        let mut incomplete = good.clone();
        incomplete.complete = false;
        let mut oversized = good.clone();
        oversized.metadata.step = u32::MAX;
        let mut bad_stride = good.clone();
        bad_stride.metadata.step = 2;
        let mut bad_stamp = good.clone();
        bad_stamp.metadata.header_stamp.nanoseconds = 1_000_000_000;
        let mut bad_dimensions = good;
        bad_dimensions.metadata.width = 2049;
        for sample in [
            short,
            incomplete,
            oversized,
            bad_stride,
            bad_stamp,
            bad_dimensions,
        ] {
            assert!(matches!(
                decode_compact_image(&sample),
                Err(ImageSampleError::Compact(_))
            ));
        }
    }
}
