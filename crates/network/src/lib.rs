//! Host-local networking policy. No ROS, GUI or background activation dependencies.
pub mod adapters;
pub mod preferences;
pub mod profiles;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Middleware {
    Fast,
    Cyclone,
}
impl Middleware {
    pub fn from_cyclone(cyclone: bool) -> Self {
        if cyclone {
            Self::Cyclone
        } else {
            Self::Fast
        }
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "mode", deny_unknown_fields)]
pub enum Selection {
    #[default]
    Lan,
    Adapter {
        id: String,
    },
    Local,
    Custom {
        profile: Option<String>,
    },
}
#[derive(Clone, Debug)]
pub struct ConnectionConfig {
    pub domain: u16,
    pub middleware: Middleware,
    pub selection: Selection,
    pub managed_environment_approved: bool,
}
impl ConnectionConfig {
    /// Retained desktop/development compatibility: inherit existing DDS settings.
    pub fn inherited(domain: u16, cyclone: bool) -> Self {
        Self {
            domain,
            middleware: Middleware::from_cyclone(cyclone),
            selection: Selection::Custom { profile: None },
            managed_environment_approved: false,
        }
    }
}
pub mod firewall;
#[cfg(windows)]
pub mod windows;

/// Store packages never invoke the elevated firewall helper.
pub const MANAGED_FIREWALL_AVAILABLE: bool = !cfg!(feature = "store-distribution");
