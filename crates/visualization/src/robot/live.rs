//! Model-bound presentation state. No ROS, filesystem work or automatic start.
use super::{LoadId, LoadedRobot, RobotError, RobotErrorKind, MAX_LINKS};
use crate::spatial::RigidTransform;
use studio_protocol::robot::{Configuration, LinkPose, LinkResult, Sample};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RobotToken {
    model: LoadId,
    activation: u64,
}

pub struct RobotLive {
    loaded: LoadedRobot,
    presented: std::sync::Arc<crate::spatial::robot_geometry::PreparedRobot>,
    appearance: super::RobotAppearance,
    model: LoadId,
    activation: u64,
    configuration: Option<Configuration>,
    sequence: u64,
    poses: Vec<Option<RigidTransform>>,
    links: Vec<LinkResult>,
}
fn invalid(message: &str) -> RobotError {
    RobotError::new(RobotErrorKind::Pose, message)
}
impl RobotLive {
    pub fn new(model: LoadId, loaded: LoadedRobot) -> Result<Self, RobotError> {
        let names = &loaded.prepared.link_names;
        if model.0 == 0
            || names.is_empty()
            || names.len() > MAX_LINKS
            || names.len() != loaded.model.links.len()
            || names
                .iter()
                .zip(&loaded.model.links)
                .any(|(name, link)| name != &link.name)
            || names
                .iter()
                .any(|name| !studio_protocol::spatial::valid_frame(name))
        {
            return Err(invalid(
                "Loaded Robot identities do not match prepared link geometry",
            ));
        }
        let poses = vec![None; names.len()];
        Ok(Self {
            presented: std::sync::Arc::clone(&loaded.prepared),
            appearance: super::RobotAppearance::default(),
            loaded,
            model,
            activation: 0,
            configuration: None,
            sequence: 0,
            poses,
            links: Vec::new(),
        })
    }
    /// Called only for an explicit new activation. The exact model frames are
    /// bound before accepting any result; equal link counts alone are unsafe.
    pub fn start(&mut self, configuration: Configuration) -> Result<RobotToken, RobotError> {
        configuration.validate().map_err(|e| invalid(&e))?;
        if configuration.frames != self.loaded.prepared.link_names {
            return Err(invalid(
                "Robot TF request does not match the loaded model link identities/order",
            ));
        }
        let activation = self
            .activation
            .checked_add(1)
            .ok_or_else(|| invalid("Robot activation identifiers exhausted"))?;
        self.stop();
        self.activation = activation;
        self.configuration = Some(configuration);
        Ok(RobotToken {
            model: self.model,
            activation,
        })
    }
    pub fn stop(&mut self) {
        self.configuration = None;
        self.poses.fill(None);
        self.links.clear();
        self.sequence = 0;
    }
    pub fn loaded(&self) -> &LoadedRobot {
        &self.loaded
    }
    pub fn prepared(&self) -> &crate::spatial::robot_geometry::PreparedRobot {
        &self.presented
    }
    pub fn appearance(&self) -> &super::RobotAppearance {
        &self.appearance
    }
    /// Build first; invalid or stale link names leave current presentation intact.
    pub fn set_appearance(&mut self, appearance: super::RobotAppearance) -> Result<(), RobotError> {
        let presented = super::present_robot(&self.loaded.prepared, &appearance)?;
        self.presented = std::sync::Arc::new(presented);
        self.appearance = appearance;
        Ok(())
    }
    pub fn model_id(&self) -> LoadId {
        self.model
    }
    pub fn poses(&self) -> &[Option<RigidTransform>] {
        &self.poses
    }
    pub fn links(&self) -> &[LinkResult] {
        &self.links
    }
    /// Reject stale ownership before touching current presentation. A malformed
    /// batch from this owner clears geometry; Missing clears only that link.
    pub fn apply(
        &mut self,
        token: RobotToken,
        sequence: u64,
        sample: &Sample,
    ) -> Result<(), RobotError> {
        if token.model != self.model
            || token.activation != self.activation
            || self.configuration.is_none()
            || sequence == 0
            || sequence <= self.sequence
        {
            return Err(invalid("Stale Robot model, activation or batch sequence"));
        }
        if let Err(error) = sample.validate(self.configuration.as_ref().unwrap()) {
            self.poses.fill(None);
            self.links.clear();
            return Err(invalid(&error));
        }
        let mut poses = Vec::with_capacity(sample.links.len());
        for link in &sample.links {
            poses.push(match link.pose {
                LinkPose::Ready { components: c, .. } => Some(
                    RigidTransform::new([c[0], c[1], c[2]], [c[3], c[4], c[5], c[6]])
                        .ok_or_else(|| invalid("Invalid Robot rigid transform"))?,
                ),
                _ => None,
            });
        }
        self.poses = poses;
        self.links.clone_from(&sample.links);
        self.sequence = sequence;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        robot::{load_model, LoadOptions},
        spatial::robot_geometry::prepare_robot,
    };
    use std::{
        path::Path,
        sync::{atomic::AtomicBool, Arc},
    };
    use studio_protocol::{
        robot::{Counters, PosePolicy},
        spatial_input::SpatialClockPolicy,
    };
    fn live(id: u64) -> RobotLive {
        let cancel = AtomicBool::new(false);
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/robot_visualizer/articulated.urdf");
        let options = LoadOptions {
            package_roots: [("fixture_robot".into(), path.parent().unwrap().to_owned())].into(),
        };
        let model = load_model(&path, &options, &cancel).unwrap();
        let prepared = prepare_robot(&model, &cancel).unwrap();
        RobotLive::new(
            LoadId(id),
            LoadedRobot {
                model: Arc::new(model),
                prepared: Arc::new(prepared),
            },
        )
        .unwrap()
    }
    fn config(live: &RobotLive) -> Configuration {
        Configuration {
            fixed_frame: "map".into(),
            frames: live.loaded.prepared.link_names.clone(),
            clock_policy: SpatialClockPolicy::VerifiedSystem,
            dynamic_tf_topic: "/tf".into(),
            dynamic_tf_best_effort: false,
            static_tf_topic: "/tf_static".into(),
        }
    }
    fn sample(config: &Configuration) -> Sample {
        Sample {
            contract_version: 1,
            fixed_frame: config.fixed_frame.clone(),
            policy: PosePolicy::LatestAvailable,
            links: config
                .frames
                .iter()
                .map(|frame| LinkResult {
                    frame: frame.clone(),
                    pose: LinkPose::Ready {
                        resolved_stamp_ns: 0,
                        components: [0., 0., 0., 0., 0., 0., 1.],
                    },
                })
                .collect(),
            counters: Counters::default(),
        }
    }
    #[test]
    fn model_and_activation_identity_reject_same_sized_old_results_without_clearing_peers() {
        let mut old = live(1);
        let config = config(&old);
        let old_token = old.start(config.clone()).unwrap();
        let sample = sample(&config);
        let mut new = live(2);
        let token = new.start(config.clone()).unwrap();
        new.apply(token, 1, &sample).unwrap();
        assert!(new.apply(old_token, 2, &sample).is_err());
        assert!(new.poses().iter().all(Option::is_some));
        new.stop();
        assert!(new.poses().iter().all(Option::is_none));
        let fresh = new.start(config).unwrap();
        assert!(new.apply(token, 2, &sample).is_err());
        assert!(new.poses().iter().all(Option::is_none));
        new.apply(fresh, 1, &sample).unwrap();
        assert!(new.apply(fresh, 1, &sample).is_err());
        assert!(new.poses().iter().all(Option::is_some));
    }
    #[test]
    fn exact_model_frames_partial_missing_recovery_and_invalid_clearing() {
        let mut live = live(3);
        let config = config(&live);
        let mut wrong = config.clone();
        wrong.frames.swap(0, 1);
        assert!(live.start(wrong).is_err());
        let token = live.start(config.clone()).unwrap();
        let mut batch = sample(&config);
        batch.links[1].pose = LinkPose::Missing;
        live.apply(token, 1, &batch).unwrap();
        assert!(live.poses()[0].is_some());
        assert!(live.poses()[1].is_none());
        batch = sample(&config);
        live.apply(token, 2, &batch).unwrap();
        assert!(live.poses().iter().all(Option::is_some));
        batch.links.swap(0, 1);
        assert!(live.apply(token, 3, &batch).is_err());
        assert!(live.poses().iter().all(Option::is_none));
        assert!(live.links().is_empty());
    }
    #[test]
    fn local_appearance_preserves_activation_poses_and_recovers_after_invalid_edit() {
        let mut live = live(4);
        let configuration = config(&live);
        let token = live.start(configuration.clone()).unwrap();
        let batch = sample(&configuration);
        live.apply(token, 1, &batch).unwrap();
        let before = live.poses().to_vec();
        let appearance = super::super::RobotAppearance {
            visual: false,
            collision: true,
            opacity_permille: 500,
            links: vec![],
        };
        live.set_appearance(appearance.clone()).unwrap();
        assert_eq!(live.poses(), before);
        assert_eq!(live.appearance(), &appearance);
        assert!(live.prepared().visuals.iter().all(|v| !v.visible));
        let invalid = super::super::RobotAppearance {
            links: vec![super::super::LinkAppearance {
                name: "stale_link".into(),
                visible: true,
                opacity_permille: 1000,
            }],
            ..Default::default()
        };
        assert!(live.set_appearance(invalid).is_err());
        assert_eq!(live.appearance(), &appearance);
        live.apply(token, 2, &batch).unwrap();
        live.set_appearance(Default::default()).unwrap();
        assert!(live.prepared().visuals.iter().all(|v| v.visible));
        assert_eq!(live.poses(), before);
    }
    #[test]
    fn desktop_and_wire_link_limits_agree() {
        assert_eq!(MAX_LINKS, studio_protocol::robot::MAX_LINKS);
    }
}
