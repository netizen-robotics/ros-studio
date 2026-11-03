//! Complete String extraction, independent of the generic diagnostic preview.
use std::ffi::{c_char, c_void, CStr};
use studio_protocol::robot_description::MAX_DOCUMENT_BYTES;
type Ptr = *mut c_void;
extern "C" {
    fn sp_schema(owner: Ptr, response: i32) -> Ptr;
    fn sp_message(owner: Ptr, response: i32) -> Ptr;
    fn sp_count(schema: Ptr) -> usize;
    fn sp_member(schema: Ptr, index: usize) -> Ptr;
    fn sp_name(member: Ptr) -> *const c_char;
    fn sp_type(member: Ptr) -> i32;
    fn sp_array(member: Ptr) -> i32;
    fn sp_field(message: Ptr, member: Ptr) -> Ptr;
    fn sp_string_size(field: Ptr) -> usize;
    fn sp_string_data(field: Ptr) -> *const c_char;
}

/// Copy one complete bounded `std_msgs/msg/String` from C-owned storage.
///
/// # Safety
/// `owner` must be a live Probe/TopicProbe with standard C introspection metadata
/// and initialized message storage, exclusively owned by this worker throughout
/// the call. Only the C getters know the native struct layout.
pub unsafe fn native_description(owner: Ptr) -> Result<String, String> {
    if owner.is_null() {
        return Err("Null native description owner".into());
    }
    let schema = sp_schema(owner, 0);
    let message = sp_message(owner, 0);
    if schema.is_null() || message.is_null() || sp_count(schema) != 1 {
        return Err("Description requires exact std_msgs/msg/String schema".into());
    }
    let member = sp_member(schema, 0);
    if member.is_null() {
        return Err("Null native description member".into());
    }
    let name = sp_name(member);
    if name.is_null()
        || CStr::from_ptr(name).to_bytes() != b"data"
        || sp_type(member) != 16
        || sp_array(member) != 0
    {
        return Err("Description requires one scalar String data field".into());
    }
    let field = sp_field(message, member);
    if field.is_null() {
        return Err("Null native description field".into());
    }
    let size = sp_string_size(field);
    if !(1..=MAX_DOCUMENT_BYTES).contains(&size) {
        return Err("Robot description must contain 1 byte to 1 MiB".into());
    }
    // Validate native size before accessing or allocating document storage.
    let data = sp_string_data(field);
    if data.is_null() {
        return Err("Null native description storage".into());
    }
    std::str::from_utf8(std::slice::from_raw_parts(data.cast(), size))
        .map(str::to_owned)
        .map_err(|_| "Robot description is not UTF-8".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::CString;
    extern "C" {
        fn sp_create() -> Ptr;
        fn sp_destroy(owner: Ptr);
        fn sp_load_message(
            owner: Ptr,
            intro: *const c_char,
            package: *const c_char,
            name: *const c_char,
        ) -> i32;
        fn sp_string_set(field: Ptr, data: *const c_char, size: usize) -> i32;
    }
    struct Fixture(Ptr);
    impl Drop for Fixture {
        fn drop(&mut self) {
            unsafe { sp_destroy(self.0) };
        }
    }
    impl Fixture {
        fn load(name: &str) -> Self {
            let path = crate::native_schema::test_support::introspection_library("std_msgs");
            let strings =
                [path.to_str().unwrap(), "std_msgs", name].map(|s| CString::new(s).unwrap());
            let owner = Self(unsafe { sp_create() });
            assert!(!owner.0.is_null());
            assert_eq!(
                unsafe {
                    sp_load_message(
                        owner.0,
                        strings[0].as_ptr(),
                        strings[1].as_ptr(),
                        strings[2].as_ptr(),
                    )
                },
                0
            );
            owner
        }
        fn set(&mut self, bytes: &[u8]) {
            unsafe {
                let field = sp_field(sp_message(self.0, 0), sp_member(sp_schema(self.0, 0), 0));
                assert_ne!(sp_string_set(field, bytes.as_ptr().cast(), bytes.len()), 0);
            }
        }
    }
    #[test]
    fn full_native_string_exceeds_preview_without_truncation_and_max_is_exact() {
        let mut owner = Fixture::load("String");
        for length in [65_537, MAX_DOCUMENT_BYTES] {
            let text = "a".repeat(length - 2) + "é";
            owner.set(text.as_bytes());
            assert_eq!(unsafe { native_description(owner.0) }.unwrap(), text);
        }
        owner.set(&vec![b'a'; MAX_DOCUMENT_BYTES + 1]);
        assert!(unsafe { native_description(owner.0) }
            .unwrap_err()
            .contains("1 MiB"));
        owner.set(&[255]);
        assert!(unsafe { native_description(owner.0) }
            .unwrap_err()
            .contains("UTF-8"));
        owner.set(&[]);
        assert!(unsafe { native_description(owner.0) }.is_err());
    }
    #[test]
    fn null_owner_and_different_native_schema_are_rejected() {
        assert!(unsafe { native_description(std::ptr::null_mut()) }.is_err());
        let owner = Fixture::load("Int32");
        assert!(unsafe { native_description(owner.0) }.is_err());
    }
}
