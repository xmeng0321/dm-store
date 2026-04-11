use dm_store_lib::ParamType;
use serde::Deserialize;

use crate::error::DmManagerError;
use crate::schema::ValueConstraint;

/// JSON deserialization struct for an object definition.
#[derive(Deserialize, Debug)]
pub struct ObjectDefinition {
    pub object: String,
    #[serde(default = "default_read_only")]
    pub access: String,
    #[serde(default)]
    pub parameters: Vec<ParamDefinition>,
    #[serde(default, rename = "uniqueKeys")]
    pub unique_keys: Option<String>,
}

/// JSON deserialization struct for a parameter definition.
#[derive(Deserialize, Debug)]
pub struct ParamDefinition {
    pub name: String,
    #[serde(default = "default_read_only")]
    pub access: String,
    #[serde(rename = "dataType")]
    pub data_type: String,
    pub default: Option<String>,
    #[serde(rename = "const")]
    pub const_value: Option<String>,
    #[serde(rename = "enum")]
    pub enum_values: Option<Vec<String>>,
    #[serde(default, rename = "pathRef")]
    pub path_ref: Option<Vec<String>>,
}

fn default_read_only() -> String {
    "readOnly".to_string()
}

/// Parsed data type information.
#[derive(Debug)]
pub struct ParsedDataType {
    pub param_type: ParamType,
    pub constraint: ValueConstraint,
    pub is_list: bool,
}

/// Parse a JSON dataType string like "unsignedInt(0:61440)" or "string(:64)" or "pathRef[]".
pub fn parse_data_type(
    raw: &str,
    enum_values: Option<&[String]>,
) -> Result<ParsedDataType, DmManagerError> {
    let mut s = raw.trim();
    let is_list = s.ends_with("[]");
    if is_list {
        s = &s[..s.len() - 2];
    }

    // Check for range/length constraint: type(min:max) or type(:max)
    let (base_type, constraint) = if let Some(paren_start) = s.find('(') {
        if !s.ends_with(')') {
            return Err(DmManagerError::Schema(format!(
                "missing closing ')' in data type: {}",
                raw
            )));
        }
        let base = &s[..paren_start];
        let constraint_str = &s[paren_start + 1..s.len() - 1]; // strip ( and )
        let constraint = parse_constraint(base, constraint_str)?;
        (base, constraint)
    } else {
        (s, ValueConstraint::None)
    };

    // Override constraint for enum types
    let constraint = if let Some(variants) = enum_values {
        ValueConstraint::Enum(variants.to_vec())
    } else {
        constraint
    };

    let param_type = map_base_type(base_type)?;

    Ok(ParsedDataType {
        param_type,
        constraint,
        is_list,
    })
}

fn parse_constraint(
    base_type: &str,
    constraint_str: &str,
) -> Result<ValueConstraint, DmManagerError> {
    let parts: Vec<&str> = constraint_str.split(':').collect();
    if parts.len() != 2 {
        return Err(DmManagerError::Schema(format!(
            "invalid constraint: ({})",
            constraint_str
        )));
    }

    if base_type == "string" {
        // String constraint: (:max_length)
        let max = if parts[1].is_empty() {
            None
        } else {
            Some(parts[1].parse::<usize>().map_err(|_| {
                DmManagerError::Schema(format!("invalid length constraint: {}", parts[1]))
            })?)
        };
        Ok(ValueConstraint::Length { max })
    } else if matches!(
        base_type,
        "unsignedInt" | "unsignedLong" | "StatsCounter32" | "StatsCounter64"
    ) {
        let min =
            if parts[0].is_empty() {
                None
            } else {
                Some(parts[0].parse::<u64>().map_err(|_| {
                    DmManagerError::Schema(format!("invalid range min: {}", parts[0]))
                })?)
            };
        let max =
            if parts[1].is_empty() {
                None
            } else {
                Some(parts[1].parse::<u64>().map_err(|_| {
                    DmManagerError::Schema(format!("invalid range max: {}", parts[1]))
                })?)
            };
        Ok(ValueConstraint::UnsignedRange { min, max })
    } else {
        // Numeric constraint: (min:max)
        let min =
            if parts[0].is_empty() {
                None
            } else {
                Some(parts[0].parse::<i64>().map_err(|_| {
                    DmManagerError::Schema(format!("invalid range min: {}", parts[0]))
                })?)
            };
        let max =
            if parts[1].is_empty() {
                None
            } else {
                Some(parts[1].parse::<i64>().map_err(|_| {
                    DmManagerError::Schema(format!("invalid range max: {}", parts[1]))
                })?)
            };
        Ok(ValueConstraint::SignedRange { min, max })
    }
}

fn map_base_type(base: &str) -> Result<ParamType, DmManagerError> {
    match base {
        "string" => Ok(ParamType::String),
        "int" => Ok(ParamType::Int),
        "unsignedInt" => Ok(ParamType::UnsignedInt),
        "long" => Ok(ParamType::Long),
        "unsignedLong" => Ok(ParamType::UnsignedLong),
        "boolean" => Ok(ParamType::Boolean),
        "dateTime" => Ok(ParamType::DateTime),
        "hexBinary" => Ok(ParamType::HexBinary),
        "base64" => Ok(ParamType::Base64),
        // TR-181 derived types mapped to base types
        "enum" => Ok(ParamType::String),
        "pathRef" => Ok(ParamType::String),
        "StatsCounter32" => Ok(ParamType::UnsignedInt),
        "StatsCounter64" => Ok(ParamType::UnsignedLong),
        _ => Err(DmManagerError::Schema(format!(
            "unknown data type: {}",
            base
        ))),
    }
}

/// Parse a JSON string or file content into object definitions.
pub fn parse_json(json: &str) -> Result<Vec<ObjectDefinition>, DmManagerError> {
    let defs: Vec<ObjectDefinition> = serde_json::from_str(json)?;
    Ok(defs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_simple_types() {
        let r = parse_data_type("boolean", None).unwrap();
        assert_eq!(r.param_type, ParamType::Boolean);
        assert_eq!(r.constraint, ValueConstraint::None);
        assert!(!r.is_list);
    }

    #[test]
    fn test_parse_string_length() {
        let r = parse_data_type("string(:64)", None).unwrap();
        assert_eq!(r.param_type, ParamType::String);
        assert_eq!(r.constraint, ValueConstraint::Length { max: Some(64) });
    }

    #[test]
    fn test_parse_uint_range() {
        let r = parse_data_type("unsignedInt(0:61440)", None).unwrap();
        assert_eq!(r.param_type, ParamType::UnsignedInt);
        assert_eq!(
            r.constraint,
            ValueConstraint::UnsignedRange {
                min: Some(0),
                max: Some(61440)
            }
        );
    }

    #[test]
    fn test_parse_int_range() {
        let r = parse_data_type("int(1:4094)", None).unwrap();
        assert_eq!(r.param_type, ParamType::Int);
        assert_eq!(
            r.constraint,
            ValueConstraint::SignedRange {
                min: Some(1),
                max: Some(4094)
            }
        );
    }

    #[test]
    fn test_parse_list_type() {
        let r = parse_data_type("pathRef[]", None).unwrap();
        assert_eq!(r.param_type, ParamType::String);
        assert!(r.is_list);
    }

    #[test]
    fn test_parse_list_with_range() {
        let r = parse_data_type("unsignedInt(0:7)[]", None).unwrap();
        assert_eq!(r.param_type, ParamType::UnsignedInt);
        assert!(r.is_list);
        assert_eq!(
            r.constraint,
            ValueConstraint::UnsignedRange {
                min: Some(0),
                max: Some(7)
            }
        );
    }

    #[test]
    fn test_parse_invalid_constraint_requires_closing_paren() {
        let err = parse_data_type("unsignedInt(0:5", None).unwrap_err();
        assert!(matches!(err, DmManagerError::Schema(_)));
    }

    #[test]
    fn test_parse_unsigned_long_full_range() {
        let r = parse_data_type("unsignedLong(0:18446744073709551615)", None).unwrap();
        assert_eq!(r.param_type, ParamType::UnsignedLong);
        assert_eq!(
            r.constraint,
            ValueConstraint::UnsignedRange {
                min: Some(0),
                max: Some(u64::MAX)
            }
        );
    }

    #[test]
    fn test_parse_enum() {
        let variants = vec!["Disabled".to_string(), "Enabled".to_string()];
        let r = parse_data_type("enum", Some(&variants)).unwrap();
        assert_eq!(r.param_type, ParamType::String);
        assert_eq!(r.constraint, ValueConstraint::Enum(variants));
    }

    #[test]
    fn test_parse_stats_counter() {
        let r = parse_data_type("StatsCounter32", None).unwrap();
        assert_eq!(r.param_type, ParamType::UnsignedInt);
    }

    #[test]
    fn test_parse_json_vlanbridge_snippet() {
        let json = r#"[
            {
                "object": "Device.Bridging.",
                "access": "readOnly",
                "parameters": [
                    {
                        "name": "MaxBridgeEntries",
                        "access": "readOnly",
                        "dataType": "unsignedInt",
                        "const": "20"
                    }
                ]
            }
        ]"#;
        let defs = parse_json(json).unwrap();
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].object, "Device.Bridging.");
        assert_eq!(defs[0].parameters.len(), 1);
        assert_eq!(defs[0].parameters[0].name, "MaxBridgeEntries");
        assert_eq!(defs[0].parameters[0].const_value, Some("20".to_string()));
    }
}
