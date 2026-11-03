//! Cross-process exclusion between application startup and managed replacement.
use fs2::FileExt;
use std::{
    fs::{self, File, OpenOptions},
    path::{Path, PathBuf},
};
pub struct AppLease(File);
impl AppLease {
    pub fn acquire(state: &Path, application: &str) -> Result<Self, String> {
        Self::lock(state, application, true)
    }
    pub fn running(state: &Path, application: &str) -> Result<Self, String> {
        Self::lock(state, application, false)
    }
    fn lock(state: &Path, application: &str, exclusive: bool) -> Result<Self, String> {
        if !matches!(application, "visualizer" | "tools" | "launcher") {
            return Err("Unknown application".into());
        }
        fs::create_dir_all(state).map_err(|e| e.to_string())?;
        let file = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .open(state.join(format!("{application}.lock")))
            .map_err(|e| e.to_string())?;
        (if exclusive {
            FileExt::try_lock_exclusive(&file)
        } else {
            FileExt::try_lock_shared(&file)
        })
        .map_err(|_| {
            "Application is running or another installation is in progress. Close it and retry."
                .to_string()
        })?;
        Ok(Self(file))
    }
}
impl Drop for AppLease {
    fn drop(&mut self) {
        let _ = FileExt::unlock(&self.0);
    }
}
pub fn state_directory() -> Result<PathBuf, String> {
    #[cfg(windows)]
    {
        let local = std::env::var_os("LOCALAPPDATA")
            .ok_or("Local application data directory unavailable")?;
        Ok(PathBuf::from(local).join("ROS Studio"))
    }
    #[cfg(not(windows))]
    {
        let home = std::env::var_os("HOME").ok_or("Home directory unavailable")?;
        Ok(PathBuf::from(home).join("Library/Application Support/ROS Studio"))
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(windows)]
    #[test]
    fn windows_state_directory_does_not_require_unix_home() {
        if std::env::var_os("STUDIO_STATE_DIRECTORY_TEST_CHILD").is_some() {
            assert!(std::env::var_os("HOME").is_none());
            let expected =
                PathBuf::from(std::env::var_os("LOCALAPPDATA").unwrap()).join("ROS Studio");
            assert_eq!(state_directory().unwrap(), expected);
            return;
        }
        let result = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "tests::windows_state_directory_does_not_require_unix_home",
                "--nocapture",
            ])
            .env("STUDIO_STATE_DIRECTORY_TEST_CHILD", "1")
            .env_remove("HOME")
            .output()
            .unwrap();
        assert!(result.status.success(), "{result:?}");
    }
    #[test]
    fn running_instances_allow_foreground_open_but_exclude_updates() {
        let root = std::env::temp_dir().join(format!("studio-running-{}", std::process::id()));
        let first = AppLease::running(&root, "tools").unwrap();
        let second = AppLease::running(&root, "tools").unwrap();
        assert!(AppLease::acquire(&root, "tools").is_err());
        drop(first);
        assert!(AppLease::acquire(&root, "tools").is_err());
        drop(second);
        let update = AppLease::acquire(&root, "tools").unwrap();
        assert!(AppLease::running(&root, "tools").is_err());
        drop(update);
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn launcher_starts_and_excludes_bootstrap_replacement() {
        let root =
            std::env::temp_dir().join(format!("studio-launcher-lease-{}", std::process::id()));
        let running = AppLease::acquire(&root, "launcher").unwrap();
        assert!(AppLease::acquire(&root, "launcher").is_err());
        assert!(AppLease::running(&root, "tools").is_ok());
        drop(running);
        assert!(AppLease::acquire(&root, "launcher").is_ok());
        fs::remove_dir_all(root).unwrap();
    }
    #[test]
    fn concurrent_start_or_replacement_is_excluded() {
        let root = std::env::temp_dir().join(format!("studio-lease-{}", std::process::id()));
        let lease = AppLease::acquire(&root, "tools").unwrap();
        assert!(AppLease::acquire(&root, "tools").is_err());
        assert!(AppLease::acquire(&root, "visualizer").is_ok());
        drop(lease);
        assert!(AppLease::acquire(&root, "tools").is_ok());
        fs::remove_dir_all(root).unwrap();
    }
}
