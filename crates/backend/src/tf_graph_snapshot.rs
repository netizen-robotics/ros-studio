//! Read-only graph presentation of the existing admitted native tf2 buffer.
//! No reader, second transform engine, or runtime activation lives here.
use super::{Failure, SpatialSession, Status, StreamKind, MAX_STAMP_NS};
use std::sync::atomic::{AtomicBool, Ordering};
use studio_protocol::tf_graph::{self, EdgeKind, Frame, LatestPose, Parent, Sample};

impl SpatialSession {
    /// Complete bounded admitted graph, latest native pose per frame. `now_ns`
    /// must be the verified System clock, never a receipt-time substitute for
    /// an acquisition lookup. Return None on cancellation; never partial data.
    pub fn graph_snapshot(
        &mut self,
        fixed_frame: &str,
        now_ns: i64,
        cancel: &AtomicBool,
    ) -> Result<Option<Sample>, Failure> {
        self.require_active()?;
        if !studio_protocol::spatial::valid_frame(fixed_frame)
            || !(1..=MAX_STAMP_NS).contains(&now_ns)
        {
            return Err(Failure::new(
                Status::Invalid,
                "TF graph requires a valid fixed frame and verified positive System time",
            ));
        }
        let mut names: Vec<_> = self.admission.frames.iter().cloned().collect();
        names.sort();
        let mut frames = Vec::with_capacity(names.len());
        for name in names {
            if cancel.load(Ordering::Acquire) {
                return Ok(None);
            }
            let parent = self.admission.edges.get(&name).map(|e| Parent {
                frame: e.parent.clone(),
                kind: match e.kind {
                    StreamKind::Static => EdgeKind::Static,
                    StreamKind::Dynamic => EdgeKind::Dynamic,
                },
                stamp_ns: e.last_stamp,
            });
            let pose = match self.lookup_latest(fixed_frame, &name) {
                Ok(value) if value.stamp_ns > now_ns => LatestPose::Extrapolated,
                Ok(value)
                    if value.stamp_ns > 0 && now_ns - value.stamp_ns > tf_graph::STALE_AFTER_NS =>
                {
                    LatestPose::Stale
                }
                Ok(value) => LatestPose::Ready {
                    resolved_stamp_ns: value.stamp_ns,
                    components: value.components,
                },
                Err(error) => match error.status {
                    Status::Missing => LatestPose::Missing,
                    Status::Stale => LatestPose::Stale,
                    Status::Extrapolated => LatestPose::Extrapolated,
                    // Invalid/native/cyclic failures must stop the graph owner
                    // explicitly, rather than mislabel an unknown cause.
                    _ => return Err(error),
                },
            };
            frames.push(Frame { name, parent, pose });
        }
        if cancel.load(Ordering::Acquire) {
            return Ok(None);
        }
        let c = self.counters();
        let sample = Sample {
            contract_version: tf_graph::CONTRACT_VERSION,
            fixed_frame: fixed_frame.into(),
            frames,
            counters: studio_protocol::spatial::Counters {
                tf_accepted: c.accepted,
                tf_rejected: c.rejected,
                duplicate_statics: c.duplicate_statics,
                native_errors: c.native_errors,
                ..Default::default()
            },
        };
        sample
            .validate()
            .map_err(|e| Failure::new(Status::Invalid, e))?;
        Ok(Some(sample))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::spatial_session::{ClockPolicy, Transform};
    fn tf(parent: &str, child: &str, stamp_ns: i64, x: f64) -> Transform {
        Transform {
            parent: parent.into(),
            child: child.into(),
            stamp_ns,
            components: [x, 0., 0., 0., 0., 0., 1.],
        }
    }
    fn session() -> SpatialSession {
        let mut session = SpatialSession::new().unwrap();
        session
            .activate_explicit(ClockPolicy::VerifiedSystem)
            .unwrap();
        session
    }
    fn snap(session: &mut SpatialSession, fixed: &str, now: i64) -> Sample {
        session
            .graph_snapshot(fixed, now, &AtomicBool::new(false))
            .unwrap()
            .unwrap()
    }
    #[test]
    fn empty_static_dynamic_disconnected_and_missing_fixed_are_explicit() {
        let mut session = session();
        assert!(snap(&mut session, "map", 10_000_000_000).frames.is_empty());
        session
            .insert_batch(
                StreamKind::Static,
                &[tf("map", "sensor", 0, 1.), tf("other", "remote", 0, 2.)],
            )
            .unwrap();
        session
            .insert_batch(StreamKind::Dynamic, &[tf("map", "base", 9_000_000_000, 3.)])
            .unwrap();
        let graph = snap(&mut session, "map", 10_000_000_000);
        assert_eq!(
            graph
                .frames
                .iter()
                .map(|f| f.name.as_str())
                .collect::<Vec<_>>(),
            ["base", "map", "other", "remote", "sensor"]
        );
        assert!(
            matches!(graph.frames[0].pose,LatestPose::Ready{resolved_stamp_ns:9_000_000_000,components} if components[0]==3.)
        );
        assert_eq!(
            graph.frames[0].parent.as_ref().unwrap().kind,
            EdgeKind::Dynamic
        );
        assert!(matches!(graph.frames[2].pose, LatestPose::Missing));
        assert!(matches!(graph.frames[3].pose, LatestPose::Missing));
        assert!(matches!(
            graph.frames[4].pose,
            LatestPose::Ready {
                resolved_stamp_ns: 0,
                ..
            }
        ));
        assert!(snap(&mut session, "absent", 10_000_000_000)
            .frames
            .iter()
            .all(|f| f.pose == LatestPose::Missing));
    }
    #[test]
    fn stale_future_reset_and_cancellation_do_not_reuse_live_poses() {
        let mut session = session();
        session
            .insert_batch(StreamKind::Static, &[tf("map", "sensor", 0, 1.)])
            .unwrap();
        session
            .insert_batch(
                StreamKind::Dynamic,
                &[tf("sensor", "base", 9_000_000_000, 3.)],
            )
            .unwrap();
        let exact = snap(&mut session, "map", 11_000_000_000);
        assert!(matches!(exact.frames[0].pose, LatestPose::Ready { .. }));
        assert_eq!(
            snap(&mut session, "map", 11_000_000_001).frames[0].pose,
            LatestPose::Stale
        );
        assert_eq!(
            snap(&mut session, "map", 8_000_000_000).frames[0].pose,
            LatestPose::Extrapolated
        );
        assert!(session
            .graph_snapshot("map", 10_000_000_000, &AtomicBool::new(true))
            .unwrap()
            .is_none());
        assert!(session
            .graph_snapshot("map", 0, &AtomicBool::new(false))
            .is_err());
        session.reset().unwrap();
        assert!(session
            .graph_snapshot("map", 10_000_000_000, &AtomicBool::new(false))
            .is_err());
        assert!(session.activate_explicit(ClockPolicy::Simulated).is_err());
        session
            .activate_explicit(ClockPolicy::VerifiedSystem)
            .unwrap();
        let reset = snap(&mut session, "map", 10_000_000_000);
        assert_eq!(reset.frames[0].parent.as_ref().unwrap().stamp_ns, None);
        assert_eq!(reset.frames[0].pose, LatestPose::Missing);
        assert!(matches!(
            reset.frames[2].pose,
            LatestPose::Ready {
                resolved_stamp_ns: 0,
                ..
            }
        ));
    }
    #[test]
    fn rejected_cycle_never_enters_the_admitted_snapshot() {
        let mut session = session();
        session
            .insert_batch(StreamKind::Static, &[tf("map", "base", 0, 1.)])
            .unwrap();
        let failure = session
            .insert_batch(StreamKind::Static, &[tf("base", "map", 0, 1.)])
            .unwrap_err();
        assert_eq!(failure.status, Status::Cyclic);
        let graph = snap(&mut session, "map", 10_000_000_000);
        assert_eq!(graph.frames.len(), 2);
        assert_eq!(graph.frames[1].parent, None);
        assert_eq!(graph.counters.tf_rejected, 1);
        assert!(graph.encode().is_ok());
    }
    #[test]
    fn graph_never_expands_frame_admission_for_inspection() {
        let mut session = session();
        for n in 0..crate::spatial_session::MAX_EDGES {
            session
                .insert_batch(
                    StreamKind::Static,
                    &[tf(&format!("p{n:03}"), &format!("c{n:03}"), 0, 1.)],
                )
                .unwrap();
        }
        assert_eq!(session.frame_count(), tf_graph::MAX_FRAMES);
        let sample = snap(&mut session, "unseen", 10_000_000_000);
        assert_eq!(session.frame_count(), tf_graph::MAX_FRAMES);
        assert_eq!(sample.frames.len(), tf_graph::MAX_FRAMES);
        assert_eq!(
            sample.frames.iter().filter(|f| f.parent.is_some()).count(),
            tf_graph::MAX_EDGES
        );
        assert!(sample.encode().is_ok());
    }
}
