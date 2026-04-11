use std::collections::HashMap;

use dm_store_lib::path::{self, canonicalize};
use dm_store_lib::{AddResult, DmStore, DmStoreError, Parameter};

use crate::error::DmManagerError;
use crate::loader;
use crate::schema::{Access, DmSchema, ParamSchema};
use crate::validate;

/// Callback type for read-only parameters.
/// Receives the concrete path, returns the value string.
pub type ReadHook = Box<dyn Fn(&str) -> Result<String, DmManagerError> + Send>;

/// Callback type for instance enumeration.
/// Receives the concrete table path, returns sorted instance numbers.
pub type InstanceHook = Box<dyn Fn(&str) -> Result<Vec<u32>, DmManagerError> + Send>;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ObjectAvailability {
    Stored,
    HookDerived,
}

fn resolve_object_availability<F>(
    instance_hooks: &HashMap<String, InstanceHook>,
    object_exists: &F,
    object_path: &str,
) -> Result<Option<ObjectAvailability>, DmManagerError>
where
    F: Fn(&str) -> Result<bool, DmManagerError>,
{
    if object_exists(object_path)? {
        return Ok(Some(ObjectAvailability::Stored));
    }

    let Some(parent) = path::parent_path(object_path) else {
        return Ok(None);
    };

    if let Some(instance_number) = path::instance_number(object_path) {
        let template_table = canonicalize(&parent);
        if let Some(hook) = instance_hooks.get(&template_table) {
            let instances = hook(&parent).map_err(|e| DmManagerError::HookError {
                path: parent.clone(),
                reason: e.to_string(),
            })?;
            return Ok(instances
                .into_iter()
                .any(|num| num == instance_number)
                .then_some(ObjectAvailability::HookDerived));
        }
        return Ok(None);
    }

    match resolve_object_availability(instance_hooks, object_exists, &parent)? {
        Some(ObjectAvailability::HookDerived) => Ok(Some(ObjectAvailability::HookDerived)),
        Some(ObjectAvailability::Stored) | None => Ok(None),
    }
}

/// The data model manager, wrapping dm-store with schema validation and hooks.
pub struct DmManager {
    schema: DmSchema,
    store: DmStore,
    read_hooks: HashMap<String, ReadHook>,
    instance_hooks: HashMap<String, InstanceHook>,
}

impl DmManager {
    /// Create a new DmManager with a DmStore. Schema starts empty.
    pub fn new(store: DmStore) -> Self {
        DmManager {
            schema: DmSchema::new(),
            store,
            read_hooks: HashMap::new(),
            instance_hooks: HashMap::new(),
        }
    }

    /// Create with an in-memory store (for testing).
    pub fn new_in_memory() -> Result<Self, DmManagerError> {
        let store = DmStore::open_in_memory()?;
        Ok(Self::new(store))
    }

    // --- Schema loading ---

    /// Load a JSON schema file.
    pub fn load_schema_file(&mut self, path: &str) -> Result<(), DmManagerError> {
        loader::load_schema_file(path, &mut self.schema, &mut self.store)
    }

    /// Load schema from a JSON string.
    pub fn load_schema_str(&mut self, json: &str) -> Result<(), DmManagerError> {
        loader::load_schema_str(json, &mut self.schema, &mut self.store)
    }

    // --- Schema query ---

    /// Get the schema for a parameter path.
    pub fn param_schema(&self, path: &str) -> Option<&ParamSchema> {
        self.schema.get_param(path)
    }

    /// Get the schema for an object path.
    pub fn object_schema(&self, path: &str) -> Option<&crate::schema::ObjectSchema> {
        self.schema.get_object(path)
    }

    /// List all schema paths (objects and parameters, sorted).
    pub fn schema_paths(&self) -> Vec<&str> {
        let mut paths = self.schema.object_paths();
        paths.extend(self.schema.param_paths());
        paths.sort();
        paths
    }

    // --- Hook registration ---

    /// Register a read hook for a parameter template path.
    /// Overrides the default callback for read-only parameters.
    pub fn register_read_hook<F>(&mut self, template_path: &str, hook: F)
    where
        F: Fn(&str) -> Result<String, DmManagerError> + Send + 'static,
    {
        self.read_hooks
            .insert(template_path.to_string(), Box::new(hook));
    }

    /// Register an instance hook for a multi-instance object template path.
    /// When registered, instances() calls this hook instead of dm-store.
    pub fn register_instance_hook<F>(&mut self, template_table_path: &str, hook: F)
    where
        F: Fn(&str) -> Result<Vec<u32>, DmManagerError> + Send + 'static,
    {
        self.instance_hooks
            .insert(template_table_path.to_string(), Box::new(hook));
    }

    /// Register default read hooks for all read-only parameters that have no
    /// const or default value. Returns "0" for numeric types, "false" for boolean,
    /// "" for dateTime, and "default" for string types.
    pub fn register_default_read_hooks(&mut self) {
        use dm_store_lib::ParamType;

        let entries: Vec<(String, ParamType)> = self
            .schema
            .param_paths()
            .iter()
            .filter_map(|path| {
                let ps = self.schema.get_param(path)?;
                if ps.access != Access::ReadOnly {
                    return None;
                }
                if ps.const_value.is_some() || ps.default.is_some() {
                    return None;
                }
                Some((path.to_string(), ps.param_type))
            })
            .collect();

        for (path, ptype) in entries {
            let default_val = match ptype {
                ParamType::Int
                | ParamType::UnsignedInt
                | ParamType::Long
                | ParamType::UnsignedLong => "0",
                ParamType::Boolean => "false",
                ParamType::DateTime => "",
                _ => "default",
            };
            let val = default_val.to_string();
            self.register_read_hook(&path, move |_| Ok(val.clone()));
        }
    }

    // --- Data access ---

    /// Get a parameter value with schema validation and hook resolution.
    ///
    /// Resolution order:
    /// 1. Validate path exists in schema
    /// 2. const_value -> return directly
    /// 3. Writable -> dm-store first, then schema default
    /// 4. Read-only -> registered hook, then default callback (schema default/const/empty)
    pub fn get(&self, path_str: &str) -> Result<Parameter, DmManagerError> {
        let ps = self.resolve_param_schema(path_str)?;
        let object_path = path::parent_path(path_str)
            .ok_or_else(|| DmManagerError::NotInSchema(path_str.to_string()))?;
        self.ensure_object_available(&object_path, path_str)?;

        // Const value: return immediately
        if let Some(ref val) = ps.const_value {
            return Ok(Parameter {
                path: path_str.to_string(),
                value: Some(val.clone()),
                param_type: ps.param_type,
                writable: false,
            });
        }

        if ps.access == Access::ReadWrite {
            // Writable: try dm-store first
            match self.store.get(path_str) {
                Ok(p) => {
                    if p.value.is_some() {
                        return Ok(p);
                    }
                    // Value is None in store, fall back to schema default
                    Ok(Parameter {
                        path: path_str.to_string(),
                        value: ps.default.clone(),
                        param_type: ps.param_type,
                        writable: true,
                    })
                }
                Err(DmStoreError::NotFound(_)) => {
                    // Not in db yet (no instance created), return default
                    Ok(Parameter {
                        path: path_str.to_string(),
                        value: ps.default.clone(),
                        param_type: ps.param_type,
                        writable: true,
                    })
                }
                Err(e) => Err(DmManagerError::Store(e)),
            }
        } else {
            // Read-only: check registered hook first
            let template = canonicalize(path_str);
            if let Some(hook) = self.read_hooks.get(&template) {
                let val = hook(path_str).map_err(|e| DmManagerError::HookError {
                    path: path_str.to_string(),
                    reason: e.to_string(),
                })?;
                return Ok(Parameter {
                    path: path_str.to_string(),
                    value: Some(val),
                    param_type: ps.param_type,
                    writable: false,
                });
            }

            // Default callback: return schema default or empty
            let value = ps
                .default
                .clone()
                .or_else(|| ps.const_value.clone())
                .or_else(|| Some(String::new()));
            Ok(Parameter {
                path: path_str.to_string(),
                value,
                param_type: ps.param_type,
                writable: false,
            })
        }
    }

    /// Get all parameters for an object instance.
    /// Merges writable params from dm-store with read-only params from hooks/defaults.
    pub fn get_object(&self, obj_path: &str) -> Result<Vec<Parameter>, DmManagerError> {
        if !path::is_object_path(obj_path) {
            return Err(DmManagerError::NotInSchema(format!(
                "not an object path (must end with '.'): {}",
                obj_path
            )));
        }

        let template = canonicalize(obj_path);
        let obj_schema = self
            .schema
            .get_object(&template)
            .ok_or_else(|| DmManagerError::NotInSchema(obj_path.to_string()))?;
        self.ensure_object_available(obj_path, obj_path)?;

        let mut params = Vec::new();
        for leaf_name in &obj_schema.param_names {
            let param_path = format!("{}{}", obj_path, leaf_name);
            match self.get(&param_path) {
                Ok(p) => params.push(p),
                Err(DmManagerError::NotInSchema(_)) => {
                    // Skip params that aren't in schema for this concrete path
                }
                Err(e) => return Err(e),
            }
        }
        Ok(params)
    }

    /// Get instance numbers for a multi-instance object.
    /// Checks instance hook first (if registered), then dm-store.
    pub fn instances(&self, table_path: &str) -> Result<Vec<u32>, DmManagerError> {
        if !path::is_object_path(table_path) {
            return Err(DmManagerError::NotInSchema(format!(
                "not an object path: {}",
                table_path
            )));
        }

        let template = canonicalize(table_path);
        // Check for instance hook
        if let Some(hook) = self.instance_hooks.get(&template) {
            return hook(table_path);
        }

        // Default: delegate to dm-store
        self.store.instances(table_path).map_err(|e| e.into())
    }

    // --- Session ---

    /// Start a transactional session for write operations.
    pub fn session(&mut self) -> Result<DmManagerSession<'_>, DmManagerError> {
        let session = self.store.session()?;
        Ok(DmManagerSession {
            schema: &self.schema,
            store_session: session,
            read_hooks: &self.read_hooks,
            instance_hooks: &self.instance_hooks,
        })
    }

    /// Access the underlying DmStore.
    pub fn store(&self) -> &DmStore {
        &self.store
    }

    /// Mutable access to the underlying DmStore.
    pub fn store_mut(&mut self) -> &mut DmStore {
        &mut self.store
    }

    /// Access the schema.
    pub fn schema(&self) -> &DmSchema {
        &self.schema
    }

    // --- Internal helpers ---

    fn resolve_param_schema(&self, path_str: &str) -> Result<&ParamSchema, DmManagerError> {
        self.schema
            .get_param(path_str)
            .ok_or_else(|| DmManagerError::NotInSchema(path_str.to_string()))
    }

    fn ensure_object_available(
        &self,
        object_path: &str,
        missing_path: &str,
    ) -> Result<(), DmManagerError> {
        if canonicalize(object_path) == object_path {
            return Ok(());
        }

        let availability = resolve_object_availability(
            &self.instance_hooks,
            &|path| {
                self.store
                    .object_exists(path)
                    .map_err(DmManagerError::Store)
            },
            object_path,
        )?;

        if availability.is_some() {
            Ok(())
        } else {
            Err(DmManagerError::NotInDb(missing_path.to_string()))
        }
    }
}

/// A transactional session for write operations with schema validation.
pub struct DmManagerSession<'a> {
    schema: &'a DmSchema,
    store_session: dm_store_lib::session::Session<'a>,
    read_hooks: &'a HashMap<String, ReadHook>,
    instance_hooks: &'a HashMap<String, InstanceHook>,
}

impl<'a> DmManagerSession<'a> {
    /// Set a parameter value with full validation.
    ///
    /// 1. Validate path exists in schema
    /// 2. Check if writable
    /// 3. Validate value against type/constraints
    /// 4. Delegate to dm-store session
    pub fn set(&mut self, path_str: &str, value: &str) -> Result<(), DmManagerError> {
        let ps = self
            .schema
            .get_param(path_str)
            .ok_or_else(|| DmManagerError::NotInSchema(path_str.to_string()))?;

        if ps.access != Access::ReadWrite {
            return Err(DmManagerError::ReadOnly(path_str.to_string()));
        }

        // Validate value against schema constraints
        validate::validate_value(value, ps)?;

        // Delegate to dm-store
        self.store_session
            .set(path_str, value)
            .map_err(|e| match e {
                DmStoreError::NotFound(_) => DmManagerError::NotInDb(path_str.to_string()),
                other => DmManagerError::Store(other),
            })
    }

    /// Add instance to a multi-instance object.
    pub fn add(&mut self, table_path: &str) -> Result<AddResult, DmManagerError> {
        if !path::is_object_path(table_path) {
            return Err(DmManagerError::NotMultiInstance(table_path.to_string()));
        }
        self.ensure_mutable_table(table_path)?;

        // Check if an instance hook is registered - if so, instances are externally managed
        let template = canonicalize(table_path);
        if self.instance_hooks.contains_key(&template) {
            return Err(DmManagerError::Schema(format!(
                "cannot add instance: {} has an instance hook (externally managed)",
                table_path
            )));
        }

        self.store_session.add(table_path).map_err(|e| e.into())
    }

    /// Delete an instance.
    pub fn delete(&mut self, instance_path: &str) -> Result<(), DmManagerError> {
        let parent = path::parent_path(instance_path)
            .ok_or_else(|| DmManagerError::NotMultiInstance(instance_path.to_string()))?;
        self.ensure_mutable_table(&parent)?;

        // Check if instance hook is registered for the parent table
        let template = canonicalize(&parent);
        if self.instance_hooks.contains_key(&template) {
            return Err(DmManagerError::Schema(format!(
                "cannot delete instance: {} has an instance hook (externally managed)",
                parent
            )));
        }

        self.store_session
            .delete(instance_path)
            .map_err(|e| e.into())
    }

    /// Get a parameter value within the session context.
    pub fn get(&self, path_str: &str) -> Result<Parameter, DmManagerError> {
        let ps = self
            .schema
            .get_param(path_str)
            .ok_or_else(|| DmManagerError::NotInSchema(path_str.to_string()))?;
        let object_path = path::parent_path(path_str)
            .ok_or_else(|| DmManagerError::NotInSchema(path_str.to_string()))?;
        self.ensure_object_available(&object_path, path_str)?;

        if let Some(ref val) = ps.const_value {
            return Ok(Parameter {
                path: path_str.to_string(),
                value: Some(val.clone()),
                param_type: ps.param_type,
                writable: false,
            });
        }

        if ps.access == Access::ReadWrite {
            match self.store_session.get(path_str) {
                Ok(p) => {
                    if p.value.is_some() {
                        return Ok(p);
                    }
                    Ok(Parameter {
                        path: path_str.to_string(),
                        value: ps.default.clone(),
                        param_type: ps.param_type,
                        writable: true,
                    })
                }
                Err(DmStoreError::NotFound(_)) => Ok(Parameter {
                    path: path_str.to_string(),
                    value: ps.default.clone(),
                    param_type: ps.param_type,
                    writable: true,
                }),
                Err(e) => Err(DmManagerError::Store(e)),
            }
        } else {
            let template = canonicalize(path_str);
            if let Some(hook) = self.read_hooks.get(&template) {
                let val = hook(path_str).map_err(|e| DmManagerError::HookError {
                    path: path_str.to_string(),
                    reason: e.to_string(),
                })?;
                return Ok(Parameter {
                    path: path_str.to_string(),
                    value: Some(val),
                    param_type: ps.param_type,
                    writable: false,
                });
            }

            let value = ps
                .default
                .clone()
                .or_else(|| ps.const_value.clone())
                .or_else(|| Some(String::new()));
            Ok(Parameter {
                path: path_str.to_string(),
                value,
                param_type: ps.param_type,
                writable: false,
            })
        }
    }

    /// Get all parameters for an object within session context.
    pub fn get_object(&self, obj_path: &str) -> Result<Vec<Parameter>, DmManagerError> {
        if !path::is_object_path(obj_path) {
            return Err(DmManagerError::NotInSchema(format!(
                "not an object path: {}",
                obj_path
            )));
        }

        let template = canonicalize(obj_path);
        let obj_schema = self
            .schema
            .get_object(&template)
            .ok_or_else(|| DmManagerError::NotInSchema(obj_path.to_string()))?;
        self.ensure_object_available(obj_path, obj_path)?;

        let mut params = Vec::new();
        for leaf_name in &obj_schema.param_names {
            let param_path = format!("{}{}", obj_path, leaf_name);
            match self.get(&param_path) {
                Ok(p) => params.push(p),
                Err(DmManagerError::NotInSchema(_)) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(params)
    }

    /// Get instances within session context.
    pub fn instances(&self, table_path: &str) -> Result<Vec<u32>, DmManagerError> {
        let template = canonicalize(table_path);
        if let Some(hook) = self.instance_hooks.get(&template) {
            return hook(table_path);
        }
        self.store_session
            .instances(table_path)
            .map_err(|e| e.into())
    }

    /// Commit the session.
    pub fn commit(self) -> Result<(), DmManagerError> {
        self.store_session.commit().map_err(|e| e.into())
    }

    /// Abort the session (rollback).
    pub fn abort(self) -> Result<(), DmManagerError> {
        self.store_session.abort().map_err(|e| e.into())
    }

    fn ensure_object_available(
        &self,
        object_path: &str,
        missing_path: &str,
    ) -> Result<(), DmManagerError> {
        if canonicalize(object_path) == object_path {
            return Ok(());
        }

        let availability = resolve_object_availability(
            self.instance_hooks,
            &|path| {
                self.store_session
                    .object_exists(path)
                    .map_err(DmManagerError::Store)
            },
            object_path,
        )?;

        if availability.is_some() {
            Ok(())
        } else {
            Err(DmManagerError::NotInDb(missing_path.to_string()))
        }
    }

    fn ensure_mutable_table(&self, table_path: &str) -> Result<(), DmManagerError> {
        let object = self
            .schema
            .get_object(table_path)
            .ok_or_else(|| DmManagerError::NotInSchema(table_path.to_string()))?;

        if !object.is_multi {
            return Err(DmManagerError::NotMultiInstance(table_path.to_string()));
        }

        if object.access != Access::ReadWrite {
            return Err(DmManagerError::ReadOnly(table_path.to_string()));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use dm_store_lib::ParamType;

    fn setup_manager() -> DmManager {
        let mut mgr = DmManager::new_in_memory().unwrap();
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
                    },
                    {
                        "name": "Alias",
                        "access": "readWrite",
                        "dataType": "string(:64)"
                    }
                ]
            }
        ]"#;
        mgr.load_schema_str(json).unwrap();
        mgr
    }

    #[test]
    fn test_get_const_value() {
        let mgr = setup_manager();
        let p = mgr.get("Device.Bridging.MaxBridgeEntries").unwrap();
        assert_eq!(p.value, Some("20".to_string()));
        assert!(!p.writable);
    }

    #[test]
    fn test_get_readonly_default() {
        let mgr = setup_manager();
        // BridgeNumberOfEntries has no const, no hook -> default callback returns empty
        let p = mgr.get("Device.Bridging.BridgeNumberOfEntries").unwrap();
        assert_eq!(p.value, Some(String::new()));
        assert!(!p.writable);
    }

    #[test]
    fn test_get_not_in_schema() {
        let mgr = setup_manager();
        assert!(mgr.get("Device.NonExistent.Param").is_err());
    }

    #[test]
    fn test_get_missing_instance_returns_not_in_db() {
        let mgr = setup_manager();
        let result = mgr.get("Device.Bridging.Bridge.9.Enable");
        assert!(matches!(result, Err(DmManagerError::NotInDb(_))));
    }

    #[test]
    fn test_get_object_missing_instance_returns_not_in_db() {
        let mgr = setup_manager();
        let result = mgr.get_object("Device.Bridging.Bridge.9.");
        assert!(matches!(result, Err(DmManagerError::NotInDb(_))));
    }

    #[test]
    fn test_add_and_get_writable() {
        let mut mgr = setup_manager();

        // Add an instance
        let mut session = mgr.session().unwrap();
        let r = session.add("Device.Bridging.Bridge.").unwrap();
        assert_eq!(r.instance_number, 1);
        session.commit().unwrap();

        // Get writable param
        let p = mgr.get("Device.Bridging.Bridge.1.Enable").unwrap();
        assert!(p.writable);
    }

    #[test]
    fn test_set_with_validation() {
        let mut mgr = setup_manager();

        // Add instance
        let mut session = mgr.session().unwrap();
        session.add("Device.Bridging.Bridge.").unwrap();
        session.commit().unwrap();

        // Set valid value
        let mut session = mgr.session().unwrap();
        session
            .set("Device.Bridging.Bridge.1.Enable", "true")
            .unwrap();
        session.commit().unwrap();

        // Verify
        let p = mgr.get("Device.Bridging.Bridge.1.Enable").unwrap();
        assert_eq!(p.value, Some("true".to_string()));
    }

    #[test]
    fn test_set_invalid_boolean() {
        let mut mgr = setup_manager();

        let mut session = mgr.session().unwrap();
        session.add("Device.Bridging.Bridge.").unwrap();
        session.commit().unwrap();

        let mut session = mgr.session().unwrap();
        let result = session.set("Device.Bridging.Bridge.1.Enable", "invalid");
        assert!(result.is_err());
    }

    #[test]
    fn test_set_readonly_rejected() {
        let mut mgr = setup_manager();

        let mut session = mgr.session().unwrap();
        session.add("Device.Bridging.Bridge.").unwrap();
        session.commit().unwrap();

        let mut session = mgr.session().unwrap();
        let result = session.set("Device.Bridging.Bridge.1.Status", "Enabled");
        assert!(matches!(result, Err(DmManagerError::ReadOnly(_))));
    }

    #[test]
    fn test_set_string_length_validation() {
        let mut mgr = setup_manager();

        let mut session = mgr.session().unwrap();
        session.add("Device.Bridging.Bridge.").unwrap();
        session.commit().unwrap();

        // Valid: within 64 chars
        let mut session = mgr.session().unwrap();
        session
            .set("Device.Bridging.Bridge.1.Alias", "MyBridge")
            .unwrap();
        session.commit().unwrap();

        // Invalid: exceeds 64 chars
        let long_string = "a".repeat(65);
        let mut session = mgr.session().unwrap();
        let result = session.set("Device.Bridging.Bridge.1.Alias", &long_string);
        assert!(matches!(result, Err(DmManagerError::InvalidValue { .. })));
    }

    #[test]
    fn test_get_object() {
        let mut mgr = setup_manager();

        let mut session = mgr.session().unwrap();
        session.add("Device.Bridging.Bridge.").unwrap();
        session.commit().unwrap();

        let params = mgr.get_object("Device.Bridging.Bridge.1.").unwrap();
        // Should have Enable (rw), Status (ro), Alias (rw)
        assert_eq!(params.len(), 3);
    }

    #[test]
    fn test_instances() {
        let mut mgr = setup_manager();

        let mut session = mgr.session().unwrap();
        session.add("Device.Bridging.Bridge.").unwrap();
        session.add("Device.Bridging.Bridge.").unwrap();
        session.commit().unwrap();

        let nums = mgr.instances("Device.Bridging.Bridge.").unwrap();
        assert_eq!(nums, vec![1, 2]);
    }

    #[test]
    fn test_read_hook() {
        let mut mgr = setup_manager();

        // Register a hook for BridgeNumberOfEntries
        mgr.register_read_hook("Device.Bridging.BridgeNumberOfEntries", |_path| {
            Ok("5".to_string())
        });

        let p = mgr.get("Device.Bridging.BridgeNumberOfEntries").unwrap();
        assert_eq!(p.value, Some("5".to_string()));
    }

    #[test]
    fn test_read_hook_template() {
        let mut mgr = setup_manager();

        // Add instance first
        let mut session = mgr.session().unwrap();
        session.add("Device.Bridging.Bridge.").unwrap();
        session.commit().unwrap();

        // Register hook for read-only Status (template path)
        mgr.register_read_hook("Device.Bridging.Bridge.{i}.Status", |_path| {
            Ok("Enabled".to_string())
        });

        let p = mgr.get("Device.Bridging.Bridge.1.Status").unwrap();
        assert_eq!(p.value, Some("Enabled".to_string()));
    }

    #[test]
    fn test_instance_hook() {
        let mut mgr = setup_manager();

        // Register instance hook
        mgr.register_instance_hook("Device.Bridging.Bridge.", |_path| Ok(vec![10, 20, 30]));

        let nums = mgr.instances("Device.Bridging.Bridge.").unwrap();
        assert_eq!(nums, vec![10, 20, 30]);
    }

    #[test]
    fn test_get_readonly_with_instance_hook() {
        let mut mgr = setup_manager();
        mgr.register_instance_hook("Device.Bridging.Bridge.", |_path| Ok(vec![10]));
        mgr.register_read_hook("Device.Bridging.Bridge.{i}.Status", |_path| {
            Ok("Enabled".to_string())
        });

        let p = mgr.get("Device.Bridging.Bridge.10.Status").unwrap();
        assert_eq!(p.value, Some("Enabled".to_string()));
    }

    #[test]
    fn test_instance_hook_blocks_add() {
        let mut mgr = setup_manager();

        mgr.register_instance_hook("Device.Bridging.Bridge.", |_| Ok(vec![1]));

        let mut session = mgr.session().unwrap();
        let result = session.add("Device.Bridging.Bridge.");
        assert!(result.is_err());
    }

    #[test]
    fn test_readonly_table_blocks_add_and_delete() {
        let mut mgr = DmManager::new_in_memory().unwrap();
        mgr.load_schema_str(
            r#"[
                {
                    "object": "Device.ReadOnlyTable.{i}.",
                    "access": "readOnly",
                    "parameters": [
                        {
                            "name": "Status",
                            "access": "readOnly",
                            "dataType": "string"
                        }
                    ]
                }
            ]"#,
        )
        .unwrap();

        {
            let mut session = mgr.session().unwrap();
            let add_result = session.add("Device.ReadOnlyTable.");
            assert!(matches!(add_result, Err(DmManagerError::ReadOnly(_))));
        }

        {
            let mut raw = mgr.store_mut().session().unwrap();
            raw.add("Device.ReadOnlyTable.").unwrap();
            raw.commit().unwrap();
        }

        let mut session = mgr.session().unwrap();
        let delete_result = session.delete("Device.ReadOnlyTable.1.");
        assert!(matches!(delete_result, Err(DmManagerError::ReadOnly(_))));
    }

    #[test]
    fn test_session_abort() {
        let mut mgr = setup_manager();

        let mut session = mgr.session().unwrap();
        session.add("Device.Bridging.Bridge.").unwrap();
        session.commit().unwrap();

        // Set in session, then abort
        let mut session = mgr.session().unwrap();
        session
            .set("Device.Bridging.Bridge.1.Enable", "false")
            .unwrap();
        session.abort().unwrap();

        // Value should not have changed (default or original)
        let p = mgr.get("Device.Bridging.Bridge.1.Enable").unwrap();
        // The original value from add is the default (None in dm-store), so schema default applies
        assert_ne!(p.value, Some("false".to_string()));
    }

    #[test]
    fn test_schema_query() {
        let mgr = setup_manager();

        let ps = mgr
            .param_schema("Device.Bridging.Bridge.{i}.Enable")
            .unwrap();
        assert_eq!(ps.access, Access::ReadWrite);
        assert_eq!(ps.param_type, ParamType::Boolean);

        let os = mgr.object_schema("Device.Bridging.Bridge.{i}.").unwrap();
        assert_eq!(os.unique_keys, vec!["Name", "Alias"]);
    }
}
