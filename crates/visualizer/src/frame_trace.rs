//! Test-only attribution of the production App frame; no runtime telemetry.
use std::time::Instant;

pub(super) fn thread_seconds() -> f64 {
    #[cfg(unix)]
    {
        let mut time = std::mem::MaybeUninit::<libc::timespec>::uninit();
        // SAFETY: a valid writable timespec; read only after successful syscall.
        assert_eq!(
            unsafe { libc::clock_gettime(libc::CLOCK_THREAD_CPUTIME_ID, time.as_mut_ptr()) },
            0
        );
        let time = unsafe { time.assume_init() };
        time.tv_sec as f64 + time.tv_nsec as f64 / 1e9
    }
    #[cfg(not(unix))]
    {
        0.
    }
}

pub(super) struct Trace {
    wall: Instant,
    cpu: f64,
}
impl Trace {
    pub(super) fn new() -> Self {
        Self {
            wall: Instant::now(),
            cpu: thread_seconds(),
        }
    }
    pub(super) fn lap(&mut self) -> [f64; 2] {
        let wall = Instant::now();
        let cpu = thread_seconds();
        let result = [
            wall.duration_since(self.wall).as_secs_f64() * 1000.,
            (cpu - self.cpu) * 1000.,
        ];
        self.wall = wall;
        self.cpu = cpu;
        result
    }
}
