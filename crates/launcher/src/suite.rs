//! Bundled application launch only. Never downloads, installs or invokes a shell.
use crate::catalog::Application;
use std::{
    path::{Path, PathBuf},
    process::{Child, Command},
    sync::mpsc,
    thread::JoinHandle,
    time::Duration,
};

pub fn bundled_executable(root: &Path, app: Application) -> Result<PathBuf, String> {
    let name = format!("studio-{}{}", app.id(), std::env::consts::EXE_SUFFIX);
    let candidate = tool_path(root, app, &name);
    let resolved = candidate.canonicalize().map_err(|_| {
        "Application files are missing. Reinstall ROS Studio, then retry.".to_string()
    })?;
    let root = root.canonicalize().map_err(|e| e.to_string())?;
    let boundary = if cfg!(target_os = "macos")
        && root.file_name().is_some_and(|n| n == "MacOS")
        && root
            .parent()
            .is_some_and(|p| p.join("Info.plist").is_file())
    {
        root.parent().unwrap()
    } else {
        root.as_path()
    };
    if !resolved.starts_with(boundary) || !resolved.is_file() {
        return Err(
            "Application files are outside this ROS Studio installation. Reinstall the suite."
                .into(),
        );
    }
    Ok(resolved)
}
fn tool_path(root: &Path, app: Application, name: &str) -> PathBuf {
    #[cfg(target_os = "macos")]
    if root.file_name().is_some_and(|n| n == "MacOS")
        && root
            .parent()
            .is_some_and(|p| p.join("Info.plist").is_file())
    {
        return root.parent().unwrap().join(format!(
            "Applications/ROS Studio {}.app/Contents/MacOS/{name}",
            if app == Application::Tools {
                "Tools"
            } else {
                "Visualizer"
            }
        ));
    }
    let _ = app;
    root.join(name)
}
#[derive(Clone, Debug)]
pub struct Snapshot {
    pub available: [bool; 2],
    pub error: Option<String>,
}
pub enum Job {
    Inspect,
    Open(Application),
}
pub struct Worker {
    jobs: Option<mpsc::SyncSender<Job>>,
    results: mpsc::Receiver<Snapshot>,
    join: Option<JoinHandle<()>>,
    pub busy: bool,
}
impl Worker {
    pub fn new(root: PathBuf) -> Self {
        let (jobs_tx, jobs) = mpsc::sync_channel(1);
        let (tx, results) = mpsc::sync_channel(1);
        let join = std::thread::spawn(move || {
            let mut children: Vec<Child> = Vec::new();
            loop {
                children.retain_mut(|child| !matches!(child.try_wait(), Ok(Some(_))));
                let job = match jobs.recv_timeout(Duration::from_millis(100)) {
                    Ok(job) => job,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(_) => break,
                };
                let mut error = None;
                if let Job::Open(app) = job {
                    match bundled_executable(&root, app).and_then(|path| {
                        let mut cmd = launch_command(&path);
                        cmd.current_dir(&root);
                        #[cfg(windows)]
                        {
                            use std::os::windows::process::CommandExt;
                            cmd.creation_flags(0x08000000);
                        }
                        cmd.spawn().map_err(|e| {
                            format!(
                                "Could not open {}: {e}. Retry, or reinstall ROS Studio.",
                                app.id()
                            )
                        })
                    }) {
                        Ok(child) => children.push(child),
                        Err(e) => error = Some(e),
                    }
                }
                let available = [Application::Visualizer, Application::Tools]
                    .map(|app| bundled_executable(&root, app).is_ok());
                // At most one request is outstanding; never block shutdown on unread replies.
                let _ = tx.try_send(Snapshot { available, error });
            }
            // Opened apps are independent. Closing Launcher must not terminate them.
        });
        Self {
            jobs: Some(jobs_tx),
            results,
            join: Some(join),
            busy: false,
        }
    }
    pub fn submit(&mut self, job: Job) {
        if !self.busy
            && self
                .jobs
                .as_ref()
                .is_some_and(|tx| tx.try_send(job).is_ok())
        {
            self.busy = true;
        }
    }
    pub fn poll(&mut self) -> Option<Snapshot> {
        let result = self.results.try_recv().ok()?;
        self.busy = false;
        Some(result)
    }
    pub fn shutdown(&mut self) {
        self.jobs.take();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
fn launch_command(path: &Path) -> Command {
    #[cfg(target_os = "macos")]
    if let Some(bundle) = path
        .parent()
        .and_then(Path::parent)
        .and_then(Path::parent)
        .filter(|p| p.extension().is_some_and(|e| e == "app"))
    {
        let mut command = Command::new("/usr/bin/open");
        command.arg("-n").arg(bundle);
        return command;
    }
    Command::new(path)
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn missing_bundle_can_recover_without_legacy_install_lookup() {
        let root = std::env::temp_dir().join(format!("studio-suite-test-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let exe = root.join(format!("studio-tools{}", std::env::consts::EXE_SUFFIX));
        assert!(bundled_executable(&root, Application::Tools).is_err());
        std::fs::write(&exe, b"fixture").unwrap();
        assert_eq!(
            bundled_executable(&root, Application::Tools).unwrap(),
            exe.canonicalize().unwrap()
        );
        std::fs::remove_file(exe).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
    #[cfg(unix)]
    #[test]
    fn symlink_cannot_launch_outside_suite() {
        let root = std::env::temp_dir().join(format!("studio-suite-link-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let link = root.join("studio-tools");
        std::os::unix::fs::symlink("/bin/sh", &link).unwrap();
        assert!(bundled_executable(&root, Application::Tools).is_err());
        std::fs::remove_file(link).unwrap();
        std::fs::remove_dir(root).unwrap();
    }
}
