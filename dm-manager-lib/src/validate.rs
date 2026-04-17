use dm_store_lib::path::canonicalize;
use dm_store_lib::ParamType;

use crate::error::DmManagerError;
use crate::schema::{ParamSchema, ValueConstraint};

/// Validate a value against the parameter's type and constraints.
pub fn validate_value(value: &str, schema: &ParamSchema) -> Result<(), DmManagerError> {
    let path = &schema.path;

    match schema.param_type {
        ParamType::Boolean => {
            if !matches!(value, "true" | "false" | "1" | "0") {
                return Err(DmManagerError::InvalidValue {
                    path: path.clone(),
                    reason: format!("expected boolean (true/false/1/0), got '{}'", value),
                });
            }
        }
        ParamType::Int => {
            let v = value
                .parse::<i32>()
                .map_err(|_| DmManagerError::InvalidValue {
                    path: path.clone(),
                    reason: format!("expected int, got '{}'", value),
                })?;
            check_signed_range(path, v as i64, &schema.constraint)?;
        }
        ParamType::UnsignedInt => {
            let v = value
                .parse::<u32>()
                .map_err(|_| DmManagerError::InvalidValue {
                    path: path.clone(),
                    reason: format!("expected unsignedInt, got '{}'", value),
                })?;
            check_unsigned_range(path, v as u64, &schema.constraint)?;
        }
        ParamType::Long => {
            let v = value
                .parse::<i64>()
                .map_err(|_| DmManagerError::InvalidValue {
                    path: path.clone(),
                    reason: format!("expected long, got '{}'", value),
                })?;
            check_signed_range(path, v, &schema.constraint)?;
        }
        ParamType::UnsignedLong => {
            let v = value
                .parse::<u64>()
                .map_err(|_| DmManagerError::InvalidValue {
                    path: path.clone(),
                    reason: format!("expected unsignedLong, got '{}'", value),
                })?;
            check_unsigned_range(path, v, &schema.constraint)?;
        }
        ParamType::String => {
            check_string_constraint(path, value, &schema.constraint)?;
        }
        ParamType::HexBinary => {
            if !value.len().is_multiple_of(2) {
                return Err(DmManagerError::InvalidValue {
                    path: path.clone(),
                    reason: "hexBinary must have even length".to_string(),
                });
            }
            if !value.chars().all(|c| c.is_ascii_hexdigit()) {
                return Err(DmManagerError::InvalidValue {
                    path: path.clone(),
                    reason: "hexBinary must contain only hex digits".to_string(),
                });
            }
        }
        ParamType::Base64 => {
            if !value
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
            {
                return Err(DmManagerError::InvalidValue {
                    path: path.clone(),
                    reason: "invalid base64 characters".to_string(),
                });
            }
        }
        ParamType::DateTime => {
            // Basic check: not empty
            if value.is_empty() {
                return Err(DmManagerError::InvalidValue {
                    path: path.clone(),
                    reason: "dateTime cannot be empty".to_string(),
                });
            }
        }
    }

    // Validate pathRef: each comma-separated path must match an allowed target
    if !schema.path_ref.is_empty() && !value.is_empty() {
        check_path_ref(path, value, &schema.path_ref)?;
    }

    Ok(())
}

/// Validate that each comma-separated path in the value starts with one of the
/// allowed pathRef targets. Instance numbers in the value are canonicalized to {i}
/// before matching, so "Device.Bridging.Bridge.1.Port.2." matches target
/// "Device.Bridging.Bridge.{i}.Port.".
fn check_path_ref(path: &str, value: &str, allowed: &[String]) -> Result<(), DmManagerError> {
    for entry in value.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        // Canonicalize both with and without trailing dot so
        // "Device.Bridging.Bridge.1.Port.2" and "Device.Bridging.Bridge.1.Port.2."
        // both match target "Device.Bridging.Bridge.{i}.Port."
        let canonical = canonicalize(entry);
        let canonical_dot = if canonical.ends_with('.') {
            canonical.clone()
        } else {
            format!("{}.", canonical)
        };
        let matched = allowed
            .iter()
            .any(|target| canonical.starts_with(target) || canonical_dot.starts_with(target));
        if !matched {
            return Err(DmManagerError::InvalidValue {
                path: path.to_string(),
                reason: format!(
                    "path '{}' does not match allowed pathRef targets: {:?}",
                    entry, allowed
                ),
            });
        }
    }
    Ok(())
}

fn check_signed_range(
    path: &str,
    value: i64,
    constraint: &ValueConstraint,
) -> Result<(), DmManagerError> {
    if let ValueConstraint::SignedRange { min, max } = constraint {
        if let Some(min_val) = min {
            if value < *min_val {
                return Err(DmManagerError::InvalidValue {
                    path: path.to_string(),
                    reason: format!("value {} is below minimum {}", value, min_val),
                });
            }
        }
        if let Some(max_val) = max {
            if value > *max_val {
                return Err(DmManagerError::InvalidValue {
                    path: path.to_string(),
                    reason: format!("value {} exceeds maximum {}", value, max_val),
                });
            }
        }
    }
    Ok(())
}

fn check_unsigned_range(
    path: &str,
    value: u64,
    constraint: &ValueConstraint,
) -> Result<(), DmManagerError> {
    if let ValueConstraint::UnsignedRange { min, max } = constraint {
        if let Some(min_val) = min {
            if value < *min_val {
                return Err(DmManagerError::InvalidValue {
                    path: path.to_string(),
                    reason: format!("value {} is below minimum {}", value, min_val),
                });
            }
        }
        if let Some(max_val) = max {
            if value > *max_val {
                return Err(DmManagerError::InvalidValue {
                    path: path.to_string(),
                    reason: format!("value {} exceeds maximum {}", value, max_val),
                });
            }
        }
    }
    Ok(())
}

fn check_string_constraint(
    path: &str,
    value: &str,
    constraint: &ValueConstraint,
) -> Result<(), DmManagerError> {
    match constraint {
        ValueConstraint::Length { max: Some(max_len) } => {
            if value.len() > *max_len {
                return Err(DmManagerError::InvalidValue {
                    path: path.to_string(),
                    reason: format!(
                        "string length {} exceeds maximum {}",
                        value.len(),
                        max_len
                    ),
                });
            }
        }
        ValueConstraint::Length { max: None } => {}
        ValueConstraint::Enum(variants) => {
            if !variants.iter().any(|v| v == value) {
                return Err(DmManagerError::InvalidValue {
                    path: path.to_string(),
                    reason: format!("value '{}' not in allowed values: {:?}", value, variants),
                });
            }
        }
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Access;

    fn make_schema(param_type: ParamType, constraint: ValueConstraint) -> ParamSchema {
        ParamSchema {
            path: "Test.Param".to_string(),
            param_type,
            data_type_raw: String::new(),
            access: Access::ReadWrite,
            default: None,
            const_value: None,
            is_list: false,
            constraint,
            path_ref: Vec::new(),
            object_path: "Test.".to_string(),
        }
    }

    #[test]
    fn test_validate_boolean() {
        let s = make_schema(ParamType::Boolean, ValueConstraint::None);
        assert!(validate_value("true", &s).is_ok());
        assert!(validate_value("false", &s).is_ok());
        assert!(validate_value("1", &s).is_ok());
        assert!(validate_value("0", &s).is_ok());
        assert!(validate_value("yes", &s).is_err());
    }

    #[test]
    fn test_validate_uint_range() {
        let s = make_schema(
            ParamType::UnsignedInt,
            ValueConstraint::UnsignedRange {
                min: Some(0),
                max: Some(100),
            },
        );
        assert!(validate_value("50", &s).is_ok());
        assert!(validate_value("0", &s).is_ok());
        assert!(validate_value("100", &s).is_ok());
        assert!(validate_value("101", &s).is_err());
        assert!(validate_value("-1", &s).is_err());
        assert!(validate_value("abc", &s).is_err());
    }

    #[test]
    fn test_validate_string_length() {
        let s = make_schema(ParamType::String, ValueConstraint::Length { max: Some(5) });
        assert!(validate_value("hi", &s).is_ok());
        assert!(validate_value("hello", &s).is_ok());
        assert!(validate_value("toolong", &s).is_err());
    }

    #[test]
    fn test_validate_enum() {
        let s = make_schema(
            ParamType::String,
            ValueConstraint::Enum(vec!["Disabled".to_string(), "Enabled".to_string()]),
        );
        assert!(validate_value("Disabled", &s).is_ok());
        assert!(validate_value("Enabled", &s).is_ok());
        assert!(validate_value("Unknown", &s).is_err());
    }

    #[test]
    fn test_validate_unsigned_long_above_i64_max() {
        let s = make_schema(
            ParamType::UnsignedLong,
            ValueConstraint::UnsignedRange {
                min: Some(0),
                max: Some(u64::MAX),
            },
        );
        assert!(validate_value("9223372036854775808", &s).is_ok());
    }

    #[test]
    fn test_validate_hex() {
        let s = make_schema(ParamType::HexBinary, ValueConstraint::None);
        assert!(validate_value("0A1B", &s).is_ok());
        assert!(validate_value("0A1", &s).is_err()); // odd length
        assert!(validate_value("ZZZZ", &s).is_err()); // invalid chars
    }

    fn make_pathref_schema(targets: Vec<String>) -> ParamSchema {
        ParamSchema {
            path: "Test.Ref".to_string(),
            param_type: ParamType::String,
            data_type_raw: "pathRef[]".to_string(),
            access: Access::ReadWrite,
            default: None,
            const_value: None,
            is_list: true,
            constraint: ValueConstraint::None,
            path_ref: targets,
            object_path: "Test.".to_string(),
        }
    }

    #[test]
    fn test_validate_pathref_single() {
        let s = make_pathref_schema(vec!["Device.Bridging.Bridge.{i}.Port.".to_string()]);
        // Valid: concrete path with trailing dot
        assert!(validate_value("Device.Bridging.Bridge.1.Port.2.", &s).is_ok());
        // Valid: concrete path without trailing dot
        assert!(validate_value("Device.Bridging.Bridge.1.Port.2", &s).is_ok());
        // Invalid: wrong target
        assert!(validate_value("Device.WiFi.Radio.1.", &s).is_err());
        assert!(validate_value("Device.WiFi.Radio.1", &s).is_err());
    }

    #[test]
    fn test_validate_pathref_comma_separated() {
        let s = make_pathref_schema(vec!["Device.Bridging.Bridge.{i}.Port.".to_string()]);
        // Valid: multiple paths all under allowed target
        assert!(validate_value(
            "Device.Bridging.Bridge.1.Port.1.,Device.Bridging.Bridge.1.Port.2.",
            &s
        )
        .is_ok());
        // Invalid: one path is wrong
        assert!(
            validate_value("Device.Bridging.Bridge.1.Port.1.,Device.WiFi.Radio.1.", &s).is_err()
        );
    }

    #[test]
    fn test_validate_pathref_multiple_targets() {
        let s = make_pathref_schema(vec![
            "Device.Bridging.Bridge.".to_string(),
            "Device.WiFi.Radio.".to_string(),
        ]);
        // Both targets allowed
        assert!(validate_value("Device.Bridging.Bridge.1.", &s).is_ok());
        assert!(validate_value("Device.WiFi.Radio.2.", &s).is_ok());
        // Wrong target
        assert!(validate_value("Device.Other.Thing.1.", &s).is_err());
    }

    #[test]
    fn test_validate_pathref_empty_value() {
        let s = make_pathref_schema(vec!["Device.Bridging.Bridge.{i}.Port.".to_string()]);
        // Empty value is ok (no paths to validate)
        assert!(validate_value("", &s).is_ok());
    }
}
