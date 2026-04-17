use std::collections::HashMap;

use rusqlite::Connection;

use crate::error::DmStoreError;
use crate::path::{self, fnv1a_hash};
use crate::schema;
use crate::session::Session;
use crate::types::*;

pub struct DmStore {
    conn: Connection,
    config: DmStoreConfig,
    /// In-memory cache: path_hash -> list of Parameters with that hash.
    /// Uses hash as key; collisions handled by checking path equality.
    cache: Option<HashMap<i64, Vec<Parameter>>>,
    /// In-memory cache: table_path -> sorted list of instance numbers.
    instance_cache: Option<HashMap<String, Vec<u32>>>,
    savepoint_counter: u64,
}

impl DmStore {
    /// Open (or create) a database at the given path with default config (cache ON).
    pub fn open(db_path: &str) -> Result<Self, DmStoreError> {
        Self::open_with_config(db_path, DmStoreConfig::default())
    }

    /// Open with explicit config.
    pub fn open_with_config(db_path: &str, config: DmStoreConfig) -> Result<Self, DmStoreError> {
        let conn = Connection::open(db_path)?;
        Self::init(conn, config)
    }

    /// Open an in-memory database (for testing).
    pub fn open_in_memory() -> Result<Self, DmStoreError> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn, DmStoreConfig::default())
    }

    fn init(conn: Connection, config: DmStoreConfig) -> Result<Self, DmStoreError> {
        schema::init_db(&conn)?;

        let cache = if config.use_cache {
            Some(Self::load_cache(&conn)?)
        } else {
            None
        };

        let instance_cache = if config.use_cache {
            Some(Self::load_instance_cache(&conn)?)
        } else {
            None
        };

        Ok(DmStore {
            conn,
            config,
            cache,
            instance_cache,
            savepoint_counter: 0,
        })
    }

    pub(crate) fn load_cache(
        conn: &Connection,
    ) -> Result<HashMap<i64, Vec<Parameter>>, DmStoreError> {
        let mut map: HashMap<i64, Vec<Parameter>> = HashMap::new();
        let mut stmt =
            conn.prepare("SELECT path, path_hash, value, param_type, writable FROM dm_param")?;
        let rows = stmt.query_map([], |row| {
            let path: String = row.get(0)?;
            let hash: i64 = row.get(1)?;
            let value: Option<String> = row.get(2)?;
            let type_str: String = row.get(3)?;
            let writable: bool = row.get(4)?;
            Ok((path, hash, value, type_str, writable))
        })?;

        for row in rows {
            let (path, hash, value, type_str, writable) = row?;
            let param_type = ParamType::parse_name(&type_str).unwrap_or(ParamType::String);
            let param = Parameter {
                path,
                value,
                param_type,
                writable,
            };
            let bucket = map.entry(hash).or_default();
            if let Some(existing) = bucket.first() {
                log::warn!(
                    "FNV-1a hash collision on load_cache: hash={:#x}, paths=[{}, {}]",
                    hash,
                    existing.path,
                    param.path
                );
            }
            bucket.push(param);
        }

        Ok(map)
    }

    pub(crate) fn load_instance_cache(
        conn: &Connection,
    ) -> Result<HashMap<String, Vec<u32>>, DmStoreError> {
        let mut map: HashMap<String, Vec<u32>> = HashMap::new();
        let mut stmt =
            conn.prepare("SELECT parent_path, path FROM dm_object WHERE parent_path IS NOT NULL")?;
        let rows = stmt.query_map([], |row| {
            let parent: String = row.get(0)?;
            let child: String = row.get(1)?;
            Ok((parent, child))
        })?;

        for row in rows {
            let (parent, child) = row?;
            if let Ok(num) = path::leaf_name(&child).parse::<u32>() {
                map.entry(parent).or_default().push(num);
            }
        }

        for nums in map.values_mut() {
            nums.sort_unstable();
        }

        Ok(map)
    }

    /// Reload the in-memory cache from the database.
    pub fn reload_cache(&mut self) -> Result<(), DmStoreError> {
        if self.config.use_cache {
            self.cache = Some(Self::load_cache(&self.conn)?);
            self.instance_cache = Some(Self::load_instance_cache(&self.conn)?);
        }
        Ok(())
    }

    /// Begin a new session for transactional operations.
    pub fn session(&mut self) -> Result<Session<'_>, DmStoreError> {
        self.savepoint_counter += 1;
        let name = format!("dm_session_{}", self.savepoint_counter);
        Session::new(&self.conn, &mut self.cache, &mut self.instance_cache, name)
    }

    /// Quick read: get a single parameter by exact path (no session needed).
    pub fn get(&self, path: &str) -> Result<Parameter, DmStoreError> {
        path::validate_path(path)?;
        let hash = fnv1a_hash(path);

        // Try cache first
        if let Some(cache) = &self.cache {
            if let Some(params) = cache.get(&hash) {
                for p in params {
                    if p.path == path {
                        return Ok(p.clone());
                    }
                }
            }
            return Err(DmStoreError::NotFound(path.to_string()));
        }

        // Fall back to DB with hash-accelerated lookup
        Self::get_from_db(&self.conn, path, hash)
    }

    /// Borrow a parameter directly from the in-memory cache. Returns None
    /// when the cache is disabled or the path is not resolved there. No DB
    /// fallback -- callers that need guaranteed lookup should use `get`.
    /// Intended for hot read paths that want to avoid Parameter clones.
    pub fn get_cached(&self, path: &str) -> Option<&Parameter> {
        let cache = self.cache.as_ref()?;
        let hash = fnv1a_hash(path);
        cache.get(&hash)?.iter().find(|p| p.path == path)
    }

    /// Borrow the cached instance-number list for a table path. Returns
    /// None when caching is disabled. Like `get_cached`, this is a
    /// zero-copy fast path -- callers wanting the DB-backed answer should
    /// use `instances`.
    pub fn instances_cached(&self, table_path: &str) -> Option<&[u32]> {
        self.instance_cache
            .as_ref()?
            .get(table_path)
            .map(Vec::as_slice)
    }

    pub(crate) fn get_from_db(
        conn: &Connection,
        path: &str,
        hash: i64,
    ) -> Result<Parameter, DmStoreError> {
        let mut stmt = conn.prepare_cached(
            "SELECT path, value, param_type, writable FROM dm_param WHERE path_hash = ?1 AND path = ?2",
        )?;

        stmt.query_row(rusqlite::params![hash, path], |row| {
            let path: String = row.get(0)?;
            let value: Option<String> = row.get(1)?;
            let type_str: String = row.get(2)?;
            let writable: bool = row.get(3)?;
            Ok(Parameter {
                path,
                value,
                param_type: ParamType::parse_name(&type_str).unwrap_or(ParamType::String),
                writable,
            })
        })
        .map_err(|e| match e {
            rusqlite::Error::QueryReturnedNoRows => DmStoreError::NotFound(path.to_string()),
            other => DmStoreError::Sqlite(other),
        })
    }

    /// Quick read: get all parameters belonging to an object path.
    pub fn get_object(&self, object_path: &str) -> Result<Vec<Parameter>, DmStoreError> {
        path::validate_path(object_path)?;
        if !path::is_object_path(object_path) {
            return Err(DmStoreError::InvalidPath {
                path: object_path.to_string(),
                reason: "expected object path ending with '.'".to_string(),
            });
        }

        Self::get_object_from_db(&self.conn, object_path)
    }

    pub(crate) fn get_object_from_db(
        conn: &Connection,
        object_path: &str,
    ) -> Result<Vec<Parameter>, DmStoreError> {
        let mut stmt = conn.prepare_cached(
            "SELECT path, value, param_type, writable FROM dm_param WHERE object_path = ?1",
        )?;

        let rows = stmt.query_map(rusqlite::params![object_path], |row| {
            let path: String = row.get(0)?;
            let value: Option<String> = row.get(1)?;
            let type_str: String = row.get(2)?;
            let writable: bool = row.get(3)?;
            Ok(Parameter {
                path,
                value,
                param_type: ParamType::parse_name(&type_str).unwrap_or(ParamType::String),
                writable,
            })
        })?;

        let mut params = Vec::new();
        for row in rows {
            params.push(row?);
        }
        Ok(params)
    }

    pub(crate) fn object_exists_in_db(
        conn: &Connection,
        object_path: &str,
    ) -> Result<bool, DmStoreError> {
        let hash = fnv1a_hash(object_path);
        conn.query_row(
            "SELECT COUNT(*) > 0 FROM dm_object WHERE path_hash = ?1 AND path = ?2",
            rusqlite::params![hash, object_path],
            |row| row.get(0),
        )
        .map_err(DmStoreError::Sqlite)
    }

    /// Check whether an object exists by exact path.
    pub fn object_exists(&self, object_path: &str) -> Result<bool, DmStoreError> {
        path::validate_path(object_path)?;
        if !path::is_object_path(object_path) {
            return Err(DmStoreError::InvalidPath {
                path: object_path.to_string(),
                reason: "expected object path ending with '.'".to_string(),
            });
        }

        Self::object_exists_in_db(&self.conn, object_path)
    }

    /// Get all instance numbers for a multi-instance object table path.
    pub fn instances(&self, table_path: &str) -> Result<Vec<u32>, DmStoreError> {
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
        Self::instances_from_db(&self.conn, table_path)
    }

    pub(crate) fn instances_from_db(
        conn: &Connection,
        table_path: &str,
    ) -> Result<Vec<u32>, DmStoreError> {
        let mut stmt = conn.prepare_cached("SELECT path FROM dm_object WHERE parent_path = ?1")?;
        let children: Vec<String> = stmt
            .query_map(rusqlite::params![table_path], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;

        let mut nums: Vec<u32> = children
            .iter()
            .filter_map(|p| path::leaf_name(p).parse::<u32>().ok())
            .collect();
        nums.sort_unstable();
        Ok(nums)
    }

    /// Define an object in the data model schema.
    /// If the path contains `{i}`, it goes into `dm_schema_object`.
    /// Otherwise it goes into `dm_object`.
    pub fn define_object(&mut self, obj_path: &str, is_multi: bool) -> Result<(), DmStoreError> {
        path::validate_path(obj_path)?;
        if !path::is_object_path(obj_path) {
            return Err(DmStoreError::InvalidPath {
                path: obj_path.to_string(),
                reason: "object path must end with '.'".to_string(),
            });
        }

        if path::is_template_path(obj_path) {
            // Template path -> schema table
            let hash = fnv1a_hash(obj_path);
            let parent = path::parent_path(obj_path);

            // Ensure parent schema objects exist
            if let Some(ref p) = parent {
                self.ensure_schema_object_exists(p)?;
            }

            self.conn.execute(
                "INSERT INTO dm_schema_object (path, path_hash, parent_path, is_multi)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(path) DO UPDATE SET
                     path_hash = excluded.path_hash,
                     parent_path = excluded.parent_path,
                     is_multi = excluded.is_multi",
                rusqlite::params![obj_path, hash, parent, is_multi as i32],
            )?;

            // Propagate to existing instances
            self.propagate_template_object_to_instances(obj_path, is_multi)?;
        } else {
            // Concrete path -> dm_object
            let hash = fnv1a_hash(obj_path);
            let parent = path::parent_path(obj_path);

            if let Some(ref p) = parent {
                self.ensure_object_exists(p)?;
            }

            self.conn.execute(
                "INSERT INTO dm_object (path, path_hash, parent_path, is_multi)
                 VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT(path) DO UPDATE SET
                     path_hash = excluded.path_hash,
                     parent_path = excluded.parent_path,
                     is_multi = excluded.is_multi",
                rusqlite::params![obj_path, hash, parent, is_multi as i32],
            )?;

            // Update instance cache if leaf is numeric
            if let Some(icache) = &mut self.instance_cache {
                if let Some(num) = path::instance_number(obj_path) {
                    if let Some(ref p) = parent {
                        let nums = icache.entry(p.clone()).or_default();
                        if let Err(pos) = nums.binary_search(&num) {
                            nums.insert(pos, num);
                        }
                    }
                }
            }
        }

        Ok(())
    }

    /// Define a parameter in the data model schema.
    /// If the path contains `{i}`, it goes into `dm_schema_param`.
    /// Otherwise it goes into `dm_param`.
    pub fn define_parameter(
        &mut self,
        param_path: &str,
        param_type: ParamType,
        writable: bool,
        default_value: Option<&str>,
    ) -> Result<(), DmStoreError> {
        path::validate_path(param_path)?;
        if path::is_object_path(param_path) {
            return Err(DmStoreError::InvalidPath {
                path: param_path.to_string(),
                reason: "parameter path must not end with '.'".to_string(),
            });
        }

        let obj_path = path::parent_path(param_path).ok_or_else(|| DmStoreError::InvalidPath {
            path: param_path.to_string(),
            reason: "parameter must belong to an object".to_string(),
        })?;
        let name = path::leaf_name(param_path);

        if path::is_template_path(param_path) {
            // Template path -> schema table
            let hash = fnv1a_hash(param_path);

            // Ensure parent schema object exists
            self.ensure_schema_object_exists(&obj_path)?;

            let rows = self.conn.execute(
                "INSERT OR IGNORE INTO dm_schema_param (path, path_hash, object_path, name, value, param_type, writable) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![param_path, hash, obj_path, name, default_value, param_type.as_str(), writable as i32],
            )?;
            if rows == 0 {
                return Err(DmStoreError::AlreadyExists(param_path.to_string()));
            }

            // Propagate to existing instances
            self.propagate_template_param_to_instances(
                param_path,
                name,
                param_type,
                writable,
                default_value,
            )?;
        } else {
            // Concrete path -> dm_param
            let hash = fnv1a_hash(param_path);

            // Ensure parent object exists
            self.ensure_object_exists(&obj_path)?;

            let rows = self.conn.execute(
                "INSERT OR IGNORE INTO dm_param (path, path_hash, object_path, name, value, param_type, writable) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![param_path, hash, obj_path, name, default_value, param_type.as_str(), writable as i32],
            )?;
            if rows == 0 {
                return Err(DmStoreError::AlreadyExists(param_path.to_string()));
            }

            // Update cache
            if let Some(cache) = &mut self.cache {
                let param = Parameter {
                    path: param_path.to_string(),
                    value: default_value.map(|s| s.to_string()),
                    param_type,
                    writable,
                };
                cache.entry(hash).or_default().push(param);
            }
        }

        Ok(())
    }

    /// Ensure a concrete object exists in dm_object, auto-creating it (and ancestors) if needed.
    /// Only creates concrete (non-template) objects.
    fn ensure_object_exists(&mut self, obj_path: &str) -> Result<(), DmStoreError> {
        // Never insert template paths into dm_object
        if path::is_template_path(obj_path) {
            return Ok(());
        }
        let hash = fnv1a_hash(obj_path);
        let exists: bool = self.conn.query_row(
            "SELECT COUNT(*) > 0 FROM dm_object WHERE path_hash = ?1 AND path = ?2",
            rusqlite::params![hash, obj_path],
            |row| row.get(0),
        )?;
        if !exists {
            if let Some(parent) = path::parent_path(obj_path) {
                self.ensure_object_exists(&parent)?;
            }
            let parent = path::parent_path(obj_path);
            self.conn.execute(
                "INSERT OR IGNORE INTO dm_object (path, path_hash, parent_path, is_multi) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![obj_path, hash, parent, 0],
            )?;
        }
        Ok(())
    }

    /// Ensure a schema object exists in dm_schema_object, auto-creating ancestors if needed.
    fn ensure_schema_object_exists(&mut self, obj_path: &str) -> Result<(), DmStoreError> {
        if !path::is_template_path(obj_path) {
            // Concrete parent -- ensure it exists in dm_object instead
            return self.ensure_object_exists(obj_path);
        }
        let hash = fnv1a_hash(obj_path);
        let exists: bool = self.conn.query_row(
            "SELECT COUNT(*) > 0 FROM dm_schema_object WHERE path_hash = ?1 AND path = ?2",
            rusqlite::params![hash, obj_path],
            |row| row.get(0),
        )?;
        if !exists {
            if let Some(parent) = path::parent_path(obj_path) {
                self.ensure_schema_object_exists(&parent)?;
            }
            let parent = path::parent_path(obj_path);
            self.conn.execute(
                "INSERT OR IGNORE INTO dm_schema_object (path, path_hash, parent_path, is_multi) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![obj_path, hash, parent, 0],
            )?;
        }
        Ok(())
    }

    /// Collect all fully-resolved concrete paths for a template by recursively
    /// resolving each `{i}` against existing instances in `dm_object`.
    fn collect_resolved_paths(&self, tmpl: &str) -> Result<Vec<String>, DmStoreError> {
        if !path::is_template_path(tmpl) {
            return Ok(vec![tmpl.to_string()]);
        }

        let Some(pos) = tmpl.find("{i}") else {
            return Ok(vec![tmpl.to_string()]);
        };
        let table_path = &tmpl[..pos]; // e.g., "Device.WiFi."

        // Find all children of table_path with numeric leaf names
        let mut stmt = self
            .conn
            .prepare("SELECT path FROM dm_object WHERE parent_path = ?1")?;
        let all_children: Vec<String> = stmt
            .query_map(rusqlite::params![table_path], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?;
        drop(stmt);

        let instances: Vec<&String> = all_children
            .iter()
            .filter(|p| {
                let leaf = path::leaf_name(p);
                leaf.parse::<u32>().is_ok()
            })
            .collect();

        let mut results = Vec::new();
        for inst_path in instances {
            let num = path::leaf_name(inst_path);
            let resolved = tmpl.replacen("{i}", num, 1);
            // Recurse to handle additional {i} levels
            results.extend(self.collect_resolved_paths(&resolved)?);
        }

        Ok(results)
    }

    /// Propagate a template object to all existing concrete instances.
    fn propagate_template_object_to_instances(
        &mut self,
        tmpl_obj_path: &str,
        is_multi: bool,
    ) -> Result<(), DmStoreError> {
        let resolved_paths = self.collect_resolved_paths(tmpl_obj_path)?;

        for resolved in resolved_paths {
            let parent = path::parent_path(&resolved);

            // Verify parent exists in dm_object
            if let Some(ref p) = parent {
                let parent_hash = fnv1a_hash(p);
                let parent_exists: bool = self.conn.query_row(
                    "SELECT COUNT(*) > 0 FROM dm_object WHERE path_hash = ?1 AND path = ?2",
                    rusqlite::params![parent_hash, p],
                    |row| row.get(0),
                )?;
                if !parent_exists {
                    continue;
                }
            }

            let hash = fnv1a_hash(&resolved);
            self.conn.execute(
                "INSERT OR IGNORE INTO dm_object (path, path_hash, parent_path, is_multi) VALUES (?1, ?2, ?3, ?4)",
                rusqlite::params![resolved, hash, parent, is_multi as i32],
            )?;
        }

        Ok(())
    }

    /// Propagate a template parameter to all existing concrete instances.
    fn propagate_template_param_to_instances(
        &mut self,
        tmpl_param_path: &str,
        name: &str,
        param_type: ParamType,
        writable: bool,
        default_value: Option<&str>,
    ) -> Result<(), DmStoreError> {
        let resolved_paths = self.collect_resolved_paths(tmpl_param_path)?;

        for resolved in resolved_paths {
            let obj_path = path::parent_path(&resolved).ok_or_else(|| {
                DmStoreError::InvalidPath {
                    path: resolved.clone(),
                    reason: "resolved template parameter has no parent object".to_string(),
                }
            })?;

            // Verify parent object exists in dm_object
            let obj_hash = fnv1a_hash(&obj_path);
            let obj_exists: bool = self.conn.query_row(
                "SELECT COUNT(*) > 0 FROM dm_object WHERE path_hash = ?1 AND path = ?2",
                rusqlite::params![obj_hash, obj_path],
                |row| row.get(0),
            )?;
            if !obj_exists {
                continue;
            }

            let hash = fnv1a_hash(&resolved);
            let rows = self.conn.execute(
                "INSERT OR IGNORE INTO dm_param (path, path_hash, object_path, name, value, param_type, writable) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                rusqlite::params![resolved, hash, obj_path, name, default_value, param_type.as_str(), writable as i32],
            )?;
            if rows == 0 {
                continue;
            }

            if let Some(cache) = &mut self.cache {
                let param = Parameter {
                    path: resolved,
                    value: default_value.map(|s| s.to_string()),
                    param_type,
                    writable,
                };
                cache.entry(hash).or_default().push(param);
            }
        }

        Ok(())
    }

    /// Get a reference to the underlying connection (for advanced use).
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// Check if caching is enabled.
    pub fn cache_enabled(&self) -> bool {
        self.config.use_cache
    }

    /// Snapshot every row in dm_object, dm_param, dm_schema_object, and
    /// dm_schema_param for presentation by CLIs or tools. Concrete and schema
    /// tables are returned separately, each sorted by path.
    pub fn dump(&self) -> Result<DmDump, DmStoreError> {
        let objects = Self::dump_objects(&self.conn, "dm_object")?;
        let params = Self::dump_params(&self.conn, "dm_param")?;
        let schema_objects = Self::dump_objects(&self.conn, "dm_schema_object")?;
        let schema_params = Self::dump_params(&self.conn, "dm_schema_param")?;
        Ok(DmDump {
            objects,
            params,
            schema_objects,
            schema_params,
        })
    }

    fn dump_objects(
        conn: &Connection,
        table: &str,
    ) -> Result<Vec<DumpedObject>, DmStoreError> {
        // Table name is a crate-controlled constant; interpolation is safe.
        let sql = format!("SELECT path, is_multi FROM {} ORDER BY path", table);
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |row| {
            Ok(DumpedObject {
                path: row.get(0)?,
                is_multi: row.get(1)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(DmStoreError::Sqlite)
    }

    fn dump_params(
        conn: &Connection,
        table: &str,
    ) -> Result<Vec<DumpedParam>, DmStoreError> {
        let sql = format!(
            "SELECT path, value, param_type, writable FROM {} ORDER BY path",
            table
        );
        let mut stmt = conn.prepare(&sql)?;
        let rows = stmt.query_map([], |row| {
            Ok(DumpedParam {
                path: row.get(0)?,
                value: row.get(1)?,
                param_type: row.get(2)?,
                writable: row.get(3)?,
            })
        })?;
        rows.collect::<Result<Vec<_>, _>>()
            .map_err(DmStoreError::Sqlite)
    }

    /// Begin a named savepoint. Intended for interactive contexts (e.g. the
    /// REPL) that need a batch scope spanning multiple `session()` calls.
    /// Use `release_savepoint` to commit or `rollback_savepoint` to abort.
    pub fn begin_savepoint(&mut self, name: &str) -> Result<(), DmStoreError> {
        // Basic guard against SQL injection via savepoint name.
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(DmStoreError::InvalidPath {
                path: name.to_string(),
                reason: "savepoint name must be alphanumeric/underscore".to_string(),
            });
        }
        self.conn.execute_batch(&format!("SAVEPOINT {}", name))?;
        Ok(())
    }

    /// Commit a named savepoint previously opened with `begin_savepoint`.
    pub fn release_savepoint(&mut self, name: &str) -> Result<(), DmStoreError> {
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(DmStoreError::InvalidPath {
                path: name.to_string(),
                reason: "savepoint name must be alphanumeric/underscore".to_string(),
            });
        }
        self.conn
            .execute_batch(&format!("RELEASE SAVEPOINT {}", name))?;
        Ok(())
    }

    /// Abort a named savepoint and reload caches so they reflect the DB.
    pub fn rollback_savepoint(&mut self, name: &str) -> Result<(), DmStoreError> {
        if !name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return Err(DmStoreError::InvalidPath {
                path: name.to_string(),
                reason: "savepoint name must be alphanumeric/underscore".to_string(),
            });
        }
        self.conn.execute_batch(&format!(
            "ROLLBACK TO SAVEPOINT {0}; RELEASE SAVEPOINT {0};",
            name
        ))?;
        self.reload_cache()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_open_in_memory() {
        let store = DmStore::open_in_memory();
        assert!(store.is_ok());
    }

    #[test]
    fn test_define_and_get() {
        let mut store = DmStore::open_in_memory().unwrap();
        store.define_object("Device.", false).unwrap();
        store.define_object("Device.WiFi.", false).unwrap();
        store.define_object("Device.WiFi.Radio.", true).unwrap();
        store
            .define_parameter(
                "Device.WiFi.Radio.{i}.Enable",
                ParamType::Boolean,
                true,
                Some("true"),
            )
            .unwrap();

        // Template paths should NOT be in dm_param
        let conn = store.connection();
        let count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dm_param WHERE path LIKE '%{i}%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "dm_param should have no template paths");

        // Add an instance and verify we can get it
        {
            let mut session = store.session().unwrap();
            let r = session.add("Device.WiFi.Radio.").unwrap();
            assert_eq!(r.instance_number, 1);
            session.commit().unwrap();
        }
        let param = store.get("Device.WiFi.Radio.1.Enable").unwrap();
        assert_eq!(param.value.as_deref(), Some("true"));
        assert_eq!(param.param_type, ParamType::Boolean);
    }

    #[test]
    fn test_get_not_found() {
        let store = DmStore::open_in_memory().unwrap();
        let result = store.get("Device.NonExistent");
        assert!(matches!(result, Err(DmStoreError::NotFound(_))));
    }

    #[test]
    fn test_cache_disabled() {
        let config = DmStoreConfig { use_cache: false };
        let conn = Connection::open_in_memory().unwrap();
        let store = DmStore::init(conn, config).unwrap();
        assert!(!store.cache_enabled());
    }

    #[test]
    fn test_define_param_propagates_to_existing_instances() {
        let mut store = DmStore::open_in_memory().unwrap();
        store.define_object("Device.", false).unwrap();
        store.define_object("Device.SSID.", true).unwrap();
        store.define_object("Device.SSID.{i}.", false).unwrap();
        store
            .define_parameter(
                "Device.SSID.{i}.Enable",
                ParamType::Boolean,
                true,
                Some("true"),
            )
            .unwrap();

        // Add two instances first
        {
            let mut session = store.session().unwrap();
            session.add("Device.SSID.").unwrap();
            session.add("Device.SSID.").unwrap();
            session.commit().unwrap();
        }

        // Now define a NEW template param -- should propagate to instances 1 and 2
        store
            .define_parameter(
                "Device.SSID.{i}.Name",
                ParamType::String,
                true,
                Some("default"),
            )
            .unwrap();

        let p1 = store.get("Device.SSID.1.Name").unwrap();
        assert_eq!(p1.value.as_deref(), Some("default"));

        let p2 = store.get("Device.SSID.2.Name").unwrap();
        assert_eq!(p2.value.as_deref(), Some("default"));
    }

    #[test]
    fn test_schema_separation() {
        let mut store = DmStore::open_in_memory().unwrap();
        store.define_object("Device.", false).unwrap();
        store.define_object("Device.WiFi.", true).unwrap();
        store.define_object("Device.WiFi.{i}.", false).unwrap();
        store
            .define_parameter(
                "Device.WiFi.{i}.Name",
                ParamType::String,
                true,
                Some("wifi"),
            )
            .unwrap();

        // 1. dm_object and dm_param should have NO {i} rows
        let conn = store.connection();
        let obj_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dm_object WHERE path LIKE '%{i}%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(obj_count, 0, "dm_object should have no template rows");

        let param_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dm_param WHERE path LIKE '%{i}%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(param_count, 0, "dm_param should have no template rows");

        // Schema tables SHOULD have the templates
        let schema_obj_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dm_schema_object WHERE path LIKE '%{i}%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            schema_obj_count > 0,
            "dm_schema_object should have template rows"
        );

        let schema_param_count: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dm_schema_param WHERE path LIKE '%{i}%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(
            schema_param_count > 0,
            "dm_schema_param should have template rows"
        );

        // 2. After add, dm_object has concrete instance, dm_param has concrete param
        {
            let mut session = store.session().unwrap();
            let r = session.add("Device.WiFi.").unwrap();
            assert_eq!(r.instance_number, 1);
            assert_eq!(r.path, "Device.WiFi.1.");
            session.commit().unwrap();
        }

        let conn = store.connection();
        let inst_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM dm_object WHERE path = 'Device.WiFi.1.'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(inst_exists, "dm_object should have Device.WiFi.1.");

        let param_exists: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM dm_param WHERE path = 'Device.WiFi.1.Name'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(param_exists, "dm_param should have Device.WiFi.1.Name");

        let p = store.get("Device.WiFi.1.Name").unwrap();
        assert_eq!(p.value.as_deref(), Some("wifi"));
    }

    #[test]
    fn test_propagation_after_add() {
        // Define templates, add instances, then define MORE templates -- verify propagation
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

        // Add two instances
        {
            let mut session = store.session().unwrap();
            session.add("Device.Radio.").unwrap();
            session.add("Device.Radio.").unwrap();
            session.commit().unwrap();
        }

        // Define a new child template object under the template
        store
            .define_object("Device.Radio.{i}.Stats.", false)
            .unwrap();
        store
            .define_parameter(
                "Device.Radio.{i}.Stats.BytesSent",
                ParamType::UnsignedLong,
                false,
                Some("0"),
            )
            .unwrap();

        // Verify propagation: Stats object and param should exist for both instances
        let conn = store.connection();
        let stats1: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM dm_object WHERE path = 'Device.Radio.1.Stats.'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(stats1, "Device.Radio.1.Stats. should exist in dm_object");

        let stats2: bool = conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM dm_object WHERE path = 'Device.Radio.2.Stats.'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(stats2, "Device.Radio.2.Stats. should exist in dm_object");

        let p1 = store.get("Device.Radio.1.Stats.BytesSent").unwrap();
        assert_eq!(p1.value.as_deref(), Some("0"));

        let p2 = store.get("Device.Radio.2.Stats.BytesSent").unwrap();
        assert_eq!(p2.value.as_deref(), Some("0"));
    }

    #[test]
    fn test_nested_template_propagation() {
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

        // Add WiFi instance
        {
            let mut session = store.session().unwrap();
            session.add("Device.WiFi.").unwrap();
            session.commit().unwrap();
        }

        // Add SSID instance under WiFi.1
        {
            let mut session = store.session().unwrap();
            session.add("Device.WiFi.1.SSID.").unwrap();
            session.commit().unwrap();
        }

        let p = store.get("Device.WiFi.1.SSID.1.Name").unwrap();
        assert_eq!(p.value.as_deref(), Some("ssid"));

        // Verify no template paths leaked into dm_object or dm_param
        let conn = store.connection();
        let obj_tmpl: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dm_object WHERE path LIKE '%{i}%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(obj_tmpl, 0);

        let param_tmpl: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM dm_param WHERE path LIKE '%{i}%'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(param_tmpl, 0);
    }

    #[test]
    fn test_instances_basic() {
        let mut store = DmStore::open_in_memory().unwrap();
        store.define_object("Device.", false).unwrap();
        store.define_object("Device.Radio.", true).unwrap();
        store.define_object("Device.Radio.{i}.", false).unwrap();

        // No instances yet
        assert_eq!(store.instances("Device.Radio.").unwrap(), vec![]);

        // Add 3 instances
        {
            let mut session = store.session().unwrap();
            session.add("Device.Radio.").unwrap();
            session.add("Device.Radio.").unwrap();
            session.add("Device.Radio.").unwrap();
            session.commit().unwrap();
        }

        assert_eq!(store.instances("Device.Radio.").unwrap(), vec![1, 2, 3]);
    }

    #[test]
    fn test_instances_after_delete() {
        let mut store = DmStore::open_in_memory().unwrap();
        store.define_object("Device.", false).unwrap();
        store.define_object("Device.Radio.", true).unwrap();
        store.define_object("Device.Radio.{i}.", false).unwrap();

        {
            let mut session = store.session().unwrap();
            session.add("Device.Radio.").unwrap();
            session.add("Device.Radio.").unwrap();
            session.add("Device.Radio.").unwrap();
            session.commit().unwrap();
        }

        // Delete instance 2
        {
            let mut session = store.session().unwrap();
            session.delete("Device.Radio.2.").unwrap();
            session.commit().unwrap();
        }

        assert_eq!(store.instances("Device.Radio.").unwrap(), vec![1, 3]);
    }

    #[test]
    fn test_instances_no_cache() {
        let config = DmStoreConfig { use_cache: false };
        let conn = Connection::open_in_memory().unwrap();
        let mut store = DmStore::init(conn, config).unwrap();
        store.define_object("Device.", false).unwrap();
        store.define_object("Device.Radio.", true).unwrap();
        store.define_object("Device.Radio.{i}.", false).unwrap();

        {
            let mut session = store.session().unwrap();
            session.add("Device.Radio.").unwrap();
            session.add("Device.Radio.").unwrap();
            session.commit().unwrap();
        }

        assert_eq!(store.instances("Device.Radio.").unwrap(), vec![1, 2]);
    }

    #[test]
    fn test_template_first_definition_can_upgrade_parent_to_multi() {
        let mut store = DmStore::open_in_memory().unwrap();

        store.define_object("Device.WiFi.{i}.", false).unwrap();
        store.define_object("Device.WiFi.", true).unwrap();

        let is_multi: bool = store
            .connection()
            .query_row(
                "SELECT is_multi FROM dm_object WHERE path = 'Device.WiFi.'",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert!(is_multi);

        let mut session = store.session().unwrap();
        let result = session.add("Device.WiFi.").unwrap();
        assert_eq!(result.path, "Device.WiFi.1.");
    }

    #[test]
    fn test_define_parameter_duplicate_returns_already_exists() {
        let mut store = DmStore::open_in_memory().unwrap();
        store.define_object("Device.", false).unwrap();
        store.define_object("Device.Test.", false).unwrap();
        store
            .define_parameter("Device.Test.Name", ParamType::String, true, Some("hello"))
            .unwrap();

        let err = store
            .define_parameter("Device.Test.Name", ParamType::String, true, Some("world"))
            .unwrap_err();
        assert!(matches!(err, DmStoreError::AlreadyExists(path) if path == "Device.Test.Name"));

        let param = store.get("Device.Test.Name").unwrap();
        assert_eq!(param.value.as_deref(), Some("hello"));
    }

    /// Synthesise a hash collision by forcing two rows in dm_param to share
    /// the same path_hash. load_cache must keep both entries; get() must
    /// still resolve each by path equality.
    #[test]
    fn test_cache_handles_hash_collision() {
        let mut store = DmStore::open_in_memory().unwrap();
        store.define_object("Device.", false).unwrap();
        store.define_object("Device.A.", false).unwrap();
        store.define_object("Device.B.", false).unwrap();
        store
            .define_parameter("Device.A.Name", ParamType::String, true, Some("a"))
            .unwrap();
        store
            .define_parameter("Device.B.Name", ParamType::String, true, Some("b"))
            .unwrap();

        // Force both rows to share a synthetic hash value.
        let collision_hash: i64 = 0x4242_4242_4242_4242u64 as i64;
        store
            .connection()
            .execute(
                "UPDATE dm_param SET path_hash = ?1 WHERE path IN ('Device.A.Name', 'Device.B.Name')",
                rusqlite::params![collision_hash],
            )
            .unwrap();

        // Reload so the in-memory cache picks up the collision.
        store.reload_cache().unwrap();

        // Direct DB lookups bypass the cache -- they must use the fake hash.
        let a = DmStore::get_from_db(store.connection(), "Device.A.Name", collision_hash).unwrap();
        assert_eq!(a.value.as_deref(), Some("a"));
        let b = DmStore::get_from_db(store.connection(), "Device.B.Name", collision_hash).unwrap();
        assert_eq!(b.value.as_deref(), Some("b"));

        // Cache path: DmStore::get computes the real hash which no longer
        // matches the stored synthetic hash, so this exercises the DB
        // fallback path. To test cache lookup with a collision, inspect the
        // cache directly.
        let cache = store.cache.as_ref().expect("cache enabled");
        let bucket = cache.get(&collision_hash).expect("bucket for collision hash");
        assert_eq!(bucket.len(), 2, "both paths must share the bucket");
        let paths: Vec<&str> = bucket.iter().map(|p| p.path.as_str()).collect();
        assert!(paths.contains(&"Device.A.Name"));
        assert!(paths.contains(&"Device.B.Name"));
    }
}
