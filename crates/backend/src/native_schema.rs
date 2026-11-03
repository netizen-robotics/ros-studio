//! Shared bounded C introspection access. Native storage stays worker-owned.
use std::ffi::{c_char, c_void, CStr};
use studio_protocol::mapping;
pub(crate) type Ptr = *mut c_void;
extern "C" {
    pub(crate) fn sp_schema(owner: Ptr, response: i32) -> Ptr;
    pub(crate) fn sp_message(owner: Ptr, response: i32) -> Ptr;
    pub(crate) fn sp_count(schema: Ptr) -> usize;
    pub(crate) fn sp_member(schema: Ptr, index: usize) -> Ptr;
    pub(crate) fn sp_name(member: Ptr) -> *const c_char;
    pub(crate) fn sp_type(member: Ptr) -> i32;
    pub(crate) fn sp_array(member: Ptr) -> i32;
    pub(crate) fn sp_bound(member: Ptr) -> usize;
    pub(crate) fn sp_nested(member: Ptr) -> Ptr;
    pub(crate) fn sp_field(message: Ptr, member: Ptr) -> Ptr;
    pub(crate) fn sp_length(member: Ptr, field: Ptr) -> usize;
    pub(crate) fn sp_element(member: Ptr, field: Ptr, index: usize) -> Ptr;
    pub(crate) fn sp_string_size(field: Ptr) -> usize;
    pub(crate) fn sp_string_data(field: Ptr) -> *const c_char;
}
pub(crate) unsafe fn schema(root: Ptr, message: Ptr, count: usize) -> Result<(), String> {
    if root.is_null() || message.is_null() || sp_count(root) != count {
        Err("Invalid native mapping schema/storage".into())
    } else {
        Ok(())
    }
}
pub(crate) unsafe fn field(
    root: Ptr,
    message: Ptr,
    name: &str,
    kind: i32,
    array: i32,
) -> Result<(Ptr, Ptr), String> {
    if root.is_null() || message.is_null() {
        return Err("Null native mapping storage".into());
    }
    let mut found = None;
    for index in 0..sp_count(root) {
        let member = sp_member(root, index);
        if member.is_null() {
            return Err("Null native mapping member".into());
        }
        let native_name = sp_name(member);
        if native_name.is_null() {
            return Err("Null native mapping member name".into());
        }
        if CStr::from_ptr(native_name).to_bytes() == name.as_bytes() {
            if found.is_some() || sp_type(member) != kind || sp_array(member) != array {
                return Err(format!("Invalid native mapping field {name}"));
            }
            let value = sp_field(message, member);
            if value.is_null() {
                return Err(format!("Missing native mapping value {name}"));
            }
            found = Some((member, value));
        }
    }
    found.ok_or_else(|| format!("Missing native mapping field {name}"))
}
pub(crate) unsafe fn string(value: Ptr) -> Result<String, String> {
    string_bounded(value, 256)
}
pub(crate) unsafe fn string_bounded(value: Ptr, maximum: usize) -> Result<String, String> {
    let count = sp_string_size(value);
    if count > maximum {
        return Err("Native string exceeds configured byte limit".into());
    }
    if count == 0 {
        return Ok(String::new());
    }
    let raw = sp_string_data(value);
    if raw.is_null() {
        return Err("Null native mapping frame string".into());
    }
    std::str::from_utf8(std::slice::from_raw_parts(raw.cast(), count))
        .map(str::to_owned)
        .map_err(|_| "Invalid UTF-8 mapping frame".into())
}
pub(crate) unsafe fn time(root: Ptr, message: Ptr) -> Result<i64, String> {
    schema(root, message, 2)?;
    let (_, seconds) = field(root, message, "sec", 13, 0)?;
    let (_, nanos) = field(root, message, "nanosec", 12, 0)?;
    let seconds = *seconds.cast::<i32>();
    let nanos = *nanos.cast::<u32>();
    if seconds < 0 || nanos >= 1_000_000_000 {
        return Err("Invalid mapping ROS timestamp".into());
    }
    Ok(i64::from(seconds) * 1_000_000_000 + i64::from(nanos))
}
pub(crate) unsafe fn header(root: Ptr, message: Ptr) -> Result<(String, i64), String> {
    schema(root, message, 2)?;
    let (member, stamp) = field(root, message, "stamp", 18, 0)?;
    let stamp = time(sp_nested(member), stamp)?;
    let (_, frame) = field(root, message, "frame_id", 16, 0)?;
    Ok((string(frame)?, stamp))
}
pub(crate) unsafe fn pose(root: Ptr, message: Ptr) -> Result<[f64; 7], String> {
    schema(root, message, 2)?;
    let (pm, position) = field(root, message, "position", 18, 0)?;
    let ps = sp_nested(pm);
    schema(ps, position, 3)?;
    let (qm, orientation) = field(root, message, "orientation", 18, 0)?;
    let qs = sp_nested(qm);
    schema(qs, orientation, 4)?;
    let mut result = [0.; 7];
    for (index, name) in ["x", "y", "z"].iter().enumerate() {
        let (_, p) = field(ps, position, name, 2, 0)?;
        result[index] = *p.cast::<f64>();
    }
    for (index, name) in ["x", "y", "z", "w"].iter().enumerate() {
        let (_, p) = field(qs, orientation, name, 2, 0)?;
        result[index + 3] = *p.cast::<f64>();
    }
    if !mapping::valid_pose(&result) {
        return Err("Invalid finite/unit native mapping pose".into());
    }
    Ok(result)
}

#[cfg(test)]
pub(crate) mod test_support {
    use super::*;
    use std::ffi::CString;
    pub(crate) fn introspection_library(package: &str) -> std::path::PathBuf {
        std::env::split_paths(&std::env::var_os("AMENT_PREFIX_PATH").expect("Use project Pixi"))
            .map(|prefix| {
                crate::probe_bundle::library(&prefix, package, "rosidl_typesupport_introspection_c")
            })
            .find(|path| path.is_file())
            .expect("Native fixture introspection library")
    }
    extern "C" {
        fn sp_create() -> Ptr;
        fn sp_destroy(owner: Ptr);
        fn sp_load_message(
            owner: Ptr,
            intro: *const c_char,
            package: *const c_char,
            name: *const c_char,
        ) -> i32;
    }
    pub(crate) struct Fixture(pub(crate) Ptr);
    impl Drop for Fixture {
        fn drop(&mut self) {
            unsafe { sp_destroy(self.0) };
        }
    }
    impl Fixture {
        pub(crate) fn new(package: &str, name: &str, value: serde_json::Value) -> Self {
            let path = introspection_library(package);
            let strings = [path.to_str().unwrap(), package, name].map(|s| CString::new(s).unwrap());
            let fixture = Self(unsafe { sp_create() });
            assert!(!fixture.0.is_null());
            assert_eq!(
                unsafe {
                    sp_load_message(
                        fixture.0,
                        strings[0].as_ptr(),
                        strings[1].as_ptr(),
                        strings[2].as_ptr(),
                    )
                },
                0
            );
            fixture.write(value);
            fixture
        }
        pub(crate) fn write(&self, value: serde_json::Value) {
            unsafe {
                crate::service_codec::native_write(
                    sp_schema(self.0, 0),
                    sp_message(self.0, 0),
                    &value,
                )
            }
            .unwrap();
        }
        pub(crate) unsafe fn root(&self, name: &str, kind: i32, array: i32) -> (Ptr, Ptr) {
            field(
                sp_schema(self.0, 0),
                sp_message(self.0, 0),
                name,
                kind,
                array,
            )
            .unwrap()
        }
    }
}
