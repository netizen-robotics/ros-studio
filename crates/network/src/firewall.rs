//! Bounded firewall requests contain identities and scope, never programs or commands.
use serde::{Deserialize, Serialize};
use std::net::Ipv4Addr;
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Application {
    Tools,
    Visualizer,
}
impl Application {
    pub fn id(self) -> &'static str {
        match self {
            Self::Tools => "tools",
            Self::Visualizer => "visualizer",
        }
    }
}
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Operation {
    Inspect,
    Add,
    Repair,
    Remove,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", deny_unknown_fields)]
pub enum Scope {
    Private,
    Public { peers: Vec<String> },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    pub application: Application,
    pub operation: Operation,
    pub adapter_id: String,
    pub scope: Scope,
}
impl Request {
    pub fn validate(&self) -> Result<(), String> {
        if self.adapter_id.is_empty()
            || self.adapter_id.len() > 1024
            || self.adapter_id.contains('\0')
        {
            return Err("Select an adapter first".into());
        }
        if let Scope::Public { peers } = &self.scope {
            if peers.is_empty() || peers.len() > 16 {
                return Err("Public networks require 1–16 approved IPv4 peers or subnets".into());
            }
            for peer in peers {
                let parts: Vec<_> = peer.split('/').collect();
                let ip: Ipv4Addr = parts[0].parse().map_err(|_| "Invalid approved IPv4 peer")?;
                if ip.is_unspecified()
                    || ip.is_multicast()
                    || ip.is_broadcast()
                    || ip.is_loopback()
                    || ip.is_link_local()
                    || parts.len() > 2
                    || (parts.len() == 2
                        && parts[1].parse::<u8>().map_or(true, |p| p == 0 || p > 32))
                {
                    return Err(
                        "Use explicit unicast IPv4 peers or subnets with prefixes 1–32".into(),
                    );
                }
            }
        }
        Ok(())
    }
    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > 16384 {
            return Err("Firewall request exceeds limit".into());
        }
        let request: Self = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
        request.validate()?;
        Ok(request)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_commands_paths_and_unbounded_public_access() {
        assert!(Request::decode(br#"{"application":"tools","operation":"repair","adapter_id":"windows:abc","scope":{"kind":"Private"},"executable":"evil.exe"}"#).is_err());
        let mut request = Request {
            application: Application::Tools,
            operation: Operation::Repair,
            adapter_id: "windows:abc".into(),
            scope: Scope::Public {
                peers: vec!["0.0.0.0/0".into()],
            },
        };
        assert!(request.validate().is_err());
        request.scope = Scope::Public {
            peers: vec!["192.0.2.0/24".into(), "198.51.100.2".into()],
        };
        assert!(request.validate().is_ok());
    }
}
