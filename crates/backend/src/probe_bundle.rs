use crate::service_codec::LIMIT;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Component, Path, PathBuf},
};
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema_version: u32,
    ros_distro: String,
    os: String,
    arch: String,
    packages: Vec<String>,
    files: BTreeMap<String, String>,
}
pub fn bounded_read(path: &Path) -> Result<Vec<u8>, String> {
    let file = fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut data = Vec::new();
    file.take((LIMIT + 1) as u64)
        .read_to_end(&mut data)
        .map_err(|e| e.to_string())?;
    if data.len() > LIMIT {
        return Err("Input exceeds 1 MiB".into());
    }
    Ok(data)
}
fn hash(path: &Path) -> Result<String, String> {
    let mut f = fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut data = [0u8; 65536];
    loop {
        let n = f.read(&mut data).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        hasher.update(&data[..n]);
    }
    Ok(format!("{:x}", hasher.finalize()))
}
fn inventory(root: &Path, path: &Path, listed: &BTreeMap<String, String>) -> Result<(), String> {
    for entry in fs::read_dir(path).map_err(|e| e.to_string())? {
        let entry = entry.map_err(|e| e.to_string())?;
        let p = entry.path();
        let ty = entry.file_type().map_err(|e| e.to_string())?;
        if ty.is_dir() {
            inventory(root, &p, listed)?;
        } else {
            let relative = p
                .strip_prefix(root)
                .map_err(|e| e.to_string())?
                .to_string_lossy()
                .replace('\\', "/");
            if relative != "manifest.json" && !listed.contains_key(&relative) {
                return Err(format!("Unlisted bundle file: {relative}"));
            }
        }
    }
    Ok(())
}
pub fn bundle(path: &Path, package: &str) -> Result<PathBuf, String> {
    let root = path.canonicalize().map_err(|e| format!("Bundle: {e}"))?;
    let m: Manifest = serde_json::from_slice(&bounded_read(&root.join("manifest.json"))?)
        .map_err(|e| format!("Manifest: {e}"))?;
    if m.schema_version != 1
        || m.ros_distro != env!("STUDIO_ROS_DISTRO")
        || m.os != std::env::consts::OS
        || m.arch != std::env::consts::ARCH
    {
        return Err(format!(
            "Incompatible bundle: expected schema 1, {}/{}/{}",
            env!("STUDIO_ROS_DISTRO"),
            std::env::consts::OS,
            std::env::consts::ARCH
        ));
    }
    if !m.packages.iter().any(|p| p == package) {
        return Err(format!("Bundle does not declare package {package}"));
    }
    for (relative, expected) in &m.files {
        let p = Path::new(relative);
        if p.components().any(|c| !matches!(c, Component::Normal(_)))
            || relative.contains('\\')
            || relative.is_empty()
        {
            return Err(format!("Invalid bundle path {relative}"));
        }
        let full = root
            .join(p)
            .canonicalize()
            .map_err(|e| format!("Missing bundle file {relative}: {e}"))?;
        if !full.starts_with(&root) || !full.is_file() {
            return Err(format!(
                "Bundle file escapes root or is not a file: {relative}"
            ));
        }
        if hash(&full)? != *expected {
            return Err(format!("Checksum mismatch: {relative}"));
        }
    }
    inventory(&root, &root, &m.files)?;
    let index = root
        .join("share/ament_index/resource_index/packages")
        .join(package);
    if !index.is_file() {
        return Err(format!("Missing ament package index for {package}"));
    }
    Ok(root)
}
pub fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.as_bytes()[0].is_ascii_alphabetic()
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
pub fn library(root: &Path, package: &str, kind: &str) -> PathBuf {
    if cfg!(windows) {
        root.join("bin").join(format!("{package}__{kind}.dll"))
    } else {
        root.join("lib").join(format!(
            "lib{package}__{kind}.{}",
            if cfg!(target_os = "macos") {
                "dylib"
            } else {
                "so"
            }
        ))
    }
}
