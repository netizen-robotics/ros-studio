use rclrs::*;
use std::{
    io::{self, Write},
    sync::mpsc,
    time::Duration,
};
use studio_protocol::{Event, VERSION};
fn main() {
    if let Err(e) = run() {
        let _ = writeln!(
            io::stdout().lock(),
            "{}",
            serde_json::to_string(&Event::Error {
                message: studio_protocol::session::bounded_error(&format!("{e:?}"))
            })
            .unwrap()
        );
        std::process::exit(1);
    }
}
fn run() -> Result<(), Box<dyn std::error::Error>> {
    let built_distro = env!("STUDIO_ROS_DISTRO");
    if std::env::var("ROS_DISTRO").is_ok_and(|requested| requested != built_distro) {
        return Err(format!(
            "This backend was built for {built_distro}; select a matching runtime"
        )
        .into());
    }
    let mut args: Vec<String> = std::env::args().collect();
    if args[1..] == ["--observe-session"] {
        return studio_backend::observation::run(built_distro).map_err(Into::into);
    }
    if let Some(index) = args.iter().position(|arg| arg == "--interface-bundle") {
        if index < 3 || index + 1 >= args.len() {
            return Err("--interface-bundle requires TOPIC PACKAGE/msg/TYPE and a path".into());
        }
        let path = args.remove(index + 1);
        args.remove(index);
        let parts: Vec<_> = args[2].split('/').collect();
        if parts.len() != 3
            || parts[1] != "msg"
            || !parts
                .iter()
                .all(|part| studio_backend::probe_bundle::identifier(part))
        {
            return Err("Expected PACKAGE/msg/TYPE for interface bundle".into());
        }
        let root = studio_backend::probe_bundle::bundle(std::path::Path::new(&path), parts[0])?;
        let mut prefixes = vec![root];
        if let Some(existing) = std::env::var_os("AMENT_PREFIX_PATH") {
            prefixes.extend(std::env::split_paths(&existing));
        }
        // Validate and set paths before creating ROS contexts or worker threads.
        std::env::set_var("AMENT_PREFIX_PATH", std::env::join_paths(prefixes)?);
    }
    if !(args.len() == 1
        || (args.len() >= 3
            && args[3..].iter().all(|arg| {
                matches!(
                    arg.as_str(),
                    "--fixture-echo" | "--best-effort" | "--transient-local"
                )
            })))
    {
        return Err("Usage: studio-backend [TOPIC PACKAGE/msg/TYPE [--best-effort] [--transient-local] [--fixture-echo] [--interface-bundle PATH]]".into());
    }
    let context = Context::default_from_env()?;
    let mut executor = context.create_basic_executor();
    let node = executor.create_node("studio_feasibility")?;
    let (tx, rx) = mpsc::sync_channel::<Event>(128);
    std::thread::spawn(move || {
        let stdout = io::stdout();
        let mut out = stdout.lock();
        for event in rx {
            if writeln!(out, "{}", serde_json::to_string(&event).unwrap())
                .and_then(|_| out.flush())
                .is_err()
            {
                break;
            }
        }
    });
    tx.send(Event::Hello {
        version: VERSION,
        distro: built_distro.into(),
        rmw: std::env::var("RMW_IMPLEMENTATION").unwrap_or_default(),
    })?;
    let native_reader = args.len() >= 3;
    let fixture_echo = args[1..].iter().any(|arg| arg == "--fixture-echo");
    if fixture_echo
        && (args[1] != "/studio_fixture" || std::env::var("ROS_DOMAIN_ID").as_deref() != Ok("187"))
    {
        return Err("Fixture echo requires /studio_fixture on domain 187".into());
    }
    if native_reader {
        eprintln!("Using native rcl topic reader for structured dynamic-field compatibility");
        let sender = tx.clone();
        let topic = args[1].clone();
        let typ = args[2].clone();
        let best = args[3..].iter().any(|arg| arg == "--best-effort");
        let transient = args[3..].iter().any(|arg| arg == "--transient-local");
        std::thread::spawn(move || {
            let result = (|| -> Result<(), String> {
                let mut reader = studio_backend::native_topic::NativeTopic::open(
                    &topic,
                    &typ,
                    best,
                    transient,
                    fixture_echo,
                )?;
                let mut dropped = 0u64;
                loop {
                    let Some(value) = reader.poll_typed()? else {
                        continue;
                    };
                    let sample = studio_protocol::structured::Sample {
                        contract_version: studio_protocol::structured::CONTRACT_VERSION,
                        topic: topic.clone(),
                        message_type: typ.clone(),
                        source_time: None,
                        value,
                    };
                    // Validate escaped wire growth before queue admission.
                    sample.encode()?;
                    match sender.try_send(Event::TypedSample {
                        sample: Box::new(sample),
                        dropped_samples: dropped,
                    }) {
                        Ok(()) => (),
                        Err(mpsc::TrySendError::Full(_)) => {
                            dropped = dropped.saturating_add(1);
                        }
                        Err(mpsc::TrySendError::Disconnected(_)) => return Ok(()),
                    }
                }
            })();
            if let Err(message) = result {
                let _ = sender.send(Event::Error { message });
            }
        });
    }
    let graph_node = node.clone();
    std::thread::spawn(move || loop {
        match graph_node.get_topic_names_and_types() {
            Ok(topics) => {
                let _ = tx.try_send(Event::Graph {
                    topics: topics.into_iter().collect(),
                });
            }
            Err(e) => {
                let _ = tx.try_send(Event::Error {
                    message: e.to_string(),
                });
            }
        }
        std::thread::sleep(Duration::from_secs(1));
    });
    executor.spin(SpinOptions::default()).first_error()?;
    Ok(())
}
