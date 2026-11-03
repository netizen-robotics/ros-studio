//! Owning action-client wrapper. Custom goal/result/feedback layouts stay opaque.
use crate::service_codec::{native_read, native_schema, native_write};
use serde_json::Value;
use std::ffi::{c_char, c_void, CStr, CString};
type Ptr = *mut c_void;
extern "C" {
    fn ap_create() -> Ptr;
    fn ap_destroy(p: Ptr);
    fn ap_error(p: Ptr) -> *const c_char;
    fn ap_load(
        p: Ptr,
        intro: *const c_char,
        selected: *const c_char,
        generic: *const c_char,
        package: *const c_char,
        name: *const c_char,
        identifier: *const c_char,
    ) -> i32;
    fn ap_schema(p: Ptr, slot: i32) -> Ptr;
    fn ap_message(p: Ptr, slot: i32) -> Ptr;
    fn ap_connect(p: Ptr, name: *const c_char) -> i32;
    fn ap_connect_at(p: Ptr, name: *const c_char, domain: usize) -> i32;
    fn ap_available(p: Ptr) -> i32;
    fn ap_send(p: Ptr, slot: i32, sequence: *mut i64) -> i32;
    fn ap_wait(p: Ptr, timeout_ns: i64) -> i32;
    fn ap_take(p: Ptr, slot: i32, sequence: *mut i64) -> i32;
}
#[derive(Clone, Copy)]
#[repr(i32)]
pub enum Slot {
    GoalRequest = 0,
    GoalResponse = 1,
    ResultRequest = 2,
    ResultResponse = 3,
    CancelRequest = 4,
    CancelResponse = 5,
    Feedback = 6,
    Status = 7,
}
pub struct ActionProbe(Ptr);
impl Drop for ActionProbe {
    fn drop(&mut self) {
        unsafe { ap_destroy(self.0) }
    }
}
impl ActionProbe {
    pub fn load(
        paths: [&str; 3],
        package: &str,
        name: &str,
        identifier: &str,
    ) -> Result<Self, String> {
        let p = Self(unsafe { ap_create() });
        if p.0.is_null() {
            return Err("Action allocation failed".into());
        }
        let strings: Vec<CString> = [paths[0], paths[1], paths[2], package, name, identifier]
            .iter()
            .map(|s| CString::new(*s).map_err(|e| e.to_string()))
            .collect::<Result<_, _>>()?;
        p.check(unsafe {
            ap_load(
                p.0,
                strings[0].as_ptr(),
                strings[1].as_ptr(),
                strings[2].as_ptr(),
                strings[3].as_ptr(),
                strings[4].as_ptr(),
                strings[5].as_ptr(),
            )
        })?;
        for slot in 0..8 {
            unsafe {
                native_schema(ap_schema(p.0, slot))?;
            }
        }
        Ok(p)
    }
    fn check(&self, code: i32) -> Result<i32, String> {
        if code < 0 {
            Err(unsafe { CStr::from_ptr(ap_error(self.0)) }
                .to_string_lossy()
                .into_owned())
        } else {
            Ok(code)
        }
    }
    pub fn write(&mut self, slot: Slot, value: &Value) -> Result<(), String> {
        if !matches!(
            slot,
            Slot::GoalRequest | Slot::ResultRequest | Slot::CancelRequest
        ) {
            return Err("Not a request slot".into());
        }
        unsafe {
            native_write(
                ap_schema(self.0, slot as i32),
                ap_message(self.0, slot as i32),
                value,
            )
        }
    }
    pub fn read(&self, slot: Slot) -> Result<Value, String> {
        unsafe {
            native_read(
                ap_schema(self.0, slot as i32),
                ap_message(self.0, slot as i32),
            )
        }
    }
    pub fn connect(&mut self, name: &str) -> Result<(), String> {
        let name = CString::new(name).map_err(|e| e.to_string())?;
        self.check(unsafe { ap_connect(self.0, name.as_ptr()) })
            .map(|_| ())
    }
    pub fn connect_at(&mut self, name: &str, domain: usize) -> Result<(), String> {
        let name = CString::new(name).map_err(|e| e.to_string())?;
        self.check(unsafe { ap_connect_at(self.0, name.as_ptr(), domain) })
            .map(|_| ())
    }
    pub fn describe(&self, slot: Slot) -> Result<Value, String> {
        unsafe {
            crate::service_codec::describe_schema(ap_schema(self.0, slot as i32), 0, &mut 4096)
        }
    }
    pub fn available(&self) -> Result<bool, String> {
        self.check(unsafe { ap_available(self.0) }).map(|v| v == 1)
    }
    pub fn send(&mut self, slot: Slot) -> Result<i64, String> {
        let mut sequence = 0;
        self.check(unsafe { ap_send(self.0, slot as i32, &mut sequence) })?;
        Ok(sequence)
    }
    pub fn wait(&mut self, timeout_ns: i64) -> Result<i32, String> {
        self.check(unsafe { ap_wait(self.0, timeout_ns) })
    }
    pub fn take(&mut self, slot: Slot) -> Result<Option<i64>, String> {
        let mut sequence = 0;
        let code = self.check(unsafe { ap_take(self.0, slot as i32, &mut sequence) })?;
        Ok((code == 1).then_some(sequence))
    }
}
