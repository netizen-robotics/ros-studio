//! Authenticated Windows runtime inventory. Native trust checks are separate.
use crate::catalog::{self, Application, Release};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};
pub const MAX_MANIFEST: u64 = 16 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub schema: u32,
    pub application: Application,
    pub version: String,
    pub distro: String,
    pub files: BTreeMap<String, String>,
}
pub fn read(path: &Path, limit: u64) -> Result<Vec<u8>, String> {
    let mut bytes = vec![];
    fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(limit + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > limit {
        return Err("Installation metadata exceeds its bound".into());
    }
    Ok(bytes)
}
pub fn digest(path: &Path) -> Result<String, String> {
    let mut file = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hash = Sha256::new();
    let mut bytes = [0u8; 65536];
    loop {
        let n = file.read(&mut bytes).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hash.update(&bytes[..n]);
    }
    Ok(hex::encode(hash.finalize()))
}
pub fn safe_relative(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 1024
        && !name.contains(['\\', ':'])
        && Path::new(name)
            .components()
            .all(|p| matches!(p, Component::Normal(_)))
        && name.split('/').all(|part| {
            let base = part.split('.').next().unwrap_or("").to_ascii_uppercase();
            !part.ends_with(['.', ' '])
                && !matches!(
                    base.as_str(),
                    "CON"
                        | "PRN"
                        | "AUX"
                        | "NUL"
                        | "COM1"
                        | "COM2"
                        | "COM3"
                        | "COM4"
                        | "COM5"
                        | "COM6"
                        | "COM7"
                        | "COM8"
                        | "COM9"
                        | "LPT1"
                        | "LPT2"
                        | "LPT3"
                        | "LPT4"
                        | "LPT5"
                        | "LPT6"
                        | "LPT7"
                        | "LPT8"
                        | "LPT9"
                )
        })
}
fn plain(path: &Path) -> Result<fs::Metadata, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| e.to_string())?;
    if metadata.file_type().is_symlink() {
        return Err("Runtime links are unsupported".into());
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err("Runtime reparse points are unsupported".into());
        }
    }
    Ok(metadata)
}
fn inventory(
    root: &Path,
    path: &Path,
    files: &mut BTreeSet<String>,
    depth: usize,
) -> Result<(), String> {
    if depth > 64 || files.len() > 50000 {
        return Err("Runtime inventory exceeds limits".into());
    }
    for item in fs::read_dir(path).map_err(|e| e.to_string())? {
        let path = item.map_err(|e| e.to_string())?.path();
        let metadata = plain(&path)?;
        if metadata.is_dir() {
            inventory(root, &path, files, depth + 1)?;
        } else if metadata.is_file() {
            let relative = path
                .strip_prefix(root)
                .map_err(|e| e.to_string())?
                .to_str()
                .ok_or("Runtime path is not Unicode")?
                .replace('\\', "/");
            if !safe_relative(&relative) {
                return Err("Invalid Windows runtime path".into());
            }
            files.insert(relative);
        } else {
            return Err("Unsupported runtime entry".into());
        }
    }
    Ok(())
}
pub fn manifest(root: &Path) -> Result<Manifest, String> {
    serde_json::from_slice(&read(&root.join("manifest.json"), MAX_MANIFEST)?)
        .map_err(|e| e.to_string())
}
pub fn validate_files(root: &Path, release: &Release) -> Result<(), String> {
    let windows = release.windows.as_ref().ok_or("Windows metadata missing")?;
    for ancestor in root.ancestors() {
        plain(ancestor)?;
    }
    if digest(&root.join("manifest.json"))? != windows.manifest_sha256 {
        return Err("Runtime manifest does not match signed catalog".into());
    }
    let manifest = manifest(root)?;
    if manifest.schema != 1
        || manifest.application != release.application
        || manifest.version != release.version
        || manifest.distro != "jazzy"
        || manifest.files.len() > 50000
    {
        return Err("Runtime manifest identity/version invalid".into());
    }
    let mut folded = BTreeSet::new();
    for (name, expected) in &manifest.files {
        if !safe_relative(name)
            || !folded.insert(name.to_lowercase())
            || expected.len() != 64
            || hex::decode(expected).map_or(true, |b| b.len() != 32)
        {
            return Err("Invalid or duplicate runtime inventory entry".into());
        }
        let file = root.join(name);
        let mut parent = file.parent();
        while let Some(p) = parent {
            plain(p)?;
            if p == root {
                break;
            }
            parent = p.parent();
        }
        if !plain(&file)?.is_file() || digest(&file)? != *expected {
            return Err(format!("Runtime file verification failed: {name}"));
        }
    }
    for (name, expected) in [
        (
            format!("studio-{}.exe", release.application.id()),
            &windows.application_sha256,
        ),
        ("studio-backend.exe".into(), &windows.backend_sha256),
        ("studio-network-helper.exe".into(), &windows.helper_sha256),
    ] {
        if manifest.files.get(&name) != Some(expected) {
            return Err("Runtime executable roles do not match catalog".into());
        }
    }
    for required in ["runtime/manifest.json", "THIRD-PARTY-NOTICES.txt"] {
        if !manifest.files.contains_key(required) {
            return Err(format!("Missing required runtime metadata: {required}"));
        }
    }
    if !root.join("runtime/bin").is_dir() || !root.join("runtime/share/ament_index").is_dir() {
        return Err("Incomplete ROS runtime prefix".into());
    }
    let mut actual = BTreeSet::new();
    inventory(root, root, &mut actual, 0)?;
    actual.remove("manifest.json");
    actual.remove(".release-catalog.json");
    if actual != manifest.files.keys().cloned().collect() {
        return Err("Runtime has missing or unlisted files".into());
    }
    Ok(())
}
pub fn cached_release(root: &Path, app: Application) -> Result<Release, String> {
    let catalog = catalog::verify(
        &read(&root.join(".release-catalog.json"), catalog::MAX_CATALOG)?,
        &catalog::embedded_key()?,
    )?;
    let manifest = manifest(root)?;
    catalog
        .releases
        .into_iter()
        .find(|r| r.application == app && r.platform == "windows" && r.version == manifest.version)
        .ok_or("Installed release is absent from signed catalog".into())
}
pub fn target(root: &Path, app: Application) -> PathBuf {
    root.join(app.id())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn windows_archive_paths_reject_ads_devices_and_aliases() {
        for bad in [
            "../outside",
            "/absolute",
            "C:/outside",
            "a\\b",
            "a:stream",
            "CON.txt",
            "a/NUL",
            "a/trailing.",
            "a/trailing ",
        ] {
            assert!(!safe_relative(bad), "{bad}");
        }
        for good in [
            "runtime/bin/nav_msgs__rosidl_typesupport_introspection_c.dll",
            "notices/许可证.txt",
        ] {
            assert!(safe_relative(good), "{good}");
        }
    }
}

#[cfg(test)]
mod inventory_tests {
    use super::*;
    use crate::catalog::WindowsRelease;
    fn fixture() -> (PathBuf, Release) {
        static ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "studio-runtime-{}-{}",
            std::process::id(),
            ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        fs::create_dir_all(root.join("runtime/bin")).unwrap();
        let root = root.canonicalize().unwrap();
        fs::create_dir_all(root.join("runtime/share/ament_index")).unwrap();
        for file in [
            "studio-tools.exe",
            "studio-backend.exe",
            "studio-network-helper.exe",
            "runtime/bin/rmw.dll",
            "runtime/manifest.json",
            "runtime/share/ament_index/index",
            "THIRD-PARTY-NOTICES.txt",
        ] {
            fs::write(root.join(file), file).unwrap();
        }
        let files = [
            "studio-tools.exe",
            "studio-backend.exe",
            "studio-network-helper.exe",
            "runtime/bin/rmw.dll",
            "runtime/manifest.json",
            "runtime/share/ament_index/index",
            "THIRD-PARTY-NOTICES.txt",
        ]
        .into_iter()
        .map(|p| (p.to_owned(), digest(&root.join(p)).unwrap()))
        .collect();
        let manifest = Manifest {
            schema: 1,
            application: Application::Tools,
            version: "0.1.1".into(),
            distro: "jazzy".into(),
            files,
        };
        fs::write(
            root.join("manifest.json"),
            serde_json::to_vec(&manifest).unwrap(),
        )
        .unwrap();
        let mut release = crate::catalog::tests::release();
        release.platform = "windows".into();
        release.architecture = "x86_64".into();
        release.minimum_macos.clear();
        release.minimum_launcher = "0.1.0".into();
        release.windows = Some(WindowsRelease {
            minimum_build: 22000,
            manifest_sha256: digest(&root.join("manifest.json")).unwrap(),
            application_sha256: manifest.files["studio-tools.exe"].clone(),
            backend_sha256: manifest.files["studio-backend.exe"].clone(),
            helper_sha256: manifest.files["studio-network-helper.exe"].clone(),
        });
        (root, release)
    }
    #[test]
    fn runtime_inventory_detects_tampered_and_unlisted_dlls() {
        let (root, release) = fixture();
        assert!(validate_files(&root, &release).is_ok());
        fs::write(root.join("runtime/bin/injected.dll"), "injected").unwrap();
        assert!(validate_files(&root, &release)
            .unwrap_err()
            .contains("unlisted"));
        fs::remove_file(root.join("runtime/bin/injected.dll")).unwrap();
        fs::write(root.join("runtime/bin/rmw.dll"), "tampered").unwrap();
        assert!(validate_files(&root, &release)
            .unwrap_err()
            .contains("verification failed"));
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn signed_manifest_role_cannot_point_at_a_different_executable() {
        let (root, mut release) = fixture();
        release.windows.as_mut().unwrap().backend_sha256 = "aa".repeat(32);
        assert!(validate_files(&root, &release)
            .unwrap_err()
            .contains("roles"));
        fs::remove_dir_all(root).unwrap();
    }
}
