//! Conservative presentation of protocol-v1 diagnostic strings, not a schema parser.
use std::borrow::Cow;

pub fn readable(raw: &str) -> Cow<'_, str> {
    let Some(simple) = raw
        .strip_prefix("Simple(")
        .and_then(|s| s.strip_suffix(')'))
    else {
        return Cow::Borrowed(raw);
    };
    let Some((kind, value)) = simple.split_once('(') else {
        return Cow::Borrowed(raw);
    };
    let Some(value) = value.strip_suffix(')') else {
        return Cow::Borrowed(raw);
    };
    match kind {
        // Rust Debug and JSON share common string escapes. Unsupported Rust-only
        // escapes intentionally fall back to the exact diagnostic representation.
        "String" => serde_json::from_str::<String>(value)
            .map(Cow::Owned)
            .unwrap_or(Cow::Borrowed(raw)),
        "Boolean" if matches!(value, "true" | "false") => Cow::Borrowed(value),
        "Float" | "Double" if value.parse::<f64>().is_ok() => Cow::Borrowed(value),
        "Char" | "WChar" | "Octet" | "Uint8" | "Uint16" | "Uint32" | "Uint64"
            if value.parse::<u64>().is_ok() =>
        {
            Cow::Borrowed(value)
        }
        "Int8" | "Int16" | "Int32" | "Int64" if value.parse::<i64>().is_ok() => {
            Cow::Borrowed(value)
        }
        _ => Cow::Borrowed(raw),
    }
}

pub fn truncated(name: &str, raw: &str) -> bool {
    (name == "[preview truncated]" && raw == "true") || raw.ends_with(" … [preview truncated]")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scalar_values_preserve_precision_and_string_content() {
        for (raw, expected) in [
            (r#"Simple(String(""))"#, ""),
            (
                r#"Simple(String("line\n\"ROS\" café 🤖"))"#,
                "line\n\"ROS\" café 🤖",
            ),
            (
                "Simple(Uint64(18446744073709551615))",
                "18446744073709551615",
            ),
            ("Simple(Double(-0.0))", "-0.0"),
            ("Simple(Boolean(false))", "false"),
            (
                r#"Simple(String("Simple(Int32(42))"))"#,
                "Simple(Int32(42))",
            ),
        ] {
            assert_eq!(readable(raw), expected);
        }
    }

    #[test]
    fn incomplete_unknown_and_nested_values_remain_exact() {
        for raw in [
            "Simple(String(\"incomplete … [preview truncated]",
            "Simple(Message({nested: 42}))",
            "Sequence(Int32Sequence([1, 2]))",
            "Simple(Int32(not a number))",
            "Simple(LongDouble(0x1234))",
            r#"Simple(String("\u{7}"))"#,
            "plain fixture text",
        ] {
            assert_eq!(readable(raw), raw);
        }
        assert!(truncated("data", "incomplete … [preview truncated]"));
        assert!(truncated("[preview truncated]", "true"));
        assert!(!truncated("[preview truncated]", "false"));
        assert!(!truncated("data", "[preview truncated] is literal text"));
    }
}
