//! QuickJS integration for data-model handlers.
//!
//! Handler naming convention (derived from the canonical template path):
//! - `{i}` -> `i`, `.` -> `_`, trailing `.` stripped.
//! - `Device.Bridging.Bridge.{i}.Enable` -> ident `Device_Bridging_Bridge_i_Enable`.
//!
//! Handler functions live on the global object:
//! - `DM_Getter_<ident>(instances)`  -> value (string/number/bool) | null | undefined
//! - `DM_Setter_<ident>(instances, value)` -> true | false
//! - `DM_Instances_<ident>(parent_instances)` -> array of integers | null | undefined
//! - `DM_Init()` -> called once per sub-folder on first registration.
//!
//! Handlers can read from / write to the store via the `DM` global:
//! - `DM.get(path)` -> string value or null
//! - `DM.set(path, value)` -> true (throws on failure)
//! - `DM.add(tablePath)` -> new instance number
//! - `DM.del(instancePath)` -> true
//! - `DM.instances(tablePath)` -> array of numbers
//! - `DM.update(path, config)` -> true (recursive bulk write)
//!
//! `DM.update(base, config)` walks a JSON-like config object rooted at
//! `base` (an object path ending in `.`) and writes it to the store:
//! - an array value `key: [obj, obj, ...]` treats `{base}{key}.` as a
//!   multi-instance table; each element adds a fresh instance via
//!   `DM.add` and its contents are applied to `{base}{key}.{N}.`.
//! - an object value `key: { ... }` recurses with `{base}{key}.` as the
//!   new base (static sub-object).
//! - any other primitive (string/number/boolean) writes
//!   `{base}{key}` via `DM.set` (booleans become `"true"`/`"false"`).
//!
//! The bridge is installed by the caller (`call_getter`/`call_setter`/
//! `call_instances`/`call_init`) for the duration of one JS call via a
//! thread-local raw pointer and a RAII guard. Write APIs (`set`/`add`/`del`/
//! `update`) throw if only a read-only DmStore is installed.

use std::cell::Cell;
use std::fs;
use std::path::Path;
use std::ptr::NonNull;

use dm_store_lib::session::Session;
use dm_store_lib::DmStore;
use rquickjs::{
    Array, Context, Ctx, Exception, Function, Object, Result as JsResult, Runtime, Value,
};

use crate::error::DmManagerError;

/// Convert a template path to the identifier suffix used in handler names.
///
/// The suffix drops the trailing `.` (object paths), replaces `{i}` with `i`,
/// and replaces remaining `.` with `_`.
pub fn path_to_ident(template_path: &str) -> String {
    template_path
        .trim_end_matches('.')
        .replace("{i}", "i")
        .replace('.', "_")
}

fn js_err(name: &str, reason: impl std::fmt::Display) -> DmManagerError {
    DmManagerError::HookError {
        path: name.to_string(),
        reason: reason.to_string(),
    }
}

// --- Bridge thread-local state ---------------------------------------------
//
// Raw pointers installed by the caller before invoking a JS handler, cleared
// by the RAII guard. These are only dereferenced during a synchronous JS call
// from within `JsHandlers::call_*`, so lifetimes are upheld dynamically.

thread_local! {
    static BRIDGE_STORE: Cell<Option<NonNull<DmStore>>> = const { Cell::new(None) };
    // Type-erased `*mut Session<'_>`; valid only while set.
    static BRIDGE_SESSION: Cell<Option<NonNull<u8>>> = const { Cell::new(None) };
    static BRIDGE_SESSION_WRITABLE: Cell<bool> = const { Cell::new(false) };
}

/// A borrow of the database to expose to JS handlers during a call.
pub enum BridgeDb<'a, 's> {
    /// No DB access; `DM.*` calls throw.
    None,
    /// Read-only access via the manager's DmStore.
    Store(&'a DmStore),
    /// Read-only access via a shared session borrow (sees uncommitted writes
    /// in the current session).
    SessionRead(&'a Session<'s>),
    /// Read + write access via an exclusive session borrow.
    SessionWrite(&'a mut Session<'s>),
}

struct BridgeGuard;

impl Drop for BridgeGuard {
    fn drop(&mut self) {
        BRIDGE_STORE.with(|c| c.set(None));
        BRIDGE_SESSION.with(|c| c.set(None));
        BRIDGE_SESSION_WRITABLE.with(|c| c.set(false));
    }
}

fn install_bridge(db: BridgeDb<'_, '_>) -> BridgeGuard {
    // Always clear first so a prior install doesn't leak into a different mode.
    BRIDGE_STORE.with(|c| c.set(None));
    BRIDGE_SESSION.with(|c| c.set(None));
    BRIDGE_SESSION_WRITABLE.with(|c| c.set(false));

    match db {
        BridgeDb::None => {}
        BridgeDb::Store(store) => {
            BRIDGE_STORE.with(|c| c.set(Some(NonNull::from(store))));
        }
        BridgeDb::SessionRead(session) => {
            let raw = session as *const Session<'_> as *mut Session<'_> as *mut u8;
            BRIDGE_SESSION.with(|c| c.set(NonNull::new(raw)));
            BRIDGE_SESSION_WRITABLE.with(|c| c.set(false));
        }
        BridgeDb::SessionWrite(session) => {
            let raw: *mut Session<'_> = session;
            BRIDGE_SESSION.with(|c| c.set(NonNull::new(raw as *mut u8)));
            BRIDGE_SESSION_WRITABLE.with(|c| c.set(true));
        }
    }
    BridgeGuard
}

fn throw_js<'js, T>(ctx: &Ctx<'js>, msg: impl AsRef<str>) -> JsResult<T> {
    Err(Exception::throw_message(ctx, msg.as_ref()))
}

/// Format a `rquickjs::Error` from a `.call(...)` into a readable string by
/// catching the associated exception value (if any) from the context.
fn format_js_call_err(ctx: &Ctx<'_>, err: rquickjs::Error) -> String {
    let caught = rquickjs::CaughtError::from_error(ctx, err);
    caught.to_string()
}

// --- DM.* bridge implementations (thread-local-backed) ---------------------

fn dm_get<'js>(ctx: Ctx<'js>, path: String) -> JsResult<Value<'js>> {
    let result: std::result::Result<Option<String>, String> =
        BRIDGE_SESSION.with(|sess_cell| -> std::result::Result<Option<String>, String> {
            if let Some(ptr) = sess_cell.get() {
                let session: &Session<'_> = unsafe { &*(ptr.as_ptr() as *const Session<'_>) };
                return match session.get(&path) {
                    Ok(p) => Ok(p.value),
                    Err(dm_store_lib::DmStoreError::NotFound(_)) => Ok(None),
                    Err(e) => Err(e.to_string()),
                };
            }
            BRIDGE_STORE.with(|store_cell| -> std::result::Result<Option<String>, String> {
                let Some(ptr) = store_cell.get() else {
                    return Err("DM.get: no database bridge installed".to_string());
                };
                let store: &DmStore = unsafe { ptr.as_ref() };
                match store.get(&path) {
                    Ok(p) => Ok(p.value),
                    Err(dm_store_lib::DmStoreError::NotFound(_)) => Ok(None),
                    Err(e) => Err(e.to_string()),
                }
            })
        });

    match result {
        Ok(Some(s)) => Ok(rquickjs::String::from_str(ctx.clone(), &s)?.into_value()),
        Ok(None) => Ok(Value::new_null(ctx.clone())),
        Err(msg) => throw_js(&ctx, msg),
    }
}

fn dm_set<'js>(ctx: Ctx<'js>, path: String, value: String) -> JsResult<bool> {
    let result: std::result::Result<(), String> =
        BRIDGE_SESSION.with(|sess_cell| -> std::result::Result<(), String> {
            let Some(ptr) = sess_cell.get() else {
                return Err("DM.set: requires a write session".to_string());
            };
            if !BRIDGE_SESSION_WRITABLE.with(|c| c.get()) {
                return Err("DM.set: session is read-only".to_string());
            }
            let session: &mut Session<'_> = unsafe { &mut *(ptr.as_ptr() as *mut Session<'_>) };
            session.set(&path, &value).map_err(|e| e.to_string())
        });

    match result {
        Ok(()) => Ok(true),
        Err(msg) => throw_js(&ctx, msg),
    }
}

fn dm_add<'js>(ctx: Ctx<'js>, table_path: String) -> JsResult<i64> {
    let result: std::result::Result<u32, String> =
        BRIDGE_SESSION.with(|sess_cell| -> std::result::Result<u32, String> {
            let Some(ptr) = sess_cell.get() else {
                return Err("DM.add: requires a write session".to_string());
            };
            if !BRIDGE_SESSION_WRITABLE.with(|c| c.get()) {
                return Err("DM.add: session is read-only".to_string());
            }
            let session: &mut Session<'_> = unsafe { &mut *(ptr.as_ptr() as *mut Session<'_>) };
            session
                .add(&table_path)
                .map(|r| r.instance_number)
                .map_err(|e| e.to_string())
        });

    match result {
        Ok(n) => Ok(n as i64),
        Err(msg) => throw_js(&ctx, msg),
    }
}

fn dm_del<'js>(ctx: Ctx<'js>, instance_path: String) -> JsResult<bool> {
    let result: std::result::Result<(), String> =
        BRIDGE_SESSION.with(|sess_cell| -> std::result::Result<(), String> {
            let Some(ptr) = sess_cell.get() else {
                return Err("DM.del: requires a write session".to_string());
            };
            if !BRIDGE_SESSION_WRITABLE.with(|c| c.get()) {
                return Err("DM.del: session is read-only".to_string());
            }
            let session: &mut Session<'_> = unsafe { &mut *(ptr.as_ptr() as *mut Session<'_>) };
            session.delete(&instance_path).map_err(|e| e.to_string())
        });

    match result {
        Ok(()) => Ok(true),
        Err(msg) => throw_js(&ctx, msg),
    }
}

// --- DM.update -------------------------------------------------------------

fn ensure_object_base(path: &str) -> String {
    if path.is_empty() || path.ends_with('.') {
        path.to_string()
    } else {
        format!("{path}.")
    }
}

fn primitive_to_string<'js>(
    ctx: &Ctx<'js>,
    v: &Value<'js>,
    path: &str,
) -> JsResult<String> {
    if let Some(s) = v.as_string() {
        return s
            .to_string()
            .map_err(|e| Exception::throw_message(ctx, &format!("DM.update: string decode at {path}: {e}")));
    }
    if let Some(b) = v.as_bool() {
        return Ok(if b { "true" } else { "false" }.to_string());
    }
    if let Some(i) = v.as_int() {
        return Ok(i.to_string());
    }
    if let Some(f) = v.as_float() {
        return Ok(f.to_string());
    }
    throw_js(
        ctx,
        format!("DM.update: unsupported value type at {path}"),
    )
}

fn session_add_raw<'js>(ctx: &Ctx<'js>, table_path: &str) -> JsResult<u32> {
    let result: std::result::Result<u32, String> =
        BRIDGE_SESSION.with(|sess_cell| -> std::result::Result<u32, String> {
            let Some(ptr) = sess_cell.get() else {
                return Err("DM.update: requires a write session".to_string());
            };
            if !BRIDGE_SESSION_WRITABLE.with(|c| c.get()) {
                return Err("DM.update: session is read-only".to_string());
            }
            let session: &mut Session<'_> = unsafe { &mut *(ptr.as_ptr() as *mut Session<'_>) };
            session
                .add(table_path)
                .map(|r| r.instance_number)
                .map_err(|e| format!("add({table_path}): {e}"))
        });
    result.map_err(|msg| Exception::throw_message(ctx, &msg))
}

fn session_set_raw<'js>(ctx: &Ctx<'js>, path: &str, value: &str) -> JsResult<()> {
    let result: std::result::Result<(), String> =
        BRIDGE_SESSION.with(|sess_cell| -> std::result::Result<(), String> {
            let Some(ptr) = sess_cell.get() else {
                return Err("DM.update: requires a write session".to_string());
            };
            if !BRIDGE_SESSION_WRITABLE.with(|c| c.get()) {
                return Err("DM.update: session is read-only".to_string());
            }
            let session: &mut Session<'_> = unsafe { &mut *(ptr.as_ptr() as *mut Session<'_>) };
            session
                .set(path, value)
                .map_err(|e| format!("set({path}={value}): {e}"))
        });
    result.map_err(|msg| Exception::throw_message(ctx, &msg))
}

fn apply_update<'js>(ctx: &Ctx<'js>, base: &str, obj: Object<'js>) -> JsResult<()> {
    let entries: Vec<(String, Value<'js>)> = obj
        .props::<String, Value<'js>>()
        .collect::<JsResult<Vec<_>>>()?;

    for (key, value) in entries {
        if value.is_array() {
            let arr = Array::from_value(value)
                .map_err(|e| Exception::throw_message(ctx, &format!("DM.update: array cast at {base}{key}: {e}")))?;
            let table_path = format!("{base}{key}.");
            for i in 0..arr.len() {
                let elem: Value = arr
                    .get(i)
                    .map_err(|e| Exception::throw_message(ctx, &format!("DM.update: {table_path}[{i}]: {e}")))?;
                if !elem.is_object() || elem.is_array() || elem.is_function() {
                    return throw_js(
                        ctx,
                        format!("DM.update: {table_path}[{i}] must be an object"),
                    );
                }
                let elem_obj = Object::from_value(elem).map_err(|e| {
                    Exception::throw_message(ctx, &format!("DM.update: {table_path}[{i}] object cast: {e}"))
                })?;
                let inst = session_add_raw(ctx, &table_path)?;
                let inst_path = format!("{table_path}{inst}.");
                apply_update(ctx, &inst_path, elem_obj)?;
            }
        } else if value.is_null() || value.is_undefined() {
            return throw_js(
                ctx,
                format!("DM.update: null/undefined value at {base}{key}"),
            );
        } else if value.is_function() {
            return throw_js(
                ctx,
                format!("DM.update: function value not allowed at {base}{key}"),
            );
        } else if value.is_object() {
            let sub_obj = Object::from_value(value).map_err(|e| {
                Exception::throw_message(ctx, &format!("DM.update: object cast at {base}{key}: {e}"))
            })?;
            let sub_base = format!("{base}{key}.");
            apply_update(ctx, &sub_base, sub_obj)?;
        } else {
            let param_path = format!("{base}{key}");
            let val_str = primitive_to_string(ctx, &value, &param_path)?;
            session_set_raw(ctx, &param_path, &val_str)?;
        }
    }
    Ok(())
}

fn dm_update<'js>(ctx: Ctx<'js>, path: String, config: Object<'js>) -> JsResult<bool> {
    if BRIDGE_SESSION.with(|c| c.get().is_none()) {
        return throw_js(&ctx, "DM.update: requires a write session");
    }
    if !BRIDGE_SESSION_WRITABLE.with(|c| c.get()) {
        return throw_js(&ctx, "DM.update: session is read-only");
    }
    let base = ensure_object_base(&path);
    apply_update(&ctx, &base, config)?;
    Ok(true)
}

fn dm_instances<'js>(ctx: Ctx<'js>, table_path: String) -> JsResult<Vec<i64>> {
    let result: std::result::Result<Vec<u32>, String> =
        BRIDGE_SESSION.with(|sess_cell| -> std::result::Result<Vec<u32>, String> {
            if let Some(ptr) = sess_cell.get() {
                let session: &Session<'_> = unsafe { &*(ptr.as_ptr() as *const Session<'_>) };
                return session.instances(&table_path).map_err(|e| e.to_string());
            }
            BRIDGE_STORE.with(|store_cell| -> std::result::Result<Vec<u32>, String> {
                let Some(ptr) = store_cell.get() else {
                    return Err("DM.instances: no database bridge installed".to_string());
                };
                let store: &DmStore = unsafe { ptr.as_ref() };
                store.instances(&table_path).map_err(|e| e.to_string())
            })
        });

    match result {
        Ok(nums) => Ok(nums.into_iter().map(|n| n as i64).collect()),
        Err(msg) => throw_js(&ctx, msg),
    }
}

/// Owning wrapper around a QuickJS runtime + context used to host DM_* handlers.
pub struct JsHandlers {
    // Runtime must outlive the context; keep the field to extend its lifetime.
    #[allow(dead_code)]
    runtime: Runtime,
    context: Context,
}

impl JsHandlers {
    pub fn new() -> Result<Self, DmManagerError> {
        let runtime = Runtime::new()
            .map_err(|e| DmManagerError::Schema(format!("quickjs runtime init: {e}")))?;
        let context = Context::full(&runtime)
            .map_err(|e| DmManagerError::Schema(format!("quickjs context init: {e}")))?;
        let handlers = Self { runtime, context };
        handlers.install_dm_bridge()?;
        Ok(handlers)
    }

    fn install_dm_bridge(&self) -> Result<(), DmManagerError> {
        self.context.with(|ctx| -> JsResult<()> {
            let dm = Object::new(ctx.clone())?;
            dm.set("get", Function::new(ctx.clone(), dm_get)?)?;
            dm.set("set", Function::new(ctx.clone(), dm_set)?)?;
            dm.set("add", Function::new(ctx.clone(), dm_add)?)?;
            dm.set("del", Function::new(ctx.clone(), dm_del)?)?;
            dm.set("instances", Function::new(ctx.clone(), dm_instances)?)?;
            dm.set("update", Function::new(ctx.clone(), dm_update)?)?;
            ctx.globals().set("DM", dm)?;
            Ok(())
        })
        .map_err(|e| DmManagerError::Schema(format!("DM bridge install: {e}")))
    }

    /// Evaluate a single JS source file in the shared global context.
    pub fn eval_file(&self, path: &Path) -> Result<(), DmManagerError> {
        let code = fs::read_to_string(path)?;
        self.eval_source(&code, &path.display().to_string())
    }

    /// Evaluate a JS source string (with a display label for error messages).
    pub fn eval_source(&self, code: &str, label: &str) -> Result<(), DmManagerError> {
        self.context.with(|ctx| -> Result<(), DmManagerError> {
            ctx.eval::<(), _>(code.as_bytes())
                .map_err(|e| DmManagerError::Schema(format!("quickjs eval {label}: {e}")))
        })
    }

    /// Check whether a callable global with this name exists.
    pub fn has_function(&self, name: &str) -> bool {
        self.context.with(|ctx| {
            let globals: Object = ctx.globals();
            match globals.get::<_, Value>(name) {
                Ok(v) => v.is_function(),
                Err(_) => false,
            }
        })
    }

    /// Invoke a getter. `None` indicates the JS returned `null` / `undefined`.
    pub fn call_getter(
        &self,
        name: &str,
        instances: &[u32],
        db: BridgeDb<'_, '_>,
    ) -> Result<Option<String>, DmManagerError> {
        let _guard = install_bridge(db);
        let nums: Vec<i64> = instances.iter().map(|n| *n as i64).collect();
        self.context.with(|ctx| -> Result<Option<String>, DmManagerError> {
            let f: Function = ctx
                .globals()
                .get(name)
                .map_err(|e| js_err(name, format!("not a function: {e}")))?;
            let result: Value = f
                .call((nums,))
                .map_err(|e| js_err(name, format!("call failed: {}", format_js_call_err(&ctx, e))))?;
            value_to_opt_string(&result, name)
        })
    }

    /// Invoke a setter. The JS function must return a boolean; anything other
    /// than `true` is treated as failure.
    pub fn call_setter(
        &self,
        name: &str,
        instances: &[u32],
        value: &str,
        db: BridgeDb<'_, '_>,
    ) -> Result<bool, DmManagerError> {
        let _guard = install_bridge(db);
        let nums: Vec<i64> = instances.iter().map(|n| *n as i64).collect();
        let value = value.to_string();
        self.context.with(|ctx| -> Result<bool, DmManagerError> {
            let f: Function = ctx
                .globals()
                .get(name)
                .map_err(|e| js_err(name, format!("not a function: {e}")))?;
            let result: Value = f
                .call((nums, value))
                .map_err(|e| js_err(name, format!("call failed: {}", format_js_call_err(&ctx, e))))?;
            Ok(matches!(result.as_bool(), Some(true)))
        })
    }

    /// Invoke an instances handler. `None` indicates the JS returned
    /// `null` / `undefined`.
    pub fn call_instances(
        &self,
        name: &str,
        parent_instances: &[u32],
        db: BridgeDb<'_, '_>,
    ) -> Result<Option<Vec<u32>>, DmManagerError> {
        let _guard = install_bridge(db);
        let nums: Vec<i64> = parent_instances.iter().map(|n| *n as i64).collect();
        self.context
            .with(|ctx| -> Result<Option<Vec<u32>>, DmManagerError> {
                let f: Function = ctx
                    .globals()
                    .get(name)
                    .map_err(|e| js_err(name, format!("not a function: {e}")))?;
                let result: Value = f
                    .call((nums,))
                    .map_err(|e| js_err(name, format!("call failed: {e}")))?;
                if result.is_null() || result.is_undefined() {
                    return Ok(None);
                }
                let arr: Array = Array::from_value(result)
                    .map_err(|e| js_err(name, format!("expected array: {e}")))?;
                let mut out = Vec::with_capacity(arr.len());
                for i in 0..arr.len() {
                    let v: Value = arr
                        .get(i)
                        .map_err(|e| js_err(name, format!("array[{i}]: {e}")))?;
                    let n = if let Some(i) = v.as_int() {
                        i as i64
                    } else if let Some(f) = v.as_float() {
                        f as i64
                    } else {
                        return Err(js_err(name, "instance values must be numbers"));
                    };
                    if n < 0 {
                        return Err(js_err(name, format!("negative instance: {n}")));
                    }
                    out.push(n as u32);
                }
                Ok(Some(out))
            })
    }

    /// Call `DM_Init()` if it's defined on the global object. No-op otherwise.
    pub fn call_init_if_present(&self, db: BridgeDb<'_, '_>) -> Result<(), DmManagerError> {
        if !self.has_function("DM_Init") {
            return Ok(());
        }
        let _guard = install_bridge(db);
        self.context.with(|ctx| -> Result<(), DmManagerError> {
            let f: Function = ctx
                .globals()
                .get("DM_Init")
                .map_err(|e| js_err("DM_Init", format!("not a function: {e}")))?;
            let _: Value = f
                .call(())
                .map_err(|e| js_err("DM_Init", format!("call failed: {}", format_js_call_err(&ctx, e))))?;
            Ok(())
        })
    }
}

fn value_to_opt_string(v: &Value, name: &str) -> Result<Option<String>, DmManagerError> {
    if v.is_null() || v.is_undefined() {
        return Ok(None);
    }
    if let Some(s) = v.as_string() {
        let s = s
            .to_string()
            .map_err(|e| js_err(name, format!("string decode: {e}")))?;
        return Ok(Some(s));
    }
    if let Some(b) = v.as_bool() {
        return Ok(Some(if b { "true" } else { "false" }.to_string()));
    }
    if let Some(i) = v.as_int() {
        return Ok(Some(i.to_string()));
    }
    if let Some(f) = v.as_float() {
        return Ok(Some(f.to_string()));
    }
    Err(js_err(name, "getter must return string, number, or boolean"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_to_ident_simple() {
        assert_eq!(
            path_to_ident("Device.DeviceInfo.SoftwareVersion"),
            "Device_DeviceInfo_SoftwareVersion"
        );
    }

    #[test]
    fn test_path_to_ident_template() {
        assert_eq!(
            path_to_ident("Device.Bridging.Bridge.{i}.Enable"),
            "Device_Bridging_Bridge_i_Enable"
        );
    }

    #[test]
    fn test_path_to_ident_trailing_dot() {
        assert_eq!(
            path_to_ident("Device.Bridging.Bridge."),
            "Device_Bridging_Bridge"
        );
    }

    #[test]
    fn test_getter_returns_string() {
        let h = JsHandlers::new().unwrap();
        h.eval_source(
            "function DM_Getter_Foo(instances) { return 'hello'; }",
            "<test>",
        )
        .unwrap();
        assert!(h.has_function("DM_Getter_Foo"));
        let v = h.call_getter("DM_Getter_Foo", &[], BridgeDb::None).unwrap();
        assert_eq!(v, Some("hello".to_string()));
    }

    #[test]
    fn test_getter_returns_number() {
        let h = JsHandlers::new().unwrap();
        h.eval_source("function DM_Getter_N() { return 42; }", "<test>")
            .unwrap();
        let v = h.call_getter("DM_Getter_N", &[], BridgeDb::None).unwrap();
        assert_eq!(v, Some("42".to_string()));
    }

    #[test]
    fn test_getter_returns_bool() {
        let h = JsHandlers::new().unwrap();
        h.eval_source("function DM_Getter_B() { return true; }", "<test>")
            .unwrap();
        let v = h.call_getter("DM_Getter_B", &[], BridgeDb::None).unwrap();
        assert_eq!(v, Some("true".to_string()));
    }

    #[test]
    fn test_getter_undefined_returns_none() {
        let h = JsHandlers::new().unwrap();
        h.eval_source("function DM_Getter_U() { return undefined; }", "<test>")
            .unwrap();
        let v = h.call_getter("DM_Getter_U", &[], BridgeDb::None).unwrap();
        assert_eq!(v, None);
    }

    #[test]
    fn test_getter_receives_instances() {
        let h = JsHandlers::new().unwrap();
        h.eval_source(
            "function DM_Getter_X(instances) { return JSON.stringify(instances); }",
            "<test>",
        )
        .unwrap();
        let v = h
            .call_getter("DM_Getter_X", &[1, 2], BridgeDb::None)
            .unwrap();
        assert_eq!(v, Some("[1,2]".to_string()));
    }

    #[test]
    fn test_setter_true() {
        let h = JsHandlers::new().unwrap();
        h.eval_source(
            "function DM_Setter_Y(instances, value) { return value === 'ok'; }",
            "<test>",
        )
        .unwrap();
        assert!(h
            .call_setter("DM_Setter_Y", &[], "ok", BridgeDb::None)
            .unwrap());
        assert!(!h
            .call_setter("DM_Setter_Y", &[], "no", BridgeDb::None)
            .unwrap());
    }

    #[test]
    fn test_instances_array() {
        let h = JsHandlers::new().unwrap();
        h.eval_source(
            "function DM_Instances_Z(parents) { return [1, 2, 3]; }",
            "<test>",
        )
        .unwrap();
        let v = h
            .call_instances("DM_Instances_Z", &[], BridgeDb::None)
            .unwrap();
        assert_eq!(v, Some(vec![1, 2, 3]));
    }

    #[test]
    fn test_instances_null_returns_none() {
        let h = JsHandlers::new().unwrap();
        h.eval_source("function DM_Instances_N() { return null; }", "<test>")
            .unwrap();
        let v = h
            .call_instances("DM_Instances_N", &[], BridgeDb::None)
            .unwrap();
        assert_eq!(v, None);
    }

    #[test]
    fn test_init_called_once_by_caller() {
        // The handler module itself does not track "first time"; it just invokes.
        let h = JsHandlers::new().unwrap();
        h.eval_source(
            "let counter = 0; function DM_Init() { counter++; } function DM_Getter_C() { return counter; }",
            "<test>",
        )
        .unwrap();
        h.call_init_if_present(BridgeDb::None).unwrap();
        let v = h.call_getter("DM_Getter_C", &[], BridgeDb::None).unwrap();
        assert_eq!(v, Some("1".to_string()));
    }

    #[test]
    fn test_dm_set_without_bridge_throws() {
        let h = JsHandlers::new().unwrap();
        h.eval_source(
            "function DM_Getter_Trap() { DM.set('x', 'y'); return 'ok'; }",
            "<test>",
        )
        .unwrap();
        let err = h
            .call_getter("DM_Getter_Trap", &[], BridgeDb::None)
            .unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("write session"),
            "unexpected error: {msg}"
        );
    }
}
