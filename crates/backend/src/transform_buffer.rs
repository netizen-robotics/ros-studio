//! Native tf2 ownership; all frame lookup and interpolation use standard ROS tf2.
use std::ffi::{c_char, c_void, CStr, CString};
extern "C" {
    fn stf_create() -> *mut c_void;
    fn stf_destroy(p: *mut c_void);
    fn stf_error(p: *mut c_void) -> *const c_char;
    fn stf_clear(p: *mut c_void) -> i32;
    fn stf_set(
        p: *mut c_void,
        parent: *const c_char,
        child: *const c_char,
        stamp: i64,
        value: *const f64,
        is_static: i32,
    ) -> i32;
    fn stf_lookup(
        p: *mut c_void,
        target: *const c_char,
        source: *const c_char,
        stamp: i64,
        value: *mut f64,
    ) -> i32;
    fn stf_lookup_latest(
        p: *mut c_void,
        target: *const c_char,
        source: *const c_char,
        value: *mut f64,
        resolved_stamp: *mut i64,
    ) -> i32;
}
pub struct TransformBuffer(*mut c_void);
impl Drop for TransformBuffer {
    fn drop(&mut self) {
        unsafe { stf_destroy(self.0) }
    }
}
impl TransformBuffer {
    pub fn new() -> Result<Self, String> {
        let p = unsafe { stf_create() };
        if p.is_null() {
            Err("Could not create tf2 buffer".into())
        } else {
            Ok(Self(p))
        }
    }
    fn check(&self, code: i32) -> Result<(), String> {
        if code == 0 {
            Ok(())
        } else {
            Err(unsafe { CStr::from_ptr(stf_error(self.0)) }
                .to_string_lossy()
                .into_owned())
        }
    }
    /// Clear dynamic history for a clock reset, retaining accepted static transforms.
    pub fn clear(&mut self) -> Result<(), String> {
        self.check(unsafe { stf_clear(self.0) })
    }
    pub fn insert(
        &mut self,
        parent: &str,
        child: &str,
        stamp: i64,
        value: [f64; 7],
        is_static: bool,
    ) -> Result<(), String> {
        if stamp < 0 || stamp / 1_000_000_000 > i32::MAX as i64 {
            return Err("Transform stamp outside ROS Time range".into());
        }
        let parent = CString::new(parent).map_err(|e| e.to_string())?;
        let child = CString::new(child).map_err(|e| e.to_string())?;
        self.check(unsafe {
            stf_set(
                self.0,
                parent.as_ptr(),
                child.as_ptr(),
                stamp,
                value.as_ptr(),
                is_static as i32,
            )
        })
    }
    /// A zero stamp requests the latest transform, as in tf2.
    pub fn lookup(&mut self, target: &str, source: &str, stamp: i64) -> Result<[f64; 7], String> {
        if stamp < 0 {
            return Err("Negative lookup stamp".into());
        }
        let target = CString::new(target).map_err(|e| e.to_string())?;
        let source = CString::new(source).map_err(|e| e.to_string())?;
        let mut result = [0.; 7];
        self.check(unsafe {
            stf_lookup(
                self.0,
                target.as_ptr(),
                source.as_ptr(),
                stamp,
                result.as_mut_ptr(),
            )
        })?;
        Ok(result)
    }
    /// Latest available native transform plus its actual resolved stamp. This
    /// explicit Robot seam does not change positive acquisition-time lookups.
    pub fn lookup_latest(&mut self, target: &str, source: &str) -> Result<([f64; 7], i64), String> {
        let target = CString::new(target).map_err(|e| e.to_string())?;
        let source = CString::new(source).map_err(|e| e.to_string())?;
        let mut result = [0.; 7];
        let mut resolved_stamp = 0;
        self.check(unsafe {
            stf_lookup_latest(
                self.0,
                target.as_ptr(),
                source.as_ptr(),
                result.as_mut_ptr(),
                &mut resolved_stamp,
            )
        })?;
        if resolved_stamp < 0 || resolved_stamp / 1_000_000_000 > i32::MAX as i64 {
            return Err("Native resolved stamp outside ROS Time range".into());
        }
        Ok((result, resolved_stamp))
    }
}
