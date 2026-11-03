//! Only native primitive pointers cross this boundary; ROS struct layouts stay in C.
use serde_json::{Map, Value};
use std::ffi::{c_char, c_void, CStr, CString};

type Ptr = *mut c_void;
pub const LIMIT: usize = 1024 * 1024;
extern "C" {
    fn sp_redirect_stdout() -> i32;
    fn sp_restore_stdout(saved: i32);
    fn sp_write_reserved_stdout(saved: i32, bytes: *const u8, count: usize) -> i64;
    fn sp_create() -> Ptr;
    fn sp_destroy(p: Ptr);
    fn sp_error(p: Ptr) -> *const c_char;
    fn sp_load_message(
        p: Ptr,
        intro: *const c_char,
        package: *const c_char,
        name: *const c_char,
    ) -> i32;
    fn sp_load(
        p: Ptr,
        intro: *const c_char,
        dispatch: *const c_char,
        package: *const c_char,
        name: *const c_char,
        identifier: *const c_char,
    ) -> i32;
    fn sp_schema(p: Ptr, response: i32) -> Ptr;
    fn sp_message(p: Ptr, response: i32) -> Ptr;
    fn sp_count(schema: Ptr) -> usize;
    fn sp_member(schema: Ptr, i: usize) -> Ptr;
    fn sp_name(member: Ptr) -> *const c_char;
    fn sp_type(member: Ptr) -> i32;
    fn sp_bound(member: Ptr) -> usize;
    fn sp_string_bound(member: Ptr) -> usize;
    fn sp_array(member: Ptr) -> i32;
    fn sp_nested(member: Ptr) -> Ptr;
    fn sp_field(message: Ptr, member: Ptr) -> Ptr;
    fn sp_length(member: Ptr, field: Ptr) -> usize;
    fn sp_resize(member: Ptr, field: Ptr, size: usize) -> i32;
    fn sp_element(member: Ptr, field: Ptr, index: usize) -> Ptr;
    fn sp_string_set(field: Ptr, data: *const c_char, size: usize) -> i32;
    fn sp_string_data(field: Ptr) -> *const c_char;
    fn sp_string_size(field: Ptr) -> usize;
    fn sp_connect(p: Ptr, service: *const c_char) -> i32;
    fn sp_connect_at(p: Ptr, service: *const c_char, domain: usize) -> i32;
    fn sp_available(p: Ptr) -> i32;
    fn sp_send(p: Ptr, sequence: *mut i64) -> i32;
    fn sp_take(p: Ptr, timeout_ns: i64, sequence: *mut i64) -> i32;
}

/// Process-wide vendor diagnostics routing. Create before ROS objects so they
/// are destroyed before stdout is restored on scope exit. Writes through this
/// owner go to the original stdout descriptor, reserved for protocol bytes.
pub struct NativeDiagnostics(i32);
impl NativeDiagnostics {
    pub fn stderr() -> Result<Self, String> {
        let saved = unsafe { sp_redirect_stdout() };
        if saved < 0 {
            Err("Could not reserve stdout for JSON results".into())
        } else {
            Ok(Self(saved))
        }
    }
}
impl std::io::Write for NativeDiagnostics {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        // The descriptor remains owned until Drop; C borrows only this slice.
        let result = unsafe { sp_write_reserved_stdout(self.0, bytes.as_ptr(), bytes.len()) };
        if result < 0 {
            Err(std::io::Error::last_os_error())
        } else {
            Ok(result as usize)
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        // Descriptor writes have no userspace buffering.
        Ok(())
    }
}
impl Drop for NativeDiagnostics {
    fn drop(&mut self) {
        unsafe { sp_restore_stdout(self.0) }
    }
}

pub struct Probe(Ptr);
/// Generic native message ownership without a ROS context or network entities.
/// Libraries outlive message finalization. No Rust ROS-layout casts are used.
pub struct NativeMessage(Probe);
impl NativeMessage {
    pub fn load(intro: &str, package: &str, name: &str) -> Result<Self, String> {
        let probe = Probe(unsafe { sp_create() });
        if probe.0.is_null() {
            return Err("Native allocation failed".into());
        }
        let strings: Vec<CString> = [intro, package, name]
            .iter()
            .map(|s| CString::new(*s).map_err(|e| e.to_string()))
            .collect::<Result<_, _>>()?;
        probe.check(unsafe {
            sp_load_message(
                probe.0,
                strings[0].as_ptr(),
                strings[1].as_ptr(),
                strings[2].as_ptr(),
            )
        })?;
        unsafe {
            schema_supported(sp_schema(probe.0, 0), 0)?;
        }
        Ok(Self(probe))
    }
    pub fn schema(&self) -> Result<Value, String> {
        unsafe { describe_schema(sp_schema(self.0 .0, 0), 0, &mut 4096) }
    }
    pub fn write(&mut self, value: &Value) -> Result<(), String> {
        self.0.request(value)
    }
    pub fn read(&self) -> Result<Value, String> {
        unsafe {
            read_message(
                sp_schema(self.0 .0, 0),
                sp_message(self.0 .0, 0),
                0,
                &mut Budget::new(),
            )
        }
    }
}
impl Drop for Probe {
    fn drop(&mut self) {
        unsafe { sp_destroy(self.0) }
    }
}
impl Probe {
    pub fn load(
        intro: &str,
        dispatch: &str,
        package: &str,
        name: &str,
        identifier: &str,
    ) -> Result<Self, String> {
        let p = Self(unsafe { sp_create() });
        if p.0.is_null() {
            return Err("Native allocation failed".into());
        }
        let strings: Vec<CString> = [intro, dispatch, package, name, identifier]
            .iter()
            .map(|s| CString::new(*s).map_err(|e| e.to_string()))
            .collect::<Result<_, _>>()?;
        p.check(unsafe {
            sp_load(
                p.0,
                strings[0].as_ptr(),
                strings[1].as_ptr(),
                strings[2].as_ptr(),
                strings[3].as_ptr(),
                strings[4].as_ptr(),
            )
        })?;
        // Traverse both schemas before any client is created, including omitted fields.
        unsafe {
            schema_supported(sp_schema(p.0, 0), 0)?;
            schema_supported(sp_schema(p.0, 1), 0)?;
        }
        Ok(p)
    }
    fn check(&self, code: i32) -> Result<i32, String> {
        if code < 0 {
            Err(unsafe { CStr::from_ptr(sp_error(self.0)) }
                .to_string_lossy()
                .into_owned())
        } else {
            Ok(code)
        }
    }
    pub fn request(&mut self, value: &Value) -> Result<(), String> {
        unsafe {
            write_message(
                sp_schema(self.0, 0),
                sp_message(self.0, 0),
                value,
                0,
                &mut Budget::new(),
            )
        }
    }
    pub fn response(&self) -> Result<Value, String> {
        unsafe {
            read_message(
                sp_schema(self.0, 1),
                sp_message(self.0, 1),
                0,
                &mut Budget::new(),
            )
        }
    }
    pub fn connect(&mut self, service: &str) -> Result<(), String> {
        let name = CString::new(service).map_err(|e| e.to_string())?;
        self.check(unsafe { sp_connect(self.0, name.as_ptr()) })
            .map(|_| ())
    }
    pub fn connect_at(&mut self, service: &str, domain: usize) -> Result<(), String> {
        let name = CString::new(service).map_err(|e| e.to_string())?;
        self.check(unsafe { sp_connect_at(self.0, name.as_ptr(), domain) })
            .map(|_| ())
    }
    pub fn describe(&self, response: bool) -> Result<Value, String> {
        unsafe { describe_schema(sp_schema(self.0, response.into()), 0, &mut 4096) }
    }
    pub fn request_value(&self) -> Result<Value, String> {
        unsafe { native_read(sp_schema(self.0, 0), sp_message(self.0, 0)) }
    }
    pub fn available(&self) -> Result<bool, String> {
        self.check(unsafe { sp_available(self.0) }).map(|v| v == 1)
    }
    pub fn send(&mut self) -> Result<i64, String> {
        let mut seq = 0;
        self.check(unsafe { sp_send(self.0, &mut seq) })?;
        Ok(seq)
    }
    pub fn take(&mut self, timeout_ns: i64) -> Result<Option<i64>, String> {
        let mut seq = 0;
        let code = self.check(unsafe { sp_take(self.0, timeout_ns, &mut seq) })?;
        Ok((code == 1).then_some(seq))
    }
}
struct Budget {
    bytes: usize,
    elements: usize,
}
impl Budget {
    fn new() -> Self {
        Self {
            bytes: LIMIT,
            elements: 65536,
        }
    }
    fn charge(&mut self, bytes: usize, elements: usize) -> Result<(), String> {
        self.bytes = self
            .bytes
            .checked_sub(bytes)
            .ok_or("Message exceeds 1 MiB conversion budget")?;
        self.elements = self
            .elements
            .checked_sub(elements)
            .ok_or("Message exceeds 65536 element limit")?;
        Ok(())
    }
}
fn depth_check(depth: usize) -> Result<(), String> {
    if depth > 32 {
        Err("Message nesting exceeds 32".into())
    } else {
        Ok(())
    }
}
// SAFETY: metadata and data are supplied by compatible trusted native type-support libraries,
// kept loaded by Probe. Every field access is selected from its introspection descriptor.
unsafe fn schema_supported(schema: Ptr, depth: usize) -> Result<(), String> {
    depth_check(depth)?;
    if schema.is_null() {
        return Err("Missing nested schema".into());
    }
    if sp_count(schema) > 65536 {
        return Err("Schema exceeds member limit".into());
    }
    for i in 0..sp_count(schema) {
        let m = sp_member(schema, i);
        match sp_type(m) {
            1 | 2 | 4 | 6..=16 => (),
            18 => schema_supported(sp_nested(m), depth + 1)?,
            t => {
                return Err(format!(
                    "Unsupported ROS field kind {t}: {}",
                    CStr::from_ptr(sp_name(m)).to_string_lossy()
                ))
            }
        }
    }
    Ok(())
}
unsafe fn write_message(
    schema: Ptr,
    message: Ptr,
    value: &Value,
    depth: usize,
    budget: &mut Budget,
) -> Result<(), String> {
    depth_check(depth)?;
    budget.charge(2, 1)?;
    let object = value
        .as_object()
        .ok_or("Expected JSON object for ROS message")?;
    let mut known = std::collections::HashSet::new();
    for i in 0..sp_count(schema) {
        let m = sp_member(schema, i);
        let name = CStr::from_ptr(sp_name(m))
            .to_str()
            .map_err(|e| e.to_string())?;
        known.insert(name);
        if let Some(v) = object.get(name) {
            budget.charge(name.len(), 0)?;
            let field = sp_field(message, m);
            let result = if sp_array(m) == 0 {
                write_scalar(m, field, v, depth, budget)
            } else {
                let values = v
                    .as_array()
                    .ok_or_else(|| format!("{name}: expected array"))?;
                let len = values.len();
                let bound = sp_bound(m);
                if (sp_array(m) == 1 && len != bound)
                    || (sp_array(m) == 2 && bound > 0 && len > bound)
                {
                    return Err(format!(
                        "{name}: invalid array length {len} (bound {bound})"
                    ));
                }
                budget.charge(len, len)?;
                if sp_array(m) == 2 && sp_resize(m, field, len) == 0 {
                    return Err(format!("{name}: sequence allocation failed"));
                }
                for (j, v) in values.iter().enumerate() {
                    let element = sp_element(m, field, j);
                    if element.is_null() {
                        return Err(format!("{name}: array accessor unavailable"));
                    }
                    write_scalar(m, element, v, depth + 1, budget)
                        .map_err(|e| format!("{name}[{j}]: {e}"))?;
                }
                Ok(())
            };
            result.map_err(|e| format!("{name}: {e}"))?;
        }
    }
    for key in object.keys() {
        if !known.contains(key.as_str()) {
            return Err(format!("Unknown field {key}"));
        }
    }
    Ok(())
}
unsafe fn write_scalar(
    m: Ptr,
    ptr: Ptr,
    v: &Value,
    depth: usize,
    budget: &mut Budget,
) -> Result<(), String> {
    depth_check(depth)?;
    budget.charge(8, 1)?;
    macro_rules! unsigned {
        ($t:ty) => {{
            let n = v.as_u64().ok_or("Expected unsigned integer")?;
            let n = <$t>::try_from(n).map_err(|_| "Unsigned integer out of range")?;
            *(ptr as *mut $t) = n;
        }};
    }
    macro_rules! signed {
        ($t:ty) => {{
            let n = v.as_i64().ok_or("Expected signed integer")?;
            let n = <$t>::try_from(n).map_err(|_| "Signed integer out of range")?;
            *(ptr as *mut $t) = n;
        }};
    }
    match sp_type(m) {
        1 => {
            let n = v.as_f64().ok_or("Expected finite float")? as f32;
            if !n.is_finite() {
                return Err("Float out of range".into());
            }
            *(ptr as *mut f32) = n;
        }
        2 => {
            let n = v.as_f64().ok_or("Expected finite double")?;
            if !n.is_finite() {
                return Err("Double out of range".into());
            }
            *(ptr as *mut f64) = n;
        }
        4 | 7 | 8 => unsigned!(u8),
        9 => signed!(i8),
        10 => unsigned!(u16),
        11 => signed!(i16),
        12 => unsigned!(u32),
        13 => signed!(i32),
        14 => unsigned!(u64),
        15 => signed!(i64),
        6 => *(ptr as *mut bool) = v.as_bool().ok_or("Expected boolean")?,
        16 => {
            let s = v.as_str().ok_or("Expected string")?;
            budget.charge(s.len(), 0)?;
            let bound = sp_string_bound(m);
            if bound > 0 && s.len() > bound {
                return Err(format!("String exceeds bound {bound}"));
            }
            if sp_string_set(ptr, s.as_ptr().cast(), s.len()) == 0 {
                return Err("String allocation failed".into());
            }
        }
        18 => write_message(sp_nested(m), ptr, v, depth + 1, budget)?,
        t => return Err(format!("Unsupported field kind {t}")),
    }
    Ok(())
}
unsafe fn read_message(
    schema: Ptr,
    message: Ptr,
    depth: usize,
    budget: &mut Budget,
) -> Result<Value, String> {
    depth_check(depth)?;
    budget.charge(2, 1)?;
    let mut object = Map::new();
    for i in 0..sp_count(schema) {
        let m = sp_member(schema, i);
        let name = CStr::from_ptr(sp_name(m))
            .to_str()
            .map_err(|e| e.to_string())?;
        budget.charge(name.len(), 0)?;
        let field = sp_field(message, m);
        let v = if sp_array(m) == 0 {
            read_scalar(m, field, depth, budget)?
        } else {
            let len = sp_length(m, field);
            budget.charge(len, len)?;
            let mut array = Vec::with_capacity(len);
            for j in 0..len {
                let element = sp_element(m, field, j);
                if element.is_null() {
                    return Err("Array accessor unavailable".into());
                }
                array.push(read_scalar(m, element, depth + 1, budget)?);
            }
            Value::Array(array)
        };
        object.insert(name.into(), v);
    }
    Ok(Value::Object(object))
}
unsafe fn read_scalar(
    m: Ptr,
    ptr: Ptr,
    depth: usize,
    budget: &mut Budget,
) -> Result<Value, String> {
    depth_check(depth)?;
    budget.charge(8, 1)?;
    macro_rules! number {
        ($t:ty) => {
            Value::from(*(ptr as *const $t))
        };
    }
    Ok(match sp_type(m) {
        1 => {
            let n = *(ptr as *const f32);
            if !n.is_finite() {
                return Err("Non-finite response float".into());
            }
            Value::from(n)
        }
        2 => {
            let n = *(ptr as *const f64);
            if !n.is_finite() {
                return Err("Non-finite response double".into());
            }
            Value::from(n)
        }
        4 | 7 | 8 => number!(u8),
        9 => number!(i8),
        10 => number!(u16),
        11 => number!(i16),
        12 => number!(u32),
        13 => number!(i32),
        14 => number!(u64),
        15 => number!(i64),
        6 => Value::from(*(ptr as *const bool)),
        16 => {
            let size = sp_string_size(ptr);
            budget.charge(size, 0)?;
            if size == 0 {
                Value::String(String::new())
            } else {
                let data = sp_string_data(ptr);
                if data.is_null() {
                    return Err("Null response string".into());
                }
                Value::String(
                    std::str::from_utf8(std::slice::from_raw_parts(data.cast(), size))
                        .map_err(|e| e.to_string())?
                        .into(),
                )
            }
        }
        18 => read_message(sp_nested(m), ptr, depth + 1, budget)?,
        t => return Err(format!("Unsupported response field kind {t}")),
    })
}

// These adapters share the checked codec with action slots. Callers must keep
// the native allocation and its introspection library alive for the whole call.
pub(crate) unsafe fn native_schema(schema: Ptr) -> Result<(), String> {
    schema_supported(schema, 0)
}
pub(crate) unsafe fn native_write(schema: Ptr, message: Ptr, value: &Value) -> Result<(), String> {
    write_message(schema, message, value, 0, &mut Budget::new())
}
pub(crate) unsafe fn native_read(schema: Ptr, message: Ptr) -> Result<Value, String> {
    read_message(schema, message, 0, &mut Budget::new())
}

/// Decode a bounded preview without walking complete sensor arrays.
///
/// # Safety
/// `owner` must be a live native Probe or TopicProbe created by this crate, with
/// its request message initialized and exclusively owned for this call.
pub unsafe fn native_preview(owner: *mut c_void) -> Result<Value, String> {
    let mut remaining = 256;
    let value = preview_message(sp_schema(owner, 0), sp_message(owner, 0), 0, &mut remaining)?;
    if serde_json::to_vec(&value).map_err(|e| e.to_string())?.len() > 16384 {
        return Err("Preview exceeds 16 KiB output limit".into());
    }
    Ok(value)
}

unsafe fn preview_message(
    schema: Ptr,
    message: Ptr,
    depth: usize,
    remaining: &mut usize,
) -> Result<Value, String> {
    if depth >= 8 || *remaining == 0 {
        return Ok(serde_json::json!({"truncated":true}));
    }
    let mut fields = Map::new();
    let count = sp_count(schema);
    for i in 0..count.min(128) {
        if *remaining == 0 {
            break;
        }
        *remaining -= 1;
        let member = sp_member(schema, i);
        let name = CStr::from_ptr(sp_name(member))
            .to_str()
            .map_err(|e| e.to_string())?;
        if name.len() > 256 {
            return Err("Preview field name exceeds limit".into());
        }
        let field = sp_field(message, member);
        let value = if sp_array(member) == 0 {
            preview_scalar(member, field, depth, remaining)?
        } else {
            let length = sp_length(member, field);
            let mut items = Vec::new();
            for j in 0..length.min(8) {
                if *remaining == 0 {
                    break;
                }
                *remaining -= 1;
                let ptr = sp_element(member, field, j);
                if ptr.is_null() {
                    return Err("Preview array accessor unavailable".into());
                }
                items.push(preview_scalar(member, ptr, depth + 1, remaining)?);
            }
            let truncated = items.len() < length || items.iter().any(preview_is_truncated);
            serde_json::json!({"length":length,"truncated":truncated,"items":items})
        };
        fields.insert(name.into(), value);
    }
    let truncated = fields.len() < count || fields.values().any(preview_is_truncated);
    Ok(serde_json::json!({"truncated":truncated,"fields":fields}))
}

fn preview_is_truncated(value: &Value) -> bool {
    value.get("truncated").and_then(Value::as_bool) == Some(true)
}

fn preview_text(bytes: &[u8], truncated: bool) -> String {
    // The byte budget can end inside a valid UTF-8 code point. Omit that
    // unfinished suffix rather than inventing a replacement character.
    let bytes = match std::str::from_utf8(bytes) {
        Err(error) if truncated && error.error_len().is_none() => &bytes[..error.valid_up_to()],
        _ => bytes,
    };
    String::from_utf8_lossy(bytes).into_owned()
}

unsafe fn preview_scalar(
    member: Ptr,
    ptr: Ptr,
    depth: usize,
    remaining: &mut usize,
) -> Result<Value, String> {
    match sp_type(member) {
        18 => preview_message(sp_nested(member), ptr, depth + 1, remaining),
        16 => {
            let length = sp_string_size(ptr);
            let size = length.min(256);
            let text = if size == 0 {
                String::new()
            } else {
                let data = sp_string_data(ptr);
                if data.is_null() {
                    return Err("Null preview string".into());
                }
                preview_text(std::slice::from_raw_parts(data.cast(), size), size < length)
            };
            Ok(serde_json::json!({"bytes":length,"truncated":size<length,"text":text}))
        }
        _ => read_scalar(member, ptr, depth, &mut Budget::new()),
    }
}

#[cfg(test)]
mod preview_tests {
    use super::preview_text;

    #[test]
    fn budget_cut_does_not_corrupt_unicode() {
        for suffix in ["é", "机", "🤖"] {
            for split in 1..suffix.len() {
                let text = format!("{}{suffix}tail", "a".repeat(256 - split));
                let preview = preview_text(&text.as_bytes()[..256], true);
                assert_eq!(preview, "a".repeat(256 - split));
                assert!(text.starts_with(&preview));
            }
        }
    }

    #[test]
    fn complete_characters_and_invalid_source_bytes_keep_existing_behavior() {
        assert_eq!(preview_text("机器人".as_bytes(), false), "机器人");
        assert_eq!(preview_text("机器人".as_bytes(), true), "机器人");
        assert_eq!(preview_text(&[0xff], true), "�");
        assert_eq!(preview_text(&[0xe6], false), "�");
        assert_eq!(preview_text(&[], false), "");
    }
}

/// Convert a live native message directly from introspection into bounded typed
/// values. Conversion does not deserialize diagnostic JSON or debug strings.
/// Middleware-owned allocations already exist before these bounds apply.
///
/// # Safety
/// `owner` is a live exclusive Probe/TopicProbe with initialized request storage;
/// its native library and metadata must remain loaded throughout this call.
pub unsafe fn native_typed_value(
    owner: *mut c_void,
) -> Result<studio_protocol::structured::Value, String> {
    typed_message(
        sp_schema(owner, 0),
        sp_message(owner, 0),
        0,
        &mut TypedBudget::new(),
    )
}

use studio_protocol::structured::{self, Field as TypedField, FloatValue, Value as TypedValue};
struct TypedBudget {
    nodes: usize,
    // Conservative JSON bound: each retained UTF-8 byte can escape to six
    // bytes; per-node framing and metadata are charged separately.
    wire: usize,
}
impl TypedBudget {
    fn new() -> Self {
        Self {
            nodes: structured::MAX_NODES,
            wire: structured::MAX_SAMPLE_BYTES - 16 * 1024,
        }
    }
    fn node(&mut self) -> Result<(), String> {
        if self.nodes == 0 || self.wire < 256 {
            return Err("Typed conversion budget exhausted".into());
        }
        self.nodes -= 1;
        self.wire -= 256;
        Ok(())
    }
    fn available(&self) -> bool {
        self.nodes > 0 && self.wire >= 256
    }
    fn text(&mut self, bytes: usize) -> Result<(), String> {
        let escaped = bytes.checked_mul(6).ok_or("Typed text size overflow")?;
        self.wire = self
            .wire
            .checked_sub(escaped)
            .ok_or("Typed conversion byte budget exceeded")?;
        Ok(())
    }
}
fn typed_unsupported(kind: i32, reason: &str) -> TypedValue {
    TypedValue::Unsupported {
        type_name: format!("ros_type_{kind}"),
        reason: reason.into(),
    }
}
fn typed_float(n: f64) -> TypedValue {
    TypedValue::Float {
        value: if n.is_nan() {
            FloatValue::Nan
        } else if n == f64::INFINITY {
            FloatValue::PositiveInfinity
        } else if n == f64::NEG_INFINITY {
            FloatValue::NegativeInfinity
        } else {
            FloatValue::Finite(n.to_string())
        },
    }
}
fn typed_text(bytes: &[u8], original_bytes: usize) -> TypedValue {
    let text = match std::str::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) if bytes.len() < original_bytes && error.error_len().is_none() => {
            // Omit a valid character split by our retained-byte boundary.
            std::str::from_utf8(&bytes[..error.valid_up_to()]).expect("validated prefix")
        }
        Err(_) => return typed_unsupported(16, "Invalid UTF-8 in retained native string bytes"),
    };
    TypedValue::Text {
        value: text.into(),
        original_bytes,
    }
}
unsafe fn typed_message(
    schema: Ptr,
    message: Ptr,
    depth: usize,
    budget: &mut TypedBudget,
) -> Result<TypedValue, String> {
    if schema.is_null() || message.is_null() {
        return Err("Missing native typed message/schema".into());
    }
    budget.node()?;
    if depth >= structured::MAX_DEPTH {
        return Ok(typed_unsupported(18, "Typed nesting depth limit reached"));
    }
    let original_fields = sp_count(schema);
    let mut fields = Vec::new();
    for i in 0..original_fields.min(structured::MAX_FIELDS) {
        if !budget.available() {
            break;
        }
        let member = sp_member(schema, i);
        let name = CStr::from_ptr(sp_name(member))
            .to_str()
            .map_err(|_| "Invalid UTF-8 field name")?;
        if name.is_empty() || name.len() > 1024 {
            return Err("Invalid native field name length".into());
        }
        // Reserve node framing in addition to member-name escaping before read.
        if name.len().saturating_mul(6).saturating_add(256) > budget.wire {
            break;
        }
        budget.text(name.len())?;
        let ptr = sp_field(message, member);
        let value = if sp_array(member) == 0 {
            typed_scalar(member, ptr, depth + 1, budget)?
        } else {
            budget.node()?;
            let original_items = sp_length(member, ptr);
            let mut items = Vec::new();
            if depth + 1 < structured::MAX_DEPTH {
                for j in 0..original_items.min(structured::MAX_ITEMS) {
                    if !budget.available() {
                        break;
                    }
                    let element = sp_element(member, ptr, j);
                    if element.is_null() {
                        return Err("Typed array accessor unavailable".into());
                    }
                    items.push(typed_scalar(member, element, depth + 2, budget)?);
                }
            }
            TypedValue::Array {
                items,
                original_items,
            }
        };
        fields.push(TypedField {
            name: name.into(),
            value,
        });
    }
    Ok(TypedValue::Object {
        fields,
        original_fields,
    })
}
unsafe fn typed_scalar(
    member: Ptr,
    ptr: Ptr,
    depth: usize,
    budget: &mut TypedBudget,
) -> Result<TypedValue, String> {
    if ptr.is_null() {
        return Err("Null typed scalar storage".into());
    }
    if sp_type(member) == 18 {
        return typed_message(sp_nested(member), ptr, depth, budget);
    }
    budget.node()?;
    macro_rules! signed {
        ($t:ty) => {
            TypedValue::Signed {
                value: (*(ptr as *const $t)).to_string(),
            }
        };
    }
    macro_rules! unsigned {
        ($t:ty) => {
            TypedValue::Unsigned {
                value: (*(ptr as *const $t)).to_string(),
            }
        };
    }
    Ok(match sp_type(member) {
        1 => typed_float(*(ptr as *const f32) as f64),
        2 => typed_float(*(ptr as *const f64)),
        4 | 7 | 8 => unsigned!(u8),
        9 => signed!(i8),
        10 => unsigned!(u16),
        11 => signed!(i16),
        12 => unsigned!(u32),
        13 => signed!(i32),
        14 => unsigned!(u64),
        15 => signed!(i64),
        6 => TypedValue::Bool {
            value: *(ptr as *const bool),
        },
        16 => {
            let original_bytes = sp_string_size(ptr);
            let size = original_bytes
                .min(structured::MAX_TEXT_BYTES)
                .min(budget.wire / 6);
            budget.text(size)?;
            if size == 0 {
                TypedValue::Text {
                    value: String::new(),
                    original_bytes,
                }
            } else {
                let data = sp_string_data(ptr);
                if data.is_null() {
                    return Err("Null native typed string".into());
                }
                typed_text(
                    std::slice::from_raw_parts(data.cast(), size),
                    original_bytes,
                )
            }
        }
        kind => typed_unsupported(kind, "Native field kind not supported by typed observation"),
    })
}

#[cfg(test)]
mod typed_tests {
    use super::*;
    fn native(package: &str, name: &str) -> NativeMessage {
        let path = crate::native_schema::test_support::introspection_library(package);
        NativeMessage::load(path.to_str().unwrap(), package, name).unwrap()
    }
    fn typed(message: &NativeMessage) -> TypedValue {
        let value = unsafe { native_typed_value(message.0 .0) }.unwrap();
        let sample = structured::Sample {
            contract_version: structured::CONTRACT_VERSION,
            topic: "/offline".into(),
            message_type: "fixture/msg/Offline".into(),
            source_time: None,
            value: value.clone(),
        };
        sample.encode().unwrap();
        value
    }
    fn member(value: &TypedValue, name: &str) -> TypedValue {
        let TypedValue::Object { fields, .. } = value else {
            panic!("Object required")
        };
        fields
            .iter()
            .find(|f| f.name == name)
            .unwrap()
            .value
            .clone()
    }
    #[test]
    fn native_signed_unsigned_boolean_and_nested_fixed_array_are_exact() {
        for (name, input, expected) in [
            (
                "Int64",
                serde_json::json!(i64::MIN),
                TypedValue::Signed {
                    value: i64::MIN.to_string(),
                },
            ),
            (
                "UInt64",
                serde_json::json!(u64::MAX),
                TypedValue::Unsigned {
                    value: u64::MAX.to_string(),
                },
            ),
            (
                "Bool",
                serde_json::json!(true),
                TypedValue::Bool { value: true },
            ),
        ] {
            let mut message = native("std_msgs", name);
            message.write(&serde_json::json!({"data": input})).unwrap();
            assert_eq!(member(&typed(&message), "data"), expected);
        }
        let message = native("geometry_msgs", "PoseWithCovariance");
        let root = typed(&message);
        assert!(root.is_complete());
        assert!(matches!(
            member(&root, "pose"),
            TypedValue::Object {
                original_fields: 2,
                ..
            }
        ));
        assert!(
            matches!(member(&root,"covariance"), TypedValue::Array { original_items: 36, ref items } if items.len()==36)
        );
    }
    #[test]
    fn native_nonfinite_values_remain_explicit() {
        let message = native("std_msgs", "Float64");
        for (number, expected) in [
            (f64::NAN, FloatValue::Nan),
            (f64::INFINITY, FloatValue::PositiveInfinity),
            (f64::NEG_INFINITY, FloatValue::NegativeInfinity),
        ] {
            unsafe {
                let schema = sp_schema(message.0 .0, 0);
                let data = sp_field(sp_message(message.0 .0, 0), sp_member(schema, 0));
                *(data as *mut f64) = number;
            }
            assert_eq!(
                member(&typed(&message), "data"),
                TypedValue::Float { value: expected }
            );
        }
    }
    #[test]
    fn native_large_arrays_stop_at_declared_retention_limits() {
        let mut message = native("std_msgs", "Float64MultiArray");
        message
            .write(&serde_json::json!({"data": vec![2.5; structured::MAX_ITEMS + 500]}))
            .unwrap();
        let value = typed(&message);
        let TypedValue::Array {
            items,
            original_items,
        } = member(&value, "data")
        else {
            panic!("Array required")
        };
        assert_eq!(original_items, structured::MAX_ITEMS + 500);
        assert!(items.len() <= structured::MAX_ITEMS && !items.is_empty());
        assert!(!value.is_complete());
    }
    #[test]
    fn native_strings_and_aggregate_escaping_are_bounded_without_replacement() {
        let mut message = native("std_msgs", "String");
        let text = format!("{}🤖tail", "x".repeat(structured::MAX_TEXT_BYTES - 2));
        message.write(&serde_json::json!({"data": text})).unwrap();
        let TypedValue::Text {
            value,
            original_bytes,
        } = member(&typed(&message), "data")
        else {
            panic!("Text required")
        };
        assert_eq!(value.len(), structured::MAX_TEXT_BYTES - 2);
        assert_eq!(original_bytes, text.len());
        assert!(text.starts_with(&value));
        assert!(matches!(
            typed_text(&[0xff], 1),
            TypedValue::Unsupported { .. }
        ));
        assert!(matches!(
            typed_text(&[0xe6], 1),
            TypedValue::Unsupported { .. }
        ));
        // Exercise actual native String storage with invalid source bytes too.
        unsafe {
            let schema = sp_schema(message.0 .0, 0);
            let field = sp_field(sp_message(message.0 .0, 0), sp_member(schema, 0));
            assert_eq!(sp_string_set(field, [0xffu8].as_ptr().cast(), 1), 1);
        }
        assert!(matches!(
            member(&typed(&message), "data"),
            TypedValue::Unsupported { .. }
        ));
        let mut message = native("sensor_msgs", "JointState");
        message
            .write(&serde_json::json!({"name": vec!["\n".repeat(32000); 16]}))
            .unwrap();
        let value = typed(&message);
        assert!(!value.is_complete());
    }
    #[test]
    fn exhausted_nodes_preserve_original_object_count_and_depth_is_explicit() {
        let message = native("geometry_msgs", "Pose");
        unsafe {
            let root = typed_message(
                sp_schema(message.0 .0, 0),
                sp_message(message.0 .0, 0),
                0,
                &mut TypedBudget {
                    nodes: 1,
                    wire: 4096,
                },
            )
            .unwrap();
            assert!(
                matches!(root,TypedValue::Object { ref fields, original_fields:2 } if fields.is_empty())
            );
            let root = typed_message(
                sp_schema(message.0 .0, 0),
                sp_message(message.0 .0, 0),
                structured::MAX_DEPTH,
                &mut TypedBudget::new(),
            )
            .unwrap();
            assert!(matches!(root, TypedValue::Unsupported { .. }));
        }
    }
}

/// Export bounded native metadata; no native pointer escapes this module.
pub(crate) unsafe fn describe_schema(
    root: Ptr,
    depth: usize,
    budget: &mut usize,
) -> Result<Value, String> {
    if depth > 16 || root.is_null() || sp_count(root) > 256 {
        return Err("Schema exceeds depth/member budget".into());
    }
    let mut fields = Vec::new();
    for i in 0..sp_count(root) {
        *budget = budget.checked_sub(1).ok_or("Schema exceeds node budget")?;
        let m = sp_member(root, i);
        let kind = sp_type(m);
        let nested = if kind == 18 {
            describe_schema(sp_nested(m), depth + 1, budget)?
        } else {
            Value::Null
        };
        fields.push(serde_json::json!({"name":CStr::from_ptr(sp_name(m)).to_string_lossy(),"kind":kind,"array":sp_array(m),"bound":sp_bound(m),"string_bound":sp_string_bound(m),"fields":nested}));
    }
    Ok(Value::Array(fields))
}
