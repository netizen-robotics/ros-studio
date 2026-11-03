//! Verify the exact signed bytes before interpreting a bounded release catalog.
use ed25519_dalek::{Signature, VerifyingKey};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
pub const MAX_CATALOG: u64 = 256 * 1024;
pub const MAX_ARCHIVE: u64 = 2 * 1024 * 1024 * 1024;
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Application {
    Visualizer,
    Tools,
}
impl Application {
    pub fn id(self) -> &'static str {
        match self {
            Self::Visualizer => "visualizer",
            Self::Tools => "tools",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Visualizer => "ROS Studio Visualizer",
            Self::Tools => "ROS Studio Tools",
        }
    }
    pub fn bundle_id(self) -> &'static str {
        match self {
            Self::Visualizer => "dev.rosstudio.visualizer",
            Self::Tools => "dev.rosstudio.tools",
        }
    }
    pub fn bundle_name(self) -> String {
        format!("{}.app", self.name())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Release {
    pub application: Application,
    pub version: String,
    pub platform: String,
    pub architecture: String,
    pub distro: String,
    #[serde(default)]
    pub minimum_macos: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows: Option<WindowsRelease>,
    pub minimum_launcher: String,
    pub archive_url: String,
    pub archive_bytes: u64,
    pub sha256: String,
    pub bundle_id: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WindowsRelease {
    pub minimum_build: u32,
    pub manifest_sha256: String,
    pub application_sha256: String,
    pub backend_sha256: String,
    pub helper_sha256: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Catalog {
    pub schema: u32,
    pub releases: Vec<Release>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignedCatalog {
    pub payload: String,
    pub signature: String,
}
pub fn verify(bytes: &[u8], key: &[u8; 32]) -> Result<Catalog, String> {
    if bytes.len() as u64 > MAX_CATALOG {
        return Err("Catalog exceeds 256 KiB".into());
    }
    let signed: SignedCatalog = serde_json::from_slice(bytes).map_err(|e| e.to_string())?;
    let signature = hex::decode(&signed.signature).map_err(|e| e.to_string())?;
    let signature = Signature::from_slice(&signature).map_err(|e| e.to_string())?;
    VerifyingKey::from_bytes(key)
        .map_err(|e| e.to_string())?
        .verify_strict(signed.payload.as_bytes(), &signature)
        .map_err(|_| "Catalog signature is invalid")?;
    let catalog: Catalog = serde_json::from_str(&signed.payload).map_err(|e| e.to_string())?;
    if !matches!(catalog.schema, 1 | 2) || catalog.releases.len() > 16 {
        return Err("Unsupported catalog schema or release count".into());
    }
    let mut seen = HashSet::new();
    for release in &catalog.releases {
        if catalog.schema == 1 && (release.platform != "macos" || release.windows.is_some()) {
            return Err("Windows releases require catalog schema 2".into());
        }
        release.validate()?;
        if !seen.insert((
            release.application,
            &release.platform,
            &release.architecture,
        )) {
            return Err("Duplicate application/platform release".into());
        }
    }
    Ok(catalog)
}
impl Release {
    pub fn for_host(&self) -> bool {
        (cfg!(all(target_os = "macos", target_arch = "aarch64"))
            && self.platform == "macos"
            && self.architecture == "aarch64")
            || (cfg!(all(windows, target_arch = "x86_64"))
                && self.platform == "windows"
                && self.architecture == "x86_64")
    }
    pub fn validate(&self) -> Result<(), String> {
        semver::Version::parse(&self.version).map_err(|e| e.to_string())?;
        let minimum = semver::Version::parse(&self.minimum_launcher).map_err(|e| e.to_string())?;
        if minimum > semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap() {
            return Err("Update the launcher using its signed installer first".into());
        }
        let mac =
            self.platform == "macos" && self.architecture == "aarch64" && self.windows.is_none();
        let windows = self.platform == "windows"
            && self.architecture == "x86_64"
            && self.minimum_macos.is_empty()
            && self.windows.is_some();
        if (!mac && !windows)
            || self.distro != "jazzy"
            || self.bundle_id != self.application.bundle_id()
        {
            return Err("Unsupported platform/runtime or bundle identity".into());
        }
        if mac
            && (self.minimum_macos.split('.').count() < 2
                || self
                    .minimum_macos
                    .split('.')
                    .any(|n| n.parse::<u32>().is_err()))
        {
            return Err("Invalid minimum macOS version".into());
        }
        if self.windows.is_some() && minimum < semver::Version::new(0, 1, 0) {
            return Err("Windows releases require Launcher 0.1.0 or later".into());
        }
        if let Some(windows) = &self.windows {
            if windows.minimum_build < 22000
                || [
                    &windows.manifest_sha256,
                    &windows.application_sha256,
                    &windows.backend_sha256,
                    &windows.helper_sha256,
                ]
                .iter()
                .any(|hash| hash.len() != 64 || hex::decode(hash).map_or(true, |b| b.len() != 32))
            {
                return Err("Invalid Windows build or runtime identities".into());
            }
        }
        if self.archive_bytes == 0
            || self.archive_bytes > MAX_ARCHIVE
            || self.sha256.len() != 64
            || hex::decode(&self.sha256).map_or(true, |b| b.len() != 32)
        {
            return Err("Invalid archive size or SHA-256".into());
        }
        github_url(&self.archive_url)?;
        Ok(())
    }
}
pub fn github_url(value: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(value).map_err(|e| e.to_string())?;
    let parts: Vec<_> = url.path_segments().ok_or("Invalid release URL")?.collect();
    if url.scheme() != "https"
        || url.host_str() != Some("github.com")
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || parts.len() < 6
        || parts[2] != "releases"
        || parts[3] != "download"
        || parts.iter().any(|p| p.is_empty() || *p == "..")
    {
        return Err("Use a GitHub HTTPS release-asset URL".into());
    }
    Ok(url)
}
pub fn embedded_key() -> Result<[u8; 32], String> {
    let value = option_env!("STUDIO_RELEASE_PUBLIC_KEY")
        .ok_or("Release signing key is not configured in this launcher build")?;
    hex::decode(value)
        .map_err(|e| e.to_string())?
        .try_into()
        .map_err(|_| "Release public key must contain 32 bytes".into())
}
#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use ed25519_dalek::{Signer, SigningKey};
    pub fn release() -> Release {
        Release {
            application: Application::Tools,
            version: "0.1.1".into(),
            platform: "macos".into(),
            architecture: "aarch64".into(),
            distro: "jazzy".into(),
            minimum_macos: "13.0".into(),
            windows: None,
            minimum_launcher: "0.1.0".into(),
            archive_url: "https://github.com/example/studio/releases/download/v0.1.1/tools.tar.gz"
                .into(),
            archive_bytes: 100,
            sha256: "ab".repeat(32),
            bundle_id: Application::Tools.bundle_id().into(),
        }
    }
    #[test]
    fn signature_tamper_identity_and_duplicates() {
        let key = SigningKey::from_bytes(&[42; 32]);
        let encode = |catalog: &Catalog| {
            let payload = serde_json::to_string(catalog).unwrap();
            let signature = hex::encode(key.sign(payload.as_bytes()).to_bytes());
            serde_json::to_vec(&SignedCatalog { payload, signature }).unwrap()
        };
        let mut catalog = Catalog {
            schema: 1,
            releases: vec![release()],
        };
        let bytes = encode(&catalog);
        assert!(verify(&bytes, &key.verifying_key().to_bytes()).is_ok());
        assert!(verify(
            &bytes,
            &SigningKey::from_bytes(&[43; 32]).verifying_key().to_bytes()
        )
        .is_err());
        let tamper = String::from_utf8(bytes).unwrap().replace("0.1.1", "0.1.2");
        assert!(verify(tamper.as_bytes(), &key.verifying_key().to_bytes()).is_err());
        catalog.releases.push(release());
        assert!(verify(&encode(&catalog), &key.verifying_key().to_bytes()).is_err());
        catalog.releases.truncate(1);
        catalog.releases[0].bundle_id = "unrelated.app".into();
        assert!(verify(&encode(&catalog), &key.verifying_key().to_bytes()).is_err());
    }
    #[test]
    fn platform_catalog_keeps_v1_mac_and_requires_v2_windows() {
        let key = SigningKey::from_bytes(&[41; 32]);
        let sign = |catalog: &Catalog| {
            let payload = serde_json::to_string(catalog).unwrap();
            let signature = hex::encode(key.sign(payload.as_bytes()).to_bytes());
            serde_json::to_vec(&SignedCatalog { payload, signature }).unwrap()
        };
        let mac = release();
        let mut win = release();
        win.platform = "windows".into();
        win.architecture = "x86_64".into();
        win.minimum_macos.clear();
        win.minimum_launcher = "0.1.0".into();
        win.windows = Some(WindowsRelease {
            minimum_build: 22000,
            manifest_sha256: "aa".repeat(32),
            application_sha256: "bb".repeat(32),
            backend_sha256: "cc".repeat(32),
            helper_sha256: "dd".repeat(32),
        });
        let mut catalog = Catalog {
            schema: 2,
            releases: vec![mac, win],
        };
        assert!(verify(&sign(&catalog), &key.verifying_key().to_bytes()).is_ok());
        catalog.releases[1].minimum_launcher = "0.2.0".into();
        assert!(verify(&sign(&catalog), &key.verifying_key().to_bytes())
            .unwrap_err()
            .contains("Update the launcher"));
        catalog.releases[1].minimum_launcher = "0.1.0".into();
        catalog.schema = 1;
        assert!(verify(&sign(&catalog), &key.verifying_key().to_bytes())
            .unwrap_err()
            .contains("schema 2"));
        catalog.releases.truncate(1);
        assert!(verify(&sign(&catalog), &key.verifying_key().to_bytes()).is_ok());
    }
    #[test]
    fn reject_non_release_urls() {
        for url in [
            "http://github.com/a/b/releases/download/v1/app",
            "https://evil.example/a/b/releases/download/v1/app",
            "https://github.com/a/b/blob/main/file",
        ] {
            assert!(github_url(url).is_err());
        }
    }
}
