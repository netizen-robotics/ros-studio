//! Development-only actual-App health journey shared by the two root tests.
use crate::diagnostic_health::{Monitor, Phase, MESSAGE_TYPE};
use std::{
    path::PathBuf,
    time::{Duration, Instant},
};
pub trait Host {
    fn poll(&mut self);
    fn monitor(&self) -> &Monitor;
    fn topics(&self) -> &[(String, Vec<String>)];
    fn start(&mut self, topic: &str, best: bool, transient: bool);
    fn stop(&mut self);
    fn disconnect(&mut self);
    fn connect(&mut self);
    fn connected(&self) -> bool;
}
fn wait(host: &mut impl Host, label: &str, timeout: u64, predicate: impl Fn(&dyn Host) -> bool) {
    let deadline = Instant::now() + Duration::from_secs(timeout);
    loop {
        host.poll();
        if predicate(host) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "{label}: {:?} {:?}",
            host.monitor().phase,
            host.monitor().error
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}
pub fn guard() -> PathBuf {
    for (key, value) in [
        ("ROS_DOMAIN_ID", "187"),
        ("ROS_LOCALHOST_ONLY", "1"),
        ("ROS_AUTOMATIC_DISCOVERY_RANGE", "LOCALHOST"),
    ] {
        assert_eq!(std::env::var(key).as_deref(), Ok(value));
    }
    assert!(matches!(
        std::env::var("RMW_IMPLEMENTATION").as_deref(),
        Ok("rmw_fastrtps_cpp" | "rmw_cyclonedds_cpp")
    ));
    for key in [
        "ROS_STATIC_PEERS",
        "ROS_DISCOVERY_SERVER",
        "ROS_SUPER_CLIENT",
        "CYCLONEDDS_URI",
        "FASTDDS_DEFAULT_PROFILES_FILE",
        "FASTRTPS_DEFAULT_PROFILES_FILE",
        "FASTDDS_ENVIRONMENT_FILE",
    ] {
        assert!(
            std::env::var(key).unwrap_or_default().is_empty(),
            "Remove remote/custom discovery setting {key}"
        );
    }
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    assert_eq!(
        PathBuf::from(std::env::var("STUDIO_BACKEND").unwrap()),
        root.join("scripts/ui-ros-backend")
    );
    let out = PathBuf::from(std::env::var("STUDIO_UI_ROS_EVIDENCE").unwrap());
    assert!(out.is_absolute() && out.starts_with(root.join("artifacts/feasibility/evidence")));
    out
}
/// Declare before App so cancellation runs before waiting, including panic paths.
pub struct Finish;
impl Drop for Finish {
    fn drop(&mut self) {
        crate::session::finish_shutdown();
    }
}
pub fn run(host: &mut impl Host, name: &str) {
    let out = guard();
    wait(host, "discovery", 35, |a| {
        a.connected()
            && a.topics().iter().any(|(t, types)| {
                t == "/studio_fixture/diagnostics" && types.iter().any(|t| t == MESSAGE_TYPE)
            })
    });
    assert!(!host.monitor().running());
    assert_eq!(host.monitor().samples, 0);
    host.start("/studio_fixture/diagnostics", false, false);
    wait(host, "reliable reception", 20, |a| {
        !a.monitor().components.is_empty()
    });
    let component = host.monitor().components.values().next().unwrap();
    assert_eq!(component.level, 1);
    assert_eq!(component.message, "PRIVATE_DIAGNOSTIC_MESSAGE");
    assert!(!host.monitor().summary().to_string().contains("PRIVATE"));
    std::fs::write(out.join("pause-diagnostics"), b"pause").unwrap();
    wait(host, "local staleness", 10, |a| {
        a.monitor()
            .components
            .values()
            .all(|c| a.monitor().stale(c, Instant::now()))
    });
    std::fs::remove_file(out.join("pause-diagnostics")).unwrap();
    wait(host, "stale recovery", 10, |a| {
        a.monitor()
            .components
            .values()
            .all(|c| !a.monitor().stale(c, Instant::now()))
    });
    host.stop();
    wait(host, "acknowledged stop", 15, |a| !a.monitor().running());
    assert!(host.monitor().components.is_empty());
    host.start("/studio_fixture/diagnostics_best", false, false);
    wait(host, "QoS waiting", 10, |a| {
        a.monitor().phase == Phase::Waiting
    });
    let until = Instant::now() + Duration::from_secs(2);
    while Instant::now() < until {
        host.poll();
        assert!(host.monitor().components.is_empty());
        std::thread::sleep(Duration::from_millis(10));
    }
    host.stop();
    wait(host, "mismatch stop", 15, |a| !a.monitor().running());
    host.start("/studio_fixture/diagnostics_best", true, false);
    wait(host, "QoS correction", 15, |a| {
        !a.monitor().components.is_empty()
    });
    host.stop();
    wait(host, "best stop", 15, |a| !a.monitor().running());
    host.start("/studio_fixture/diagnostics_retained", false, true);
    wait(host, "retained sample", 20, |a| {
        !a.monitor().components.is_empty()
    });
    host.disconnect();
    assert!(!host.monitor().running());
    assert!(host.monitor().components.is_empty());
    crate::session::finish_shutdown();
    host.poll();
    host.connect();
    wait(host, "reconnect", 35, |a| a.connected());
    let until = Instant::now() + Duration::from_secs(1);
    while Instant::now() < until {
        host.poll();
        assert!(!host.monitor().running());
        assert!(host.monitor().components.is_empty());
        std::thread::sleep(Duration::from_millis(10));
    }
    host.disconnect();
    crate::session::finish_shutdown();
    std::fs::write(out.join(format!("{name}-health-result.json")),serde_json::to_vec_pretty(&serde_json::json!({
        "outcome":"passed","app":name,"scope":"Actual App + Rust backend; no native window claim","rmw":std::env::var("RMW_IMPLEMENTATION").unwrap(),
        "cases":["passive discovery","reliable receive","local stale and recovery","stop acknowledgement","QoS mismatch/correction","retained sample","disconnect cleanup","inactive reconnect"],
        "os":std::env::consts::OS,"cpu":std::env::consts::ARCH,"ros":"jazzy","session":20
    })).unwrap()).unwrap();
}
