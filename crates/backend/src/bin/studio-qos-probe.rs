use serde_json::json;
use std::{
    ffi::{c_char, c_void, CStr},
    time::{Duration, Instant},
};
use studio_backend::service_codec::NativeDiagnostics;
extern "C" {
    fn sq_create() -> *mut c_void;
    fn sq_destroy(p: *mut c_void);
    fn sq_error(p: *mut c_void) -> *const c_char;
    fn sq_init(p: *mut c_void, history: i32) -> i32;
    fn sq_take(p: *mut c_void, value: *mut i32) -> i32;
    fn sq_deadline(p: *mut c_void, total: *mut i32) -> i32;
    fn sq_liveliness(p: *mut c_void, alive: *mut i32, not_alive: *mut i32) -> i32;
}
struct Probe(*mut c_void);
impl Drop for Probe {
    fn drop(&mut self) {
        unsafe { sq_destroy(self.0) }
    }
}
impl Probe {
    fn check(&self, n: i32) -> Result<i32, String> {
        if n < 0 {
            Err(unsafe { CStr::from_ptr(sq_error(self.0)) }
                .to_string_lossy()
                .into_owned())
        } else {
            Ok(n)
        }
    }
}
fn run() -> Result<serde_json::Value, String> {
    if std::env::var("ROS_DOMAIN_ID").as_deref() != Ok("187")
        || std::env::var("ROS_LOCALHOST_ONLY").as_deref() != Ok("1")
    {
        return Err("Requires localhost/domain 187".into());
    }
    let mode = std::env::args().nth(1).unwrap_or_default();
    if mode != "history" && mode != "events" {
        return Err("Expected history or events".into());
    }
    let history = mode == "history";
    let p = Probe(unsafe { sq_create() });
    if p.0.is_null() {
        return Err("Allocation failed".into());
    }
    p.check(unsafe { sq_init(p.0, history as i32) })?;
    let start = Instant::now();
    let mut samples = Vec::new();
    let mut retained_drops = 0;
    let mut total = 0;
    let mut statuses = Vec::new();
    let mut last = None;
    let mut recovery_at: Option<Instant> = None;
    let mut recovered_deadline = None;
    if history {
        std::thread::sleep(Duration::from_secs(3));
    }
    while start.elapsed() < Duration::from_secs(6) {
        let mut value = 0;
        if p.check(unsafe { sq_take(p.0, &mut value) })? > 0 {
            if samples.len() < 64 {
                samples.push(value);
            } else {
                retained_drops += 1;
            }
            if value == 2 && recovery_at.is_none() {
                recovery_at = Some(Instant::now());
            }
        }
        if !history {
            let mut n = 0;
            if p.check(unsafe { sq_deadline(p.0, &mut n) })? > 0 {
                total = total.max(n);
            }
            let (mut alive, mut dead) = (0, 0);
            if p.check(unsafe { sq_liveliness(p.0, &mut alive, &mut dead) })? > 0
                && last != Some((alive, dead))
            {
                last = Some((alive, dead));
                if statuses.len() < 32 {
                    statuses.push((alive, dead));
                } else {
                    retained_drops += 1;
                }
            }
        }
        if recovered_deadline.is_none()
            && recovery_at.is_some_and(|t| t.elapsed() > Duration::from_millis(500))
        {
            recovered_deadline = Some(total);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let lost = statuses.iter().position(|x| x.1 > 0);
    let recovered = lost.is_some_and(|i| statuses.iter().skip(i + 1).any(|x| x.0 > 0 && x.1 == 0));
    let passed = retained_drops == 0
        && if history {
            samples == vec![5, 6, 7]
        } else {
            total > 0
                && recovered
                && recovered_deadline == Some(total)
                && samples.contains(&1)
                && samples.contains(&2)
        };
    Ok(
        json!({"version":1,"mode":mode,"passed":passed,"samples":samples,"retention_drops":retained_drops,"deadline_misses":total,"deadline_count_after_recovery":recovered_deadline,"liveliness_states":statuses,"recovered_after_loss":recovered,"elapsed_seconds":start.elapsed().as_secs_f64()}),
    )
}
fn main() {
    let diagnostics = NativeDiagnostics::stderr().expect("Reserve stdout");
    let result = run();
    drop(diagnostics);
    match result {
        Ok(v) => {
            println!("{v}");
            if v["passed"] != true {
                std::process::exit(1)
            }
        }
        Err(e) => {
            println!("{}", json!({"version":1,"passed":false,"error":e}));
            std::process::exit(1)
        }
    }
}
