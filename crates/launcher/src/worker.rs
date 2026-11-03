//! One operation and one result slot. Progress coalesces; no network or process I/O on UI frames.
use crate::{
    catalog::{self, Application, Catalog, Release},
    install,
};
#[cfg(test)]
use std::io::Read;
use std::{
    fs,
    io::Write,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread::JoinHandle,
    time::Duration,
};
#[derive(Clone, Default)]
pub struct Progress {
    pub text: String,
    pub completed: u64,
    pub total: u64,
}
pub enum Operation {
    Inspect,
    Refresh(String),
    Install(Box<Release>),
    Open(Application),
    Uninstall(Application),
    #[cfg(windows)]
    Network(studio_network::firewall::Request),
}
pub struct Reply {
    pub installed: [Result<Option<String>, String>; 2],
    pub catalog: Option<Catalog>,
    pub outcome: Result<String, String>,
}
pub struct Worker {
    jobs: Option<mpsc::SyncSender<Operation>>,
    results: mpsc::Receiver<Reply>,
    cancel: Arc<AtomicBool>,
    progress: Arc<Mutex<Progress>>,
    join: Option<JoinHandle<()>>,
    busy: bool,
}
impl Default for Worker {
    fn default() -> Self {
        Self::new()
    }
}
impl Worker {
    pub fn new() -> Self {
        let (jobs_tx, jobs) = mpsc::sync_channel::<Operation>(1);
        let (tx, results) = mpsc::sync_channel(1);
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(Mutex::new(Progress::default()));
        let c = cancel.clone();
        let p = progress.clone();
        let join = std::thread::spawn(move || {
            let mut verified_envelope: Option<Vec<u8>> = None;
            while let Ok(operation) = jobs.recv() {
                let mut catalog = None;
                let outcome = (|| {
                    let root = install::installed_root()?;
                    let state = studio_app_lifecycle::state_directory()?;
                    match operation {
                        Operation::Inspect => {
                            for app in [Application::Visualizer, Application::Tools] {
                                if let Ok(_lease) =
                                    studio_app_lifecycle::AppLease::acquire(&state, app.id())
                                {
                                    install::recover(&root, app)?;
                                }
                            }
                            Ok("Ready".into())
                        }
                        Operation::Refresh(url) => {
                            set_progress(&p, "Checking releases", 0, 0);
                            let key = catalog::embedded_key()?;
                            let bytes = fetch_catalog(&url, &c)?;
                            catalog = Some(catalog::verify(&bytes, &key)?);
                            verified_envelope = Some(bytes);
                            Ok("Release catalog verified".into())
                        }
                        Operation::Install(release) => {
                            release.validate()?;
                            fs::create_dir_all(&state).map_err(|e| e.to_string())?;
                            // A unique temporary download belongs only to this worker. An app lease in install serializes mutations.
                            let download = state.join(format!(
                                "download-{}-{}.tar.gz",
                                std::process::id(),
                                release.application.id()
                            ));
                            let result = (|| {
                                set_progress(&p, "Downloading", 0, release.archive_bytes);
                                download_archive(&release, &download, &c, &p)?;
                                set_progress(
                                    &p,
                                    "Verifying and installing",
                                    release.archive_bytes,
                                    release.archive_bytes,
                                );
                                check_release_os(&release)?;
                                let envelope = verified_envelope
                                    .as_deref()
                                    .ok_or("Check for updates before installing")?;
                                install::install_authenticated(
                                    &root, &state, &download, &release, envelope, &c,
                                )?;
                                #[cfg(windows)]
                                let network =
                                    crate::windows_setup::offer_network(release.application, &c);
                                #[cfg(not(windows))]
                                let network = String::new();
                                Ok(format!(
                                    "{} installed. Choose Open when ready. {network}",
                                    release.application.name()
                                ))
                            })();
                            let _ = fs::remove_file(download);
                            result
                        }
                        Operation::Uninstall(app) => {
                            install::uninstall(&root, &state, app)?;
                            Ok(format!(
                                "{} uninstalled; workspaces and preferences retained",
                                app.name()
                            ))
                        }
                        #[cfg(windows)]
                        Operation::Network(mut request) => {
                            if request.adapter_id == "automatic" {
                                let preferences = studio_network::preferences::load(
                                    &studio_network::preferences::path(request.application.id())?,
                                )?;
                                let selection = if matches!(
                                    preferences.selection,
                                    studio_network::Selection::Local
                                        | studio_network::Selection::Custom { .. }
                                ) {
                                    studio_network::Selection::Lan
                                } else {
                                    preferences.selection
                                };
                                request.adapter_id = studio_network::adapters::resolve(
                                    &selection,
                                    &studio_network::adapters::enumerate()?,
                                )?
                                .ok_or("Choose a LAN adapter in the application")?
                                .0;
                            }
                            studio_network::windows::request(&request, &c)
                        }
                        Operation::Open(app) => {
                            install::open(&root, &state, app)?;
                            Ok(format!("Launch requested for {}", app.name()))
                        }
                    }
                })();
                let installed = match install::installed_root() {
                    Ok(root) => [
                        install::installed_version(&root, Application::Visualizer),
                        install::installed_version(&root, Application::Tools),
                    ],
                    Err(e) => [Err(e.clone()), Err(e)],
                };
                if tx
                    .send(Reply {
                        installed,
                        catalog,
                        outcome,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            jobs: Some(jobs_tx),
            results,
            cancel,
            progress,
            join: Some(join),
            busy: false,
        }
    }
    pub fn submit(&mut self, operation: Operation) -> Result<(), String> {
        if self.busy {
            return Err("Finish or cancel the current operation first".into());
        }
        self.cancel.store(false, Ordering::Release);
        *self.progress.lock().unwrap() = Progress::default();
        self.jobs
            .as_ref()
            .ok_or("Launcher worker closed")?
            .try_send(operation)
            .map_err(|e| e.to_string())?;
        self.busy = true;
        Ok(())
    }
    pub fn busy(&self) -> bool {
        self.busy
    }
    pub fn progress(&self) -> Progress {
        self.progress.lock().unwrap().clone()
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Release);
    }
    pub fn poll(&mut self) -> Option<Reply> {
        match self.results.try_recv() {
            Ok(reply) => {
                self.busy = false;
                Some(reply)
            }
            Err(mpsc::TryRecvError::Disconnected) if self.busy => {
                self.busy = false;
                Some(Reply {
                    installed: [Err("Worker stopped".into()), Err("Worker stopped".into())],
                    catalog: None,
                    outcome: Err("Launcher worker ended unexpectedly; reopen the launcher".into()),
                })
            }
            _ => None,
        }
    }
    pub fn shutdown(&mut self) {
        self.cancel();
        self.jobs.take();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}
impl Drop for Worker {
    fn drop(&mut self) {
        self.cancel();
        self.jobs.take();
    }
}
fn set_progress(progress: &Mutex<Progress>, text: &str, completed: u64, total: u64) {
    *progress.lock().unwrap() = Progress {
        text: text.into(),
        completed,
        total,
    };
}
fn client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("ROS-Studio-Launcher/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(900))
        .read_timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            let url = attempt.url();
            if attempt.previous().len() > 5
                || url.scheme() != "https"
                || !matches!(
                    url.host_str(),
                    Some(
                        "github.com"
                            | "release-assets.githubusercontent.com"
                            | "objects.githubusercontent.com"
                    )
                )
            {
                attempt.error("Unexpected release redirect")
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|e| e.to_string())
}
fn runtime() -> Result<tokio::runtime::Runtime, String> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| e.to_string())
}
async fn cancellable<F: std::future::Future>(
    future: F,
    cancel: &AtomicBool,
) -> Result<F::Output, String> {
    let mut future = std::pin::pin!(future);
    loop {
        install::canceled(cancel)?;
        if let Ok(result) = tokio::time::timeout(Duration::from_millis(100), future.as_mut()).await
        {
            return Ok(result);
        }
    }
}
pub fn fetch_catalog(url: &str, cancel: &AtomicBool) -> Result<Vec<u8>, String> {
    catalog::github_url(url)?;
    runtime()?.block_on(async {
        let mut response = cancellable(client()?.get(url).send(), cancel)
            .await?
            .and_then(|r| r.error_for_status())
            .map_err(|e| e.to_string())?;
        let mut bytes = Vec::new();
        while let Some(chunk) = cancellable(response.chunk(), cancel)
            .await?
            .map_err(|e| e.to_string())?
        {
            if bytes.len().saturating_add(chunk.len()) as u64 > catalog::MAX_CATALOG {
                return Err("Catalog exceeds 256 KiB".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    })
}
struct DownloadSink<'a, W: Write> {
    output: &'a mut W,
    expected: u64,
    completed: u64,
    cancel: &'a AtomicBool,
    progress: &'a Mutex<Progress>,
}
impl<W: Write> DownloadSink<'_, W> {
    fn push(&mut self, bytes: &[u8]) -> Result<(), String> {
        install::canceled(self.cancel)?;
        self.completed = self
            .completed
            .checked_add(bytes.len() as u64)
            .ok_or("Download size overflow")?;
        if self.completed > self.expected {
            return Err("Download exceeds declared archive size".into());
        }
        self.output.write_all(bytes).map_err(|e| e.to_string())?;
        set_progress(self.progress, "Downloading", self.completed, self.expected);
        Ok(())
    }
    fn finish(&self) -> Result<(), String> {
        install::canceled(self.cancel)?;
        if self.completed != self.expected {
            Err("Download was incomplete".into())
        } else {
            Ok(())
        }
    }
}
fn download_archive(
    release: &Release,
    path: &std::path::Path,
    cancel: &AtomicBool,
    progress: &Mutex<Progress>,
) -> Result<(), String> {
    runtime()?.block_on(async {
        let mut response = cancellable(client()?.get(&release.archive_url).send(), cancel)
            .await?
            .and_then(|r| r.error_for_status())
            .map_err(|e| e.to_string())?;
        if response
            .content_length()
            .is_some_and(|n| n != release.archive_bytes)
        {
            return Err("Download length does not match catalog".into());
        }
        let mut output = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|e| e.to_string())?;
        let mut sink = DownloadSink {
            output: &mut output,
            expected: release.archive_bytes,
            completed: 0,
            cancel,
            progress,
        };
        while let Some(chunk) = cancellable(response.chunk(), cancel)
            .await?
            .map_err(|e| e.to_string())?
        {
            sink.push(&chunk)?;
        }
        sink.finish()?;
        output.sync_all().map_err(|e| e.to_string())
    })
}
#[cfg(test)]
fn copy_download(
    mut reader: impl Read,
    output: &mut impl Write,
    expected: u64,
    cancel: &AtomicBool,
    progress: &Mutex<Progress>,
) -> Result<(), String> {
    let mut sink = DownloadSink {
        output,
        expected,
        completed: 0,
        cancel,
        progress,
    };
    let mut buf = [0; 65536];
    loop {
        let n = reader.read(&mut buf).map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        sink.push(&buf[..n])?;
    }
    sink.finish()
}
fn check_release_os(release: &Release) -> Result<(), String> {
    if !release.for_host() {
        return Err("This package is for another platform".into());
    }
    #[cfg(windows)]
    {
        studio_network::windows::check_build(
            release
                .windows
                .as_ref()
                .ok_or("Missing Windows metadata")?
                .minimum_build,
        )
    }
    #[cfg(not(windows))]
    check_os(&release.minimum_macos)
}
#[cfg(not(windows))]
fn check_os(minimum: &str) -> Result<(), String> {
    let out = std::process::Command::new("/usr/bin/sw_vers")
        .arg("-productVersion")
        .output()
        .map_err(|e| e.to_string())?;
    if !out.status.success() {
        return Err("Could not determine macOS version".into());
    }
    let parse = |s: &str| -> Result<Vec<u32>, String> {
        let mut parts = s
            .trim()
            .split('.')
            .map(|p| p.parse::<u32>().map_err(|e| e.to_string()))
            .collect::<Result<Vec<_>, _>>()?;
        parts.resize(3, 0);
        Ok(parts)
    };
    if parse(&String::from_utf8_lossy(&out.stdout))? < parse(minimum)? {
        return Err(format!("This release requires macOS {minimum} or later"));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pending_network_work_cancels_without_waiting_for_the_request_timeout() {
        let cancel = Arc::new(AtomicBool::new(false));
        let signal = cancel.clone();
        let thread = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            signal.store(true, Ordering::Release);
        });
        let start = std::time::Instant::now();
        assert!(runtime()
            .unwrap()
            .block_on(cancellable(std::future::pending::<()>(), &cancel))
            .is_err());
        assert!(start.elapsed() < Duration::from_secs(2));
        thread.join().unwrap();
    }
    #[test]
    fn bounded_download_rejects_truncation_overflow_cancellation_and_disk_full() {
        let progress = Mutex::new(Progress::default());
        let cancel = AtomicBool::new(false);
        let mut out = Vec::new();
        assert!(copy_download(&b"abc"[..], &mut out, 3, &cancel, &progress).is_ok());
        assert_eq!(out, b"abc");
        assert!(copy_download(&b"abc"[..], &mut Vec::new(), 4, &cancel, &progress).is_err());
        assert!(copy_download(&b"abc"[..], &mut Vec::new(), 2, &cancel, &progress).is_err());
        assert!(copy_download(
            &b"abc"[..],
            &mut Vec::new(),
            3,
            &AtomicBool::new(true),
            &progress
        )
        .is_err());
        struct Full;
        impl Write for Full {
            fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
                Err(std::io::Error::from_raw_os_error(28))
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }
        assert!(copy_download(&b"abc"[..], &mut Full, 3, &cancel, &progress).is_err());
    }
}
