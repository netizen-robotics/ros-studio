//! Direct bounded Image extraction using opaque C-owned introspection layouts.
//! No ROS message struct layout crosses this boundary; only primitive pointers.
use std::ffi::{c_char, c_void, CStr};
use studio_protocol::image::{HeaderStamp, ImageMetadata, ImageSample};
type Ptr = *mut c_void;
extern "C" {
    fn sp_schema(owner: Ptr, response: i32) -> Ptr;
    fn sp_message(owner: Ptr, response: i32) -> Ptr;
    fn sp_count(schema: Ptr) -> usize;
    fn sp_member(schema: Ptr, index: usize) -> Ptr;
    fn sp_name(member: Ptr) -> *const c_char;
    fn sp_type(member: Ptr) -> i32;
    fn sp_array(member: Ptr) -> i32;
    fn sp_bound(member: Ptr) -> usize;
    fn sp_nested(member: Ptr) -> Ptr;
    fn sp_field(message: Ptr, member: Ptr) -> Ptr;
    fn sp_length(member: Ptr, field: Ptr) -> usize;
    fn sp_element(member: Ptr, field: Ptr, index: usize) -> Ptr;
    fn sp_string_size(field: Ptr) -> usize;
    fn sp_string_data(field: Ptr) -> *const c_char;
}
unsafe fn schema(schema: Ptr, message: Ptr, fields: usize) -> Result<(), String> {
    if schema.is_null() || message.is_null() || sp_count(schema) != fields {
        return Err("Native Image schema has missing or unexpected fields".into());
    }
    Ok(())
}
unsafe fn field(
    schema: Ptr,
    message: Ptr,
    name: &str,
    kind: i32,
    array: i32,
) -> Result<(Ptr, Ptr), String> {
    let count = sp_count(schema);
    let mut found = None;
    for index in 0..count {
        let member = sp_member(schema, index);
        if member.is_null() {
            return Err("Null native Image member metadata".into());
        }
        let native_name = sp_name(member);
        if native_name.is_null() {
            return Err("Null native Image member name".into());
        }
        if CStr::from_ptr(native_name).to_bytes() == name.as_bytes() {
            if found.is_some() || sp_type(member) != kind || sp_array(member) != array {
                return Err(format!("Invalid native Image field schema: {name}"));
            }
            let pointer = sp_field(message, member);
            if pointer.is_null() {
                return Err(format!("Missing native Image field storage: {name}"));
            }
            found = Some((member, pointer));
        }
    }
    found.ok_or_else(|| format!("Missing native Image field: {name}"))
}
unsafe fn string(pointer: Ptr, max: usize) -> Result<String, String> {
    let len = sp_string_size(pointer);
    if len > max {
        return Err("Native Image string exceeds bounded metadata budget".into());
    }
    if len == 0 {
        return Ok(String::new());
    }
    let data = sp_string_data(pointer);
    if data.is_null() {
        return Err("Null native Image string storage".into());
    }
    std::str::from_utf8(std::slice::from_raw_parts(data.cast(), len))
        .map(str::to_owned)
        .map_err(|_| "Invalid UTF-8 native Image metadata".into())
}

/// Copy one complete sensor_msgs/msg/Image from its C-owned message storage.
/// Raw length and decoded RGBA budget are checked before reading any data byte.
///
/// # Safety
/// `owner` must point to a live native Probe/TopicProbe loaded with the standard
/// sensor_msgs/msg/Image C introspection library. The owner and its message
/// must remain exclusively owned by this worker throughout the call. The C
/// getters provide valid primitive pointers and initialized native storage.
pub unsafe fn native_image(owner: Ptr, topic: &str) -> Result<ImageSample, String> {
    if owner.is_null() {
        return Err("Null native Image owner".into());
    }
    if topic.is_empty() || topic.len() > 4096 || topic.contains('\0') {
        return Err("Invalid image topic identity".into());
    }
    let root = sp_schema(owner, 0);
    let message = sp_message(owner, 0);
    schema(root, message, 7)?;
    let (header_member, header) = field(root, message, "header", 18, 0)?;
    let header_schema = sp_nested(header_member);
    schema(header_schema, header, 2)?;
    let (stamp_member, stamp) = field(header_schema, header, "stamp", 18, 0)?;
    let stamp_schema = sp_nested(stamp_member);
    schema(stamp_schema, stamp, 2)?;
    let (_, seconds) = field(stamp_schema, stamp, "sec", 13, 0)?;
    let (_, nanoseconds) = field(stamp_schema, stamp, "nanosec", 12, 0)?;
    let (_, frame_id) = field(header_schema, header, "frame_id", 16, 0)?;
    let (_, width) = field(root, message, "width", 12, 0)?;
    let (_, height) = field(root, message, "height", 12, 0)?;
    let (_, encoding) = field(root, message, "encoding", 16, 0)?;
    let (_, is_bigendian) = field(root, message, "is_bigendian", 8, 0)?;
    let (_, step) = field(root, message, "step", 12, 0)?;
    let byte_order = *is_bigendian.cast::<u8>();
    if byte_order > 1 {
        return Err("Native Image is_bigendian must be 0 or 1".into());
    }
    let metadata = ImageMetadata {
        header_stamp: HeaderStamp {
            seconds: *seconds.cast::<i32>(),
            nanoseconds: *nanoseconds.cast::<u32>(),
        },
        frame_id: string(frame_id, 256)?,
        width: *width.cast::<u32>(),
        height: *height.cast::<u32>(),
        encoding: string(encoding, 64)?,
        is_bigendian: byte_order != 0,
        step: *step.cast::<u32>(),
    };
    let expected = metadata.raw_length()?;
    let (member, data) = field(root, message, "data", 8, 2)?;
    if sp_bound(member) != 0 {
        return Err("Native Image data must be an unbounded uint8 sequence".into());
    }
    let length = sp_length(member, data);
    if length != expected {
        return Err("Native Image data length must exactly equal step × height".into());
    }
    let mut raw = Vec::with_capacity(expected);
    for index in 0..expected {
        let pointer = sp_element(member, data, index);
        if pointer.is_null() {
            return Err("Native Image data accessor unavailable".into());
        }
        raw.push(*pointer.cast::<u8>());
    }
    ImageSample::from_raw(topic, metadata, &raw)
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
        fn sp_error(owner: Ptr) -> *const c_char;
        fn sp_resize(member: Ptr, field: Ptr, size: usize) -> i32;
        fn sp_string_set(field: Ptr, data: *const c_char, size: usize) -> i32;
    }
    /// Same opaque native-message ownership as service_codec::NativeMessage,
    /// with direct fixture setters to exceed its 65536-JSON-element write cap.
    struct Fixture(Ptr);
    impl Drop for Fixture {
        fn drop(&mut self) {
            unsafe { sp_destroy(self.0) };
        }
    }
    impl Fixture {
        fn load(package: &str, name: &str) -> Self {
            let path = crate::native_schema::test_support::introspection_library(package);
            let strings =
                [path.to_str().unwrap(), package, name].map(|value| CString::new(value).unwrap());
            let fixture = Self(unsafe { sp_create() });
            assert!(!fixture.0.is_null());
            let code = unsafe {
                sp_load_message(
                    fixture.0,
                    strings[0].as_ptr(),
                    strings[1].as_ptr(),
                    strings[2].as_ptr(),
                )
            };
            assert_eq!(
                code,
                0,
                "{}",
                unsafe { CStr::from_ptr(sp_error(fixture.0)) }.to_string_lossy()
            );
            fixture
        }
        unsafe fn root_field(&self, name: &str, kind: i32, array: i32) -> (Ptr, Ptr) {
            field(
                sp_schema(self.0, 0),
                sp_message(self.0, 0),
                name,
                kind,
                array,
            )
            .unwrap()
        }
        fn u32(&mut self, name: &str, value: u32) {
            unsafe {
                *self.root_field(name, 12, 0).1.cast::<u32>() = value;
            }
        }
        fn u8(&mut self, name: &str, value: u8) {
            unsafe {
                *self.root_field(name, 8, 0).1.cast::<u8>() = value;
            }
        }
        fn encoding(&mut self, bytes: &[u8]) {
            unsafe {
                assert_ne!(
                    sp_string_set(
                        self.root_field("encoding", 16, 0).1,
                        bytes.as_ptr().cast(),
                        bytes.len()
                    ),
                    0
                );
            }
        }
        fn data(&mut self, bytes: &[u8]) {
            unsafe {
                let (member, pointer) = self.root_field("data", 8, 2);
                assert_ne!(sp_resize(member, pointer, bytes.len()), 0);
                for (index, byte) in bytes.iter().enumerate() {
                    let element = sp_element(member, pointer, index);
                    assert!(!element.is_null());
                    *element.cast::<u8>() = *byte;
                }
            }
        }
        fn stamp(&mut self, sec: i32, nsec: u32) {
            unsafe {
                let (header_member, header) = self.root_field("header", 18, 0);
                let (stamp_member, stamp) =
                    field(sp_nested(header_member), header, "stamp", 18, 0).unwrap();
                let schema = sp_nested(stamp_member);
                *field(schema, stamp, "sec", 13, 0).unwrap().1.cast::<i32>() = sec;
                *field(schema, stamp, "nanosec", 12, 0)
                    .unwrap()
                    .1
                    .cast::<u32>() = nsec;
                let frame = field(sp_nested(header_member), header, "frame_id", 16, 0)
                    .unwrap()
                    .1;
                assert_ne!(sp_string_set(frame, c"camera_optical".as_ptr(), 14), 0);
            }
        }
        fn image(width: u32, height: u32, encoding: &str, step: u32, data: &[u8]) -> Self {
            let mut fixture = Self::load("sensor_msgs", "Image");
            fixture.u32("width", width);
            fixture.u32("height", height);
            fixture.encoding(encoding.as_bytes());
            fixture.u32("step", step);
            fixture.data(data);
            fixture.stamp(-2, 999_999_999);
            fixture
        }
        fn read(&self) -> Result<ImageSample, String> {
            unsafe { native_image(self.0, "/offline/camera") }
        }
    }
    #[test]
    fn native_image_keeps_exact_bytes_padding_header_and_byte_order() {
        let raw = [
            255, 0, 0, 0, 255, 0, 99, 98, 0, 0, 255, 255, 255, 255, 97, 96,
        ];
        let mut fixture = Fixture::image(2, 2, "rgb8", 8, &raw);
        fixture.u8("is_bigendian", 1);
        let sample = fixture.read().unwrap();
        assert_eq!(sample.raw_bytes().unwrap(), raw);
        assert_eq!(
            sample.metadata.header_stamp,
            HeaderStamp {
                seconds: -2,
                nanoseconds: 999_999_999
            }
        );
        assert_eq!(sample.metadata.frame_id, "camera_optical");
        assert!(sample.metadata.is_bigendian);
        assert_eq!(
            ImageSample::decode(&sample.encode().unwrap()).unwrap(),
            sample
        );
    }
    #[test]
    fn native_image_full_budget_bypasses_typed_array_truncation() {
        let raw: Vec<u8> = (0..studio_protocol::image::MAX_RAW_BYTES)
            .map(|index| (index % 256) as u8)
            .collect();
        let fixture = Fixture::image(2048, 2048, "rgba8", 8192, &raw);
        let typed = unsafe { crate::service_codec::native_typed_value(fixture.0) }.unwrap();
        let studio_protocol::structured::Value::Object { fields, .. } = typed else {
            panic!("Typed object required")
        };
        let data = fields.iter().find(|field| field.name == "data").unwrap();
        let studio_protocol::structured::Value::Array {
            items,
            original_items,
        } = &data.value
        else {
            panic!("Typed data array required")
        };
        assert_eq!(*original_items, raw.len());
        assert!(items.len() < raw.len());
        let sample = fixture.read().unwrap();
        assert_eq!(sample.original_bytes, raw.len());
        assert!(sample.complete);
        assert_eq!(sample.raw_bytes().unwrap(), raw);
    }
    #[test]
    fn native_image_rejects_bad_lengths_metadata_utf8_and_non_image_schema() {
        for bytes in [&[1, 2][..], &[1, 2, 3, 4][..]] {
            assert!(Fixture::image(1, 1, "rgb8", 3, bytes).read().is_err());
        }
        let mut fixture = Fixture::image(1, 1, "mono8", 1, &[1]);
        fixture.u8("is_bigendian", 2);
        assert!(fixture.read().is_err());
        fixture.u8("is_bigendian", 0);
        fixture.stamp(0, 1_000_000_000);
        assert!(fixture.read().is_err());
        fixture.stamp(0, 0);
        fixture.encoding(&[255]);
        assert!(fixture.read().is_err());
        fixture.encoding(b"mono8");
        fixture.u32("step", u32::MAX);
        assert!(fixture.read().is_err());
        assert!(Fixture::load("std_msgs", "String").read().is_err());
        assert!(unsafe { native_image(std::ptr::null_mut(), "/offline") }.is_err());
    }
    #[test]
    fn native_image_retains_unsupported_encoding_for_visible_renderer_error() {
        let fixture = Fixture::image(1, 1, "16UC1", 2, &[4, 8]);
        let sample = fixture.read().unwrap();
        assert_eq!(sample.metadata.encoding, "16UC1");
        assert_eq!(sample.raw_bytes().unwrap(), [4, 8]);
    }
}
