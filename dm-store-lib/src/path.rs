use crate::error::DmStoreError;

/// Compute FNV-1a 64-bit hash of a string.
pub fn fnv1a_hash(s: &str) -> i64 {
    const FNV_OFFSET: u64 = 0xcbf29ce484222325;
    const FNV_PRIME: u64 = 0x100000001b3;

    let mut hash = FNV_OFFSET;
    for byte in s.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(FNV_PRIME);
    }
    // Store as i64 for SQLite INTEGER compatibility
    hash as i64
}

/// Check if a path refers to an object (ends with '.').
pub fn is_object_path(path: &str) -> bool {
    path.ends_with('.')
}

/// Check if a path is a template path containing `{i}`.
pub fn is_template_path(path: &str) -> bool {
    path.contains("{i}")
}

/// Get the parent object path.
/// "Device.WiFi.Radio.1.Enable" -> "Device.WiFi.Radio.1."
/// "Device.WiFi.Radio.1." -> "Device.WiFi.Radio."
/// "Device." -> None
pub fn parent_path(path: &str) -> Option<String> {
    let trimmed = path.strip_suffix('.').unwrap_or(path);
    trimmed.rfind('.').map(|pos| trimmed[..=pos].to_string())
}

/// Get the leaf name of a path.
/// "Device.WiFi.Radio.1.Enable" -> "Enable"
/// "Device.WiFi.Radio.1." -> "1"
pub fn leaf_name(path: &str) -> &str {
    let trimmed = path.strip_suffix('.').unwrap_or(path);
    match trimmed.rfind('.') {
        Some(pos) => &trimmed[pos + 1..],
        None => trimmed,
    }
}

/// Extract instance number from an instance path.
/// "Device.WiFi.Radio.1." -> Some(1)
/// "Device.WiFi.Radio." -> None
pub fn instance_number(path: &str) -> Option<u32> {
    let name = leaf_name(path);
    name.parse::<u32>().ok()
}

/// Build the template path for a multi-instance object.
/// "Device.WiFi.Radio." -> "Device.WiFi.Radio.{i}."
pub fn template_path(table_path: &str) -> String {
    format!("{}{{i}}.", table_path)
}

/// Build an instance path from a table path and instance number.
/// ("Device.WiFi.Radio.", 1) -> "Device.WiFi.Radio.1."
pub fn instance_path(table_path: &str, num: u32) -> String {
    format!("{}{}.", table_path, num)
}

/// Canonicalize a path: replace all numeric segments with `{i}`.
/// "Device.WiFi.1.SSID." -> "Device.WiFi.{i}.SSID."
/// "Device.WiFi.1.SSID.3.Name" -> "Device.WiFi.{i}.SSID.{i}.Name"
pub fn canonicalize(path: &str) -> String {
    let (trimmed, trailing_dot) = match path.strip_suffix('.') {
        Some(stripped) => (stripped, true),
        None => (path, false),
    };
    let segments: Vec<&str> = trimmed.split('.').collect();
    let result: Vec<&str> = segments
        .iter()
        .map(|s| if s.parse::<u32>().is_ok() { "{i}" } else { s })
        .collect();
    let mut out = result.join(".");
    if trailing_dot {
        out.push('.');
    }
    out
}

/// Extract instance numbers from a concrete path (in order).
/// "Device.WiFi.1.SSID." -> ["1"]
/// "Device.WiFi.1.SSID.3." -> ["1", "3"]
pub fn extract_instance_numbers(path: &str) -> Vec<String> {
    let trimmed = path.strip_suffix('.').unwrap_or(path);
    trimmed
        .split('.')
        .filter(|s| s.parse::<u32>().is_ok())
        .map(|s| s.to_string())
        .collect()
}

/// Resolve a template path by replacing {i} placeholders with numbers.
/// Numbers are consumed left-to-right.
/// resolve_template("Device.WiFi.{i}.SSID.{i}.Name", &["1", "3"]) -> "Device.WiFi.1.SSID.3.Name"
pub fn resolve_template(template: &str, numbers: &[&str]) -> String {
    let mut result = template.to_string();
    for num in numbers {
        result = result.replacen("{i}", num, 1);
    }
    result
}

/// Validate a TR-181 path. Returns Ok(()) or an error with the reason.
pub fn validate_path(path: &str) -> Result<(), DmStoreError> {
    if path.is_empty() {
        return Err(DmStoreError::InvalidPath {
            path: path.to_string(),
            reason: "path is empty".to_string(),
        });
    }

    if path.starts_with('.') {
        return Err(DmStoreError::InvalidPath {
            path: path.to_string(),
            reason: "path must not start with '.'".to_string(),
        });
    }

    if path.contains("..") {
        return Err(DmStoreError::InvalidPath {
            path: path.to_string(),
            reason: "path must not contain '..'".to_string(),
        });
    }

    let trimmed = path.strip_suffix('.').unwrap_or(path);

    for segment in trimmed.split('.') {
        if segment.is_empty() {
            return Err(DmStoreError::InvalidPath {
                path: path.to_string(),
                reason: "path contains an empty segment".to_string(),
            });
        }

        let has_braces = segment.contains('{') || segment.contains('}');
        if has_braces && segment != "{i}" {
            return Err(DmStoreError::InvalidPath {
                path: path.to_string(),
                reason: format!("invalid placeholder segment: {}", segment),
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_fnv1a_hash_deterministic() {
        let h1 = fnv1a_hash("Device.WiFi.Radio.1.Enable");
        let h2 = fnv1a_hash("Device.WiFi.Radio.1.Enable");
        assert_eq!(h1, h2);
    }

    #[test]
    fn test_fnv1a_hash_different() {
        let h1 = fnv1a_hash("Device.WiFi.Radio.1.Enable");
        let h2 = fnv1a_hash("Device.WiFi.Radio.2.Enable");
        assert_ne!(h1, h2);
    }

    #[test]
    fn test_is_object_path() {
        assert!(is_object_path("Device.WiFi.Radio.1."));
        assert!(!is_object_path("Device.WiFi.Radio.1.Enable"));
    }

    #[test]
    fn test_parent_path() {
        assert_eq!(
            parent_path("Device.WiFi.Radio.1.Enable"),
            Some("Device.WiFi.Radio.1.".to_string())
        );
        assert_eq!(
            parent_path("Device.WiFi.Radio.1."),
            Some("Device.WiFi.Radio.".to_string())
        );
        assert_eq!(parent_path("Device."), None);
    }

    #[test]
    fn test_leaf_name() {
        assert_eq!(leaf_name("Device.WiFi.Radio.1.Enable"), "Enable");
        assert_eq!(leaf_name("Device.WiFi.Radio.1."), "1");
        assert_eq!(leaf_name("Device."), "Device");
    }

    #[test]
    fn test_instance_number() {
        assert_eq!(instance_number("Device.WiFi.Radio.1."), Some(1));
        assert_eq!(instance_number("Device.WiFi.Radio."), None);
        assert_eq!(instance_number("Device.WiFi.Radio.42."), Some(42));
    }

    #[test]
    fn test_template_path() {
        assert_eq!(
            template_path("Device.WiFi.Radio."),
            "Device.WiFi.Radio.{i}."
        );
    }

    #[test]
    fn test_instance_path() {
        assert_eq!(
            instance_path("Device.WiFi.Radio.", 3),
            "Device.WiFi.Radio.3."
        );
    }

    #[test]
    fn test_validate_path() {
        assert!(validate_path("Device.WiFi.Radio.1.Enable").is_ok());
        assert!(validate_path("Device.").is_ok());
        assert!(validate_path("Device.WiFi.{i}.Enable").is_ok());
        assert!(validate_path("").is_err());
        assert!(validate_path(".Device").is_err());
        assert!(validate_path("Device..WiFi").is_err());
        assert!(validate_path("Device.WiFi.{x}.Enable").is_err());
        assert!(validate_path("Device.WiFi.foo{i}.Enable").is_err());
    }

    #[test]
    fn test_canonicalize() {
        assert_eq!(canonicalize("Device.WiFi.1.SSID."), "Device.WiFi.{i}.SSID.");
        assert_eq!(
            canonicalize("Device.WiFi.1.SSID.3.Name"),
            "Device.WiFi.{i}.SSID.{i}.Name"
        );
        assert_eq!(canonicalize("Device.WiFi.SSID."), "Device.WiFi.SSID.");
        assert_eq!(
            canonicalize("Device.WiFi.{i}.SSID."),
            "Device.WiFi.{i}.SSID."
        );
        assert_eq!(canonicalize("Device.WiFi.1."), "Device.WiFi.{i}.");
    }

    #[test]
    fn test_extract_instance_numbers() {
        assert_eq!(extract_instance_numbers("Device.WiFi.1.SSID."), vec!["1"]);
        assert_eq!(
            extract_instance_numbers("Device.WiFi.1.SSID.3."),
            vec!["1", "3"]
        );
        assert_eq!(
            extract_instance_numbers("Device.WiFi.1.SSID.3.Name"),
            vec!["1", "3"]
        );
        assert!(extract_instance_numbers("Device.WiFi.SSID.").is_empty());
    }

    #[test]
    fn test_resolve_template() {
        assert_eq!(
            resolve_template("Device.WiFi.{i}.SSID.{i}.Name", &["1", "3"]),
            "Device.WiFi.1.SSID.3.Name"
        );
        assert_eq!(
            resolve_template("Device.WiFi.{i}.Enable", &["5"]),
            "Device.WiFi.5.Enable"
        );
        assert_eq!(
            resolve_template("Device.WiFi.{i}.", &["1"]),
            "Device.WiFi.1."
        );
        // More numbers than placeholders: extras are ignored
        assert_eq!(
            resolve_template("Device.WiFi.{i}.Enable", &["1", "2"]),
            "Device.WiFi.1.Enable"
        );
    }
}
