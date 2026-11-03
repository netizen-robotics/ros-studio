//! Robot contract1 values carried by the paired session7 adapters.
//! Robot poses use explicit latest-available native TF, never scan acquisition TF.
use crate::{spatial::valid_frame, spatial_input::SpatialClockPolicy, structured};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

pub const CONTRACT_VERSION: u32 = 1;
pub const MAX_LINKS: usize = 128;
pub const RESOURCE_COST: usize = 2;
pub const MAX_HZ: u32 = 10;
pub const MAX_SAMPLE_BYTES: usize = structured::MAX_SAMPLE_BYTES;
/// Paired session7 command budget for maximum escaped link frame names.
pub const REQUIRED_COMMAND_BYTES: usize = 128 * 1024;
pub use crate::latest_pose::LatestPose as LinkPose;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Configuration {
    pub fixed_frame: String,
    pub frames: Vec<String>,
    pub clock_policy: SpatialClockPolicy,
    pub dynamic_tf_topic: String,
    pub dynamic_tf_best_effort: bool,
    pub static_tf_topic: String,
}
impl Configuration {
    pub fn validate(&self) -> Result<(), String> {
        fn topic(value: &str) -> bool {
            value.len() <= 256
                && value.starts_with('/')
                && value[1..].split('/').all(|part| {
                    part.bytes()
                        .next()
                        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
                        && part.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
                })
        }
        if !valid_frame(&self.fixed_frame)
            || self.clock_policy != SpatialClockPolicy::VerifiedSystem
            || self.frames.is_empty()
            || self.frames.len() > MAX_LINKS
            || self.frames.iter().any(|name| !valid_frame(name))
            || self.frames.iter().collect::<HashSet<_>>().len() != self.frames.len()
            || !topic(&self.dynamic_tf_topic)
            || !topic(&self.static_tf_topic)
            || self.dynamic_tf_topic == self.static_tf_topic
        {
            return Err("Invalid Robot frames, TF inputs or unverified System clock".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PosePolicy {
    LatestAvailable,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkResult {
    pub frame: String,
    pub pose: LinkPose,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Counters {
    pub tf_dynamic_batches: u64,
    pub tf_static_batches: u64,
    pub tf_accepted: u64,
    pub tf_rejected: u64,
    pub duplicate_statics: u64,
    pub native_errors: u64,
    pub pose_batches: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    pub contract_version: u32,
    pub fixed_frame: String,
    pub policy: PosePolicy,
    /// Complete requested-frame order, including unavailable links. Not a
    /// synchronized snapshot: each Ready result retains its own native stamp.
    pub links: Vec<LinkResult>,
    pub counters: Counters,
}
impl Sample {
    /// Validate the bounded wire shape before a subscription is available.
    /// Delivery must additionally validate the exact active configuration.
    pub fn validate_structure(&self) -> Result<(), String> {
        if self.links.is_empty() || self.links.len() > MAX_LINKS {
            return Err("Robot batch exceeds the link count bound".into());
        }
        let c = Configuration {
            fixed_frame: self.fixed_frame.clone(),
            frames: self.links.iter().map(|l| l.frame.clone()).collect(),
            clock_policy: SpatialClockPolicy::VerifiedSystem,
            dynamic_tf_topic: "/tf".into(),
            dynamic_tf_best_effort: false,
            static_tf_topic: "/tf_static".into(),
        };
        self.validate(&c)
    }
    pub fn encode_wire(&self) -> Result<Vec<u8>, String> {
        self.validate_structure()?;
        let mut bytes = crate::session::encode_line(self, MAX_SAMPLE_BYTES)?;
        bytes.pop();
        Ok(bytes)
    }
    pub fn validate(&self, configuration: &Configuration) -> Result<(), String> {
        configuration.validate()?;
        if self.contract_version != CONTRACT_VERSION
            || self.fixed_frame != configuration.fixed_frame
            || self.links.len() != configuration.frames.len()
        {
            return Err("Robot batch does not match the requested configuration".into());
        }
        for (result, frame) in self.links.iter().zip(&configuration.frames) {
            if &result.frame != frame {
                return Err("Robot link identities/order differ from the request".into());
            }
            result.pose.validate()?;
        }
        Ok(())
    }
    pub fn encode(&self, configuration: &Configuration) -> Result<Vec<u8>, String> {
        self.validate(configuration)?;
        let mut bytes = crate::session::encode_line(self, MAX_SAMPLE_BYTES)?;
        bytes.pop();
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8], configuration: &Configuration) -> Result<Self, String> {
        if bytes.len() > MAX_SAMPLE_BYTES {
            return Err("Robot batch exceeds the sample byte budget".into());
        }
        let result: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        result.validate(configuration)?;
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn configuration() -> Configuration {
        Configuration {
            fixed_frame: "map".into(),
            frames: vec!["base".into(), "arm".into()],
            clock_policy: SpatialClockPolicy::VerifiedSystem,
            dynamic_tf_topic: "/tf".into(),
            dynamic_tf_best_effort: false,
            static_tf_topic: "/tf_static".into(),
        }
    }
    fn sample() -> Sample {
        Sample {
            contract_version: CONTRACT_VERSION,
            fixed_frame: "map".into(),
            policy: PosePolicy::LatestAvailable,
            links: vec![
                LinkResult {
                    frame: "base".into(),
                    pose: LinkPose::Ready {
                        resolved_stamp_ns: 0,
                        components: [0., 0., 0., 0., 0., 0., 1.],
                    },
                },
                LinkResult {
                    frame: "arm".into(),
                    pose: LinkPose::Missing,
                },
            ],
            counters: Counters::default(),
        }
    }
    #[test]
    fn static_identity_zero_and_partial_missing_roundtrip_without_receipt_time() {
        let c = configuration();
        let s = sample();
        assert_eq!(Sample::decode(&s.encode(&c).unwrap(), &c).unwrap(), s);
        let mut later = s.clone();
        later.links[1].pose = LinkPose::Ready {
            resolved_stamp_ns: 42,
            components: [1., 2., 3., 0., 0., 0., 1.],
        };
        assert!(
            later.validate(&c).is_ok(),
            "distinct link result times are explicit"
        );
    }
    #[test]
    fn contradicting_link_batches_and_bad_poses_are_rejected() {
        let c = configuration();
        let s = sample();
        let mut bad = s.clone();
        bad.links.swap(0, 1);
        assert!(bad.validate(&c).is_err());
        bad = s.clone();
        bad.links.pop();
        assert!(bad.validate(&c).is_err());
        for (stamp, components) in [
            (-1, [0., 0., 0., 0., 0., 0., 1.]),
            (0, [0., 0., 0., 0., 0., 0., 2.]),
            (0, [f64::NAN, 0., 0., 0., 0., 0., 1.]),
            (0, [1_000_001., 0., 0., 0., 0., 0., 1.]),
        ] {
            bad = s.clone();
            bad.links[0].pose = LinkPose::Ready {
                resolved_stamp_ns: stamp,
                components,
            };
            assert!(bad.validate(&c).is_err());
        }
        bad = s;
        bad.contract_version += 1;
        assert!(bad.validate(&c).is_err());
    }
    #[test]
    fn clock_capacity_duplicate_and_tf_sources_are_validated() {
        let c = configuration();
        for policy in [SpatialClockPolicy::Unknown, SpatialClockPolicy::Simulated] {
            let mut bad = c.clone();
            bad.clock_policy = policy;
            assert!(bad.validate().is_err());
        }
        let mut bad = c.clone();
        bad.frames = vec!["base".into(); 2];
        assert!(bad.validate().is_err());
        bad.frames = (0..=MAX_LINKS).map(|i| format!("link{i}")).collect();
        assert!(bad.validate().is_err());
        bad = c;
        bad.dynamic_tf_topic = bad.static_tf_topic.clone();
        assert!(bad.validate().is_err());
    }
    #[test]
    fn maximum_names_require_the_reviewed_future_command_budget() {
        let mut c = configuration();
        c.frames = (0..MAX_LINKS)
            .map(|i| format!("{i:03}{}", "\\".repeat(253)))
            .collect();
        c.validate().unwrap();
        let encoded = serde_json::to_vec(&c).unwrap();
        assert!(encoded.len() > 4096);
        assert!(encoded.len() + 1024 < REQUIRED_COMMAND_BYTES);
        assert!(
            encoded.len() + 1024 < crate::session::MAX_COMMAND_BYTES,
            "Robot values must fit the live session command budget"
        );
        assert!(Sample::decode(&vec![b' '; MAX_SAMPLE_BYTES + 1], &configuration()).is_err());
    }
}
