//! Paired Spatial contract1. Pure values only: native tf2 and egui stay outside.
use crate::{spatial_input::*, structured};
use serde::{Deserialize, Serialize};
pub const CONTRACT_VERSION: u32 = 1;
pub const MAX_SCAN_RAYS: usize = structured::MAX_ITEMS;
pub const MAX_SAMPLE_BYTES: usize = structured::MAX_SAMPLE_BYTES;
pub const MAX_FRAMES: usize = 256;
pub const MAX_EDGES: usize = 128;
pub const DYNAMIC_INSERTS_PER_WINDOW: usize = 256;
pub const CACHE_WINDOW_NS: i64 = 10_000_000_000;
pub const PENDING_TIMEOUT_MS: u64 = 1000;
pub const RETRY_INTERVAL_MS: u64 = 50;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub fixed_frame: String,
    pub clock_policy: SpatialClockPolicy,
    pub dynamic_tf_topic: String,
    pub dynamic_tf_best_effort: bool,
    /// Always Reliable + TransientLocal; deliberately no unsafe toggle.
    pub static_tf_topic: String,
}
pub fn valid_frame(frame: &str) -> bool {
    !frame.is_empty()
        && frame.len() <= 256
        && !frame.starts_with('/')
        && !frame.chars().any(|c| c.is_whitespace() || c.is_control())
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Counters {
    pub scans_received: u64,
    pub pending_replaced: u64,
    pub lookup_timeouts: u64,
    pub tf_dynamic_batches: u64,
    pub tf_static_batches: u64,
    pub tf_accepted: u64,
    pub tf_rejected: u64,
    pub duplicate_statics: u64,
    pub native_errors: u64,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case", deny_unknown_fields)]
pub enum TfOutcome {
    Ready { components: [f64; 7] },
    Missing,
    Stale,
    Extrapolated,
    LookupTimeout,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    pub contract_version: u32,
    pub scan: Box<structured::Sample>,
    pub fixed_frame: String,
    pub source_frame: String,
    pub acquisition_stamp_ns: i64,
    pub tf: TfOutcome,
    pub counters: Counters,
}
impl Sample {
    pub fn validate(&self) -> Result<(), String> {
        if self.contract_version != CONTRACT_VERSION || !valid_frame(&self.fixed_frame) {
            return Err("Invalid Spatial contract or fixed frame".into());
        }
        let scan = decode_laser_scan(&self.scan, SpatialClockPolicy::VerifiedSystem)
            .map_err(|e| e.to_string())?;
        if !valid_frame(&self.source_frame)
            || self.source_frame != scan.frame
            || self.acquisition_stamp_ns != scan.stamp_ns
        {
            return Err("Spatial transform identity contradicts acquisition header".into());
        }
        if let TfOutcome::Ready { components } = self.tf {
            let norm = components[3..].iter().map(|v| v * v).sum::<f64>();
            if components.iter().any(|v| !v.is_finite())
                || components[..3].iter().any(|v| v.abs() > 1_000_000.)
                || (norm - 1.).abs() > 1e-6
            {
                return Err("Invalid Spatial rigid transform".into());
            }
        }
        Ok(())
    }
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        // Reuse the bounded serializer rather than allocating an unchecked JSON Vec.
        let mut bytes = crate::session::encode_line(self, MAX_SAMPLE_BYTES)?;
        // Payload JSON is embedded in one outer event frame, not its own line.
        bytes.pop();
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        session::Event,
        structured::{Field, FloatValue, Value},
        subscriptions::{Generation, SubscriptionId},
    };
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
    fn f(value: f64) -> Value {
        Value::Float {
            value: FloatValue::Finite(value.to_string()),
        }
    }
    fn sample(rays: usize) -> Sample {
        Sample {
            contract_version: CONTRACT_VERSION,
            fixed_frame: "map".into(),
            source_frame: "laser".into(),
            acquisition_stamp_ns: 2_000_000_003,
            tf: TfOutcome::Ready {
                components: [1., 2., 3., 0., 0., 0., 1.],
            },
            counters: Counters::default(),
            scan: Box::new(structured::Sample {
                contract_version: structured::CONTRACT_VERSION,
                topic: "/scan".into(),
                message_type: SCAN_MESSAGE_TYPE.into(),
                source_time: None,
                value: object(vec![
                    (
                        "header",
                        object(vec![
                            (
                                "frame_id",
                                Value::Text {
                                    value: "laser".into(),
                                    original_bytes: 5,
                                },
                            ),
                            (
                                "stamp",
                                object(vec![
                                    ("sec", Value::Signed { value: "2".into() }),
                                    ("nanosec", Value::Unsigned { value: "3".into() }),
                                ]),
                            ),
                        ]),
                    ),
                    ("angle_min", f(0.)),
                    ("angle_max", f((rays - 1) as f64)),
                    ("angle_increment", f(1.)),
                    ("time_increment", f(0.)),
                    ("scan_time", f(0.1)),
                    ("range_min", f(0.1)),
                    ("range_max", f(10.)),
                    (
                        "ranges",
                        Value::Array {
                            original_items: rays,
                            items: vec![f(1.); rays],
                        },
                    ),
                    (
                        "intensities",
                        Value::Array {
                            original_items: 0,
                            items: vec![],
                        },
                    ),
                ]),
            }),
        }
    }
    #[test]
    fn spatial_wire_preserves_acquisition_identity_and_final_counters_under_reserved_budget() {
        let mut sample = sample(MAX_SCAN_RAYS);
        let bytes = sample.encode().unwrap();
        assert!(bytes.len() < MAX_SAMPLE_BYTES);
        for (sequence, tf) in [
            (1, TfOutcome::Missing),
            (
                2,
                TfOutcome::Ready {
                    components: [1., 2., 3., 0., 0., 0., 1.],
                },
            ),
        ] {
            sample.tf = tf;
            let event = Event::SpatialObservation {
                generation: Generation(2),
                subscription: SubscriptionId(3),
                sequence,
                sample: Box::new(sample.clone()),
                backend_drops: 9,
            };
            let decoded = Event::decode(&event.encode().unwrap()).unwrap();
            assert!(
                matches!(decoded,Event::SpatialObservation{sample,sequence:s,..} if s==sequence&&sample.acquisition_stamp_ns==2_000_000_003)
            );
        }
        let counters = Counters {
            scans_received: u64::MAX,
            tf_rejected: u64::MAX,
            ..Counters::default()
        };
        let event = Event::Drops {
            generation: Generation(2),
            subscription: SubscriptionId(3),
            backend_drops: u64::MAX,
            spatial_counters: Some(counters),
        };
        let wire = event.encode().unwrap();
        assert!(wire.len() < crate::subscriptions::MAX_RESERVED_BYTES);
        assert!(
            matches!(Event::decode(&wire).unwrap(),Event::Drops{spatial_counters:Some(c),..} if c==counters)
        );
    }
    #[test]
    fn incomplete_oversized_or_contradictory_spatial_data_never_becomes_a_ready_transform() {
        for kind in 0..7 {
            let mut sample = sample(2);
            match kind {
                0 => sample.contract_version = 2,
                1 => sample.source_frame = "other".into(),
                2 => sample.acquisition_stamp_ns = 0,
                3 => sample.fixed_frame = "/map".into(),
                4 => {
                    sample.tf = TfOutcome::Ready {
                        components: [0., 0., 0., 0., 0., 0., 2.],
                    }
                }
                5 => sample.scan.message_type = TF_MESSAGE_TYPE.into(),
                _ => {
                    if let Value::Object { fields, .. } = &mut sample.scan.value {
                        if let Value::Array { original_items, .. } = &mut fields
                            .iter_mut()
                            .find(|f| f.name == "ranges")
                            .unwrap()
                            .value
                        {
                            *original_items = 3;
                        }
                    }
                }
            }
            assert!(sample.encode().is_err(), "kind{kind}");
        }
        assert!(sample(MAX_SCAN_RAYS + 1).encode().is_err());
    }
}
