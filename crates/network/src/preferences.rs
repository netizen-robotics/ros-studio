use crate::Selection;
use serde::{Deserialize, Serialize};
use std::{fs, io::Write, path::Path};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preferences {
    pub version: u32,
    pub selection: Selection,
    pub managed_environment_approved: bool,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            version: 1,
            selection: Selection::Lan,
            managed_environment_approved: false,
        }
    }
}
fn validate(p: &Preferences) -> Result<(), String> {
    if p.version != 1 {
        return Err("Unsupported network settings version".into());
    }
    if let Selection::Adapter { id } = &p.selection {
        if id.is_empty() || id.len() > 1024 {
            return Err("Invalid saved adapter identity".into());
        }
    }
    if let Selection::Custom {
        profile: Some(path),
    } = &p.selection
    {
        if path.is_empty() || path.len() > 4096 {
            return Err("Invalid custom profile path".into());
        }
    }
    Ok(())
}
pub fn load(path: &Path) -> Result<Preferences, String> {
    use std::io::Read;
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Preferences::default()),
        Err(e) => return Err(format!("Cannot read network settings: {e}")),
    };
    let mut bytes = vec![];
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 65536 {
        return Err("Network settings exceed 64 KiB".into());
    }
    let p: Preferences = serde_json::from_slice(&bytes).map_err(|_| {
        "Network settings are damaged. Select settings and Save to replace them.".to_string()
    })?;
    validate(&p)?;
    Ok(p)
}
pub fn save(path: &Path, p: &Preferences) -> Result<(), String> {
    validate(p)?;
    let parent = path.parent().ok_or("Missing settings directory")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let temporary = parent.join(format!(
        ".network-{}-{}.tmp",
        std::process::id(),
        crate::profiles::next_id()
    ));
    let result = (|| {
        let mut f = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|e| e.to_string())?;
        f.write_all(&serde_json::to_vec(p).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
        fs::rename(&temporary, path).map_err(|e| e.to_string())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
pub fn path(application: &str) -> Result<std::path::PathBuf, String> {
    if !matches!(application, "tools" | "visualizer") {
        return Err("Unknown network application".into());
    }
    Ok(studio_app_lifecycle::state_directory()?.join(format!("network-{application}.json")))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roundtrip_corruption_and_independent_apps() {
        let root = std::env::temp_dir().join(format!(
            "studio-network-prefs-{}-{}",
            std::process::id(),
            crate::profiles::next_id()
        ));
        let p = root.join("tools.json");
        assert!(matches!(load(&p).unwrap().selection, Selection::Lan));
        save(
            &p,
            &Preferences {
                selection: Selection::Adapter {
                    id: "stable-guid".into(),
                },
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(
            load(&p).unwrap().selection,
            Selection::Adapter {
                id: "stable-guid".into()
            }
        );
        save(
            &p,
            &Preferences {
                selection: Selection::Local,
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(load(&p).unwrap().selection, Selection::Local);
        assert_eq!(
            load(&root.join("visualizer.json")).unwrap().selection,
            Selection::Lan
        );
        fs::write(&p, b"broken").unwrap();
        assert!(load(&p).is_err());
        fs::remove_dir_all(root).unwrap();
    }
}
