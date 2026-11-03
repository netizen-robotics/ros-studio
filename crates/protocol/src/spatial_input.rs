//! Pure typed adapters. These validate input; native tf2 still owns lookup,
//! interpolation, graph freshness and cache admission. Run on a worker.
use crate::structured::{ClockDomain, FloatValue, PathSegment, Sample, Value, MAX_ITEMS};
use std::{collections::HashSet, fmt};

pub const SCAN_MESSAGE_TYPE: &str = "sensor_msgs/msg/LaserScan";
pub const TF_MESSAGE_TYPE: &str = "tf2_msgs/msg/TFMessage";
/// Stricter than the wire array cap; tree/node budgets may reject earlier.
pub const MAX_TF_TRANSFORMS: usize = 128;

/// Supplied by the session's verified clock policy, never inferred from a
/// header, wall-clock proximity or local receipt time. Unknown is the default.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SpatialClockPolicy {
    #[default]
    Unknown,
    VerifiedSystem,
    Simulated,
}

#[derive(Debug)]
pub struct TypedScan {
    pub frame: String,
    pub stamp_ns: i64,
    pub angle_min: f64,
    pub angle_max: f64,
    pub angle_increment: f64,
    pub time_increment: f64,
    pub scan_time: f64,
    pub range_min: f64,
    pub range_max: f64,
    pub ranges: Vec<f32>,
    pub intensities: Vec<f32>,
}
/// This comes from the identified subscription configuration. A topic spelling
/// alone does not establish durability or turn a dynamic stream into static TF.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TfStreamKind {
    Dynamic,
    Static,
}
#[derive(Clone, Debug, PartialEq)]
pub struct TypedTransform {
    pub parent: String,
    pub child: String,
    pub stamp_ns: i64,
    /// Exact received components in native tf2 insert order: tx,ty,tz,qx,qy,qz,qw.
    pub components: [f64; 7],
}
#[derive(Clone, Debug, PartialEq)]
pub struct TypedTfBatch {
    pub kind: TfStreamKind,
    pub transforms: Vec<TypedTransform>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SpatialSampleError {
    WrongMessageType(&'static str),
    InvalidContract,
    Incomplete,
    InvalidField(&'static str),
    ClockUnverified,
    UnsupportedClock,
    SourceTimeMismatch,
    TransformLimit,
    DuplicateChild,
}
impl fmt::Display for SpatialSampleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongMessageType(expected) => write!(f, "Spatial input requires {expected}."),
            Self::InvalidContract => write!(f, "Invalid typed spatial contract or payload budget."),
            Self::Incomplete => write!(f, "Incomplete spatial input rejected; publish a complete bounded sample."),
            Self::InvalidField(field) => write!(f, "Invalid or missing spatial field: {field}."),
            Self::ClockUnverified => write!(f, "System-clock policy must be explicitly verified before displaying spatial data."),
            Self::UnsupportedClock => write!(f, "Simulated time is unsupported in this spatial slice."),
            Self::SourceTimeMismatch => write!(f, "Source timestamp provenance contradicts the typed header or verified clock policy."),
            Self::TransformLimit => write!(f, "TF batch exceeds the 128-transform input budget."),
            Self::DuplicateChild => write!(f, "TF batch contains conflicting repeated child frames."),
        }
    }
}
impl std::error::Error for SpatialSampleError {}

fn member<'a>(value: &'a Value, name: &'static str) -> Result<&'a Value, SpatialSampleError> {
    match value {
        Value::Object { fields, .. } => fields
            .iter()
            .find(|field| field.name == name)
            .map(|field| &field.value)
            .ok_or(SpatialSampleError::InvalidField(name)),
        _ => Err(SpatialSampleError::InvalidField(name)),
    }
}
fn frame(value: &Value, name: &'static str) -> Result<String, SpatialSampleError> {
    match member(value, name)? {
        Value::Text { value, .. }
            if !value.is_empty()
                && value.len() <= 256
                && !value.chars().any(|c| c.is_whitespace() || c.is_control()) =>
        {
            Ok(value.clone())
        }
        _ => Err(SpatialSampleError::InvalidField(name)),
    }
}
fn stamp(value: &Value) -> Result<i64, SpatialSampleError> {
    let sec = match member(value, "sec")? {
        Value::Signed { value } => value
            .parse::<i32>()
            .map_err(|_| SpatialSampleError::InvalidField("sec"))?,
        _ => return Err(SpatialSampleError::InvalidField("sec")),
    };
    let nsec = match member(value, "nanosec")? {
        Value::Unsigned { value } => value
            .parse::<u32>()
            .map_err(|_| SpatialSampleError::InvalidField("nanosec"))?,
        _ => return Err(SpatialSampleError::InvalidField("nanosec")),
    };
    if sec < 0 || nsec >= 1_000_000_000 {
        return Err(SpatialSampleError::InvalidField("stamp"));
    }
    Ok(i64::from(sec) * 1_000_000_000 + i64::from(nsec))
}
fn finite(value: &Value, name: &'static str) -> Result<f64, SpatialSampleError> {
    match member(value, name)? {
        Value::Float {
            value: FloatValue::Finite(value),
        } => value
            .parse::<f64>()
            .ok()
            .filter(|number| number.is_finite())
            .ok_or(SpatialSampleError::InvalidField(name)),
        _ => Err(SpatialSampleError::InvalidField(name)),
    }
}
fn finite32(value: &Value, name: &'static str) -> Result<f64, SpatialSampleError> {
    let number = finite(value, name)? as f32;
    if number.is_finite() {
        Ok(f64::from(number))
    } else {
        Err(SpatialSampleError::InvalidField(name))
    }
}
fn floats(value: &Value, name: &'static str) -> Result<Vec<f32>, SpatialSampleError> {
    let Value::Array { items, .. } = member(value, name)? else {
        return Err(SpatialSampleError::InvalidField(name));
    };
    if items.len() > MAX_ITEMS {
        return Err(SpatialSampleError::InvalidField(name));
    }
    items
        .iter()
        .map(|item| match item {
            Value::Float {
                value: FloatValue::Finite(value),
            } => value
                .parse::<f32>()
                .ok()
                .filter(|number| number.is_finite())
                .ok_or(SpatialSampleError::InvalidField(name)),
            Value::Float {
                value: FloatValue::Nan,
            } => Ok(f32::NAN),
            Value::Float {
                value: FloatValue::PositiveInfinity,
            } => Ok(f32::INFINITY),
            Value::Float {
                value: FloatValue::NegativeInfinity,
            } => Ok(f32::NEG_INFINITY),
            _ => Err(SpatialSampleError::InvalidField(name)),
        })
        .collect()
}
fn validate(
    sample: &Sample,
    expected: &'static str,
    policy: SpatialClockPolicy,
) -> Result<(), SpatialSampleError> {
    if sample.message_type != expected {
        return Err(SpatialSampleError::WrongMessageType(expected));
    }
    sample
        .encode()
        .map_err(|_| SpatialSampleError::InvalidContract)?;
    if !sample.value.is_complete() {
        return Err(SpatialSampleError::Incomplete);
    }
    match policy {
        SpatialClockPolicy::Unknown => return Err(SpatialSampleError::ClockUnverified),
        SpatialClockPolicy::Simulated => return Err(SpatialSampleError::UnsupportedClock),
        SpatialClockPolicy::VerifiedSystem => (),
    }
    if let Some(time) = &sample.source_time {
        if time.clock != ClockDomain::System {
            return Err(SpatialSampleError::SourceTimeMismatch);
        }
        let valid_path = match (expected, time.header_path.as_slice()) {
            (SCAN_MESSAGE_TYPE, [PathSegment::Member(header), PathSegment::Member(stamp)]) => {
                header == "header" && stamp == "stamp"
            }
            (
                TF_MESSAGE_TYPE,
                [PathSegment::Member(transforms), PathSegment::Index(_), PathSegment::Member(header), PathSegment::Member(stamp)],
            ) => transforms == "transforms" && header == "header" && stamp == "stamp",
            _ => false,
        };
        if !valid_path {
            return Err(SpatialSampleError::SourceTimeMismatch);
        }
        let declared = sample
            .value
            .lookup(&time.header_path)
            .map_err(|_| SpatialSampleError::SourceTimeMismatch)?;
        let ns = stamp(declared).map_err(|_| SpatialSampleError::SourceTimeMismatch)?;
        if time
            .seconds
            .checked_mul(1_000_000_000)
            .and_then(|sec| sec.checked_add(i64::from(time.nanoseconds)))
            != Some(ns)
        {
            return Err(SpatialSampleError::SourceTimeMismatch);
        }
    }
    Ok(())
}

pub fn decode_laser_scan(
    sample: &Sample,
    policy: SpatialClockPolicy,
) -> Result<TypedScan, SpatialSampleError> {
    validate(sample, SCAN_MESSAGE_TYPE, policy)?;
    let value = &sample.value;
    let header = member(value, "header")?;
    let frame = frame(header, "frame_id")?;
    let stamp_ns = stamp(member(header, "stamp")?)?;
    if stamp_ns == 0 {
        return Err(SpatialSampleError::InvalidField("stamp"));
    }
    let angle_min = finite32(value, "angle_min")?;
    let angle_max = finite32(value, "angle_max")?;
    let angle_increment = finite32(value, "angle_increment")?;
    let time_increment = finite32(value, "time_increment")?;
    let scan_time = finite32(value, "scan_time")?;
    let range_min = finite32(value, "range_min")?;
    let range_max = finite32(value, "range_max")?;
    let ranges = floats(value, "ranges")?;
    let intensities = floats(value, "intensities")?;
    if ranges.is_empty() {
        return Err(SpatialSampleError::InvalidField("ranges"));
    }
    if !intensities.is_empty() && intensities.len() != ranges.len() {
        return Err(SpatialSampleError::InvalidField("intensities"));
    }
    if angle_increment <= 0.0 || angle_max < angle_min {
        return Err(SpatialSampleError::InvalidField("angle_increment"));
    }
    let last_angle = angle_min + (ranges.len() - 1) as f64 * angle_increment;
    let tolerance = 1e-5
        * last_angle
            .abs()
            .max(angle_min.abs())
            .max(angle_max.abs())
            .max(1.0);
    if !last_angle.is_finite() || (last_angle - angle_max).abs() > tolerance {
        return Err(SpatialSampleError::InvalidField("angle_max"));
    }
    if time_increment < 0.0 || scan_time < 0.0 {
        return Err(SpatialSampleError::InvalidField("scan_time"));
    }
    if range_min < 0.0 || range_min >= range_max || range_max > 1_000_000.0 {
        return Err(SpatialSampleError::InvalidField("range_max"));
    }
    Ok(TypedScan {
        frame,
        stamp_ns,
        angle_min,
        angle_max,
        angle_increment,
        time_increment,
        scan_time,
        range_min,
        range_max,
        ranges,
        intensities,
    })
}

pub fn decode_tf_message(
    sample: &Sample,
    kind: TfStreamKind,
    policy: SpatialClockPolicy,
) -> Result<TypedTfBatch, SpatialSampleError> {
    validate(sample, TF_MESSAGE_TYPE, policy)?;
    let Value::Array { items, .. } = member(&sample.value, "transforms")? else {
        return Err(SpatialSampleError::InvalidField("transforms"));
    };
    if items.len() > MAX_TF_TRANSFORMS {
        return Err(SpatialSampleError::TransformLimit);
    }
    let mut children = HashSet::with_capacity(items.len());
    let mut transforms = Vec::with_capacity(items.len());
    for item in items {
        let header = member(item, "header")?;
        let parent = frame(header, "frame_id")?;
        let child = frame(item, "child_frame_id")?;
        if parent == child {
            return Err(SpatialSampleError::InvalidField("child_frame_id"));
        }
        if !children.insert(child.clone()) {
            return Err(SpatialSampleError::DuplicateChild);
        }
        let stamp_ns = stamp(member(header, "stamp")?)?;
        if kind == TfStreamKind::Dynamic && stamp_ns == 0 {
            return Err(SpatialSampleError::InvalidField("stamp"));
        }
        let transform = member(item, "transform")?;
        let translation = member(transform, "translation")?;
        let rotation = member(transform, "rotation")?;
        let components = [
            finite(translation, "x")?,
            finite(translation, "y")?,
            finite(translation, "z")?,
            finite(rotation, "x")?,
            finite(rotation, "y")?,
            finite(rotation, "z")?,
            finite(rotation, "w")?,
        ];
        if components[..3]
            .iter()
            .any(|coordinate| coordinate.abs() > 1_000_000.0)
            || (components[3..].iter().map(|v| v * v).sum::<f64>() - 1.0).abs() > 1e-6
        {
            return Err(SpatialSampleError::InvalidField("transform"));
        }
        transforms.push(TypedTransform {
            parent,
            child,
            stamp_ns,
            components,
        });
    }
    Ok(TypedTfBatch { kind, transforms })
}
