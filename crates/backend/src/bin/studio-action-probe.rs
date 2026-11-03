use serde_json::{json, Value};
use std::{
    collections::HashMap,
    path::Path,
    time::{Duration, Instant},
};
use studio_backend::{
    action_codec::{ActionProbe, Slot},
    probe_bundle::{bounded_read, bundle, identifier, library},
    service_codec::LIMIT,
};
use uuid::Uuid;
type Error = (String, String);
fn stage(code: &str) -> impl FnOnce(String) -> Error + '_ {
    move |message| (code.into(), message)
}
fn main() {
    let mut result = json!({"version":1,"outcome":"error","goal_state":"not_sent","goal_sequence":null,"result_sequence":null,"cancel_sequence":null,"feedback":[],"feedback_count":0,"feedback_dropped":0,"foreign_feedback_ignored":0,"statuses":[]});
    let ok = match run(&mut result) {
        Ok(()) => true,
        Err((code, message)) => {
            result["error"] = json!({"code":code,"message":message});
            false
        }
    };
    let serialized = serde_json::to_vec(&result).expect("JSON output");
    if serialized.len() + 1 > LIMIT {
        println!(
            "{}",
            json!({"version":1,"outcome":"error","goal_state":result["goal_state"],"goal_id":result["goal_id"],"error":{"code":"output_limit","message":"Action result exceeds 1 MiB"}})
        );
        std::process::exit(1);
    }
    println!("{}", String::from_utf8(serialized).expect("JSON UTF-8"));
    if !ok {
        std::process::exit(1);
    }
}
fn run(out: &mut Value) -> Result<(), Error> {
    let raw: Vec<String> = std::env::args().skip(1).collect();
    let mut args = HashMap::new();
    if !raw.len().is_multiple_of(2) {
        return Err(stage("arguments")("Expected --key value pairs".into()));
    }
    for pair in raw.chunks_exact(2) {
        if ![
            "--bundle",
            "--action",
            "--type",
            "--goal-file",
            "--timeout-ms",
            "--cancel-after-feedback",
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
    let name = required("--action")?;
    let ty = required("--type")?;
    if name.len() > 256 || ty.len() > 256 {
        return Err(stage("arguments")("Action/type name too long".into()));
    }
    out["action"] = json!(name);
    out["type"] = json!(ty);
    let parts: Vec<&str> = ty.split('/').collect();
    if parts.len() != 3 || parts[1] != "action" || !identifier(parts[0]) || !identifier(parts[2]) {
        return Err(stage("arguments")("Expected PACKAGE/action/Type".into()));
    }
    let suffix = name
        .strip_prefix("/studio_fixture/")
        .ok_or_else(|| stage("fixture_guard")("Action must be under /studio_fixture/".into()))?;
    if !suffix.split('/').all(identifier) {
        return Err(stage("fixture_guard")("Invalid fixture action name".into()));
    }
    if std::env::var("ROS_DOMAIN_ID").is_ok_and(|v| v != "187")
        || std::env::var("ROS_DISTRO").is_ok_and(|v| v != env!("STUDIO_ROS_DISTRO"))
    {
        return Err(stage("fixture_guard")(
            "Action probe requires domain 187 and matching distro".into(),
        ));
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
    let cancel_after: Option<u64> = args
        .get("--cancel-after-feedback")
        .map(|v| v.parse())
        .transpose()
        .map_err(|_| stage("arguments")("Invalid feedback threshold".into()))?;
    if cancel_after == Some(0) {
        return Err(stage("arguments")(
            "Cancellation threshold must be positive".into(),
        ));
    }
    let root = bundle(Path::new(&required("--bundle")?), parts[0]).map_err(stage("bundle"))?;
    let goal: Value = serde_json::from_slice(
        &bounded_read(Path::new(&required("--goal-file")?)).map_err(stage("input"))?,
    )
    .map_err(|e| stage("input")(e.to_string()))?;
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
    for (key, value) in [
        ("ROS_DOMAIN_ID", "187"),
        ("ROS_LOCALHOST_ONLY", "1"),
        ("ROS_AUTOMATIC_DISCOVERY_RANGE", "LOCALHOST"),
        ("RCUTILS_LOGGING_USE_STDOUT", "0"),
        ("SKIP_DEFAULT_XML", "1"),
    ] {
        std::env::set_var(key, value);
    }
    let rmw = std::env::var("RMW_IMPLEMENTATION").unwrap_or_else(|_| "rmw_fastrtps_cpp".into());
    if !["rmw_fastrtps_cpp", "rmw_cyclonedds_cpp"].contains(&rmw.as_str()) {
        return Err(stage("fixture_guard")("Unsupported test middleware".into()));
    }
    std::env::set_var("RMW_IMPLEMENTATION", &rmw);
    out["rmw"] = json!(rmw);
    let mut prefixes = vec![root.clone()];
    if let Some(existing) = std::env::var_os("AMENT_PREFIX_PATH") {
        prefixes.extend(std::env::split_paths(&existing));
    }
    std::env::set_var(
        "AMENT_PREFIX_PATH",
        std::env::join_paths(prefixes).map_err(|e| stage("bundle")(e.to_string()))?,
    );
    let selected = if rmw == "rmw_fastrtps_cpp" {
        "rosidl_typesupport_fastrtps_c"
    } else {
        "rosidl_typesupport_introspection_c"
    };
    let paths = [
        library(&root, parts[0], "rosidl_typesupport_introspection_c"),
        library(&root, parts[0], selected),
        library(&root, parts[0], "rosidl_typesupport_c"),
    ];
    let _diagnostics =
        studio_backend::service_codec::NativeDiagnostics::stderr().map_err(stage("diagnostics"))?;
    let mut client = ActionProbe::load(
        [
            &paths[0].to_string_lossy(),
            &paths[1].to_string_lossy(),
            &paths[2].to_string_lossy(),
        ],
        parts[0],
        parts[2],
        selected,
    )
    .map_err(stage("type_support"))?;
    let goal_id = json!({"uuid":Uuid::new_v4().as_bytes().to_vec()});
    out["goal_id"] = goal_id.clone();
    client
        .write(Slot::GoalRequest, &json!({"goal_id":goal_id,"goal":goal}))
        .map_err(stage("validation"))?;
    client
        .write(Slot::ResultRequest, &json!({"goal_id":goal_id}))
        .map_err(stage("validation"))?;
    client
        .write(
            Slot::CancelRequest,
            &json!({"goal_info":{"goal_id":goal_id,"stamp":{"sec":0,"nanosec":0}}}),
        )
        .map_err(stage("validation"))?;
    let deadline = Instant::now() + Duration::from_millis(timeout);
    client.connect(&name).map_err(stage("connect"))?;
    loop {
        if Instant::now() >= deadline {
            return Err(stage("server_unavailable")(
                "Deadline expired; goal not sent".into(),
            ));
        }
        if client.available().map_err(stage("discovery"))? {
            break;
        }
        std::thread::sleep(
            Duration::from_millis(10).min(deadline.saturating_duration_since(Instant::now())),
        );
    }
    if Instant::now() >= deadline {
        return Err(stage("server_unavailable")(
            "Deadline expired; goal not sent".into(),
        ));
    }
    out["goal_state"] = json!("may_be_running");
    let goal_sequence = client.send(Slot::GoalRequest).map_err(stage("goal_send"))?;
    out["goal_sequence"] = json!(goal_sequence);
    let mut result_sequence = None;
    let mut cancel_sequence = None;
    let mut cancel_received = false;
    let mut accepted = false;
    let mut terminal = false;
    let mut feedback_count = 0u64;
    let mut feedback_bytes = 0usize;
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(stage("action_timeout")(
                "Deadline expired; no automatic cancel or replay; goal may still run".into(),
            ));
        }
        let ready = client
            .wait(remaining.min(Duration::from_millis(100)).as_nanos() as i64)
            .map_err(stage("wait"))?;
        for (bit, slot) in [
            (4, Slot::GoalResponse),
            (1, Slot::Feedback),
            (2, Slot::Status),
            (8, Slot::CancelResponse),
            (16, Slot::ResultResponse),
        ] {
            if ready & bit == 0 {
                continue;
            }
            let Some(sequence) = client.take(slot).map_err(stage("take"))? else {
                continue;
            };
            let value = client.read(slot).map_err(stage("decode"))?;
            match slot {
                Slot::GoalResponse if sequence == goal_sequence && !accepted => {
                    out["goal_response"] = value.clone();
                    match value["accepted"].as_bool() {
                        Some(false) => {
                            out["goal_state"] = json!("rejected");
                            out["outcome"] = json!("passed");
                            return Ok(());
                        }
                        Some(true) => {
                            accepted = true;
                            out["goal_state"] = json!("accepted");
                            let seq = client
                                .send(Slot::ResultRequest)
                                .map_err(stage("result_send"))?;
                            result_sequence = Some(seq);
                            out["result_sequence"] = json!(seq);
                        }
                        None => {
                            return Err(stage("decode")(
                                "Goal response missing accepted flag".into(),
                            ))
                        }
                    }
                }
                Slot::Feedback => {
                    if value["goal_id"] != goal_id {
                        out["foreign_feedback_ignored"] =
                            json!(out["foreign_feedback_ignored"].as_u64().unwrap_or(0) + 1);
                        continue;
                    }
                    feedback_count += 1;
                    out["feedback_count"] = json!(feedback_count);
                    let size = serde_json::to_vec(&value["feedback"])
                        .map_err(|e| stage("decode")(e.to_string()))?
                        .len();
                    if out["feedback"].as_array().unwrap().len() < 64
                        && feedback_bytes + size <= 256 * 1024
                    {
                        feedback_bytes += size;
                        out["feedback"]
                            .as_array_mut()
                            .unwrap()
                            .push(value["feedback"].clone());
                    } else {
                        out["feedback_dropped"] =
                            json!(out["feedback_dropped"].as_u64().unwrap_or(0) + 1);
                    }
                }
                Slot::Status => {
                    if let Some(statuses) = value["status_list"].as_array() {
                        for status in statuses {
                            if status["goal_info"]["goal_id"] == goal_id
                                && out["statuses"].as_array().unwrap().len() < 32
                            {
                                out["statuses"]
                                    .as_array_mut()
                                    .unwrap()
                                    .push(status["status"].clone());
                            }
                        }
                    }
                }
                Slot::CancelResponse if Some(sequence) == cancel_sequence => {
                    cancel_received = true;
                    // A successful cancel RPC does not imply our goal was accepted
                    // for cancellation: inspect both the code and goal membership.
                    out["cancel_accepted"] = json!(
                        value["return_code"] == 0
                            && value["goals_canceling"]
                                .as_array()
                                .is_some_and(|goals| goals
                                    .iter()
                                    .any(|goal| goal["goal_id"] == goal_id))
                    );
                    out["cancel_response"] = value;
                }
                Slot::ResultResponse if Some(sequence) == result_sequence => {
                    if !matches!(value["status"].as_i64(), Some(4..=6)) {
                        return Err(stage("decode")("Non-terminal action result status".into()));
                    }
                    out["goal_state"] = json!(match value["status"].as_i64().unwrap() {
                        4 => "succeeded",
                        5 => "canceled",
                        _ => "aborted",
                    });
                    out["result_response"] = value;
                    terminal = true;
                }
                _ => (),
            }
        }
        if accepted
            && !terminal
            && cancel_sequence.is_none()
            && cancel_after.is_some_and(|n| feedback_count >= n)
        {
            let seq = client
                .send(Slot::CancelRequest)
                .map_err(stage("cancel_send"))?;
            cancel_sequence = Some(seq);
            out["cancel_sequence"] = json!(seq);
        }
        if terminal && (cancel_sequence.is_none() || cancel_received) {
            out["outcome"] = json!("passed");
            return Ok(());
        }
    }
}
