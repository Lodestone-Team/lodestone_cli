//! Turning command line text into the core's typed setting values.

use serde_json::{json, Value};

use crate::api::error::{CliError, CliResult};

pub fn parse_bool(input: &str) -> CliResult<bool> {
    match input.trim().to_lowercase().as_str() {
        "true" | "yes" | "on" | "1" => Ok(true),
        "false" | "no" | "off" | "0" => Ok(false),
        _ => Err(CliError::usage(format!("`{input}` is not true or false"))),
    }
}

/// A `ConfigurableValue` (`{"type": ..., "value": ...}`) for `input`, checked against a
/// `ConfigurableValueType`.
pub fn configurable_value(value_type: &Value, input: &str) -> CliResult<Value> {
    let ty = value_type["type"].as_str().unwrap_or("String");
    let bound = |key: &str| value_type[key].as_f64();
    let check_range = |n: f64| -> CliResult<()> {
        if let Some(min) = bound("min") {
            if n < min {
                return Err(CliError::usage(format!(
                    "{input} is below the minimum {min}"
                )));
            }
        }
        if let Some(max) = bound("max") {
            if n > max {
                return Err(CliError::usage(format!(
                    "{input} is above the maximum {max}"
                )));
            }
        }
        Ok(())
    };
    let value = match ty {
        "Boolean" => json!(parse_bool(input)?),
        "Integer" => {
            let n: i32 = input
                .trim()
                .parse()
                .map_err(|_| CliError::usage(format!("`{input}` is not a whole number")))?;
            check_range(n as f64)?;
            json!(n)
        }
        "UnsignedInteger" => {
            let n: u32 = input.trim().parse().map_err(|_| {
                CliError::usage(format!("`{input}` is not a positive whole number"))
            })?;
            check_range(n as f64)?;
            json!(n)
        }
        "Float" => {
            let n: f32 = input
                .trim()
                .parse()
                .map_err(|_| CliError::usage(format!("`{input}` is not a number")))?;
            check_range(n as f64)?;
            json!(n)
        }
        "Enum" => {
            let options: Vec<&str> = value_type["options"]
                .as_array()
                .map(|o| o.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            let chosen = options
                .iter()
                .find(|o| **o == input)
                .or_else(|| options.iter().find(|o| o.eq_ignore_ascii_case(input)));
            match chosen {
                Some(option) => json!(option),
                None => {
                    return Err(CliError::usage(format!(
                        "`{input}` is not one of: {}",
                        options.join(", ")
                    )))
                }
            }
        }
        _ => json!(input),
    };
    Ok(json!({ "type": ty, "value": value }))
}

/// A short description of a `ConfigurableValueType`.
pub fn describe_type(value_type: &Value) -> String {
    let ty = value_type["type"].as_str().unwrap_or("?");
    match ty {
        "Enum" => {
            let options: Vec<&str> = value_type["options"]
                .as_array()
                .map(|o| o.iter().filter_map(Value::as_str).collect())
                .unwrap_or_default();
            if options.len() > 6 {
                format!("one of {} options", options.len())
            } else {
                format!("one of {}", options.join("|"))
            }
        }
        "UnsignedInteger" | "Integer" | "Float" => {
            let min = value_type["min"].as_f64();
            let max = value_type["max"].as_f64();
            let name = match ty {
                "Float" => "number",
                _ => "integer",
            };
            match (min, max) {
                (Some(min), Some(max)) => format!("{name} {min}..{max}"),
                (Some(min), None) => format!("{name} >= {min}"),
                (None, Some(max)) => format!("{name} <= {max}"),
                (None, None) => name.to_string(),
            }
        }
        "Boolean" => "true|false".to_string(),
        _ => "text".to_string(),
    }
}

/// The value inside a `ConfigurableValue`, as text.
pub fn display_value(value: &Value) -> String {
    match &value["value"] {
        Value::Null if value.is_null() => "-".to_string(),
        Value::String(s) => s.clone(),
        Value::Null => "-".to_string(),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn booleans() {
        for t in ["true", "Yes", "on", "1"] {
            assert!(parse_bool(t).unwrap());
        }
        for f in ["false", "NO", "off", "0"] {
            assert!(!parse_bool(f).unwrap());
        }
        assert!(parse_bool("maybe").is_err());
    }

    #[test]
    fn typed_values() {
        assert_eq!(
            configurable_value(
                &json!({"type": "UnsignedInteger", "min": 1, "max": 65535}),
                "25565"
            )
            .unwrap(),
            json!({"type": "UnsignedInteger", "value": 25565})
        );
        assert!(configurable_value(
            &json!({"type": "UnsignedInteger", "min": 1, "max": 65535}),
            "0"
        )
        .is_err());
        assert!(configurable_value(&json!({"type": "UnsignedInteger"}), "-1").is_err());
        assert_eq!(
            configurable_value(&json!({"type": "Integer"}), "-5").unwrap(),
            json!({"type": "Integer", "value": -5})
        );
        assert_eq!(
            configurable_value(&json!({"type": "Boolean"}), "off").unwrap(),
            json!({"type": "Boolean", "value": false})
        );
        assert_eq!(
            configurable_value(
                &json!({"type": "String", "regex": null}),
                "A Minecraft Server"
            )
            .unwrap(),
            json!({"type": "String", "value": "A Minecraft Server"})
        );
        assert_eq!(
            configurable_value(
                &json!({"type": "Enum", "options": ["peaceful", "easy", "hard"]}),
                "EASY"
            )
            .unwrap(),
            json!({"type": "Enum", "value": "easy"})
        );
        assert!(configurable_value(&json!({"type": "Enum", "options": ["a"]}), "b").is_err());
        assert_eq!(
            configurable_value(&json!({"type": "Float", "min": 0.0}), "0.5").unwrap(),
            json!({"type": "Float", "value": 0.5})
        );
    }

    #[test]
    fn descriptions() {
        assert_eq!(describe_type(&json!({"type": "Boolean"})), "true|false");
        assert_eq!(
            describe_type(&json!({"type": "UnsignedInteger", "min": 1, "max": 65535})),
            "integer 1..65535"
        );
        assert_eq!(
            display_value(&json!({"type": "Boolean", "value": true})),
            "true"
        );
        assert_eq!(display_value(&Value::Null), "-");
    }
}
