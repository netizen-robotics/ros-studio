//! Worker-only adapter for complete typed Image messages. Contract2 arrays cap
//! this compatibility path at 2048 raw bytes; larger frames need a byte envelope.
use super::{decode, DecodeError, DecodedFrame, RawFrame};
use std::fmt;
use studio_protocol::structured::{Sample, SourceTime, Value};

pub const IMAGE_MESSAGE_TYPE: &str = "sensor_msgs/msg/Image";

/// ROS header stamp as declared, without inventing a clock domain or TF epoch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HeaderStamp {
    pub seconds: i32,
    pub nanoseconds: u32,
}

#[derive(Debug)]
pub struct TypedImage {
    pub frame: DecodedFrame,
    pub header_stamp: HeaderStamp,
    pub frame_id: String,
    /// Forward only explicitly supplied clock/provenance metadata.
    pub source_time: Option<SourceTime>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageSampleError {
    WrongMessageType,
    InvalidContract,
    Incomplete,
    InvalidField(&'static str),
    Compact(String),
    Decode(DecodeError),
}
impl fmt::Display for ImageSampleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongMessageType => write!(f, "Image view requires {IMAGE_MESSAGE_TYPE}."),
            Self::InvalidContract => write!(f, "Invalid typed image contract or payload budget."),
            Self::Incomplete => write!(f, "Image data is incomplete or unsupported. Publish a smaller image; typed arrays are limited to 2048 bytes."),
            Self::InvalidField(name) => write!(f, "Invalid or missing Image field: {name}."),
            Self::Compact(message) => write!(f, "Invalid compact Image: {message}."),
            Self::Decode(error) => error.fmt(f),
        }
    }
}
impl std::error::Error for ImageSampleError {}

fn member<'a>(value: &'a Value, name: &'static str) -> Result<&'a Value, ImageSampleError> {
    if let Value::Object { fields, .. } = value {
        if let Some(field) = fields.iter().find(|field| field.name == name) {
            return Ok(&field.value);
        }
    }
    Err(ImageSampleError::InvalidField(name))
}
fn uint(value: &Value, name: &'static str) -> Result<u32, ImageSampleError> {
    match member(value, name)? {
        Value::Unsigned { value } => value
            .parse()
            .map_err(|_| ImageSampleError::InvalidField(name)),
        _ => Err(ImageSampleError::InvalidField(name)),
    }
}
fn text<'a>(value: &'a Value, name: &'static str) -> Result<&'a str, ImageSampleError> {
    match member(value, name)? {
        Value::Text { value, .. } => Ok(value),
        _ => Err(ImageSampleError::InvalidField(name)),
    }
}

/// Validate and decode on one serialized transport worker, never an active UI
/// frame. No diagnostic parsing, coercion, truncation, or header-clock guessing.
pub fn decode_typed_image(sample: &Sample) -> Result<TypedImage, ImageSampleError> {
    if sample.message_type != IMAGE_MESSAGE_TYPE {
        return Err(ImageSampleError::WrongMessageType);
    }
    // Also protects direct in-process callers from malformed canonical numbers,
    // duplicate members and unchecked tree/string budgets.
    sample
        .encode()
        .map_err(|_| ImageSampleError::InvalidContract)?;
    if !sample.value.is_complete() {
        return Err(ImageSampleError::Incomplete);
    }
    let value = &sample.value;
    let header = member(value, "header")?;
    let stamp = member(header, "stamp")?;
    let seconds = match member(stamp, "sec")? {
        Value::Signed { value } => value
            .parse::<i32>()
            .map_err(|_| ImageSampleError::InvalidField("sec"))?,
        _ => return Err(ImageSampleError::InvalidField("sec")),
    };
    let nanoseconds = uint(stamp, "nanosec")?;
    if nanoseconds >= 1_000_000_000 {
        return Err(ImageSampleError::InvalidField("nanosec"));
    }
    let frame_id = text(header, "frame_id")?;
    let width = uint(value, "width")?;
    let height = uint(value, "height")?;
    let step = uint(value, "step")?;
    let encoding = text(value, "encoding")?;
    let big_endian = match uint(value, "is_bigendian")? {
        0 => false,
        1 => true,
        _ => return Err(ImageSampleError::InvalidField("is_bigendian")),
    };
    let Value::Array { items, .. } = member(value, "data")? else {
        return Err(ImageSampleError::InvalidField("data"));
    };
    let bytes = items
        .iter()
        .map(|item| match item {
            Value::Unsigned { value } => value
                .parse::<u8>()
                .map_err(|_| ImageSampleError::InvalidField("data")),
            _ => Err(ImageSampleError::InvalidField("data")),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let frame = decode(RawFrame {
        width,
        height,
        encoding,
        step,
        big_endian,
        data: &bytes,
    })
    .map_err(ImageSampleError::Decode)?;
    Ok(TypedImage {
        frame,
        header_stamp: HeaderStamp {
            seconds,
            nanoseconds,
        },
        frame_id: frame_id.to_owned(),
        source_time: sample.source_time.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use studio_protocol::structured::{
        ClockDomain, Field, PathSegment, CONTRACT_VERSION, MAX_ITEMS,
    };

    fn unsigned(value: u32) -> Value {
        Value::Unsigned {
            value: value.to_string(),
        }
    }
    fn text(value: &str) -> Value {
        Value::Text {
            value: value.into(),
            original_bytes: value.len(),
        }
    }
    fn object(fields: Vec<(&str, Value)>) -> Value {
        Value::Object {
            original_fields: fields.len(),
            fields: fields
                .into_iter()
                .map(|(name, value)| Field {
                    name: name.into(),
                    value,
                })
                .collect(),
        }
    }
    fn image() -> Sample {
        Sample {
            contract_version: CONTRACT_VERSION,
            topic: "/fixture/camera".into(),
            message_type: IMAGE_MESSAGE_TYPE.into(),
            source_time: None,
            value: object(vec![
                (
                    "header",
                    object(vec![
                        (
                            "stamp",
                            object(vec![
                                ("sec", Value::Signed { value: "-2".into() }),
                                ("nanosec", unsigned(999_999_999)),
                            ]),
                        ),
                        ("frame_id", text("camera_optical")),
                    ]),
                ),
                ("width", unsigned(2)),
                ("height", unsigned(2)),
                ("encoding", text("rgb8")),
                ("is_bigendian", unsigned(0)),
                ("step", unsigned(8)),
                (
                    "data",
                    Value::Array {
                        items: [
                            255, 0, 0, 0, 255, 0, 99, 99, 0, 0, 255, 255, 255, 255, 99, 99,
                        ]
                        .into_iter()
                        .map(unsigned)
                        .collect(),
                        original_items: 16,
                    },
                ),
            ]),
        }
    }
    fn field<'a>(value: &'a mut Value, path: &[&str]) -> &'a mut Value {
        let (name, rest) = path.split_first().unwrap();
        let Value::Object { fields, .. } = value else {
            panic!("test object")
        };
        let value = &mut fields
            .iter_mut()
            .find(|field| field.name == *name)
            .unwrap()
            .value;
        if rest.is_empty() {
            value
        } else {
            field(value, rest)
        }
    }
    fn set(sample: &mut Sample, path: &[&str], value: Value) {
        *field(&mut sample.value, path) = value;
    }

    #[test]
    fn typed_image_preserves_colors_stride_orientation_and_timestamp_provenance() {
        let mut sample = image();
        let source = SourceTime {
            seconds: -2,
            nanoseconds: 999_999_999,
            clock: ClockDomain::Ros { epoch: 7 },
            header_path: vec![
                PathSegment::Member("header".into()),
                PathSegment::Member("stamp".into()),
            ],
        };
        sample.source_time = Some(source.clone());
        for endian in [0, 1] {
            set(&mut sample, &["is_bigendian"], unsigned(endian));
            let decoded = decode_typed_image(&sample).unwrap();
            assert_eq!(decoded.frame.size(), [2, 2]);
            assert_eq!(decoded.frame.encoding(), "rgb8");
            assert_eq!(
                decoded
                    .frame
                    .pixels()
                    .iter()
                    .map(|pixel| pixel.to_array())
                    .collect::<Vec<_>>(),
                [
                    [255, 0, 0, 255],
                    [0, 255, 0, 255],
                    [0, 0, 255, 255],
                    [255, 255, 255, 255]
                ]
            );
            assert_eq!(
                decoded.header_stamp,
                HeaderStamp {
                    seconds: -2,
                    nanoseconds: 999_999_999
                }
            );
            assert_eq!(decoded.frame_id, "camera_optical");
            assert_eq!(decoded.source_time, Some(source.clone()));
        }
        sample.source_time = None;
        assert_eq!(decode_typed_image(&sample).unwrap().source_time, None);
    }

    #[test]
    fn typed_image_rejects_partial_data_and_metadata_without_coercion() {
        let mut sample = image();
        let Value::Array { original_items, .. } = field(&mut sample.value, &["data"]) else {
            unreachable!()
        };
        *original_items += 1;
        assert_eq!(
            decode_typed_image(&sample).unwrap_err(),
            ImageSampleError::Incomplete
        );
        let mut sample = image();
        let Value::Text { original_bytes, .. } = field(&mut sample.value, &["encoding"]) else {
            unreachable!()
        };
        *original_bytes += 1;
        assert_eq!(
            decode_typed_image(&sample).unwrap_err(),
            ImageSampleError::Incomplete
        );
        for (path, value, expected) in [
            (vec!["width"], Value::Signed { value: "2".into() }, "width"),
            (
                vec!["height"],
                Value::Unsigned {
                    value: "4294967296".into(),
                },
                "height",
            ),
            (vec!["is_bigendian"], unsigned(2), "is_bigendian"),
            (
                vec!["is_bigendian"],
                Value::Bool { value: false },
                "is_bigendian",
            ),
            (
                vec!["header", "stamp", "sec"],
                Value::Signed {
                    value: "2147483648".into(),
                },
                "sec",
            ),
            (
                vec!["header", "stamp", "nanosec"],
                unsigned(1_000_000_000),
                "nanosec",
            ),
            (vec!["data"], text("255,0,0"), "data"),
            (
                vec!["data"],
                Value::Array {
                    items: vec![unsigned(256)],
                    original_items: 1,
                },
                "data",
            ),
        ] {
            let mut sample = image();
            set(&mut sample, &path, value);
            assert_eq!(
                decode_typed_image(&sample).unwrap_err(),
                ImageSampleError::InvalidField(expected)
            );
        }
        let mut sample = image();
        sample.message_type = "sensor_msgs/msg/CompressedImage".into();
        assert_eq!(
            decode_typed_image(&sample).unwrap_err(),
            ImageSampleError::WrongMessageType
        );
        let mut sample = image();
        sample.contract_version += 1;
        assert_eq!(
            decode_typed_image(&sample).unwrap_err(),
            ImageSampleError::InvalidContract
        );
    }

    #[test]
    fn typed_image_does_not_invent_missing_fields_or_ignore_bad_contract_shapes() {
        let mut sample = image();
        let Value::Object {
            fields,
            original_fields,
        } = &mut sample.value
        else {
            unreachable!()
        };
        fields.retain(|field| field.name != "width");
        *original_fields -= 1;
        assert_eq!(
            decode_typed_image(&sample).unwrap_err(),
            ImageSampleError::InvalidField("width")
        );
        let mut sample = image();
        let Value::Object {
            fields,
            original_fields,
        } = &mut sample.value
        else {
            unreachable!()
        };
        fields.push(fields[0].clone());
        *original_fields += 1;
        assert_eq!(
            decode_typed_image(&sample).unwrap_err(),
            ImageSampleError::InvalidContract
        );
        let mut sample = image();
        set(
            &mut sample,
            &["width"],
            Value::Unsigned { value: "02".into() },
        );
        assert_eq!(
            decode_typed_image(&sample).unwrap_err(),
            ImageSampleError::InvalidContract
        );
    }

    #[test]
    fn typed_image_enforces_transport_and_decoder_budgets_independently() {
        let mut sample = image();
        set(&mut sample, &["width"], unsigned(64));
        set(&mut sample, &["height"], unsigned(32));
        set(&mut sample, &["step"], unsigned(64));
        set(&mut sample, &["encoding"], text("mono8"));
        set(
            &mut sample,
            &["data"],
            Value::Array {
                items: vec![unsigned(63); MAX_ITEMS],
                original_items: MAX_ITEMS,
            },
        );
        let wire = sample.encode().unwrap();
        assert_eq!(
            decode_typed_image(&Sample::decode(&wire).unwrap())
                .unwrap()
                .frame
                .pixels()
                .len(),
            MAX_ITEMS
        );
        set(
            &mut sample,
            &["data"],
            Value::Array {
                items: vec![unsigned(63); MAX_ITEMS + 1],
                original_items: MAX_ITEMS + 1,
            },
        );
        assert_eq!(
            decode_typed_image(&sample).unwrap_err(),
            ImageSampleError::InvalidContract
        );
        let mut sample = image();
        set(&mut sample, &["step"], unsigned(u32::MAX));
        assert_eq!(
            decode_typed_image(&sample).unwrap_err(),
            ImageSampleError::Decode(DecodeError::TooLarge)
        );
        let mut sample = image();
        set(&mut sample, &["encoding"], text("16SC1"));
        assert_eq!(
            decode_typed_image(&sample).unwrap_err(),
            ImageSampleError::Decode(DecodeError::UnsupportedEncoding)
        );
        let mut sample = image();
        set(&mut sample, &["step"], unsigned(5));
        assert_eq!(
            decode_typed_image(&sample).unwrap_err(),
            ImageSampleError::Decode(DecodeError::InvalidStride)
        );
        let mut sample = image();
        set(&mut sample, &["height"], unsigned(1));
        assert_eq!(
            decode_typed_image(&sample).unwrap_err(),
            ImageSampleError::Decode(DecodeError::PayloadLength {
                expected: 8,
                actual: 16
            })
        );
    }
}
