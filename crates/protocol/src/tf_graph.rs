//! Bounded admitted TF topology and independently resolved latest poses.
//! A graph owner shares the scene's two native readers; no per-frame endpoints.
pub use crate::latest_pose::LatestPose;
use crate::{latest_pose::MAX_STAMP_NS, spatial};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet};
pub const CONTRACT_VERSION: u32 = 1;
pub const MAX_FRAMES: usize = spatial::MAX_FRAMES;
pub const MAX_EDGES: usize = spatial::MAX_EDGES;
pub const MAX_HZ: u32 = 5;
pub const MAX_SAMPLE_BYTES: usize = 512 * 1024;
pub const MAX_OWNERS: usize = 1;
pub const STALE_AFTER_NS: i64 = 2_000_000_000;

/// One boxed nested capability record keeps the common Event envelope small.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub contract_version: u32,
    pub max_hz: u32,
    pub max_sample_bytes: usize,
    pub max_owners: usize,
}
impl Default for Capabilities {
    fn default() -> Self {
        Self {
            contract_version: CONTRACT_VERSION,
            max_hz: MAX_HZ,
            max_sample_bytes: MAX_SAMPLE_BYTES,
            max_owners: MAX_OWNERS,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EdgeKind {
    Static,
    Dynamic,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Parent {
    pub frame: String,
    pub kind: EdgeKind,
    /// Last accepted source stamp; None only for cleared dynamic history.
    pub stamp_ns: Option<i64>,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Frame {
    pub name: String,
    pub parent: Option<Parent>,
    pub pose: LatestPose,
}
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    pub contract_version: u32,
    pub fixed_frame: String,
    /// Sorted complete admitted names, including unavailable/disconnected frames.
    pub frames: Vec<Frame>,
    pub counters: spatial::Counters,
}
impl Sample {
    pub fn validate(&self) -> Result<(), String> {
        if self.contract_version != CONTRACT_VERSION
            || !spatial::valid_frame(&self.fixed_frame)
            || self.frames.len() > MAX_FRAMES
        {
            return Err("Invalid TF graph contract, fixed frame or frame bound".into());
        }
        let mut previous: Option<&str> = None;
        let names: HashSet<_> = self.frames.iter().map(|f| f.name.as_str()).collect();
        let mut parents = BTreeMap::new();
        for frame in &self.frames {
            if !spatial::valid_frame(&frame.name)
                || previous.is_some_and(|p| p >= frame.name.as_str())
            {
                return Err("TF graph names must be valid, unique and sorted".into());
            }
            previous = Some(&frame.name);
            frame.pose.validate()?;
            if let Some(parent) = &frame.parent {
                if !names.contains(parent.frame.as_str())
                    || parent.frame == frame.name
                    || parent
                        .stamp_ns
                        .is_some_and(|s| !(0..=MAX_STAMP_NS).contains(&s))
                    || (parent.kind == EdgeKind::Static && parent.stamp_ns.is_none())
                {
                    return Err("Invalid TF graph parent, edge kind or timestamp".into());
                }
                parents.insert(frame.name.as_str(), parent.frame.as_str());
            }
        }
        if parents.len() > MAX_EDGES {
            return Err("TF graph edge bound exceeded".into());
        }
        for name in parents.keys() {
            let mut at = *name;
            let mut visited = HashSet::new();
            while let Some(parent) = parents.get(at) {
                if !visited.insert(at) {
                    return Err("Cyclic admitted TF graph".into());
                }
                at = parent;
            }
        }
        Ok(())
    }
    pub fn encode(&self) -> Result<Vec<u8>, String> {
        self.validate()?;
        let mut bytes = crate::session::encode_line(self, MAX_SAMPLE_BYTES)?;
        bytes.pop();
        Ok(bytes)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_SAMPLE_BYTES {
            return Err("TF graph sample exceeds byte budget".into());
        }
        let sample: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        sample.validate()?;
        Ok(sample)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn sample() -> Sample {
        Sample {
            contract_version: CONTRACT_VERSION,
            fixed_frame: "map".into(),
            frames: vec![
                Frame {
                    name: "base".into(),
                    parent: Some(Parent {
                        frame: "map".into(),
                        kind: EdgeKind::Dynamic,
                        stamp_ns: Some(1),
                    }),
                    pose: LatestPose::Missing,
                },
                Frame {
                    name: "map".into(),
                    parent: None,
                    pose: LatestPose::Ready {
                        resolved_stamp_ns: 0,
                        components: [0., 0., 0., 0., 0., 0., 1.],
                    },
                },
            ],
            counters: spatial::Counters::default(),
        }
    }
    #[test]
    fn empty_and_complete_graph_roundtrip_preserves_unavailable_frames() {
        let mut graph = sample();
        assert_eq!(Sample::decode(&graph.encode().unwrap()).unwrap(), graph);
        graph.frames.clear();
        assert!(graph.encode().is_ok());
    }
    #[test]
    fn rejects_hostile_topology_poses_and_wire_fields() {
        let original = sample();
        for mode in 0..8 {
            let mut graph = original.clone();
            match mode {
                0 => graph.frames.reverse(),
                1 => graph.frames[1].name = "base".into(),
                2 => graph.frames[0].parent.as_mut().unwrap().frame = "absent".into(),
                3 => {
                    graph.frames[1].parent = Some(Parent {
                        frame: "base".into(),
                        kind: EdgeKind::Static,
                        stamp_ns: Some(0),
                    })
                }
                4 => graph.frames[0].parent.as_mut().unwrap().stamp_ns = Some(-1),
                5 => {
                    graph.frames[1].pose = LatestPose::Ready {
                        resolved_stamp_ns: 0,
                        components: [f64::NAN, 0., 0., 0., 0., 0., 1.],
                    }
                }
                6 => {
                    graph.frames[0].parent = Some(Parent {
                        frame: "map".into(),
                        kind: EdgeKind::Static,
                        stamp_ns: None,
                    })
                }
                _ => graph.contract_version += 1,
            }
            assert!(graph.encode().is_err(), "mode {mode}");
        }
        let mut wire = serde_json::to_value(original).unwrap();
        wire["runtime_authorized"] = true.into();
        assert!(Sample::decode(&serde_json::to_vec(&wire).unwrap()).is_err());
        assert!(Sample::decode(&vec![b' '; MAX_SAMPLE_BYTES + 1]).is_err());
    }
    #[test]
    fn maximum_disconnected_graph_and_edge_limits() {
        let mut graph = sample();
        graph.frames = (0..MAX_FRAMES)
            .map(|n| Frame {
                name: format!("frame_{n:03}"),
                parent: None,
                pose: LatestPose::Missing,
            })
            .collect();
        for n in 0..MAX_EDGES {
            graph.frames[n].parent = Some(Parent {
                frame: format!("frame_{:03}", n + MAX_EDGES),
                kind: EdgeKind::Dynamic,
                stamp_ns: Some(1),
            });
        }
        assert!(graph.encode().unwrap().len() < MAX_SAMPLE_BYTES);
        graph.frames[MAX_EDGES].parent = Some(Parent {
            frame: graph.frames[MAX_EDGES + 1].name.clone(),
            kind: EdgeKind::Dynamic,
            stamp_ns: Some(1),
        });
        assert!(graph.validate().is_err());
        graph.frames.push(Frame {
            name: "overflow".into(),
            parent: None,
            pose: LatestPose::Missing,
        });
        assert!(graph.validate().is_err());
    }
}
