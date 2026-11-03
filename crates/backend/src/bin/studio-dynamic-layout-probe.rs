//! Offline regression probe: no ROS context, discovery, publishers or subscribers.
use rclrs::DynamicMessage;
use serde_json::json;
use studio_backend::service_codec::NativeMessage;

fn main() {
    let mut rows = Vec::new();
    for (typ, field) in [
        ("std_msgs/msg/String", "data"),
        ("sensor_msgs/msg/Image", "data"),
        ("sensor_msgs/msg/PointCloud2", "data"),
        ("sensor_msgs/msg/JointState", "position"),
    ] {
        let result = std::panic::catch_unwind(|| -> Result<String, String> {
            let message = DynamicMessage::new(typ.try_into().map_err(|e| format!("{e}"))?)
                .map_err(|e| e.to_string())?;
            let value = message.get(field).ok_or("Missing field")?;
            Ok(format!("{value:?}"))
        });
        let (passed, detail) = match result {
            Ok(Ok(value)) => (true, value),
            Ok(Err(error)) => (false, error),
            Err(payload) => (
                false,
                payload
                    .downcast_ref::<String>()
                    .cloned()
                    .or_else(|| payload.downcast_ref::<&str>().map(|s| s.to_string()))
                    .unwrap_or_else(|| "dynamic field access panicked".into()),
            ),
        };
        rows.push(json!({"path":"rclrs","type":typ,"field":field,"passed":passed,"detail":detail}));
        let native = (|| -> Result<serde_json::Value, String> {
            let parts: Vec<_> = typ.split('/').collect();
            let prefixes = std::env::var_os("AMENT_PREFIX_PATH").ok_or("Missing runtime prefix")?;
            let prefix = std::env::split_paths(&prefixes)
                .next()
                .ok_or("Empty runtime prefix")?;
            let library = prefix.join("lib").join(format!(
                "lib{}__rosidl_typesupport_introspection_c.{}",
                parts[0],
                std::env::consts::DLL_EXTENSION
            ));
            let mut message = NativeMessage::load(
                library.to_str().ok_or("Non UTF-8 path")?,
                parts[0],
                parts[2],
            )?;
            let expected = if typ == "std_msgs/msg/String" {
                json!("native round trip")
            } else if field == "position" {
                json!([-2.5, 0.0, 8.25])
            } else {
                json!([0, 1, 127, 255])
            };
            let request = serde_json::Value::Object(
                [(field.to_string(), expected.clone())]
                    .into_iter()
                    .collect(),
            );
            message.write(&request)?;
            let actual = message.read()?;
            if actual[field] != expected {
                return Err(format!("Unexpected field: {actual}"));
            }
            Ok(actual)
        })();
        rows.push(json!({"path":"native-introspection","type":typ,"field":field,"passed":native.is_ok(),"detail":native}));
    }
    let passed = rows.iter().all(|row| row["passed"] == true);
    println!(
        "{}",
        json!({"version":1,"distro":env!("STUDIO_ROS_DISTRO"),"scope":"offline field inspection and native small-value round trip; no DDS","passed":passed,"checks":rows})
    );
    if !passed {
        std::process::exit(1);
    }
}
