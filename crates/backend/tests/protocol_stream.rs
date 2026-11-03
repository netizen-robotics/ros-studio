//! Descriptor changes are process-wide, so exercise them only in child processes.
use std::{
    io::{BufRead, Read, Write},
    process::{Command, Stdio},
};
use studio_backend::service_codec::NativeDiagnostics;

#[test]
fn stream_child() {
    let Ok(mode) = std::env::var("STUDIO_STREAM_ISOLATION_CHILD") else {
        return;
    };
    let mut wire = NativeDiagnostics::stderr().unwrap();
    if mode == "broken" {
        eprintln!("CHILD_READY");
        let mut ready = [0];
        std::io::stdin().read_exact(&mut ready).unwrap();
        let failed = wire.write_all(b"{\"event\":\"ready\"}\n").is_err();
        // Avoid the test harness writing its own result into the closed pipe.
        std::process::exit(if failed { 0 } else { 2 });
    }
    std::io::stdout()
        .write_all(b"rust descriptor diagnostic\n")
        .unwrap();
    extern "C" {
        fn puts(value: *const std::ffi::c_char) -> i32;
    }
    // Exercise C stdio buffering too: Drop must flush it before restoring fd1.
    unsafe { puts(c"native buffered diagnostic".as_ptr()) };
    wire.write_all(b"{\"event\":\"ready\"}\n").unwrap();
    wire.flush().unwrap();
    drop(wire);
    println!("restored stdout");
}

#[test]
fn vendor_diagnostics_cannot_corrupt_protocol_and_closed_wire_returns_error() {
    let executable = std::env::current_exe().unwrap();
    let output = Command::new(&executable)
        .args(["--exact", "stream_child", "--nocapture"])
        .env("STUDIO_STREAM_ISOLATION_CHILD", "normal")
        .output()
        .unwrap();
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8(output.stdout).unwrap();
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stdout.contains("{\"event\":\"ready\"}\n"),
        "stdout={stdout:?}; stderr={stderr:?}"
    );
    assert!(stdout.contains("restored stdout"));
    assert!(!stdout.contains("diagnostic"), "{stdout}");
    assert!(stderr.contains("rust descriptor diagnostic"));
    assert!(stderr.contains("native buffered diagnostic"));

    let mut child = Command::new(executable)
        .args(["--exact", "stream_child", "--nocapture"])
        .env("STUDIO_STREAM_ISOLATION_CHILD", "broken")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut ready = String::new();
    std::io::BufReader::new(child.stderr.take().unwrap())
        .read_line(&mut ready)
        .unwrap();
    assert_eq!(ready.trim(), "CHILD_READY");
    drop(child.stdout.take());
    child.stdin.take().unwrap().write_all(b"x").unwrap();
    assert!(child.wait().unwrap().success());
}
