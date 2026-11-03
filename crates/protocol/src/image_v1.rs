//! Compact bounded sensor_msgs/Image contract. Session capability/version
//! negotiation is separate; importing this module never activates a wire mode.
use serde::{Deserialize, Serialize};
use std::io::Write;

pub const CONTRACT_VERSION: u32 = 1;
pub const MESSAGE_TYPE: &str = "sensor_msgs/msg/Image";
pub const MAX_RAW_BYTES: usize = 256 * 1024;
pub const MAX_SAMPLE_BYTES: usize = 512 * 1024;
pub const MAX_BASE64_BYTES: usize = MAX_RAW_BYTES.div_ceil(3) * 4;
pub const MAX_DIMENSION: u32 = 2048;

/// The declared ROS header stamp, without guessing a clock domain.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HeaderStamp {
    pub seconds: i32,
    pub nanoseconds: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageMetadata {
    pub header_stamp: HeaderStamp,
    pub frame_id: String,
    pub width: u32,
    pub height: u32,
    pub encoding: String,
    pub is_bigendian: bool,
    pub step: u32,
}
impl ImageMetadata {
    /// Checked before raw allocation or native element reads. Unsupported
    /// encodings remain explicit; only known channel layouts get stride checks.
    pub fn raw_length(&self) -> Result<usize, String> {
        if self.header_stamp.nanoseconds >= 1_000_000_000
            || self.frame_id.len() > 256
            || self.frame_id.contains('\0')
            || self.encoding.is_empty()
            || self.encoding.len() > 64
            || self.encoding.chars().any(char::is_control)
        {
            return Err("Invalid bounded image header or encoding metadata".into());
        }
        if self.width == 0
            || self.height == 0
            || self.width > MAX_DIMENSION
            || self.height > MAX_DIMENSION
        {
            return Err("Image dimensions must be nonzero and at most 2048 per axis".into());
        }
        let pixels = (self.width as usize)
            .checked_mul(self.height as usize)
            .ok_or("Image pixel count overflow")?;
        if pixels
            .checked_mul(4)
            .is_none_or(|bytes| bytes > MAX_RAW_BYTES)
        {
            return Err("Image decoded RGBA budget exceeds 256 KiB".into());
        }
        let channels = match self.encoding.as_str() {
            "mono8" => Some(1),
            "rgb8" | "bgr8" => Some(3),
            "rgba8" | "bgra8" => Some(4),
            _ => None,
        };
        if self.step == 0
            || channels
                .is_some_and(|channels| u64::from(self.step) < u64::from(self.width) * channels)
        {
            return Err("Image row stride is smaller than declared pixels".into());
        }
        let bytes = (self.step as usize)
            .checked_mul(self.height as usize)
            .ok_or("Image raw length overflow")?;
        if bytes > MAX_RAW_BYTES {
            return Err("Image raw payload exceeds 256 KiB".into());
        }
        Ok(bytes)
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ImageSample {
    pub contract_version: u32,
    pub topic: String,
    pub message_type: String,
    pub metadata: ImageMetadata,
    pub original_bytes: usize,
    pub complete: bool,
    pub data_base64: String,
}
impl ImageSample {
    pub fn from_raw(topic: &str, metadata: ImageMetadata, raw: &[u8]) -> Result<Self, String> {
        let expected = metadata.raw_length()?;
        if raw.len() != expected {
            return Err(
                "Image payload must exactly match step × height, including row padding".into(),
            );
        }
        let value = Self {
            contract_version: CONTRACT_VERSION,
            topic: topic.into(),
            message_type: MESSAGE_TYPE.into(),
            metadata,
            original_bytes: raw.len(),
            complete: true,
            data_base64: encode_base64(raw),
        };
        value.validate()?;
        Ok(value)
    }
    pub fn validate(&self) -> Result<(), String> {
        if self.contract_version != CONTRACT_VERSION || self.message_type != MESSAGE_TYPE {
            return Err("Incompatible compact Image contract or message type".into());
        }
        if self.topic.is_empty() || self.topic.len() > 4096 || self.topic.contains('\0') {
            return Err("Invalid image topic identity".into());
        }
        let expected = self.metadata.raw_length()?;
        if !self.complete || self.original_bytes != expected {
            return Err("Incomplete or inconsistent raw Image payload".into());
        }
        check_base64(&self.data_base64, expected)
    }
    /// Decode only after validate has bounded metadata, text and exact length.
    pub fn raw_bytes(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        crate::binary::decode_exact(&self.data_base64, self.original_bytes, MAX_RAW_BYTES)
    }
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        struct Bounded(Vec<u8>);
        impl Write for Bounded {
            fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
                if bytes.len() > MAX_SAMPLE_BYTES.saturating_sub(self.0.len()) {
                    return Err(std::io::Error::other(
                        "Compact Image sample exceeds 512 KiB wire budget",
                    ));
                }
                self.0.extend_from_slice(bytes);
                Ok(bytes.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        let mut output = Bounded(Vec::new());
        serde_json::to_writer(&mut output, self).map_err(|error| error.to_string())?;
        Ok(output.0)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_SAMPLE_BYTES {
            return Err("Compact Image sample exceeds 512 KiB wire budget".into());
        }
        let value: Self = serde_json::from_slice(bytes).map_err(|error| error.to_string())?;
        value.validate()?;
        Ok(value)
    }
}

use crate::binary::encode as encode_base64;
fn check_base64(text: &str, expected: usize) -> Result<(), String> {
    crate::binary::validate_exact(text, expected, MAX_RAW_BYTES)
}

#[cfg(test)]
mod tests {
    use super::*;
    fn metadata(width: u32, height: u32, encoding: &str, step: u32) -> ImageMetadata {
        ImageMetadata {
            header_stamp: HeaderStamp {
                seconds: -2,
                nanoseconds: 999_999_999,
            },
            frame_id: "camera_optical".into(),
            width,
            height,
            encoding: encoding.into(),
            is_bigendian: false,
            step,
        }
    }
    fn sample(raw: &[u8]) -> ImageSample {
        ImageSample::from_raw(
            "/camera",
            metadata(raw.len() as u32, 1, "mono8", raw.len() as u32),
            raw,
        )
        .unwrap()
    }
    #[test]
    fn base64_matches_known_vectors_and_all_byte_values() {
        for (raw, encoded) in [
            ("", ""),
            ("f", "Zg=="),
            ("fo", "Zm8="),
            ("foo", "Zm9v"),
            ("foob", "Zm9vYg=="),
            ("fooba", "Zm9vYmE="),
            ("foobar", "Zm9vYmFy"),
        ] {
            assert_eq!(encode_base64(raw.as_bytes()), encoded);
            check_base64(encoded, raw.len()).unwrap();
            if !raw.is_empty() {
                assert_eq!(sample(raw.as_bytes()).raw_bytes().unwrap(), raw.as_bytes());
            }
        }
        let raw: Vec<u8> = (0..=255).collect();
        assert_eq!(sample(&raw).raw_bytes().unwrap(), raw);
    }
    #[test]
    fn malformed_base64_never_accepts_noncanonical_padding_or_partial_data() {
        for (expected, text) in [
            (1, "/x=="),
            (2, "//9="),
            (1, "_w=="),
            (1, "-w=="),
            (1, "Zg= "),
            (1, "Zg==\n"),
            (1, "Zg"),
            (1, "=g=="),
            (2, "Zg=="),
            (3, "Zg=="),
            (4, "Zg==Zg=="),
            (1, "é=="),
        ] {
            assert!(check_base64(text, expected).is_err(), "{text:?}");
        }
        let mut sample = sample(&[255]);
        sample.data_base64 = "/x==".into();
        assert!(sample.raw_bytes().is_err());
        let mut sample = self::sample(&[1]);
        sample.complete = false;
        assert!(sample.encode().is_err());
        sample.complete = true;
        sample.original_bytes = 2;
        assert!(sample.encode().is_err());
    }
    #[test]
    fn full_256_kib_frame_roundtrips_below_wire_budget_with_exact_metadata() {
        let raw: Vec<u8> = (0..MAX_RAW_BYTES)
            .map(|index| (index % 256) as u8)
            .collect();
        let mut metadata = metadata(256, 256, "rgba8", 1024);
        metadata.is_bigendian = true;
        let sample = ImageSample::from_raw("/fixture/camera", metadata.clone(), &raw).unwrap();
        let wire = sample.encode().unwrap();
        assert!(wire.len() < MAX_SAMPLE_BYTES);
        let decoded = ImageSample::decode(&wire).unwrap();
        assert_eq!(decoded.metadata, metadata);
        assert_eq!(decoded.raw_bytes().unwrap(), raw);
        assert_eq!(decoded.original_bytes, MAX_RAW_BYTES);
        assert_eq!(decoded.data_base64.len(), MAX_BASE64_BYTES);
    }
    #[test]
    fn metadata_stride_completeness_and_unknown_encoding_have_explicit_semantics() {
        let padded = metadata(2, 2, "rgb8", 8);
        assert!(ImageSample::from_raw("/camera", padded.clone(), &[0; 15]).is_err());
        assert!(ImageSample::from_raw("/camera", padded.clone(), &[0; 17]).is_err());
        assert!(ImageSample::from_raw("/camera", padded, &[0; 16]).is_ok());
        for meta in [
            metadata(0, 1, "mono8", 1),
            metadata(2049, 1, "mono8", 2049),
            metadata(257, 256, "mono8", 257),
            metadata(2, 1, "rgb8", 5),
            metadata(1, 2048, "mono8", u32::MAX),
            metadata(1, 1, "", 1),
        ] {
            assert!(meta.raw_length().is_err());
        }
        let mut meta = metadata(1, 1, "mono8", 1);
        meta.header_stamp.nanoseconds = 1_000_000_000;
        assert!(meta.raw_length().is_err());
        let mut meta = metadata(1, 1, "mono8", 1);
        meta.frame_id = "x".repeat(257);
        assert!(meta.raw_length().is_err());
        // Keep the actual unsupported encoding for an actionable renderer error.
        let unknown =
            ImageSample::from_raw("/camera", metadata(1, 1, "16UC1", 2), &[1, 2]).unwrap();
        assert_eq!(unknown.metadata.encoding, "16UC1");
        assert_eq!(unknown.raw_bytes().unwrap(), [1, 2]);
    }
    #[test]
    fn wire_limits_identity_and_schema_are_checked_before_use() {
        assert!(ImageSample::decode(&vec![b' '; MAX_SAMPLE_BYTES + 1]).is_err());
        let good = sample(&[1, 2, 3]);
        let wire = good.encode().unwrap();
        let mut json: serde_json::Value = serde_json::from_slice(&wire).unwrap();
        for key in ["contract_version", "message_type"] {
            let mut value = json.clone();
            value[key] = if key == "contract_version" {
                99.into()
            } else {
                "std_msgs/msg/String".into()
            };
            assert!(ImageSample::decode(&serde_json::to_vec(&value).unwrap()).is_err());
        }
        json["metadata"]["unknown"] = true.into();
        assert!(ImageSample::decode(&serde_json::to_vec(&json).unwrap()).is_err());
        let mut bad = good.clone();
        bad.data_base64 = "A".repeat(MAX_BASE64_BYTES + 1);
        assert!(bad.validate().is_err());
        let mut bad = good;
        bad.metadata.width = 4;
        assert!(bad.validate().is_err());
    }
}
