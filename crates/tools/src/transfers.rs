//! Bounded local transfer workers. Joins happen only after active UI frames end.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    mpsc, Condvar, Mutex,
};
static WORKERS: Mutex<usize> = Mutex::new(0);
static FINISHED: Condvar = Condvar::new();
static CLOSING: AtomicBool = AtomicBool::new(false);
struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        let mut workers = WORKERS.lock().unwrap_or_else(|e| e.into_inner());
        *workers -= 1;
        FINISHED.notify_all();
    }
}
pub fn is_idle() -> bool {
    *WORKERS.lock().unwrap_or_else(|e| e.into_inner()) == 0
}
pub fn request_shutdown() {
    CLOSING.store(true, Ordering::Release);
}
pub fn finish_shutdown() {
    let mut workers = WORKERS.lock().unwrap_or_else(|e| e.into_inner());
    while *workers != 0 {
        workers = FINISHED.wait(workers).unwrap_or_else(|e| e.into_inner());
    }
}
pub fn start(path: String, save: Option<String>) -> mpsc::Receiver<Result<String, String>> {
    let (tx, rx) = mpsc::sync_channel(1);
    let mut workers = WORKERS.lock().unwrap_or_else(|e| e.into_inner());
    if *workers >= 16 || CLOSING.load(Ordering::Acquire) {
        let _ = tx.send(Err(
            "File worker unavailable during cleanup or at capacity".into()
        ));
        return rx;
    }
    *workers += 1;
    drop(workers);
    std::thread::spawn(move || {
        let _guard = Guard;
        let result = (|| {
            if !std::path::Path::new(&path).is_absolute() {
                return Err("Choose an absolute YAML file path".into());
            }
            if CLOSING.load(Ordering::Acquire) {
                return Err("File transfer cancelled during shutdown".into());
            }
            if let Some(data) = save {
                if data.len() > studio_protocol::tools::MAX_PAYLOAD {
                    return Err("YAML exceeds64KiB".into());
                }
                studio_workspace::workspace::save_local_bytes(
                    std::path::Path::new(&path),
                    data.as_bytes(),
                )
                .map_err(|e| e.to_string())?;
                Ok(data)
            } else {
                let data = studio_workspace::workspace::read_local_bytes(
                    std::path::Path::new(&path),
                    studio_protocol::tools::MAX_PAYLOAD,
                )
                .map_err(|e| e.to_string())?;
                if data.len() > studio_protocol::tools::MAX_PAYLOAD {
                    return Err("YAML exceeds64KiB".into());
                }
                String::from_utf8(data).map_err(|e| e.to_string())
            }
        })();
        let _ = tx.send(result);
    });
    rx
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn oversized_transfer_preserves_existing_file_and_roundtrips_exact_yaml() {
        let path = std::env::temp_dir().join(format!("tools-yaml-{}.yaml", std::process::id()));
        std::fs::write(&path, "original").unwrap();
        let p = path.to_string_lossy().into_owned();
        assert!(start(
            p.clone(),
            Some("x".repeat(studio_protocol::tools::MAX_PAYLOAD + 1))
        )
        .recv()
        .unwrap()
        .is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "original");
        let text = "/test:\n  ros__parameters:\n    exact: 9223372036854775807\n";
        assert_eq!(
            start(p.clone(), Some(text.into())).recv().unwrap().unwrap(),
            text
        );
        assert_eq!(start(p, None).recv().unwrap().unwrap(), text);
        std::fs::remove_file(path).unwrap();
    }
}
