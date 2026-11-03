//! Complete bounded PointCloud2 extraction through opaque native introspection.
use std::ffi::{c_char, c_void, CStr};
use studio_protocol::{
    cloud::{self, DecodedCloud, Field, Metadata},
    image::HeaderStamp,
};
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
unsafe fn schema(schema: Ptr, message: Ptr, count: usize) -> Result<(), String> {
    if schema.is_null() || message.is_null() || sp_count(schema) != count {
        Err("Invalid native PointCloud2 schema".into())
    } else {
        Ok(())
    }
}
unsafe fn field(
    schema: Ptr,
    message: Ptr,
    name: &str,
    kind: i32,
    array: i32,
) -> Result<(Ptr, Ptr), String> {
    if schema.is_null() || message.is_null() {
        return Err("Null native PointCloud2 storage".into());
    }
    let mut found = None;
    for index in 0..sp_count(schema) {
        let member = sp_member(schema, index);
        if member.is_null() {
            return Err("Null native PointCloud2 member".into());
        }
        let native_name = sp_name(member);
        if native_name.is_null() {
            return Err("Null native PointCloud2 member name".into());
        }
        if CStr::from_ptr(native_name).to_bytes() == name.as_bytes() {
            if found.is_some() || sp_type(member) != kind || sp_array(member) != array {
                return Err(format!("Invalid native PointCloud2 field: {name}"));
            }
            let pointer = sp_field(message, member);
            if pointer.is_null() {
                return Err(format!("Missing native PointCloud2 field storage: {name}"));
            }
            found = Some((member, pointer));
        }
    }
    found.ok_or_else(|| format!("Missing native PointCloud2 field: {name}"))
}
unsafe fn string(pointer: Ptr, max: usize) -> Result<String, String> {
    let len = sp_string_size(pointer);
    if len > max {
        return Err("PointCloud2 metadata string budget exceeded".into());
    }
    if len == 0 {
        return Ok(String::new());
    }
    let raw = sp_string_data(pointer);
    if raw.is_null() {
        return Err("Null PointCloud2 metadata string".into());
    }
    std::str::from_utf8(std::slice::from_raw_parts(raw.cast(), len))
        .map(str::to_owned)
        .map_err(|_| "Invalid UTF-8 PointCloud2 metadata".into())
}
unsafe fn boolean(root: Ptr, message: Ptr, name: &str) -> Result<bool, String> {
    let (_, raw) = field(root, message, name, 6, 0)?;
    match *raw.cast::<u8>() {
        0 => Ok(false),
        1 => Ok(true),
        _ => Err(format!("Invalid native PointCloud2 boolean {name}")),
    }
}
/// Extract a complete bounded standard PointCloud2 message; validate all metadata
/// and source byte counts before copying its native byte sequence.
///
/// # Safety
/// Owner must be a live exclusive worker-local TopicProbe/Probe loaded with
/// sensor_msgs/msg/PointCloud2 C introspection. Native getters must return valid
/// initialized primitive pointers for the duration of this call.
pub unsafe fn native_cloud(owner: Ptr, topic: &str) -> Result<DecodedCloud, String> {
    if owner.is_null() || topic.is_empty() || topic.len() > 256 || topic.contains('\0') {
        return Err("Invalid native PointCloud2 owner/topic".into());
    }
    let root = sp_schema(owner, 0);
    let message = sp_message(owner, 0);
    schema(root, message, 9)?;
    let (hm, header) = field(root, message, "header", 18, 0)?;
    let hs = sp_nested(hm);
    schema(hs, header, 2)?;
    let (sm, stamp) = field(hs, header, "stamp", 18, 0)?;
    let ss = sp_nested(sm);
    schema(ss, stamp, 2)?;
    let (_, seconds) = field(ss, stamp, "sec", 13, 0)?;
    let (_, nanos) = field(ss, stamp, "nanosec", 12, 0)?;
    let (_, frame) = field(hs, header, "frame_id", 16, 0)?;
    let (_, width) = field(root, message, "width", 12, 0)?;
    let (_, height) = field(root, message, "height", 12, 0)?;
    let (_, point_step) = field(root, message, "point_step", 12, 0)?;
    let (_, row_step) = field(root, message, "row_step", 12, 0)?;
    let (fm, fields) = field(root, message, "fields", 18, 2)?;
    if sp_bound(fm) != 0 {
        return Err("PointCloud2 fields must be an unbounded standard sequence".into());
    }
    let count = sp_length(fm, fields);
    if count == 0 || count > cloud::MAX_FIELDS {
        return Err("PointCloud2 field count outside bounds".into());
    }
    let fs = sp_nested(fm);
    let mut decoded_fields = Vec::with_capacity(count);
    for index in 0..count {
        let entry = sp_element(fm, fields, index);
        schema(fs, entry, 4)?;
        let (_, name) = field(fs, entry, "name", 16, 0)?;
        let (_, offset) = field(fs, entry, "offset", 12, 0)?;
        let (_, datatype) = field(fs, entry, "datatype", 8, 0)?;
        let (_, count) = field(fs, entry, "count", 12, 0)?;
        decoded_fields.push(Field {
            name: string(name, cloud::MAX_FIELD_NAME_BYTES)?,
            offset: *offset.cast::<u32>(),
            datatype: *datatype.cast::<u8>(),
            count: *count.cast::<u32>(),
        });
    }
    let metadata = Metadata {
        header_stamp: HeaderStamp {
            seconds: *seconds.cast::<i32>(),
            nanoseconds: *nanos.cast::<u32>(),
        },
        frame_id: string(frame, 256)?,
        width: *width.cast::<u32>(),
        height: *height.cast::<u32>(),
        fields: decoded_fields,
        is_bigendian: boolean(root, message, "is_bigendian")?,
        point_step: *point_step.cast::<u32>(),
        row_step: *row_step.cast::<u32>(),
        is_dense: boolean(root, message, "is_dense")?,
    };
    let expected = metadata.raw_length()?;
    let (dm, data) = field(root, message, "data", 8, 2)?;
    if sp_bound(dm) != 0 || sp_length(dm, data) != expected {
        return Err("PointCloud2 data length must match complete bounded rows".into());
    }
    let mut raw = Vec::with_capacity(expected);
    for index in 0..expected {
        let pointer = sp_element(dm, data, index);
        if pointer.is_null() {
            return Err("PointCloud2 byte accessor unavailable".into());
        }
        raw.push(*pointer.cast::<u8>());
    }
    metadata.decode(&raw)
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
        fn sp_resize(member: Ptr, field: Ptr, size: usize) -> i32;
    }
    struct Fixture(Ptr);
    impl Drop for Fixture {
        fn drop(&mut self) {
            unsafe { sp_destroy(self.0) };
        }
    }
    impl Fixture {
        fn new() -> Self {
            let path = crate::native_schema::test_support::introspection_library("sensor_msgs");
            let strings = [path.to_str().unwrap(), "sensor_msgs", "PointCloud2"]
                .map(|s| CString::new(s).unwrap());
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
            let raw: Vec<_> = [1f32, 2., 3.]
                .into_iter()
                .flat_map(f32::to_le_bytes)
                .collect();
            let value = serde_json::json!({"header":{"stamp":{"sec":2,"nanosec":3},"frame_id":"laser"},"height":1,"width":1,"fields":[{"name":"x","offset":0,"datatype":7,"count":1},{"name":"y","offset":4,"datatype":7,"count":1},{"name":"z","offset":8,"datatype":7,"count":1}],"is_bigendian":false,"point_step":12,"row_step":12,"data":raw,"is_dense":true});
            unsafe {
                crate::service_codec::native_write(
                    sp_schema(fixture.0, 0),
                    sp_message(fixture.0, 0),
                    &value,
                )
            }
            .unwrap();
            fixture
        }
        unsafe fn root(&self, name: &str, kind: i32, array: i32) -> (Ptr, Ptr) {
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
    #[test]
    fn complete_native_cloud_exceeds_generic_array_preview_without_truncation() {
        let fixture = Fixture::new();
        let first = unsafe { native_cloud(fixture.0, "/cloud") }.unwrap();
        assert_eq!(first.points, vec![[1., 2., 3.]]);
        assert_eq!(first.acquisition_stamp_ns, 2_000_000_003);
        unsafe {
            // Exercise the full source-point limit with legal organized
            // dimensions; the independent per-dimension cap remains 4096.
            *fixture.root("width", 12, 0).1.cast::<u32>() = 4096;
            *fixture.root("height", 12, 0).1.cast::<u32>() = 2;
            *fixture.root("row_step", 12, 0).1.cast::<u32>() = 4096 * 12;
            let (m, data) = fixture.root("data", 8, 2);
            assert_ne!(sp_resize(m, data, 8192 * 12), 0);
            // ROS primitive sequence growth does not initialize new elements.
            // Supply actual finite zero-coordinate fixture bytes explicitly.
            for index in 0..8192 * 12 {
                *sp_element(m, data, index).cast::<u8>() = 0;
            }
        }
        let full = unsafe { native_cloud(fixture.0, "/cloud") }.unwrap();
        assert_eq!(full.source_points, 8192);
        assert_eq!(full.points.len(), 8192);
        assert_eq!(full.skipped_points, 0);
    }
    #[test]
    fn native_packed_color_and_numeric_intensity_reach_the_shared_contract() {
        for big in [false, true] {
            let fixture = Fixture::new();
            let mut raw = Vec::new();
            for n in [1f32, 2., 3.] {
                raw.extend_from_slice(&if big {
                    n.to_be_bytes()
                } else {
                    n.to_le_bytes()
                });
            }
            let color = 0xffc10203u32; // NaN FLOAT32 bits remain a valid packed color.
            raw.extend_from_slice(&if big {
                color.to_be_bytes()
            } else {
                color.to_le_bytes()
            });
            raw.extend_from_slice(&if big {
                (-1234i16).to_be_bytes()
            } else {
                (-1234i16).to_le_bytes()
            });
            let value = serde_json::json!({"header":{"stamp":{"sec":2,"nanosec":3},"frame_id":"laser"},"height":1,"width":1,
                "fields":[{"name":"x","offset":0,"datatype":7,"count":1},{"name":"y","offset":4,"datatype":7,"count":1},{"name":"z","offset":8,"datatype":7,"count":1},
                {"name":"rgba","offset":12,"datatype":7,"count":1},{"name":"intensity","offset":16,"datatype":3,"count":1}],
                "is_bigendian":big,"point_step":18,"row_step":18,"data":raw,"is_dense":true});
            unsafe {
                crate::service_codec::native_write(
                    sp_schema(fixture.0, 0),
                    sp_message(fixture.0, 0),
                    &value,
                )
            }
            .unwrap();
            let decoded = unsafe { native_cloud(fixture.0, "/cloud") }.unwrap();
            assert_eq!(decoded.points, vec![[1., 2., 3.]]);
            assert_eq!(
                decoded.channels,
                cloud::Channels {
                    color: cloud::Channel::Values(vec![[193, 2, 3, 255]]),
                    intensity: cloud::Channel::Values(vec![Some(-1234.)])
                }
            );
        }
    }
    #[test]
    fn oversized_metadata_and_incomplete_bytes_fail_before_decoding() {
        let fixture = Fixture::new();
        unsafe {
            *fixture.root("width", 12, 0).1.cast::<u32>() = 8193;
        }
        assert!(unsafe { native_cloud(fixture.0, "/cloud") }.is_err());
        unsafe {
            *fixture.root("width", 12, 0).1.cast::<u32>() = 1;
            let (m, data) = fixture.root("data", 8, 2);
            assert_ne!(sp_resize(m, data, 11), 0);
        }
        assert!(unsafe { native_cloud(fixture.0, "/cloud") }.is_err());
        assert!(unsafe { native_cloud(std::ptr::null_mut(), "/cloud") }.is_err());
    }
}
