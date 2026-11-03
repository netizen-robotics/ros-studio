//! Shared frontend configuration and status values. This module performs no I/O
//! and never starts a subscription when a topic or draft changes.

#[derive(Default, Debug, PartialEq, Eq)]
pub enum Phase {
    #[default]
    Disconnected,
    Starting,
    Connected,
    Failed,
}

/// ROS2's default when ROS_DOMAIN_ID is unset. Fixtures override it explicitly.
pub const DEFAULT_DOMAIN_ID: u16 = 0;

pub struct Connection {
    pub phase: Phase,
    pub status: String,
    pub domain: String,
    pub cyclone: bool,
    pub actual: Option<(String, String)>,
}

impl Default for Connection {
    fn default() -> Self {
        Self {
            phase: Phase::Disconnected,
            status: "Disconnected".into(),
            domain: DEFAULT_DOMAIN_ID.to_string(),
            cyclone: false,
            actual: None,
        }
    }
}

/// Validate without modifying the active connection. Blank uses the ROS default.
pub fn domain_id(domain: &str) -> Result<u16, String> {
    if domain.is_empty() {
        return Ok(DEFAULT_DOMAIN_ID);
    }
    domain
        .parse::<u16>()
        .ok()
        .filter(|n| *n <= 232)
        .ok_or_else(|| "Domain must be an integer from 0 to 232".into())
}

impl Connection {
    pub fn domain_id(&self) -> Result<u16, String> {
        domain_id(&self.domain)
    }
}

pub fn topic_type_available(
    topics: &[(String, Vec<String>)],
    topic: &str,
    message_type: &str,
) -> bool {
    topics
        .iter()
        .any(|(name, types)| name == topic && types.iter().any(|ty| ty == message_type))
}

#[derive(Default)]
pub struct Discovery {
    pub topics: Vec<(String, Vec<String>)>,
    pub query: String,
    pub topic: String,
    pub ty: String,
    pub received_graph: bool,
}

impl Discovery {
    /// A single discovered type can fill the draft. Ambiguity requires a choice.
    pub fn select(&mut self, topic: &str) {
        self.topic = topic.into();
        self.ty = self
            .topics
            .iter()
            .find(|(name, _)| name == topic)
            .filter(|(_, types)| types.len() == 1)
            .map(|(_, types)| types[0].clone())
            .unwrap_or_default();
    }

    pub fn valid_selection(&self) -> bool {
        topic_type_available(&self.topics, &self.topic, &self.ty)
    }

    pub fn matches(&self, name: &str, types: &[String]) -> bool {
        let query = self.query.to_lowercase();
        name.to_lowercase().contains(&query)
            || types.iter().any(|ty| ty.to_lowercase().contains(&query))
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ViewStatus {
    #[default]
    Stopped,
    Starting,
    Waiting,
    Receiving,
    Stopping,
    NeedsRecovery,
}

impl ViewStatus {
    pub fn label(self) -> &'static str {
        match self {
            Self::Stopped => "Stopped",
            Self::Starting => "Starting subscription…",
            Self::Waiting => "Subscribed; waiting for samples…",
            Self::Receiving => "Receiving",
            Self::Stopping => "Stopping; waiting for cleanup…",
            Self::NeedsRecovery => "Cleanup unconfirmed; retry stop or disconnect",
        }
    }

    /// Acknowledged subscription states, not enabled intent or reader ownership.
    /// Starting/Stopping may still reserve resources in the session registry.
    pub fn active(self) -> bool {
        matches!(self, Self::Waiting | Self::Receiving)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn domain_limits_reject_invalid_drafts_without_touching_connection() {
        let connection = Connection::default();
        for domain in ["0", "187", "232"] {
            assert_eq!(domain_id(domain).unwrap().to_string(), domain);
        }
        for domain in ["233", "65536", "-1", " 187", "187 ", "1.5", "abc"] {
            assert_eq!(
                domain_id(domain).unwrap_err(),
                "Domain must be an integer from 0 to 232"
            );
        }
        assert_eq!(connection.domain_id(), Ok(0));
        assert_eq!(domain_id(""), Ok(0));
        assert_eq!(connection.phase, Phase::Disconnected);
        assert!(connection.actual.is_none());
    }

    #[test]
    fn topic_selection_clears_old_type_for_ambiguous_empty_or_missing_sources() {
        let mut discovery = Discovery {
            topics: vec![
                ("/single".into(), vec!["pkg/msg/One".into()]),
                (
                    "/multi".into(),
                    vec!["pkg/msg/One".into(), "pkg/msg/Two".into()],
                ),
                ("/empty".into(), vec![]),
            ],
            ..Default::default()
        };
        discovery.select("/single");
        assert!(discovery.valid_selection());
        discovery.select("/multi");
        assert!(discovery.ty.is_empty());
        assert!(!discovery.valid_selection());
        discovery.ty = "pkg/msg/Two".into();
        assert!(discovery.valid_selection());
        for topic in ["/empty", "/missing"] {
            discovery.select(topic);
            assert!(discovery.ty.is_empty());
            assert!(!discovery.valid_selection());
        }
    }

    #[test]
    fn graph_refresh_invalidates_removed_type_without_replacing_the_draft() {
        let mut discovery = Discovery {
            topics: vec![("/source".into(), vec!["pkg/msg/One".into()])],
            ..Default::default()
        };
        discovery.select("/source");
        discovery.topics[0].1 = vec!["pkg/msg/Two".into()];
        assert!(!discovery.valid_selection());
        assert_eq!(discovery.ty, "pkg/msg/One");
        assert!(!topic_type_available(
            &discovery.topics,
            "/other",
            "pkg/msg/Two"
        ));
        discovery.topics.clear();
        assert!(!discovery.valid_selection());
        assert_eq!(discovery.topic, "/source");
    }

    #[test]
    fn searching_matches_names_and_types_without_replacing_selection() {
        let discovery = Discovery {
            query: "IMAGE".into(),
            topic: "/selected".into(),
            ty: "pkg/msg/Selected".into(),
            ..Default::default()
        };
        assert!(discovery.matches("/image_raw", &[]));
        assert!(discovery.matches("/camera", &["sensor_msgs/msg/Image".into()]));
        assert!(!discovery.matches("/scan", &["sensor_msgs/msg/LaserScan".into()]));
        assert_eq!(discovery.topic, "/selected");
        assert_eq!(discovery.ty, "pkg/msg/Selected");
    }
}
