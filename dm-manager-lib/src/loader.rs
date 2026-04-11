use std::collections::HashSet;
use std::fs;

use dm_store_lib::path::{self, is_template_path};
use dm_store_lib::DmStore;

use crate::error::DmManagerError;
use crate::parser::{self, ObjectDefinition, ParamDefinition};
use crate::schema::{Access, DmSchema, ObjectSchema, ParamSchema};

/// Load a JSON schema file and populate the schema + register writable items in dm-store.
pub fn load_schema_file(
    path: &str,
    schema: &mut DmSchema,
    store: &mut DmStore,
) -> Result<(), DmManagerError> {
    let json = fs::read_to_string(path)?;
    load_schema_str(&json, schema, store)
}

/// Load schema from a JSON string.
pub fn load_schema_str(
    json: &str,
    schema: &mut DmSchema,
    store: &mut DmStore,
) -> Result<(), DmManagerError> {
    let defs = parser::parse_json(json)?;
    load_schema(defs, schema, store)
}

/// Load schema from parsed definitions.
pub fn load_schema(
    defs: Vec<ObjectDefinition>,
    schema: &mut DmSchema,
    store: &mut DmStore,
) -> Result<(), DmManagerError> {
    // Sort definitions by path depth to ensure parents are defined before children
    let mut sorted_defs = defs;
    sorted_defs.sort_by_key(|d| d.object.matches('.').count());

    // Track objects we've registered in dm-store to avoid duplicates
    let mut registered_objects: HashSet<String> = HashSet::new();

    for def in &sorted_defs {
        let obj_path = &def.object;

        // Validate the object path
        path::validate_path(obj_path).map_err(|e| DmManagerError::Schema(e.to_string()))?;

        if !path::is_object_path(obj_path) {
            return Err(DmManagerError::Schema(format!(
                "object path must end with '.': {}",
                obj_path
            )));
        }

        let access = parse_access(&def.access)?;
        let is_template = is_template_path(obj_path);

        // Parse unique keys
        let unique_keys = def
            .unique_keys
            .as_deref()
            .map(|s| s.split(',').map(|k| k.trim().to_string()).collect())
            .unwrap_or_default();

        // Auto-create implicit parent objects in schema
        ensure_parent_objects_in_schema(obj_path, schema);

        // Add object to schema
        let parent = path::parent_path(obj_path);
        schema.add_object(ObjectSchema {
            path: obj_path.clone(),
            access,
            is_template,
            is_multi: false,
            parent_path: parent.clone(),
            unique_keys,
            param_names: Vec::new(),
            child_object_paths: Vec::new(),
        });

        // If this object's last segment is {i}, mark its parent as multi-instance
        // e.g. "Device.Bridging.Bridge.{i}." -> parent "Device.Bridging.Bridge." is multi
        let leaf = path::leaf_name(obj_path);
        if leaf == "{i}" {
            if let Some(ref parent_path) = parent {
                schema.mark_multi(parent_path, access);
            }
        }

        // Register object hierarchy in dm-store
        ensure_object_in_store(obj_path, store, &mut registered_objects)?;

        // Process parameters
        for param_def in &def.parameters {
            load_parameter(obj_path, param_def, schema, store)?;
        }
    }

    Ok(())
}

fn load_parameter(
    obj_path: &str,
    param_def: &ParamDefinition,
    schema: &mut DmSchema,
    store: &mut DmStore,
) -> Result<(), DmManagerError> {
    // Skip parameters with unresolvable placeholders (e.g., {BBF_VENDOR_PREFIX})
    if param_def.name.contains('{') && !param_def.name.contains("{i}") {
        log::debug!(
            "skipping parameter with vendor placeholder: {}{}",
            obj_path,
            param_def.name
        );
        return Ok(());
    }

    let param_path = format!("{}{}", obj_path, param_def.name);
    let access = parse_access(&param_def.access)?;

    let parsed = parser::parse_data_type(&param_def.data_type, param_def.enum_values.as_deref())?;

    // Add to schema (all parameters, both ro and rw)
    schema.add_param(ParamSchema {
        path: param_path.clone(),
        param_type: parsed.param_type,
        data_type_raw: param_def.data_type.clone(),
        access,
        default: param_def.default.clone(),
        const_value: param_def.const_value.clone(),
        is_list: parsed.is_list,
        constraint: parsed.constraint,
        path_ref: param_def.path_ref.clone().unwrap_or_default(),
        object_path: obj_path.to_string(),
    });

    // Only register writable parameters in dm-store
    if access == Access::ReadWrite {
        store
            .define_parameter(
                &param_path,
                parsed.param_type,
                true, // writable
                param_def
                    .default
                    .as_deref()
                    .or(param_def.const_value.as_deref()),
            )
            .map_err(|e| {
                // AlreadyExists is fine when loading multiple files
                if matches!(e, dm_store_lib::DmStoreError::AlreadyExists(_)) {
                    log::debug!("parameter already defined: {}", param_path);
                    return DmManagerError::Schema(String::new());
                }
                DmManagerError::Store(e)
            })
            .or_else(|e| {
                if let DmManagerError::Schema(ref s) = e {
                    if s.is_empty() {
                        return Ok(());
                    }
                }
                Err(e)
            })?;
    }

    Ok(())
}

/// Ensure all ancestor objects exist in the schema.
fn ensure_parent_objects_in_schema(obj_path: &str, schema: &mut DmSchema) {
    let mut current = path::parent_path(obj_path);
    while let Some(parent) = current {
        if schema.has_object(&parent) {
            break;
        }
        let grandparent = path::parent_path(&parent);
        let is_template = is_template_path(&parent);

        schema.add_object(ObjectSchema {
            path: parent.clone(),
            access: Access::ReadOnly,
            is_template,
            is_multi: false,
            parent_path: grandparent.clone(),
            unique_keys: Vec::new(),
            param_names: Vec::new(),
            child_object_paths: Vec::new(),
        });

        current = grandparent;
    }
}

/// Ensure an object and its ancestors are registered in dm-store.
fn ensure_object_in_store(
    obj_path: &str,
    store: &mut DmStore,
    registered: &mut HashSet<String>,
) -> Result<(), DmManagerError> {
    if registered.contains(obj_path) {
        return Ok(());
    }

    // First ensure parents are registered
    if let Some(parent) = path::parent_path(obj_path) {
        ensure_object_in_store(&parent, store, registered)?;
    }

    // If the last segment is {i}, this is a template instance object
    // and its parent is the multi-instance table
    let leaf = path::leaf_name(obj_path);
    if leaf == "{i}" {
        if let Some(parent) = path::parent_path(obj_path) {
            let _ = store.define_object(&parent, true);
            registered.insert(parent);
        }
    }

    let _ = store.define_object(obj_path, false);

    registered.insert(obj_path.to_string());
    Ok(())
}

fn parse_access(access: &str) -> Result<Access, DmManagerError> {
    match access {
        "readOnly" => Ok(Access::ReadOnly),
        "readWrite" => Ok(Access::ReadWrite),
        _ => Err(DmManagerError::Schema(format!(
            "unknown access mode: {}",
            access
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_load_vlanbridge_snippet() {
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
                    },
                    {
                        "name": "BridgeNumberOfEntries",
                        "access": "readOnly",
                        "dataType": "unsignedInt"
                    }
                ]
            },
            {
                "object": "Device.Bridging.Bridge.{i}.",
                "uniqueKeys": "Name,Alias",
                "access": "readWrite",
                "parameters": [
                    {
                        "name": "Enable",
                        "access": "readWrite",
                        "dataType": "boolean"
                    },
                    {
                        "name": "Status",
                        "access": "readOnly",
                        "dataType": "enum",
                        "enum": ["Disabled", "Enabled", "Error"],
                        "default": "Disabled"
                    }
                ]
            }
        ]"#;

        let mut schema = DmSchema::new();
        let mut store = DmStore::open_in_memory().unwrap();

        load_schema_str(json, &mut schema, &mut store).unwrap();

        // Verify schema
        assert!(schema.has_object("Device."));
        assert!(schema.has_object("Device.Bridging."));
        assert!(schema.has_object("Device.Bridging.Bridge.{i}."));
        assert!(schema.has_param("Device.Bridging.MaxBridgeEntries"));
        assert!(schema.has_param("Device.Bridging.Bridge.{i}.Enable"));
        assert!(schema.has_param("Device.Bridging.Bridge.{i}.Status"));

        // Verify read-only param has const value in schema
        let max = schema
            .get_param("Device.Bridging.MaxBridgeEntries")
            .unwrap();
        assert_eq!(max.const_value, Some("20".to_string()));
        assert_eq!(max.access, Access::ReadOnly);

        // Verify writable param is in dm-store schema
        // (Add an instance and check that the writable param was propagated)
        let mut session = store.session().unwrap();
        let r = session.add("Device.Bridging.Bridge.").unwrap();
        assert_eq!(r.instance_number, 1);

        // Writable param should exist in dm-store
        let p = session.get("Device.Bridging.Bridge.1.Enable").unwrap();
        assert!(p.writable);

        session.commit().unwrap();
    }
}
