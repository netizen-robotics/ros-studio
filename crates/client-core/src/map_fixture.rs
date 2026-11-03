//! Development-only local synthetic map fixture lifecycle, shared by app and
//! transport acceptance. Never included without the explicit test-support feature.
use serde_json::Value;
use std::{
    fs::File,
    io::{BufRead, BufReader, Write},
    path::Path,
    process::{Child, ChildStdin, Command as Process, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};
pub struct Managed {
    pub child: Child,
    pub input: Option<ChildStdin>,
}
impl Managed {
    pub fn spawn(mut command: Process) -> Self {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .unwrap();
        let input = child.stdin.take();
        Self { child, input }
    }
    pub fn finish(&mut self) {
        self.input = None;
        let until = Instant::now() + Duration::from_secs(15);
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                assert!(status.success(), "Child {}: {status}", self.child.id());
                return;
            }
            assert!(
                Instant::now() < until,
                "Child shutdown timeout {}",
                self.child.id()
            );
            thread::sleep(Duration::from_millis(5));
        }
    }
}
impl Drop for Managed {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
/// Shared bounded-command synthetic publisher harness for local acceptance.
pub struct FixturePublisher {
    pub process: Managed,
    output: mpsc::Receiver<Value>,
    reader: Option<thread::JoinHandle<()>>,
}
/// Compatibility name for existing Map acceptance callers.
pub type MapPublisher = FixturePublisher;
impl FixturePublisher {
    pub fn new(root: &Path, prefix: &Path, folder: &Path) -> Self {
        Self::start(root, prefix, folder, false)
    }
    /// Explicit developer qualification with reviewed repository DDS fixtures.
    /// The normal constructor and runtime builds never enable these profiles.
    pub fn with_diagnostic_profile(root: &Path, prefix: &Path, folder: &Path) -> Self {
        Self::start(root, prefix, folder, true)
    }
    fn start(root: &Path, prefix: &Path, folder: &Path, diagnostic: bool) -> Self {
        let mut command = Process::new(prefix.join("bin/python"));
        command
            .arg(root.join("crates/backend/tests/fixtures/map_publisher.py"))
            .stderr(File::create(folder.join("publisher.stderr.log")).unwrap());
        if diagnostic {
            command
                .arg("--diagnostic-profile")
                .env("STUDIO_MAP_DIAGNOSTIC_PROFILES", "1")
                .env("STUDIO_UI_ROS_EVIDENCE", folder);
        }
        Self::from_command(command, folder)
    }
    pub fn from_command(command: Process, folder: &Path) -> Self {
        let mut process = Managed::spawn(command);
        let stdout = process.child.stdout.take().unwrap();
        let (tx, output) = mpsc::channel::<Value>();
        let reader = thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                if tx
                    .send(serde_json::from_str(&line.unwrap()).unwrap())
                    .is_err()
                {
                    break;
                }
            }
        });
        let ready = output.recv_timeout(Duration::from_secs(20)).unwrap();
        std::fs::write(
            folder.join("publisher-ready.json"),
            serde_json::to_vec_pretty(&ready).unwrap(),
        )
        .unwrap();
        assert_eq!(ready["ready"], true);
        assert_eq!(ready["pid"], process.child.id());
        Self {
            process,
            output,
            reader: Some(reader),
        }
    }
    pub fn send(&mut self, command: &str) -> Value {
        writeln!(self.process.input.as_mut().unwrap(), "{command}").unwrap();
        self.output.recv_timeout(Duration::from_secs(20)).unwrap()
    }
    pub fn no_sources(&mut self) -> Value {
        let until = Instant::now() + Duration::from_secs(15);
        loop {
            let counts = self.send("counts");
            if ["full", "updates", "full_best", "updates_best", "static_tf"]
                .iter()
                .all(|key| counts[*key] == 0)
            {
                return counts;
            }
            assert!(Instant::now() < until, "Sources still allocated: {counts}");
            thread::sleep(Duration::from_millis(50));
        }
    }
    pub fn finish(&mut self) {
        self.process.finish();
        self.reader.take().unwrap().join().unwrap();
    }
}
