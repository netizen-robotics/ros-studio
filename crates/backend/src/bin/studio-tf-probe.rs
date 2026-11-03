//! Deterministic, offline native tf2 buffer fixture. No ROS endpoints or robot access.
use serde_json::json;
use studio_backend::{service_codec::NativeDiagnostics, transform_buffer::TransformBuffer};
fn main() {
    let _diagnostics = NativeDiagnostics::stderr().expect("Reserve JSON stdout");
    let mut tf = TransformBuffer::new().expect("tf2 buffer");
    let mut checks = Vec::new();
    let identity = |x: f64| [x, 0., 0., 0., 0., 0., 1.];
    tf.insert("map", "odom", 0, identity(10.), true).unwrap();
    tf.insert("odom", "base", 1_000_000_000, identity(0.), false)
        .unwrap();
    tf.insert("odom", "base", 3_000_000_000, identity(4.), false)
        .unwrap();
    tf.insert("base", "sensor", 0, identity(1.), true).unwrap();
    for (name, target, source, time, x) in [
        ("interpolated-chain", "map", "sensor", 2_000_000_000, 13.),
        ("latest-chain", "map", "sensor", 0, 15.),
        ("inverse-chain", "sensor", "map", 2_000_000_000, -13.),
    ] {
        let actual = tf.lookup(target, source, time);
        checks.push(json!({"test":name,"passed":actual.as_ref().is_ok_and(|v|(v[0]-x).abs()<1e-9 && v[1].abs()<1e-9 && v[2].abs()<1e-9 && (v[6]-1.).abs()<1e-9),"actual":actual,"expected_x":x}));
    }
    let q = std::f64::consts::FRAC_1_SQRT_2;
    tf.insert("map", "rotated", 0, [0., 0., 0., 0., 0., q, q], true)
        .unwrap();
    tf.insert("rotated", "tip", 0, identity(2.), true).unwrap();
    let actual = tf.lookup("map", "tip", 0);
    checks.push(json!({"test":"rotation-composition","passed":actual.as_ref().is_ok_and(|v|v[0].abs()<1e-9 && (v[1]-2.).abs()<1e-9 && (v[5]-q).abs()<1e-9),"actual":actual}));
    tf.insert("map", "turn", 1_000_000_000, identity(0.), false)
        .unwrap();
    tf.insert(
        "map",
        "turn",
        3_000_000_000,
        [0., 0., 0., 0., 0., 1., 0.],
        false,
    )
    .unwrap();
    let actual = tf.lookup("map", "turn", 2_000_000_000);
    checks.push(json!({"test":"quaternion-interpolation","passed":actual.as_ref().is_ok_and(|v|(v[5].abs()-q).abs()<1e-9 && (v[6].abs()-q).abs()<1e-9),"actual":actual}));
    tf.insert("island", "other", 0, identity(0.), true).unwrap();
    for (name, target, source, time) in [
        ("past-extrapolation", "map", "sensor", 500_000_000),
        ("future-extrapolation", "map", "sensor", 4_000_000_000),
        ("unknown-frame", "map", "missing", 0),
        ("disconnected-tree", "map", "other", 0),
    ] {
        let actual = tf.lookup(target, source, time);
        let expected = match name {
            "past-extrapolation" => "extrapolation into the past",
            "future-extrapolation" => "extrapolation into the future",
            "unknown-frame" => "does not exist",
            _ => "unconnected trees",
        };
        checks.push(json!({"test":name,"passed":actual.as_ref().is_err_and(|e|e.contains(expected)),"expected_error":expected,"actual":actual}));
    }
    for (name, parent, child, v) in [
        ("invalid-quaternion", "map", "bad", [0.; 7]),
        ("self-transform", "map", "map", identity(0.)),
        ("nan-transform", "map", "nan", identity(f64::NAN)),
    ] {
        let actual = tf.insert(parent, child, 0, v, true);
        checks.push(json!({"test":name,"passed":actual.is_err(),"actual":actual}));
    }
    tf.clear().unwrap();
    let actual = tf.lookup("map", "sensor", 2_000_000_000);
    checks.push(
        json!({"test":"clear-removes-dynamic-history","passed":actual.is_err(),"actual":actual}),
    );
    let actual = tf.lookup("map", "odom", 0);
    checks.push(json!({"test":"clear-preserves-static-transform","passed":actual.as_ref().is_ok_and(|v|v[0]==10.),"actual":actual}));
    tf.insert("odom", "base", 500_000_000, identity(8.), false)
        .unwrap();
    let actual = tf.lookup("map", "sensor", 500_000_000);
    checks.push(json!({"test":"new-epoch-after-clear","passed":actual.as_ref().is_ok_and(|v|v[0]==19.),"actual":actual}));
    let passed = checks.iter().all(|x| x["passed"] == true);
    drop(tf);
    drop(_diagnostics);
    println!(
        "{}",
        json!({"version":1,"scope":"offline native tf2 buffer; no /tf or /clock delivery tested","passed":passed,"checks":checks})
    );
    if !passed {
        std::process::exit(1)
    }
}
