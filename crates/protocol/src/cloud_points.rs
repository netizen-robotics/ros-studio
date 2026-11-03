//! Canonical cloud2 XYZ records leave room for complete aligned attributes
//! within the unchanged 512 KiB sample envelope. Count is checked before decode.
use super::{MAX_COORDINATE, MAX_SOURCE_POINTS};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Wire {
    count: usize,
    data_base64: String,
}
pub fn serialize<S: Serializer>(points: &[[f32; 3]], serializer: S) -> Result<S::Ok, S::Error> {
    if points.len() > MAX_SOURCE_POINTS
        || points
            .iter()
            .flatten()
            .any(|n| !n.is_finite() || f64::from(n.abs()) > MAX_COORDINATE)
    {
        return Err(serde::ser::Error::custom(
            "Invalid bounded cloud2 positions",
        ));
    }
    let mut raw = Vec::with_capacity(points.len() * 12);
    for n in points.iter().flatten() {
        raw.extend_from_slice(&n.to_le_bytes());
    }
    Wire {
        count: points.len(),
        data_base64: crate::binary::encode(&raw),
    }
    .serialize(serializer)
}
pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<[f32; 3]>, D::Error> {
    let wire = Wire::deserialize(deserializer)?;
    if wire.count > MAX_SOURCE_POINTS {
        return Err(serde::de::Error::custom(
            "cloud2 position count exceeds 8192",
        ));
    }
    let raw =
        crate::binary::decode_exact(&wire.data_base64, wire.count * 12, MAX_SOURCE_POINTS * 12)
            .map_err(serde::de::Error::custom)?;
    let mut points = Vec::with_capacity(wire.count);
    for bytes in raw.chunks_exact(12) {
        let point = std::array::from_fn(|i| {
            f32::from_le_bytes(bytes[i * 4..i * 4 + 4].try_into().unwrap())
        });
        if point
            .iter()
            .any(|n| !n.is_finite() || f64::from(n.abs()) > MAX_COORDINATE)
        {
            return Err(serde::de::Error::custom("Invalid cloud2 position"));
        }
        points.push(point);
    }
    Ok(points)
}
