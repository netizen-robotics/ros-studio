//! Owned session profiles. Caller retains this guard until its backend has exited.
use crate::{
    adapters::{self, Adapter},
    ConnectionConfig, Middleware, Selection,
};
use std::{
    fs,
    io::Write,
    net::Ipv4Addr,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(1);
pub(crate) fn next_id() -> u64 {
    NEXT.fetch_add(1, Ordering::Relaxed)
}
pub const OVERRIDES: &[&str] = &[
    "ROS_LOCALHOST_ONLY",
    "ROS_AUTOMATIC_DISCOVERY_RANGE",
    "ROS_STATIC_PEERS",
    "ROS_DISCOVERY_SERVER",
    "ROS_SUPER_CLIENT",
    "CYCLONEDDS_URI",
    "FASTRTPS_DEFAULT_PROFILES_FILE",
    "FASTDDS_DEFAULT_PROFILES_FILE",
    "FASTDDS_ENVIRONMENT_FILE",
    "FASTDDS_BUILTIN_TRANSPORTS",
    "RMW_FASTRTPS_USE_QOS_FROM_XML",
];
pub fn inherited_overrides() -> Vec<String> {
    OVERRIDES
        .iter()
        .filter(|name| std::env::var_os(name).is_some_and(|v| !v.is_empty()))
        .map(|s| s.to_string())
        .collect()
}

pub struct Profile {
    pub bound: Option<(String, Ipv4Addr)>,
    directory: Option<PathBuf>,
    xml: Option<PathBuf>,
    middleware: Middleware,
    local: bool,
    custom: bool,
}
fn xml(middleware: Middleware, ip: Ipv4Addr, local: bool, domain: u16) -> String {
    let max_participant = ((65535u32 - (7400 + u32::from(domain) * 250) - 11) / 2).min(255);
    let initial_peers = max_participant + 1;
    let discovery = format!("<Discovery><MaxAutoParticipantIndex>{max_participant}</MaxAutoParticipantIndex>{}</Discovery>", if local {"<ParticipantIndex>auto</ParticipantIndex><Peers><Peer Address=\"127.0.0.1\"/></Peers>"} else {""});
    match middleware {
        Middleware::Cyclone=>format!("<CycloneDDS xmlns=\"https://cdds.io/config\"><Domain Id=\"any\"><General><Interfaces><NetworkInterface address=\"{ip}\"/></Interfaces>{}</General>{}</Domain></CycloneDDS>",if local {"<AllowMulticast>false</AllowMulticast>"}else{""},discovery),
        Middleware::Fast=>format!("<profiles xmlns=\"http://www.eprosima.com/XMLSchemas/fastRTPS_Profiles\"><transport_descriptors><transport_descriptor><transport_id>studio_ipv4</transport_id><type>UDPv4</type><interfaceWhiteList><address>{ip}</address></interfaceWhiteList><maxInitialPeersRange>{initial_peers}</maxInitialPeersRange></transport_descriptor></transport_descriptors><participant profile_name=\"studio_network\" is_default_profile=\"true\"><rtps><useBuiltinTransports>false</useBuiltinTransports><userTransports><transport_id>studio_ipv4</transport_id></userTransports>{}</rtps></participant></profiles>",if local {"<builtin><metatrafficUnicastLocatorList><locator/></metatrafficUnicastLocatorList><initialPeersList><locator><udpv4><address>127.0.0.1</address></udpv4></locator></initialPeersList></builtin>"}else{""}),
    }
}
impl Profile {
    pub fn prepare(config: &ConnectionConfig) -> Result<Self, String> {
        if matches!(
            config.selection,
            Selection::Custom { .. } | Selection::Local
        ) {
            Self::with_adapters(config, &[])
        } else {
            Self::with_adapters(config, &adapters::enumerate()?)
        }
    }
    pub fn with_adapters(config: &ConnectionConfig, adapters: &[Adapter]) -> Result<Self, String> {
        if config.domain > 232 {
            return Err("ROS domain must be between 0 and 232".into());
        }
        let custom = matches!(config.selection, Selection::Custom { .. });
        if !custom && !config.managed_environment_approved && !inherited_overrides().is_empty() {
            return Err("Inherited DDS settings detected. Choose managed or Custom configuration in Network settings.".into());
        }
        let bound = adapters::resolve(&config.selection, adapters)?;
        let mut profile = Self {
            bound,
            directory: None,
            xml: None,
            middleware: config.middleware,
            local: matches!(config.selection, Selection::Local),
            custom,
        };
        if let Selection::Custom {
            profile: Some(path),
        } = &config.selection
        {
            if path.len() > 4096 {
                return Err("Custom profile path is too long".into());
            }
            let path = std::path::Path::new(path)
                .canonicalize()
                .map_err(|_| "Custom DDS profile is unavailable".to_string())?;
            let size = path.metadata().map_err(|e| e.to_string())?.len();
            if !path.is_file() || size == 0 || size > 1024 * 1024 {
                return Err("Custom profile must be a file between 1 byte and 1 MiB".into());
            }
            profile.xml = Some(path);
        }
        if let Some((_, ip)) = profile.bound {
            let directory = std::env::temp_dir().join(format!(
                "ros-studio-network-{}-{}",
                std::process::id(),
                next_id()
            ));
            fs::create_dir(&directory).map_err(|e| e.to_string())?;
            profile.directory = Some(directory.clone());
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))
                    .map_err(|e| e.to_string())?;
            }
            let file = directory.join("dds.xml");
            let mut output = fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&file)
                .map_err(|e| e.to_string())?;
            output
                .write_all(xml(config.middleware, ip, profile.local, config.domain).as_bytes())
                .map_err(|e| e.to_string())?;
            profile.xml = Some(file);
        }
        Ok(profile)
    }
    pub fn apply(&self, command: &mut Command) {
        let Some(xml) = &self.xml else {
            return;
        };
        if !self.custom {
            for name in OVERRIDES {
                command.env_remove(name);
            }
            command.env(
                "ROS_AUTOMATIC_DISCOVERY_RANGE",
                if self.local { "LOCALHOST" } else { "SUBNET" },
            );
        }
        match self.middleware {
            Middleware::Cyclone => {
                command.env("CYCLONEDDS_URI", xml);
            }
            Middleware::Fast => {
                command
                    .env("FASTRTPS_DEFAULT_PROFILES_FILE", xml)
                    .env("FASTDDS_DEFAULT_PROFILES_FILE", xml)
                    .env("SKIP_DEFAULT_XML", "1");
            }
        }
    }
    pub fn path(&self) -> Option<&std::path::Path> {
        self.xml.as_deref()
    }
    pub fn still_available(&self) -> Result<(), String> {
        if let Some((id, address)) = &self.bound {
            if id == "loopback" {
                return Ok(());
            }
            if !adapters::enumerate()?
                .iter()
                .any(|a| &a.id == id && a.usable() && a.addresses.contains(address))
            {
                return Err("Network adapter or address changed. Choose Network settings and reconnect explicitly.".into());
            }
        }
        Ok(())
    }
}
impl Drop for Profile {
    fn drop(&mut self) {
        if let Some(directory) = &self.directory {
            let _ = fs::remove_dir_all(directory);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fast_peer_range_does_not_wrap_into_another_domains_ports() {
        assert!(xml(Middleware::Fast, Ipv4Addr::LOCALHOST, true, 232)
            .contains("<maxInitialPeersRange>63</maxInitialPeersRange>"));
        assert!(xml(Middleware::Fast, Ipv4Addr::LOCALHOST, true, 187)
            .contains("<maxInitialPeersRange>256</maxInitialPeersRange>"));
    }
    #[test]
    fn participant_range_stays_inside_domain_udp_ports() {
        let last = xml(Middleware::Cyclone, Ipv4Addr::LOCALHOST, true, 232);
        assert!(last.contains("<MaxAutoParticipantIndex>62</MaxAutoParticipantIndex>"));
        let fixture = xml(Middleware::Cyclone, Ipv4Addr::LOCALHOST, true, 187);
        assert!(fixture.contains("<MaxAutoParticipantIndex>255</MaxAutoParticipantIndex>"));
    }
    #[test]
    fn custom_unicode_file_and_inherited_discovery_are_never_modified() {
        let root = std::env::temp_dir().join(format!(
            "studio-custom-{}-{}",
            std::process::id(),
            next_id()
        ));
        fs::create_dir(&root).unwrap();
        let file = root.join("custom δ.xml");
        fs::write(&file, "<external-profile/>").unwrap();
        let config = ConnectionConfig {
            domain: 187,
            middleware: Middleware::Cyclone,
            selection: Selection::Custom {
                profile: Some(file.to_string_lossy().into_owned()),
            },
            managed_environment_approved: false,
        };
        let profile = Profile::with_adapters(&config, &[]).unwrap();
        let mut command = Command::new("unused");
        command.env("ROS_STATIC_PEERS", "external-peer");
        profile.apply(&mut command);
        assert!(command
            .get_envs()
            .any(|(key, value)| key == "ROS_STATIC_PEERS" && value.unwrap() == "external-peer"));
        drop(profile);
        assert_eq!(fs::read_to_string(&file).unwrap(), "<external-profile/>");
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn owned_profiles_isolate_child_and_cleanup_even_failed_start() {
        for middleware in [Middleware::Fast, Middleware::Cyclone] {
            let config = ConnectionConfig {
                domain: 187,
                middleware,
                selection: Selection::Local,
                managed_environment_approved: true,
            };
            let profile = Profile::with_adapters(&config, &[]).unwrap();
            let file = profile.xml.clone().unwrap();
            let mut command = Command::new("does-not-exist");
            profile.apply(&mut command);
            assert!(command
                .get_envs()
                .any(|(key, value)| key == "ROS_STATIC_PEERS" && value.is_none()));
            assert!(fs::read_to_string(&file).unwrap().contains("127.0.0.1"));
            assert!(command.spawn().is_err());
            drop(profile);
            assert!(!file.exists());
        }
        let config = ConnectionConfig::inherited(187, true);
        let profile = Profile::with_adapters(&config, &[]).unwrap();
        let mut command = Command::new("unused");
        profile.apply(&mut command);
        assert_eq!(command.get_envs().count(), 0);
    }
}
