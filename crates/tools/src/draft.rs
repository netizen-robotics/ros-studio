//! Validate native metadata drafts before submitting work to the backend.
use serde_json::Value;
pub fn validate(value: &Value, fields: &Value) -> Result<(), String> {
    check(value, fields, 0)
}
fn check(value: &Value, fields: &Value, depth: usize) -> Result<(), String> {
    if depth > 16 {
        return Err("Message nesting exceeds 16 levels".into());
    }
    let object = value.as_object().ok_or("Expected a message mapping")?;
    let fields = fields.as_array().ok_or("Load the interface schema first")?;
    for key in object.keys() {
        if !fields.iter().any(|f| f["name"].as_str() == Some(key)) {
            return Err(format!("Unknown field {key}"));
        }
    }
    for f in fields {
        let name = f["name"].as_str().ok_or("Invalid field metadata")?;
        if let Some(value) = object.get(name) {
            let result = if f["array"].as_u64().unwrap_or(0) > 0 {
                let a = value
                    .as_array()
                    .ok_or_else(|| format!("{name}: expected an array"))?;
                let bound = f["bound"].as_u64().unwrap_or(0) as usize;
                if (f["array"] == 1 && a.len() != bound)
                    || (f["array"] == 2 && bound > 0 && a.len() > bound)
                    || a.len() > 65536
                {
                    return Err(format!("{name}: invalid array length"));
                }
                for (i, v) in a.iter().enumerate() {
                    scalar(v, f, depth + 1).map_err(|e| format!("{name}[{i}]: {e}"))?;
                }
                Ok(())
            } else {
                scalar(value, f, depth + 1)
            };
            result.map_err(|e| format!("{name}: {e}"))?;
        }
    }
    Ok(())
}
fn scalar(v: &Value, f: &Value, depth: usize) -> Result<(), String> {
    let valid = match f["kind"].as_u64().unwrap_or(0) {
        1 => v.as_f64().is_some_and(|n| (n as f32).is_finite()),
        2 => v.as_f64().is_some_and(|n| n.is_finite()),
        4 | 7 | 8 => v.as_u64().is_some_and(|n| u8::try_from(n).is_ok()),
        9 => v.as_i64().is_some_and(|n| i8::try_from(n).is_ok()),
        10 => v.as_u64().is_some_and(|n| u16::try_from(n).is_ok()),
        11 => v.as_i64().is_some_and(|n| i16::try_from(n).is_ok()),
        12 => v.as_u64().is_some_and(|n| u32::try_from(n).is_ok()),
        13 => v.as_i64().is_some_and(|n| i32::try_from(n).is_ok()),
        14 => v.as_u64().is_some(),
        15 => v.as_i64().is_some(),
        6 => v.is_boolean(),
        16 => v.as_str().is_some_and(|s| {
            let bound = f["string_bound"].as_u64().unwrap_or(0) as usize;
            (bound == 0 || s.len() <= bound) && s.len() <= 65536
        }),
        18 => return check(v, &f["fields"], depth),
        _ => return Err("Unsupported ROS field type".into()),
    };
    if valid {
        Ok(())
    } else {
        Err("Value has the wrong type or is outside the interface bounds".into())
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn validates_nested_ranges_and_unknown_fields() {
        let schema = json!([{"name":"nested","kind":18,"fields":[{"name":"value","kind":14}]}]);
        assert!(validate(&json!({"nested":{"value":u64::MAX}}), &schema).is_ok());
        assert!(validate(&json!({"nested":{"value":-1}}), &schema).is_err());
        assert!(validate(&json!({"extra":0}), &schema).is_err());
    }
    #[test]
    fn array_shape_and_string_bounds_are_enforced() {
        let schema = json!([{"name":"strings","kind":16,"array":1,"bound":2,"string_bound":3}]);
        assert!(validate(&json!({"strings":["one","two"]}), &schema).is_ok());
        assert!(validate(&json!({"strings":["long","two"]}), &schema).is_err());
        assert!(validate(&json!({"strings":["one"]}), &schema).is_err());
    }
}
