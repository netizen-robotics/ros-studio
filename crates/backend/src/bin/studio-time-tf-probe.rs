//! Local domain-187 fixture proof. Dynamic ROS subscriptions feed standard tf2 and rcl time.
use rclrs::*;
use serde_json::{json, Value as Json};
use std::{
    collections::BTreeSet,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc,
    },
    time::{Duration, Instant},
};
use studio_backend::{service_codec::NativeDiagnostics, transform_buffer::TransformBuffer};
fn decode(v: Value<'_>, depth: usize) -> Result<Json, String> {
    if depth > 16 {
        return Err("Fixture nesting limit".into());
    }
    Ok(match v {
        Value::Simple(SimpleValue::Int32(x)) => json!(*x),
        Value::Simple(SimpleValue::Uint32(x)) => json!(*x),
        Value::Simple(SimpleValue::Double(x)) if x.is_finite() => json!(*x),
        Value::Simple(SimpleValue::String(x)) => json!(x.to_string()),
        Value::Simple(SimpleValue::Message(m)) => {
            let mut out = serde_json::Map::new();
            for (k, v) in m.iter() {
                out.insert(k.into(), decode(v, depth + 1)?);
            }
            Json::Object(out)
        }
        Value::Sequence(SequenceValue::MessageSequence(s)) => {
            if s.as_slice().len() > 128 {
                return Err("Fixture sequence limit".into());
            }
            let mut out = Vec::new();
            for m in s.as_slice() {
                let mut fields = serde_json::Map::new();
                for (k, v) in m.iter() {
                    fields.insert(k.into(), decode(v, depth + 1)?);
                }
                out.push(Json::Object(fields));
            }
            Json::Array(out)
        }
        _ => return Err("Unexpected fixture field kind".into()),
    })
}
fn ns(v: &Json) -> Result<i64, String> {
    let sec = v["sec"].as_i64().ok_or("Missing seconds")?;
    let nano = v["nanosec"].as_i64().ok_or("Missing nanoseconds")?;
    if sec < 0 || !(0..1_000_000_000).contains(&nano) {
        return Err("Invalid fixture timestamp".into());
    }
    Ok(sec * 1_000_000_000 + nano)
}
fn run() -> Result<Json, Box<dyn std::error::Error>> {
    if std::env::var("ROS_DOMAIN_ID").as_deref() != Ok("187")
        || std::env::var("ROS_LOCALHOST_ONLY").as_deref() != Ok("1")
    {
        return Err("Requires localhost/domain 187".into());
    }
    let context = Context::default_from_env()?;
    let mut executor = context.create_basic_executor();
    let node = executor.create_node("studio_time_tf_fixture")?;
    let (tx, rx) = mpsc::sync_channel(128);
    let drops = Arc::new(AtomicU64::new(0));
    let mut subscriptions = Vec::new();
    for (topic, ty, key, qos) in [
        (
            "/clock",
            "rosgraph_msgs/msg/Clock",
            "clock",
            QoSProfile::topics_default().best_effort(),
        ),
        (
            "/tf",
            "tf2_msgs/msg/TFMessage",
            "transforms",
            QoSProfile::topics_default(),
        ),
        (
            "/tf_static",
            "tf2_msgs/msg/TFMessage",
            "transforms",
            QoSProfile::topics_default().transient_local(),
        ),
    ] {
        let sender = tx.clone();
        let dropped = drops.clone();
        subscriptions.push(node.create_dynamic_subscription(
            ty.try_into()?,
            topic.qos(qos),
            move |msg: DynamicMessage, _: MessageInfo| {
                let value = msg
                    .get(key)
                    .ok_or_else(|| "Missing fixture field".to_string())
                    .and_then(|v| decode(v, 0));
                if sender.try_send((topic, value)).is_err() {
                    dropped.fetch_add(1, Ordering::Relaxed);
                }
            },
        )?);
    }
    let mut buffer = TransformBuffer::new()?;
    let (clock, source) = Clock::with_source();
    let mut last = None;
    let mut last_update = Instant::now();
    let start = Instant::now();
    let deadline = start + Duration::from_secs(9);
    let mut checks = BTreeSet::new();
    if clock.now().nsec == 0 {
        checks.insert("sim-clock-zero-before-first-sample");
    }
    let mut clocks = BTreeSet::new();
    let mut resets = 0;
    let mut errors = Vec::new();
    let mut static_received = 0;
    while Instant::now() < deadline {
        for error in executor.spin(SpinOptions::default().timeout(Duration::from_millis(20))) {
            if !error.is_timeout() {
                return Err(error.into());
            }
        }
        for (topic, value) in rx.try_iter().take(128) {
            let value = match value {
                Ok(v) => v,
                Err(e) => {
                    if errors.len() < 64 {
                        errors.push(e);
                    }
                    continue;
                }
            };
            if topic == "/clock" {
                let time = ns(&value)?;
                if last.is_some_and(|old| time < old) {
                    buffer.clear()?;
                    resets += 1;
                    if buffer.lookup("map", "sensor", 0).is_err() {
                        checks.insert("backward-jump-clears-dynamic");
                    }
                    if buffer.lookup("map", "odom", 0).is_ok_and(|v| v[0] == 10.) {
                        checks.insert("backward-jump-retains-static");
                    }
                }
                source.set_ros_time_override(time);
                last = Some(time);
                last_update = Instant::now();
                clocks.insert(time);
                if clock.now().nsec == time {
                    checks.insert("ros-clock-follows-topic");
                }
            } else {
                if topic == "/tf_static" {
                    static_received += 1;
                }
                for t in value.as_array().ok_or("Expected transforms")? {
                    let parent = t["header"]["frame_id"].as_str().ok_or("Missing frame")?;
                    let child = t["child_frame_id"].as_str().ok_or("Missing child")?;
                    let stamp = ns(&t["header"]["stamp"])?;
                    let transform = &t["transform"];
                    let mut v = [0.; 7];
                    for (i, (group, key)) in [
                        ("translation", "x"),
                        ("translation", "y"),
                        ("translation", "z"),
                        ("rotation", "x"),
                        ("rotation", "y"),
                        ("rotation", "z"),
                        ("rotation", "w"),
                    ]
                    .iter()
                    .enumerate()
                    {
                        v[i] = transform[group][key]
                            .as_f64()
                            .ok_or("Missing transform component")?;
                    }
                    if let Err(e) = buffer.insert(parent, child, stamp, v, topic == "/tf_static") {
                        if errors.len() < 64 {
                            errors.push(e);
                        }
                    }
                }
            }
        }
        if let Some(time) = last {
            if last_update.elapsed() > Duration::from_millis(150) && clock.now().nsec == time {
                checks.insert("paused-clock-stays-fixed");
            }
            let lookup = buffer.lookup("map", "sensor", time);
            if time == 10_000_000_000 && lookup.as_ref().is_ok_and(|v| (v[0] - 13.).abs() < 1e-9) {
                checks.insert("static-late-join-and-dynamic-composition");
            }
            if time == 2_000_000_000
                && resets == 1
                && lookup.as_ref().is_ok_and(|v| (v[0] - 19.).abs() < 1e-9)
            {
                checks.insert("new-epoch-transform");
            }
            if time == 20_000_000_000 && lookup.is_err() {
                checks.insert("forward-clock-no-stale-transform");
            }
        }
    }
    let elapsed = start.elapsed().as_secs_f64();
    if (9.0..11.0).contains(&elapsed) {
        checks.insert("monotonic-deadline-during-paused-clock");
    }
    let passed = checks.len() == 9
        && clocks == BTreeSet::from([2_000_000_000, 10_000_000_000, 20_000_000_000])
        && resets == 1
        && errors.is_empty()
        && drops.load(Ordering::Relaxed) == 0
        && static_received > 0;
    Ok(
        json!({"version":1,"passed":passed,"checks":checks,"clocks":clocks,"resets":resets,"static_messages":static_received,"drops":drops.load(Ordering::Relaxed),"errors":errors,"elapsed_seconds":elapsed}),
    )
}
fn main() {
    let diagnostics = NativeDiagnostics::stderr().expect("Reserve stdout");
    let result = run();
    drop(diagnostics);
    match result {
        Ok(value) => {
            println!("{value}");
            if value["passed"] != true {
                std::process::exit(1)
            }
        }
        Err(e) => {
            println!(
                "{}",
                json!({"version":1,"passed":false,"error":e.to_string()})
            );
            std::process::exit(1)
        }
    }
}
