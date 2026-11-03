//! Persistent observation process lifecycle and bounded command IO, off the UI thread.
use crate::{backend_pipe, bundled_distro};
use std::{
    io::Write,
    path::PathBuf,
    process::{Command as ProcessCommand, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Condvar, Mutex,
    },
    time::{Duration, Instant},
};
use studio_protocol::session::Command;

static PROCESS_SLOT: Mutex<()> = Mutex::new(());
pub use studio_network::ConnectionConfig;
pub fn inherited_config(domain: u16, cyclone: bool) -> ConnectionConfig {
    ConnectionConfig::inherited(domain, cyclone)
}
// Count before spawn: acquiring PROCESS_SLOT alone can miss a queued launch.
static WORKERS: Mutex<usize> = Mutex::new(0);
static WORKERS_FINISHED: Condvar = Condvar::new();
pub fn finish_shutdown() {
    let mut workers = WORKERS.lock().unwrap_or_else(|error| error.into_inner());
    while *workers != 0 {
        workers = WORKERS_FINISHED
            .wait(workers)
            .unwrap_or_else(|error| error.into_inner());
    }
}
struct WorkerGuard(Arc<AtomicBool>);
impl Drop for WorkerGuard {
    fn drop(&mut self) {
        let mut workers = WORKERS.lock().unwrap_or_else(|error| error.into_inner());
        *workers -= 1;
        self.0.store(true, Ordering::Release);
        WORKERS_FINISHED.notify_all();
    }
}
pub struct Session {
    finished: Arc<AtomicBool>,
    cancel: Arc<AtomicBool>,
    commands: mpsc::SyncSender<Command>,
    pub shared: Arc<Mutex<backend_pipe::Shared>>,
    /// Process spawn result. Protocol Ready still requires separate validation.
    pub ready: mpsc::Receiver<Result<(), String>>,
    pub terminal: mpsc::Receiver<String>,
}
impl Session {
    pub fn completion(&self) -> Arc<AtomicBool> {
        self.finished.clone()
    }
    /// Exercise App state transitions through the real ingress path without a child.
    #[cfg(any(test, feature = "test-support"))]
    pub fn fixture(prior_generation: u64) -> (Self, mpsc::Receiver<Command>) {
        let (commands, outgoing) =
            mpsc::sync_channel(studio_protocol::subscriptions::COMMAND_QUEUE_CAPACITY);
        let (_, ready) = mpsc::sync_channel(1);
        let (_, terminal) = mpsc::sync_channel(1);
        (
            Self {
                finished: Arc::new(AtomicBool::new(true)),
                cancel: Arc::new(AtomicBool::new(false)),
                commands,
                shared: Arc::new(Mutex::new(backend_pipe::Shared::new(prior_generation))),
                ready,
                terminal,
            },
            outgoing,
        )
    }
    pub fn send(&self, command: Command) -> Result<(), String> {
        if self.cancel.load(Ordering::Relaxed) {
            return Err("Session is stopping".into());
        }
        self.commands
            .try_send(command)
            .map_err(|error| match error {
                mpsc::TrySendError::Full(_) => {
                    "Command queue full; disconnect and reconnect explicitly".into()
                }
                mpsc::TrySendError::Disconnected(_) => {
                    "Backend command writer closed; reconnect explicitly".into()
                }
            })
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub fn launch(config: studio_network::ConnectionConfig, prior_generation: u64) -> Session {
    let cancel = Arc::new(AtomicBool::new(false));
    let shared = Arc::new(Mutex::new(backend_pipe::Shared::new(prior_generation)));
    let (ready_tx, ready) = mpsc::sync_channel(1);
    let (terminal_tx, terminal) = mpsc::sync_channel(1);
    let (commands, command_rx) =
        mpsc::sync_channel::<Command>(studio_protocol::subscriptions::COMMAND_QUEUE_CAPACITY);
    let finished = Arc::new(AtomicBool::new(false));
    let completed = finished.clone();
    let stopped = cancel.clone();
    let state = shared.clone();
    *WORKERS.lock().unwrap_or_else(|error| error.into_inner()) += 1;
    std::thread::spawn(move || {
        let _worker = WorkerGuard(completed);
        let _slot = PROCESS_SLOT
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if stopped.load(Ordering::Relaxed) {
            let _ = ready_tx.try_send(Err("Startup canceled".into()));
            return;
        }
        // The profile must outlive every child IO worker and the native process.
        let profile = match studio_network::profiles::Profile::prepare(&config) {
            Ok(profile) => profile,
            Err(error) => {
                let _ = ready_tx.try_send(Err(error));
                return;
            }
        };
        let result = command(
            config.domain,
            config.middleware == studio_network::Middleware::Cyclone,
        )
        .and_then(|mut cmd| {
            profile.apply(&mut cmd);
            if stopped.load(Ordering::Relaxed) {
                return Err("Startup canceled".into());
            }
            cmd.spawn()
                .map_err(|e| format!("Could not start ROS backend: {e}"))
        });
        let mut child = match result {
            Ok(child) => child,
            Err(error) => {
                let _ = ready_tx.try_send(Err(error));
                return;
            }
        };
        let reader = backend_pipe::spawn(
            child.stdout.take().expect("piped stdout"),
            state,
            terminal_tx.clone(),
        );
        let stdin = child.stdin.take().expect("piped stdin");
        let writer_cancel = stopped.clone();
        let writer_terminal = terminal_tx.clone();
        let writer = std::thread::spawn(move || {
            let mut stdin = stdin;
            while !writer_cancel.load(Ordering::Relaxed) {
                match command_rx.recv_timeout(Duration::from_millis(20)) {
                    Ok(command) => {
                        let result = command.encode().and_then(|bytes| {
                            stdin
                                .write_all(&bytes)
                                .and_then(|()| stdin.flush())
                                .map_err(|error| error.to_string())
                        });
                        if let Err(error) = result {
                            let _ = writer_terminal.try_send(format!(
                                "Backend command write failed: {error}; reconnect explicitly"
                            ));
                            break;
                        }
                    }
                    Err(mpsc::RecvTimeoutError::Timeout) => {}
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                }
            }
            // Dropping stdin requests orderly native-handle cleanup through EOF.
        });
        if ready_tx.try_send(Ok(())).is_err() {
            stopped.store(true, Ordering::Relaxed);
        }
        let mut shutdown_started = None;
        let mut network_checked = Instant::now();
        loop {
            if !stopped.load(Ordering::Relaxed)
                && network_checked.elapsed() >= Duration::from_secs(1)
            {
                network_checked = Instant::now();
                if let Err(error) = profile.still_available() {
                    let _ = terminal_tx.try_send(error);
                    stopped.store(true, Ordering::Relaxed);
                }
            }
            match child.try_wait() {
                Ok(Some(_)) => break,
                Err(error) => {
                    let _ =
                        terminal_tx.try_send(format!("Could not inspect backend process: {error}"));
                    break;
                }
                Ok(None) => {}
            }
            if stopped.load(Ordering::Relaxed) || writer.is_finished() || reader.is_finished() {
                stopped.store(true, Ordering::Relaxed);
                let start = *shutdown_started.get_or_insert_with(Instant::now);
                if start.elapsed() >= Duration::from_secs(2) {
                    let _ = child.kill();
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        stopped.store(true, Ordering::Relaxed);
        let _ = child.wait();
        let _ = writer.join();
        let _ = reader.join();
    });
    Session {
        finished,
        cancel,
        commands,
        shared,
        ready,
        terminal,
    }
}

fn command(domain: u16, cyclone: bool) -> Result<ProcessCommand, String> {
    let executable = std::env::current_exe().map_err(|e| e.to_string())?;
    let root = executable.parent().ok_or("Missing executable directory")?;
    let path = std::env::var_os("STUDIO_BACKEND")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            root.join(if cfg!(windows) {
                "studio-backend.exe"
            } else {
                "studio-backend"
            })
        });
    let mut cmd = ProcessCommand::new(path);
    cmd.env("ROS_DOMAIN_ID", domain.to_string());
    if let Some(bundle) = [
        Some(root.join("runtime")),
        root.parent().map(|p| p.join("Resources")),
        root.parent().map(PathBuf::from),
    ]
    .into_iter()
    .flatten()
    .find(|candidate| candidate.join("share/ament_index").is_dir())
    {
        let manifest =
            std::fs::read_to_string(bundle.join("manifest.json")).map_err(|e| e.to_string())?;
        cmd.env("ROS_DISTRO", bundled_distro(&manifest)?)
            .env("AMENT_PREFIX_PATH", &bundle)
            .env("RCUTILS_LOGGING_USE_STDOUT", "0");
    }
    #[cfg(windows)]
    if root.join("runtime/bin").is_dir() {
        let mut paths = vec![root.join("runtime/bin")];
        paths.extend(std::env::split_paths(
            &std::env::var_os("PATH").unwrap_or_default(),
        ));
        cmd.env(
            "PATH",
            std::env::join_paths(paths).map_err(|e| e.to_string())?,
        );
    }
    cmd.env(
        "RMW_IMPLEMENTATION",
        if cyclone {
            "rmw_cyclonedds_cpp"
        } else {
            "rmw_fastrtps_cpp"
        },
    );
    cmd.arg("--observe-session")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    Ok(cmd)
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn session_arguments_are_explicit_and_never_replay_topic_configuration() {
        for cyclone in [false, true] {
            let cmd = command(187, cyclone).unwrap();
            assert_eq!(cmd.get_args().collect::<Vec<_>>(), ["--observe-session"]);
            assert!(cmd
                .get_envs()
                .any(|(key, value)| key == "ROS_DOMAIN_ID" && value.unwrap() == "187"));
            assert!(cmd
                .get_envs()
                .any(|(key, value)| key == "RMW_IMPLEMENTATION"
                    && value.unwrap()
                        == if cyclone {
                            "rmw_cyclonedds_cpp"
                        } else {
                            "rmw_fastrtps_cpp"
                        }));
        }
    }
    #[test]
    fn command_queue_failure_is_synchronous_and_does_not_hide_pressure() {
        use studio_protocol::subscriptions::{Generation, RequestId, SubscriptionId};
        let (s, _rx) = Session::fixture(0);
        let stop = Command::Stop {
            generation: Generation(1),
            request: RequestId(1),
            subscription: SubscriptionId(1),
        };
        for _ in 0..studio_protocol::subscriptions::COMMAND_QUEUE_CAPACITY {
            s.send(stop.clone()).unwrap();
        }
        assert!(s.send(stop).unwrap_err().contains("full"));
    }
}
