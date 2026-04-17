use std::collections::HashMap;

use rusqlite::Connection;

use crate::error::{DmStoreError, ResultExt};
use crate::path::{self, fnv1a_hash};
use crate::store::DmStore;
use crate::types::*;

pub struct Session<'a> {
    conn: &'a Connection,
    cache: &'a mut Option<HashMap<i64, Vec<Parameter>>>,
    instance_cache: &'a mut Option<HashMap<String, Vec<u32>>>,
    savepoint_name: String,
    closed: bool,
}

impl<'a> Session<'a> {
    pub(crate) fn new(
        conn: &'a Connection,
        cache: &'a mut Option<HashMap<i64, Vec<Parameter>>>,
        instance_cache: &'a mut Option<HashMap<String, Vec<u32>>>,
        savepoint_name: String,
    ) -> Result<Self, DmStoreError> {
        conn.execute_batch(&format!("SAVEPOINT {}", savepoint_name))
            .ctx_with(|| format!("opening session savepoint {}", savepoint_name))?;
        Ok(Session {
            conn,
            cache,
            instance_cache,
            savepoint_name,
            closed: false,
        })
    }

    fn check_open(&self) -> Result<(), DmStoreError> {
        if self.closed {
            Err(DmStoreError::SessionClosed)
        } else {
            Ok(())
        }
    }

    fn refresh_caches_from_db(&mut self) -> Result<(), DmStoreError> {
        // Only refresh caches that are currently enabled -- never flip a
        // disabled cache back on by assigning Some(...) unconditionally.
        if let Some(cache) = self.cache.as_mut() {
            *cache = DmStore::load_cache(self.conn)?;
        }
        if let Some(icache) = self.instance_cache.as_mut() {
            *icache = DmStore::load_instance_cache(self.conn)?;
        }
        Ok(())
    }

    /// Get a single parameter by exact path.
    pub fn get(&self, param_path: &str) -> Result<Parameter, DmStoreError> {
        self.check_open()?;
        path::validate_path(param_path)?;
        let hash = fnv1a_hash(param_path);

        // Try cache first
        if let Some(cache) = &self.cache {
            if let Some(params) = cache.get(&hash) {
                for p in params {
                    if p.path == param_path {
                        return Ok(p.clone());
                    }
                }
            }
            return Err(DmStoreError::NotFound(param_path.to_string()));
        }

        DmStore::get_from_db(self.conn, param_path, hash)
    }

    /// Get all parameters belonging to an object.
    pub fn get_object(&self, object_path: &str) -> Result<Vec<Parameter>, DmStoreError> {
        self.check_open()?;
        path::validate_path(object_path)?;
        if !path::is_object_path(object_path) {
            return Err(DmStoreError::InvalidPath {
                path: object_path.to_string(),
                reason: "expected object path ending with '.'".to_string(),
            });
        }

        // Always read from DB for object queries (cache is keyed by individual param hash)
        DmStore::get_object_from_db(self.conn, object_path)
    }

    /// Check whether an object exists by exact path.
    pub fn object_exists(&self, object_path: &str) -> Result<bool, DmStoreError> {
        self.check_open()?;
        path::validate_path(object_path)?;
        if !path::is_object_path(object_path) {
            return Err(DmStoreError::InvalidPath {
                path: object_path.to_string(),
                reason: "expected object path ending with '.'".to_string(),
            });
        }

        DmStore::object_exists_in_db(self.conn, object_path)
    }

    /// Get all instance numbers for a multi-instance object table path.
    pub fn instances(&self, table_path: &str) -> Result<Vec<u32>, DmStoreError> {
        self.check_open()?;
        path::validate_path(table_path)?;
        if !path::is_object_path(table_path) {
            return Err(DmStoreError::InvalidPath {
                path: table_path.to_string(),
                reason: "expected object path ending with '.'".to_string(),
            });
        }

        // Try instance cache first
        if let Some(icache) = &self.instance_cache {
            return Ok(icache.get(table_path).cloned().unwrap_or_default());
        }

        // Fall back to DB
        DmStore::instances_from_db(self.conn, table_path)
    }

    /// Set a parameter value.
    pub fn set(&mut self, param_path: &str, value: &str) -> Result<(), DmStoreError> {
        self.check_open()?;
        path::validate_path(param_path)?;
        let hash = fnv1a_hash(param_path);

        // Check writable
        let param = DmStore::get_from_db(self.conn, param_path, hash)?;
        if !param.writable {
            return Err(DmStoreError::ReadOnly(param_path.to_string()));
        }

        let rows = self.conn.execute(
            "UPDATE dm_param SET value = ?1 WHERE path_hash = ?2 AND path = ?3",
            rusqlite::params![value, hash, param_path],
        )?;

        if rows == 0 {
            return Err(DmStoreError::NotFound(param_path.to_string()));
        }

        // Update cache
        if let Some(cache) = &mut self.cache {
            if let Some(params) = cache.get_mut(&hash) {
                for p in params.iter_mut() {
                    if p.path == param_path {
                        p.value = Some(value.to_string());
                        break;
                    }
                }
            }
        }

        Ok(())
    }

    /// Set multiple parameters atomically.
    pub fn set_many(&mut self, updates: &[(&str, &str)]) -> Result<(), DmStoreError> {
        for (path, value) in updates {
            self.set(path, value)?;
        }
        Ok(())
    }

    /// Add a new instance to a multi-instance object.
    pub fn add(&mut self, table_path: &str) -> Result<AddResult, DmStoreError> {
        self.check_open()?;
        path::validate_path(table_path)?;
        if !path::is_object_path(table_path) {
            return Err(DmStoreError::InvalidPath {
                path: table_path.to_string(),
                reason: "expected object path ending with '.'".to_string(),
            });
        }

        // Verify it's a multi-instance object in dm_object
        let is_multi: bool = self
            .conn
            .query_row(
                "SELECT is_multi FROM dm_object WHERE path_hash = ?1 AND path = ?2",
                rusqlite::params![fnv1a_hash(table_path), table_path],
                |row| row.get(0),
            )
            .map_err(|e| match e {
                rusqlite::Error::QueryReturnedNoRows => {
                    DmStoreError::NotFound(table_path.to_string())
                }
                other => DmStoreError::Sqlite(other),
            })?;

        if !is_multi {
            return Err(DmStoreError::NotMultiInstance(table_path.to_string()));
        }

        // Find next instance number by looking at dm_object children with numeric leaves
        let next_num = self.find_next_instance_number(table_path)?;
        let inst_path = path::instance_path(table_path, next_num);
        let inst_hash = fnv1a_hash(&inst_path);

        // Compute the canonical template path for this table
        let canonical_table = path::canonicalize(table_path);
        let template_path = format!("{}{{i}}.", canonical_table);

        // Compute instance_numbers: all numbers from table_path + the new one
        let mut instance_numbers = path::extract_instance_numbers(table_path);
        instance_numbers.push(next_num.to_string());
        let instance_number_refs: Vec<&str> = instance_numbers.iter().map(|s| s.as_str()).collect();

        // Insert the new instance object into dm_object
        self.conn.execute(
            "INSERT INTO dm_object (path, path_hash, parent_path, is_multi) VALUES (?1, ?2, ?3, 0)",
            rusqlite::params![inst_path, inst_hash, table_path],
        )?;

        // Copy params from schema: find schema params where object_path = template_path
        self.copy_schema_params(&template_path, &instance_number_refs)?;

        // Copy child schema objects (non-{i} leaf children of the template)
        self.copy_schema_children(&template_path, &instance_number_refs)?;

        // Update instance cache
        if let Some(icache) = &mut self.instance_cache {
            let nums = icache.entry(table_path.to_string()).or_default();
            if let Err(pos) = nums.binary_search(&next_num) {
                nums.insert(pos, next_num);
            }
        }

        Ok(AddResult {
            instance_number: next_num,
            path: inst_path,
        })
    }

    /// Find the next instance number for a table path by examining existing children.
    fn find_next_instance_number(&self, table_path: &str) -> Result<u32, DmStoreError> {
        // Use instance cache if available (sorted, so last() is max)
        if let Some(icache) = &self.instance_cache {
            let max = icache
                .get(table_path)
                .and_then(|nums| nums.last().copied())
                .unwrap_or(0);
            return Ok(max + 1);
        }

        let mut stmt = self
            .conn
            .prepare("SELECT path FROM dm_object WHERE parent_path = ?1")?;
        let children: Vec<String> = stmt
            .query_map(rusqlite::params![table_path], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;

        let max_num = children
            .iter()
            .filter_map(|p| path::leaf_name(p).parse::<u32>().ok())
            .max()
            .unwrap_or(0);

        Ok(max_num + 1)
    }

    /// Copy schema params from a template object to a concrete instance.
    fn copy_schema_params(
        &mut self,
        tmpl_obj_path: &str,
        instance_numbers: &[&str],
    ) -> Result<(), DmStoreError> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT path, name, value, param_type, writable FROM dm_schema_param WHERE object_path = ?1",
        )?;

        let rows: Vec<(String, String, Option<String>, String, bool)> = stmt
            .query_map(rusqlite::params![tmpl_obj_path], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, bool>(4)?,
                ))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        for (tmpl_param_path, name, value, param_type, writable) in rows {
            let new_param_path = path::resolve_template(&tmpl_param_path, instance_numbers);
            let new_obj_path = path::resolve_template(tmpl_obj_path, instance_numbers);
            let new_hash = fnv1a_hash(&new_param_path);

            let rows = self.conn.execute(
                "INSERT OR IGNORE INTO dm_param (path, path_hash, object_path, name, value, param_type, writable) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![new_param_path, new_hash, new_obj_path, name, value, param_type, writable as i32],
            )?;
            if rows == 0 {
                continue;
            }

            // Update cache
            if let Some(cache) = &mut self.cache {
                let pt = ParamType::parse_name(&param_type).unwrap_or(ParamType::String);
                let param = Parameter {
                    path: new_param_path,
                    value: value.clone(),
                    param_type: pt,
                    writable,
                };
                cache.entry(new_hash).or_default().push(param);
            }
        }

        Ok(())
    }

    /// Copy child schema objects (non-{i} leaf) from a template to concrete instances.
    /// Recursively copies their params and their own children.
    fn copy_schema_children(
        &mut self,
        tmpl_path: &str,
        instance_numbers: &[&str],
    ) -> Result<(), DmStoreError> {
        // Find child schema objects whose parent is tmpl_path
        let mut stmt = self
            .conn
            .prepare_cached("SELECT path, is_multi FROM dm_schema_object WHERE parent_path = ?1")?;

        let children: Vec<(String, bool)> = stmt
            .query_map(rusqlite::params![tmpl_path], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        for (child_tmpl_path, child_is_multi) in children {
            let child_leaf = path::leaf_name(&child_tmpl_path);

            // Skip {i} template children -- they represent future instances, not concrete data
            if child_leaf == "{i}" {
                continue;
            }

            let child_inst_path = path::resolve_template(&child_tmpl_path, instance_numbers);
            let child_inst_hash = fnv1a_hash(&child_inst_path);
            let child_parent = path::parent_path(&child_inst_path);

            self.conn.execute(
                "INSERT OR IGNORE INTO dm_object (path, path_hash, parent_path, is_multi) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![child_inst_path, child_inst_hash, child_parent, child_is_multi as i32],
            )?;

            // Copy params for this child
            self.copy_schema_params(&child_tmpl_path, instance_numbers)?;

            // Recurse into grandchildren
            self.copy_schema_children(&child_tmpl_path, instance_numbers)?;
        }

        Ok(())
    }

    /// Delete an instance from a multi-instance object.
    /// Recursively deletes all descendant objects and their parameters.
    pub fn delete(&mut self, instance_path: &str) -> Result<(), DmStoreError> {
        self.check_open()?;
        path::validate_path(instance_path)?;
        if !path::is_object_path(instance_path) {
            return Err(DmStoreError::InvalidPath {
                path: instance_path.to_string(),
                reason: "expected object path ending with '.'".to_string(),
            });
        }

        if path::instance_number(instance_path).is_none() {
            return Err(DmStoreError::NotAnInstance(instance_path.to_string()));
        }

        let hash = fnv1a_hash(instance_path);

        // Verify the object exists
        let exists: bool = self.conn.query_row(
            "SELECT COUNT(*) > 0 FROM dm_object WHERE path_hash = ?1 AND path = ?2",
            rusqlite::params![hash, instance_path],
            |row| row.get(0),
        )?;
        if !exists {
            return Err(DmStoreError::NotFound(instance_path.to_string()));
        }

        // Collect the full subtree: the instance itself + all descendants via BFS on parent_path
        let mut all_objects = vec![instance_path.to_string()];
        let mut queue = vec![instance_path.to_string()];
        while let Some(parent) = queue.pop() {
            let mut stmt = self
                .conn
                .prepare("SELECT path FROM dm_object WHERE parent_path = ?1")?;
            let children: Vec<String> = stmt
                .query_map(rusqlite::params![parent], |row| row.get::<_, String>(0))?
                .collect::<Result<Vec<_>, _>>()?;
            for child in children {
                all_objects.push(child.clone());
                queue.push(child);
            }
        }

        // Collect all param (path, hash) across the entire subtree for cache invalidation
        let all_param_entries: Vec<(String, i64)> = if self.cache.is_some() {
            let mut entries = Vec::new();
            for obj in &all_objects {
                let mut stmt = self
                    .conn
                    .prepare("SELECT path, path_hash FROM dm_param WHERE object_path = ?1")?;
                let params: Vec<(String, i64)> = stmt
                    .query_map(rusqlite::params![obj], |row| {
                        Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                    })?
                    .collect::<Result<Vec<_>, _>>()?;
                entries.extend(params);
            }
            entries
        } else {
            Vec::new()
        };

        // Delete all objects in reverse order (leaves first) so FK constraints are satisfied.
        // CASCADE on dm_param will auto-delete each object's params.
        for obj in all_objects.iter().rev() {
            let obj_hash = fnv1a_hash(obj);
            self.conn.execute(
                "DELETE FROM dm_object WHERE path_hash = ?1 AND path = ?2",
                rusqlite::params![obj_hash, obj],
            )?;
        }

        // Invalidate param cache
        if let Some(cache) = &mut self.cache {
            for (pp, ph) in all_param_entries {
                if let Some(params) = cache.get_mut(&ph) {
                    params.retain(|p| p.path != pp);
                }
            }
        }

        // Invalidate instance cache
        if let Some(icache) = &mut self.instance_cache {
            // Remove the deleted instance from its parent's entry
            if let Some(parent) = path::parent_path(instance_path) {
                if let Some(num) = path::instance_number(instance_path) {
                    if let Some(nums) = icache.get_mut(&parent) {
                        if let Ok(pos) = nums.binary_search(&num) {
                            nums.remove(pos);
                        }
                    }
                }
            }
            // Remove any instance cache entries for deleted subtree objects
            for obj in &all_objects {
                icache.remove(obj);
            }
        }

        Ok(())
    }

    /// Commit all changes in this session.
    pub fn commit(mut self) -> Result<(), DmStoreError> {
        self.check_open()?;
        self.conn
            .execute_batch(&format!("RELEASE SAVEPOINT {}", self.savepoint_name))
            .ctx_with(|| format!("committing session savepoint {}", self.savepoint_name))?;
        self.closed = true;
        Ok(())
    }

    /// Abort (rollback) all changes in this session.
    pub fn abort(mut self) -> Result<(), DmStoreError> {
        self.check_open()?;
        self.do_rollback()?;
        Ok(())
    }

    fn do_rollback(&mut self) -> Result<(), DmStoreError> {
        self.conn
            .execute_batch(&format!(
                "ROLLBACK TO SAVEPOINT {0}; RELEASE SAVEPOINT {0};",
                self.savepoint_name
            ))
            .ctx_with(|| format!("rolling back session savepoint {}", self.savepoint_name))?;
        self.closed = true;
        self.refresh_caches_from_db()?;
        Ok(())
    }
}

impl<'a> Drop for Session<'a> {
    fn drop(&mut self) {
        if !self.closed {
            let _ = self.do_rollback();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_session_set_and_commit() {
        let mut store = DmStore::open_in_memory().unwrap();
        store.define_object("Device.", false).unwrap();
        store.define_object("Device.Test.", false).unwrap();
        store
            .define_parameter("Device.Test.Name", ParamType::String, true, Some("hello"))
            .unwrap();

        {
            let mut session = store.session().unwrap();
            session.set("Device.Test.Name", "world").unwrap();
            let p = session.get("Device.Test.Name").unwrap();
            assert_eq!(p.value.as_deref(), Some("world"));
            session.commit().unwrap();
        }

        let p = store.get("Device.Test.Name").unwrap();
        assert_eq!(p.value.as_deref(), Some("world"));
    }

    #[test]
    fn test_session_abort() {
        let mut store = DmStore::open_in_memory().unwrap();
        store.define_object("Device.", false).unwrap();
        store.define_object("Device.Test.", false).unwrap();
        store
            .define_parameter("Device.Test.Name", ParamType::String, true, Some("hello"))
            .unwrap();

        {
            let mut session = store.session().unwrap();
            session.set("Device.Test.Name", "world").unwrap();
            session.abort().unwrap();
        }

        let p = store.get("Device.Test.Name").unwrap();
        assert_eq!(p.value.as_deref(), Some("hello"));
    }

    #[test]
    fn test_session_drop_restores_param_cache() {
        let mut store = DmStore::open_in_memory().unwrap();
        store.define_object("Device.", false).unwrap();
        store.define_object("Device.Test.", false).unwrap();
        store
            .define_parameter("Device.Test.Name", ParamType::String, true, Some("hello"))
            .unwrap();

        {
            let mut session = store.session().unwrap();
            session.set("Device.Test.Name", "world").unwrap();
        }

        let p = store.get("Device.Test.Name").unwrap();
        assert_eq!(p.value.as_deref(), Some("hello"));
    }

    #[test]
    fn test_session_drop_restores_instance_cache() {
        let mut store = DmStore::open_in_memory().unwrap();
        store.define_object("Device.", false).unwrap();
        store.define_object("Device.Radio.", true).unwrap();
        store.define_object("Device.Radio.{i}.", false).unwrap();

        {
            let mut session = store.session().unwrap();
            let result = session.add("Device.Radio.").unwrap();
            assert_eq!(result.path, "Device.Radio.1.");
        }

        assert_eq!(store.instances("Device.Radio.").unwrap(), vec![]);

        {
            let mut session = store.session().unwrap();
            let result = session.add("Device.Radio.").unwrap();
            assert_eq!(result.path, "Device.Radio.1.");
            session.commit().unwrap();
        }

        assert_eq!(store.instances("Device.Radio.").unwrap(), vec![1]);
    }

    #[test]
    fn test_session_add_and_delete() {
        let mut store = DmStore::open_in_memory().unwrap();
        store.define_object("Device.", false).unwrap();
        store.define_object("Device.Radio.", true).unwrap();
        store.define_object("Device.Radio.{i}.", false).unwrap();
        store
            .define_parameter(
                "Device.Radio.{i}.Enable",
                ParamType::Boolean,
                true,
                Some("true"),
            )
            .unwrap();
        store
            .define_parameter(
                "Device.Radio.{i}.Channel",
                ParamType::UnsignedInt,
                true,
                Some("0"),
            )
            .unwrap();

        let result;
        {
            let mut session = store.session().unwrap();
            result = session.add("Device.Radio.").unwrap();
            assert_eq!(result.instance_number, 1);
            assert_eq!(result.path, "Device.Radio.1.");

            // Verify params were copied
            let p = session.get("Device.Radio.1.Enable").unwrap();
            assert_eq!(p.value.as_deref(), Some("true"));

            let p2 = session.get("Device.Radio.1.Channel").unwrap();
            assert_eq!(p2.value.as_deref(), Some("0"));

            session.commit().unwrap();
        }

        // Add another instance
        {
            let mut session = store.session().unwrap();
            let r2 = session.add("Device.Radio.").unwrap();
            assert_eq!(r2.instance_number, 2);
            session.commit().unwrap();
        }

        // Delete instance 1
        {
            let mut session = store.session().unwrap();
            session.delete("Device.Radio.1.").unwrap();
            session.commit().unwrap();
        }

        // Instance 1 should be gone -- reload cache after delete+commit
        store.reload_cache().unwrap();
        let r = store.get("Device.Radio.1.Enable");
        assert!(matches!(r, Err(DmStoreError::NotFound(_))));

        // Instance 2 should still exist
        let p = store.get("Device.Radio.2.Enable").unwrap();
        assert_eq!(p.value.as_deref(), Some("true"));
    }

    #[test]
    fn test_nested_delete_invalidates_caches() {
        let mut store = DmStore::open_in_memory().unwrap();
        store.define_object("Device.", false).unwrap();
        store.define_object("Device.WiFi.", true).unwrap();
        store.define_object("Device.WiFi.{i}.", false).unwrap();
        store.define_object("Device.WiFi.{i}.SSID.", true).unwrap();
        store
            .define_object("Device.WiFi.{i}.SSID.{i}.", false)
            .unwrap();
        store
            .define_parameter(
                "Device.WiFi.{i}.SSID.{i}.Name",
                ParamType::String,
                true,
                Some("ssid"),
            )
            .unwrap();

        // Create WiFi.1 with SSID.1 and SSID.2
        {
            let mut s = store.session().unwrap();
            s.add("Device.WiFi.").unwrap();
            s.add("Device.WiFi.1.SSID.").unwrap();
            s.add("Device.WiFi.1.SSID.").unwrap();
            s.commit().unwrap();
        }
        assert_eq!(store.instances("Device.WiFi.1.SSID.").unwrap(), vec![1, 2]);

        // Delete the nested SSID.1
        {
            let mut s = store.session().unwrap();
            s.delete("Device.WiFi.1.SSID.1.").unwrap();
            s.commit().unwrap();
        }

        // Instance cache must reflect removal: SSID has [2], WiFi still has [1]
        assert_eq!(store.instances("Device.WiFi.1.SSID.").unwrap(), vec![2]);
        assert_eq!(store.instances("Device.WiFi.").unwrap(), vec![1]);

        // Param cache must no longer serve the deleted param
        let r = store.get("Device.WiFi.1.SSID.1.Name");
        assert!(matches!(r, Err(DmStoreError::NotFound(_))));

        // Sibling survives
        let p = store.get("Device.WiFi.1.SSID.2.Name").unwrap();
        assert_eq!(p.value.as_deref(), Some("ssid"));

        // Delete the whole WiFi.1 -- must drop the nested table icache entry too
        {
            let mut s = store.session().unwrap();
            s.delete("Device.WiFi.1.").unwrap();
            s.commit().unwrap();
        }
        assert!(store
            .instances("Device.WiFi.1.SSID.")
            .unwrap()
            .is_empty());
        assert!(store.instances("Device.WiFi.").unwrap().is_empty());
    }

    #[test]
    fn test_session_read_only_param() {
        let mut store = DmStore::open_in_memory().unwrap();
        store.define_object("Device.", false).unwrap();
        store.define_object("Device.Info.", false).unwrap();
        store
            .define_parameter(
                "Device.Info.ModelName",
                ParamType::String,
                false,
                Some("TestModel"),
            )
            .unwrap();

        let mut session = store.session().unwrap();
        let result = session.set("Device.Info.ModelName", "Other");
        assert!(matches!(result, Err(DmStoreError::ReadOnly(_))));
    }

    #[test]
    fn test_get_object() {
        let mut store = DmStore::open_in_memory().unwrap();
        store.define_object("Device.", false).unwrap();
        store.define_object("Device.Test.", false).unwrap();
        store
            .define_parameter("Device.Test.A", ParamType::String, true, Some("1"))
            .unwrap();
        store
            .define_parameter("Device.Test.B", ParamType::String, true, Some("2"))
            .unwrap();

        let session = store.session().unwrap();
        let params = session.get_object("Device.Test.").unwrap();
        assert_eq!(params.len(), 2);
    }

    #[test]
    fn test_nested_add() {
        let mut store = DmStore::open_in_memory().unwrap();
        store.define_object("Device.", false).unwrap();
        store.define_object("Device.WiFi.", true).unwrap();
        store.define_object("Device.WiFi.{i}.", false).unwrap();
        store.define_object("Device.WiFi.{i}.SSID.", true).unwrap();
        store
            .define_object("Device.WiFi.{i}.SSID.{i}.", false)
            .unwrap();
        store
            .define_parameter(
                "Device.WiFi.{i}.Name",
                ParamType::String,
                true,
                Some("wifi"),
            )
            .unwrap();
        store
            .define_parameter(
                "Device.WiFi.{i}.SSID.{i}.Name",
                ParamType::String,
                true,
                Some("ssid"),
            )
            .unwrap();

        // Add WiFi.1
        {
            let mut session = store.session().unwrap();
            let r = session.add("Device.WiFi.").unwrap();
            assert_eq!(r.instance_number, 1);
            session.commit().unwrap();
        }

        // Verify WiFi.1.Name exists
        let p = store.get("Device.WiFi.1.Name").unwrap();
        assert_eq!(p.value.as_deref(), Some("wifi"));

        // Verify WiFi.1.SSID. table was created (child schema object)
        let conn = store.connection();
        let ssid_table: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM dm_object WHERE path = 'Device.WiFi.1.SSID.'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            ssid_table,
            "Device.WiFi.1.SSID. should exist as a multi-instance table"
        );

        // Verify it's multi-instance
        let is_multi: bool = conn
            .query_row(
                "SELECT is_multi FROM dm_object WHERE path = 'Device.WiFi.1.SSID.'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(is_multi);

        // Add SSID.1 under WiFi.1
        {
            let mut session = store.session().unwrap();
            let r = session.add("Device.WiFi.1.SSID.").unwrap();
            assert_eq!(r.instance_number, 1);
            session.commit().unwrap();
        }

        let p = store.get("Device.WiFi.1.SSID.1.Name").unwrap();
        assert_eq!(p.value.as_deref(), Some("ssid"));
    }

    #[test]
    fn test_session_instances() {
        let mut store = DmStore::open_in_memory().unwrap();
        store.define_object("Device.", false).unwrap();
        store.define_object("Device.Radio.", true).unwrap();
        store.define_object("Device.Radio.{i}.", false).unwrap();

        // Within a session, instances should reflect adds
        {
            let mut session = store.session().unwrap();
            assert_eq!(session.instances("Device.Radio.").unwrap(), vec![]);

            session.add("Device.Radio.").unwrap();
            assert_eq!(session.instances("Device.Radio.").unwrap(), vec![1]);

            session.add("Device.Radio.").unwrap();
            assert_eq!(session.instances("Device.Radio.").unwrap(), vec![1, 2]);

            session.commit().unwrap();
        }

        // After delete, instances should reflect removal
        {
            let mut session = store.session().unwrap();
            session.delete("Device.Radio.1.").unwrap();
            assert_eq!(session.instances("Device.Radio.").unwrap(), vec![2]);
            session.commit().unwrap();
        }

        assert_eq!(store.instances("Device.Radio.").unwrap(), vec![2]);
    }
}
