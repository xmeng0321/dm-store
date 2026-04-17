use std::{fmt, str::FromStr};

/// TR-181 parameter type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamType {
    String,
    Int,
    UnsignedInt,
    Long,
    UnsignedLong,
    Boolean,
    DateTime,
    HexBinary,
    Base64,
}

impl ParamType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ParamType::String => "string",
            ParamType::Int => "int",
            ParamType::UnsignedInt => "unsignedInt",
            ParamType::Long => "long",
            ParamType::UnsignedLong => "unsignedLong",
            ParamType::Boolean => "boolean",
            ParamType::DateTime => "dateTime",
            ParamType::HexBinary => "hexBinary",
            ParamType::Base64 => "base64",
        }
    }

    pub fn parse_name(s: &str) -> Option<ParamType> {
        s.parse().ok()
    }
}

impl FromStr for ParamType {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "string" => Ok(ParamType::String),
            "int" => Ok(ParamType::Int),
            "unsignedInt" => Ok(ParamType::UnsignedInt),
            "long" => Ok(ParamType::Long),
            "unsignedLong" => Ok(ParamType::UnsignedLong),
            "boolean" => Ok(ParamType::Boolean),
            "dateTime" => Ok(ParamType::DateTime),
            "hexBinary" => Ok(ParamType::HexBinary),
            "base64" => Ok(ParamType::Base64),
            _ => Err(()),
        }
    }
}

impl fmt::Display for ParamType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A parameter with its metadata and value.
#[derive(Debug, Clone)]
pub struct Parameter {
    pub path: String,
    pub value: Option<String>,
    pub param_type: ParamType,
    pub writable: bool,
}

impl fmt::Display for Parameter {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let val = self.value.as_deref().unwrap_or("(empty)");
        let rw = if self.writable {
            "writable"
        } else {
            "read-only"
        };
        write!(f, "{} = {} ({}, {})", self.path, val, self.param_type, rw)
    }
}

/// An object node in the data model tree.
#[derive(Debug, Clone)]
pub struct Object {
    pub path: String,
    pub is_multi_instance: bool,
}

/// Result of an Add operation.
#[derive(Debug, Clone)]
pub struct AddResult {
    pub instance_number: u32,
    pub path: String,
}

/// Configuration for opening a DmStore.
#[derive(Debug, Clone)]
pub struct DmStoreConfig {
    /// Enable in-memory HashMap cache for O(1) exact lookups. Default: true.
    pub use_cache: bool,
}

impl Default for DmStoreConfig {
    fn default() -> Self {
        DmStoreConfig { use_cache: true }
    }
}

/// Row from dm_object / dm_schema_object, as surfaced by `DmStore::dump`.
#[derive(Debug, Clone)]
pub struct DumpedObject {
    pub path: String,
    pub is_multi: bool,
}

/// Row from dm_param / dm_schema_param, as surfaced by `DmStore::dump`.
#[derive(Debug, Clone)]
pub struct DumpedParam {
    pub path: String,
    pub value: Option<String>,
    pub param_type: String,
    pub writable: bool,
}

/// Structured snapshot of everything in the store. Returned by `DmStore::dump`
/// so CLIs can render without preparing their own SQL.
#[derive(Debug, Clone, Default)]
pub struct DmDump {
    pub objects: Vec<DumpedObject>,
    pub params: Vec<DumpedParam>,
    pub schema_objects: Vec<DumpedObject>,
    pub schema_params: Vec<DumpedParam>,
}
