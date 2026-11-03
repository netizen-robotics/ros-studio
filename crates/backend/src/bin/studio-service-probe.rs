use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::Path,
    time::{Duration, Instant},
};
use studio_backend::{
    probe_bundle::{bounded_read, bundle, identifier, library},
    service_codec::{Probe, LIMIT},
};
fn main() {
    let started = Instant::now();
    let tracing = std::env::var("STUDIO_STARTUP_TRACE").as_deref() == Ok("1");
    let trace = |name: &str| {
        if tracing {
            let unix_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as u64;
            eprintln!(
                "{}",
                json!({"startup_trace":{"stage":name,"elapsed_ms":started.elapsed().as_secs_f64()*1000.0,"unix_ms":unix_ms}})
            );
        }
    };
    trace("main");
    let mut result = json!({"version":1,"outcome":"error","service":null,"type":null,"request_sequence":null,"request_state":"not_sent"});
    let success = match run(&mut result, &trace) {
        Ok(()) => true,
        Err((code, message)) => {
            result["error"] = json!({"code":code,"message":message});
            false
        }
    };
    trace("run_finished");
    let output = serde_json::to_vec(&result).expect("JSON result serialization");
    if output.len() + 1 > LIMIT {
        result = json!({"version":1,"outcome":"error","request_state":result["request_state"],"request_sequence":result["request_sequence"],"error":{"code":"output_limit","message":"Result exceeds 1 MiB output limit"}});
        println!("{result}");
        std::process::exit(1);
    }
    println!("{}", String::from_utf8(output).expect("JSON UTF-8"));
    if !success {
        std::process::exit(1);
    }
}
type Error = (String, String);
fn stage(code: &str) -> impl FnOnce(String) -> Error + '_ {
    move |message| (code.into(), message)
}
fn run(result: &mut Value, trace: &impl Fn(&str)) -> Result<(), Error> {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut args = HashMap::new();
    if !raw.len().is_multiple_of(2) {
        return Err(stage("arguments")("Expected --key value pairs".into()));
    }
    for pair in raw.chunks_exact(2) {
        if ![
            "--bundle",
            "--service",
            "--type",
            "--request-file",
            "--timeout-ms",
        ]
        .contains(&pair[0].as_str())
            || args.insert(pair[0].clone(), pair[1].clone()).is_some()
        {
            return Err(stage("arguments")("Unknown or duplicate option".into()));
        }
    }
    let required = |key: &str| {
        args.get(key)
            .cloned()
            .ok_or_else(|| stage("arguments")(format!("Missing {key}")))
    };
    let service = required("--service")?;
    let ty = required("--type")?;
    if service.len() > 256 || ty.len() > 256 {
        return Err(stage("arguments")(
            "Service/type name exceeds 256 bytes".into(),
        ));
    }
    result["service"] = json!(service);
    result["type"] = json!(ty);
    let parts: Vec<&str> = ty.split('/').collect();
    if parts.len() != 3 || parts[1] != "srv" || !identifier(parts[0]) || !identifier(parts[2]) {
        return Err(stage("arguments")("Expected PACKAGE/srv/Type".into()));
    }
    let suffix = service
        .strip_prefix("/studio_fixture/")
        .ok_or_else(|| stage("fixture_guard")("Service must be under /studio_fixture/".into()))?;
    if !suffix.split('/').all(identifier) {
        return Err(stage("fixture_guard")(
            "Invalid fixture service name".into(),
        ));
    }
    if std::env::var("ROS_DOMAIN_ID").is_ok_and(|v| v != "187") {
        return Err(stage("fixture_guard")(
            "Service probe requires ROS_DOMAIN_ID=187".into(),
        ));
    }
    if std::env::var("ROS_DISTRO").is_ok_and(|v| v != env!("STUDIO_ROS_DISTRO")) {
        return Err(stage("fixture_guard")("Wrong backend ROS distro".into()));
    }
    let timeout: u64 = args
        .get("--timeout-ms")
        .map(String::as_str)
        .unwrap_or("10000")
        .parse()
        .map_err(|_| stage("arguments")("Invalid timeout".into()))?;
    if !(1..=60000).contains(&timeout) {
        return Err(stage("arguments")("Timeout must be 1..60000 ms".into()));
    }
    trace("bundle_validation_start");
    let root = bundle(Path::new(&required("--bundle")?), parts[0]).map_err(stage("bundle"))?;
    trace("bundle_validation_end");
    let request: Value = serde_json::from_slice(
        &bounded_read(Path::new(&required("--request-file")?)).map_err(stage("input"))?,
    )
    .map_err(|e| stage("input")(e.to_string()))?;
    // Explicit fixture isolation, regardless of inherited DDS discovery configuration.
    for key in [
        "CYCLONEDDS_URI",
        "FASTRTPS_DEFAULT_PROFILES_FILE",
        "FASTDDS_DEFAULT_PROFILES_FILE",
        "ROS_DISCOVERY_SERVER",
        "ROS_SUPER_CLIENT",
        "RMW_FASTRTPS_USE_QOS_FROM_XML",
        "ROS_STATIC_PEERS",
        "FASTDDS_ENVIRONMENT_FILE",
    ] {
        std::env::remove_var(key);
    }
    std::env::set_var("ROS_DOMAIN_ID", "187");
    std::env::set_var("SKIP_DEFAULT_XML", "1");
    std::env::set_var("ROS_LOCALHOST_ONLY", "1");
    std::env::set_var("ROS_AUTOMATIC_DISCOVERY_RANGE", "LOCALHOST");
    std::env::set_var("RCUTILS_LOGGING_USE_STDOUT", "0");
    let rmw = std::env::var("RMW_IMPLEMENTATION").unwrap_or_else(|_| "rmw_fastrtps_cpp".into());
    if !["rmw_fastrtps_cpp", "rmw_cyclonedds_cpp"].contains(&rmw.as_str()) {
        return Err(stage("fixture_guard")("Unsupported test middleware".into()));
    }
    std::env::set_var("RMW_IMPLEMENTATION", &rmw);
    result["rmw"] = json!(rmw);
    let mut prefixes = vec![root.clone()];
    if let Some(existing) = std::env::var_os("AMENT_PREFIX_PATH") {
        prefixes.extend(std::env::split_paths(&existing));
    }
    std::env::set_var(
        "AMENT_PREFIX_PATH",
        std::env::join_paths(prefixes).map_err(|e| stage("bundle")(e.to_string()))?,
    );
    let support_identifier = if rmw == "rmw_fastrtps_cpp" {
        "rosidl_typesupport_fastrtps_c"
    } else {
        "rosidl_typesupport_introspection_c"
    };
    let _diagnostics =
        studio_backend::service_codec::NativeDiagnostics::stderr().map_err(stage("diagnostics"))?;
    trace("type_support_start");
    let mut probe = Probe::load(
        &library(&root, parts[0], "rosidl_typesupport_introspection_c").to_string_lossy(),
        &library(&root, parts[0], support_identifier).to_string_lossy(),
        parts[0],
        parts[2],
        support_identifier,
    )
    .map_err(stage("type_support"))?;
    trace("type_support_end");
    probe.request(&request).map_err(stage("validation"))?;
    let deadline = Instant::now() + Duration::from_millis(timeout);
    trace("connect_start");
    probe.connect(&service).map_err(stage("connect"))?;
    trace("connect_end");
    loop {
        if Instant::now() >= deadline {
            return Err(stage("server_unavailable")(
                "Deadline expired; request not sent".into(),
            ));
        }
        if probe.available().map_err(stage("discovery"))? {
            break;
        }
        std::thread::sleep(
            Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
    if Instant::now() >= deadline {
        return Err(stage("server_unavailable")(
            "Deadline expired; request not sent".into(),
        ));
    }
    // No retry path exists. A send error is conservatively treated as an uncertain delivery.
    result["request_state"] = json!("may_have_executed");
    let sequence = probe.send().map_err(stage("send"))?;
    result["request_sequence"] = json!(sequence);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(stage("response_timeout")(
                "Deadline expired after send; request may have executed".into(),
            ));
        }
        if let Some(received) = probe
            .take(remaining.min(Duration::from_millis(100)).as_nanos() as i64)
            .map_err(stage("response"))?
        {
            if received == sequence {
                result["request_state"] = json!("response_received");
                result["response"] = probe.response().map_err(stage("response_decode"))?;
                result["outcome"] = json!("passed");
                return Ok(());
            }
        }
    }
}
