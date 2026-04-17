use std::collections::HashMap;
use std::fmt;

use dm_store_lib::path;
use dm_store_lib::ParamType;

/// Access mode for an object or parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    ReadOnly,
    ReadWrite,
}

impl fmt::Display for Access {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Access::ReadOnly => write!(f, "readOnly"),
            Access::ReadWrite => write!(f, "readWrite"),
        }
    }
}

/// Value constraints for a parameter.
#[derive(Debug, Clone, PartialEq)]
pub enum ValueConstraint {
    None,
    SignedRange { min: Option<i64>, max: Option<i64> },
    UnsignedRange { min: Option<u64>, max: Option<u64> },
    Length { max: Option<usize> },
    Enum(Vec<String>),
}

/// Schema definition of a parameter.
#[derive(Debug, Clone)]
pub struct ParamSchema {
    /// Full template path, e.g. "Device.Bridging.Bridge.{i}.Enable"
    pub path: String,
    /// The dm-store ParamType used for storage.
    pub param_type: ParamType,
    /// Original data type string from JSON, e.g. "unsignedInt(0:61440)".
    pub data_type_raw: String,
    /// Access mode.
    pub access: Access,
    /// Default value (if any).
    pub default: Option<String>,
    /// Constant value (immutable; returned directly).
    pub const_value: Option<String>,
    /// Whether this is a list type (e.g., pathRef[]).
    pub is_list: bool,
    /// Value constraint.
    pub constraint: ValueConstraint,
    /// Path reference targets (for pathRef / pathRef[] types).
    pub path_ref: Vec<String>,
    /// The parent object path.
    pub object_path: String,
}

/// Schema definition of an object.
#[derive(Debug, Clone)]
pub struct ObjectSchema {
    /// Full path, e.g. "Device.Bridging.Bridge.{i}."
    pub path: String,
    /// Access mode of the object itself.
    pub access: Access,
    /// Whether this is a template path (contains {i}).
    pub is_template: bool,
    /// Whether this is a multi-instance table (parent of {i} templates).
    pub is_multi: bool,
    /// Parent object path.
    pub parent_path: Option<String>,
    /// Unique key names (for multi-instance objects).
    pub unique_keys: Vec<String>,
    /// Leaf names of child parameters.
    pub param_names: Vec<String>,
    /// Paths of direct child objects.
    pub child_object_paths: Vec<String>,
}

/// The complete in-memory schema, keyed by template paths.
#[derive(Default)]
pub struct DmSchema {
    objects: HashMap<String, ObjectSchema>,
    params: HashMap<String, ParamSchema>,
}

impl DmSchema {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn add_object(&mut self, obj: ObjectSchema) {
        // Register as child of parent
        if let Some(ref parent) = obj.parent_path {
            if let Some(parent_obj) = self.objects.get_mut(parent) {
                if !parent_obj.child_object_paths.contains(&obj.path) {
                    parent_obj.child_object_paths.push(obj.path.clone());
                }
            }
        }
        self.objects.insert(obj.path.clone(), obj);
    }

    /// Mark an object as multi-instance table and update its access mode.
    pub fn mark_multi(&mut self, path: &str, child_access: Access) {
        if let Some(obj) = self.objects.get_mut(path) {
            obj.is_multi = true;
            // Propagate access from the template child (add/del operate on the table)
            if child_access == Access::ReadWrite {
                obj.access = Access::ReadWrite;
            }
        }
    }

    pub fn add_param(&mut self, param: ParamSchema) {
        // Register leaf name in parent object
        let leaf = path::leaf_name(&param.path).to_string();
        if let Some(obj) = self.objects.get_mut(&param.object_path) {
            if !obj.param_names.contains(&leaf) {
                obj.param_names.push(leaf);
            }
        }
        self.params.insert(param.path.clone(), param);
    }

    /// Look up a parameter schema. Accepts both template and concrete paths.
    pub fn get_param(&self, path_str: &str) -> Option<&ParamSchema> {
        // Try exact match first (template path)
        if let Some(s) = self.params.get(path_str) {
            return Some(s);
        }
        // Canonicalize concrete path to template and try again
        let canonical = path::canonicalize(path_str);
        self.params.get(&canonical)
    }

    /// Look up an object schema. Accepts both template and concrete paths.
    pub fn get_object(&self, path_str: &str) -> Option<&ObjectSchema> {
        if let Some(s) = self.objects.get(path_str) {
            return Some(s);
        }
        let canonical = path::canonicalize(path_str);
        self.objects.get(&canonical)
    }

    /// Check if an object exists in the schema.
    pub fn has_object(&self, path_str: &str) -> bool {
        self.get_object(path_str).is_some()
    }

    /// Check if a parameter exists in the schema.
    pub fn has_param(&self, path_str: &str) -> bool {
        self.get_param(path_str).is_some()
    }

    /// List all object paths in the schema (sorted).
    pub fn object_paths(&self) -> Vec<&str> {
        let mut paths: Vec<&str> = self.objects.keys().map(|s| s.as_str()).collect();
        paths.sort();
        paths
    }

    /// List all parameter paths in the schema (sorted).
    pub fn param_paths(&self) -> Vec<&str> {
        let mut paths: Vec<&str> = self.params.keys().map(|s| s.as_str()).collect();
        paths.sort();
        paths
    }

    /// Get all parameter schemas for an object (by template object path).
    pub fn params_for_object(&self, obj_path: &str) -> Vec<&ParamSchema> {
        let canonical = if path::is_template_path(obj_path) {
            obj_path.to_string()
        } else {
            path::canonicalize(obj_path)
        };
        self.params
            .values()
            .filter(|p| p.object_path == canonical)
            .collect()
    }
}
