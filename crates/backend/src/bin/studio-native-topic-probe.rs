//! Synthetic topic echo through native rcl; never used by the desktop launcher.
use serde_json::json;
use std::ffi::{c_char, c_void, CStr, CString};
use std::time::{Duration, Instant};
use studio_backend::service_codec::NativeDiagnostics;
extern "C" {
    fn tp_create() -> *mut c_void;
    fn tp_destroy(p: *mut c_void);
    fn tp_connect(
        p: *mut c_void,
        intro: *const c_char,
        dispatch: *const c_char,
        package: *const c_char,
        name: *const c_char,
        identifier: *const c_char,
        best: i32,
    ) -> i32;
    fn tp_echo(p: *mut c_void, timeout_ns: i64) -> i32;
    fn sp_error(p: *mut c_void) -> *const c_char;
}
struct Topic(*mut c_void);
impl Drop for Topic {
    fn drop(&mut self) {
        unsafe { tp_destroy(self.0) };
    }
}
impl Topic {
    fn check(&self, code: i32) -> Result<i32, String> {
        if code < 0 {
            Err(unsafe { CStr::from_ptr(sp_error(self.0)) }
                .to_string_lossy()
                .into_owned())
        } else {
            Ok(code)
        }
    }
}
fn run() -> Result<u64, String> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let duration = if let Some(index) = args.iter().position(|arg| arg == "--duration-ms") {
        let value = args
            .get(index + 1)
            .ok_or("Missing duration")?
            .parse::<u64>()
            .map_err(|e| e.to_string())?;
        if !(1..=28000).contains(&value) {
            return Err("Duration must be 1..28000 ms".into());
        }
        args.drain(index..=index + 1);
        Duration::from_millis(value)
    } else {
        Duration::from_secs(28)
    };
    if !(args.len() == 3 || args.len() == 4)
        || args[0] != "/studio_fixture"
        || args[2] != "--fixture-echo"
        || (args.len() == 4 && args[3] != "--best-effort")
    {
        return Err("Usage: studio-native-topic-probe /studio_fixture PACKAGE/msg/TYPE --fixture-echo [--best-effort]".into());
    }
    if std::env::var("ROS_DOMAIN_ID").as_deref() != Ok("187")
        || std::env::var("ROS_LOCALHOST_ONLY").as_deref() != Ok("1")
        || std::env::var("ROS_DISTRO").as_deref() != Ok(env!("STUDIO_ROS_DISTRO"))
    {
        return Err("Requires matching ROS_DISTRO, domain187 and localhost".into());
    }
    let parts: Vec<_> = args[1].split('/').collect();
    if parts.len() != 3
        || parts[1] != "msg"
        || parts
            .iter()
            .any(|p| p.is_empty() || !p.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
    {
        return Err("Expected PACKAGE/msg/TYPE".into());
    }
    let prefixes = std::env::var_os("AMENT_PREFIX_PATH").ok_or("Missing runtime prefix")?;
    let prefix = std::env::split_paths(&prefixes)
        .next()
        .ok_or("Empty runtime prefix")?;
    let rmw = std::env::var("RMW_IMPLEMENTATION").map_err(|e| e.to_string())?;
    let identifier = match rmw.as_str() {
        "rmw_fastrtps_cpp" => "rosidl_typesupport_fastrtps_c",
        "rmw_cyclonedds_cpp" => "rosidl_typesupport_introspection_c",
        _ => return Err("Only fixture Fast DDS/Cyclone DDS supported".into()),
    };
    let library = |id: &str| {
        prefix
            .join("lib")
            .join(format!(
                "lib{}__{}.{}",
                parts[0],
                id,
                std::env::consts::DLL_EXTENSION
            ))
            .to_string_lossy()
            .into_owned()
    };
    let strings = [
        library("rosidl_typesupport_introspection_c"),
        library(identifier),
        parts[0].into(),
        parts[2].into(),
        identifier.into(),
    ]
    .into_iter()
    .map(|v| CString::new(v).map_err(|e| e.to_string()))
    .collect::<Result<Vec<_>, _>>()?;
    let _diagnostics = NativeDiagnostics::stderr()?;
    let topic = Topic(unsafe { tp_create() });
    if topic.0.is_null() {
        return Err("Native allocation failed".into());
    }
    topic.check(unsafe {
        tp_connect(
            topic.0,
            strings[0].as_ptr(),
            strings[1].as_ptr(),
            strings[2].as_ptr(),
            strings[3].as_ptr(),
            strings[4].as_ptr(),
            i32::from(args.len() == 4),
        )
    })?;
    let deadline = Instant::now() + duration;
    let mut count = 0;
    while Instant::now() < deadline {
        let received = topic.check(unsafe { tp_echo(topic.0, 100_000_000) })?;
        if received == 1 && count == 0 {
            let preview = unsafe { studio_backend::service_codec::native_preview(topic.0) }?;
            eprintln!("{}", json!({"native_preview":preview}));
        }
        count += received as u64;
    }
    Ok(count)
}
fn main() {
    let result = run();
    println!(
        "{}",
        json!({"version":1,"distro":env!("STUDIO_ROS_DISTRO"),"result":result})
    );
    if result.is_err() {
        std::process::exit(1);
    }
}
