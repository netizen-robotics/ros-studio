//! Local asset locations only; validation never opens files or authorizes TF.
use super::{invalid, token_valid, topic_valid, Durability, Error, Qos, Reliability};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RobotAssets {
    pub urdf_path: String,
    pub package_roots: Vec<PackageRoot>,
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageRoot {
    pub package: String,
    pub path: String,
}

/// Local presentation only; these values never authorize a source or TF reader.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RobotAppearance {
    pub visual: bool,
    pub collision: bool,
    pub opacity_permille: u16,
    pub links: Vec<LinkAppearance>,
}
impl Default for RobotAppearance {
    fn default() -> Self {
        Self {
            visual: true,
            collision: false,
            opacity_permille: 1000,
            links: Vec::new(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LinkAppearance {
    pub name: String,
    pub visible: bool,
    pub opacity_permille: u16,
}
impl RobotAppearance {
    pub fn validate(&self) -> Result<(), Error> {
        let mut names = HashSet::new();
        if self.opacity_permille > 1000
            || self.links.len() > 128
            || self.links.iter().any(|link| {
                link.opacity_permille > 1000
                    || link.name.len() > 256
                    || !studio_protocol::spatial::valid_frame(&link.name)
                    || !names.insert(&link.name)
            })
        {
            return Err(invalid(
                "Robot appearance requires opacity0..1000 and at most128 unique bounded link names",
            ));
        }
        Ok(())
    }
}
/// Configuration only. Captured XML, digest and live identities are never saved.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RobotTopicSource {
    pub topic: String,
    pub message_type: String,
    pub qos: Qos,
    pub asset_root: Option<String>,
    pub package_roots: Vec<PackageRoot>,
}
impl Default for RobotTopicSource {
    fn default() -> Self {
        Self {
            topic: String::new(),
            message_type: studio_protocol::robot_description::MESSAGE_TYPE.into(),
            qos: Qos {
                reliability: Reliability::Reliable,
                durability: Durability::TransientLocal,
            },
            asset_root: None,
            package_roots: Vec::new(),
        }
    }
}
fn path_valid(value: &str) -> bool {
    !value.is_empty() && value.len() <= 4096 && !value.chars().any(char::is_control)
}
fn roots_valid(roots: &[PackageRoot]) -> bool {
    let mut packages = HashSet::new();
    roots.len() <= 16
        && roots.iter().all(|root| {
            token_valid(&root.package)
                && root.package.len() <= 256
                && packages.insert(&root.package)
                && path_valid(&root.path)
        })
}
impl RobotTopicSource {
    pub fn validate(&self) -> Result<(), Error> {
        if !topic_valid(&self.topic)
            || self.message_type != studio_protocol::robot_description::MESSAGE_TYPE
            || self.asset_root.as_deref().is_some_and(|p| !path_valid(p))
            || !roots_valid(&self.package_roots)
        {
            return Err(invalid("Robot topic requires exact std_msgs/msg/String, a valid topic and bounded explicit local asset roots"));
        }
        Ok(())
    }
}
impl RobotAssets {
    pub fn validate(&self) -> Result<(), Error> {
        if !path_valid(&self.urdf_path) || !roots_valid(&self.package_roots) {
            return Err(invalid("Robot requires a local URDF path and at most 16 unique explicit package roots, with paths of 1–4096 bytes"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod appearance_tests {
    use super::*;
    #[test]
    fn appearance_bounds_duplicate_names_and_runtime_fields_reject() {
        let mut settings = RobotAppearance {
            links: (0..128)
                .map(|i| LinkAppearance {
                    name: format!("link_{i}"),
                    visible: true,
                    opacity_permille: 0,
                })
                .collect(),
            ..Default::default()
        };
        settings.validate().unwrap();
        settings.links.push(settings.links[0].clone());
        assert!(settings.validate().is_err());
        settings.links.pop();
        settings.links[1].name = settings.links[0].name.clone();
        assert!(settings.validate().is_err());
        for name in ["", "/leading", "bad name", "bad\nname"] {
            settings.links = vec![LinkAppearance {
                name: name.into(),
                visible: true,
                opacity_permille: 1000,
            }];
            assert!(settings.validate().is_err());
        }
        settings = RobotAppearance::default();
        settings.opacity_permille = 1001;
        assert!(settings.validate().is_err());
        let mut value = serde_json::to_value(RobotAppearance::default()).unwrap();
        value["enabled"] = true.into();
        assert!(serde_json::from_value::<RobotAppearance>(value).is_err());
    }
}
