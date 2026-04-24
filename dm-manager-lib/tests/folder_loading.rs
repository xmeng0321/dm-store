//! Integration tests for loading a default folder of schemas + handlers.

use std::fs;
use std::path::PathBuf;

use dm_manager_lib::DmManager;

/// Build a throwaway default folder tree. Returns the root path; the process
/// tempdir base is `std::env::temp_dir()` + a unique suffix based on the test
/// name so parallel tests don't collide.
fn make_default_folder(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("dm-manager-folder-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    root
}

#[test]
fn loads_schemas_from_each_subfolder() {
    let root = make_default_folder("schemas");

    // Sub-folder A: just a schema.
    let a = root.join("device_info");
    fs::create_dir_all(&a).unwrap();
    fs::write(
        a.join("schema.json"),
        r#"[
            {
                "object": "Device.DeviceInfo.",
                "access": "readOnly",
                "parameters": [
                    {"name": "SoftwareVersion", "access": "readOnly", "dataType": "string"}
                ]
            }
        ]"#,
    )
    .unwrap();

    // Sub-folder B: schema + handler.
    let b = root.join("bridging");
    fs::create_dir_all(b.join("handlers")).unwrap();
    fs::write(
        b.join("schema.json"),
        r#"[
            {
                "object": "Device.Bridging.Bridge.{i}.",
                "access": "readWrite",
                "parameters": [
                    {"name": "Enable", "access": "readWrite", "dataType": "boolean"}
                ]
            }
        ]"#,
    )
    .unwrap();
    fs::write(
        b.join("handlers/bridge.js"),
        "function DM_Instances_Device_Bridging_Bridge() { return [1, 2]; }\n",
    )
    .unwrap();

    let mut mgr = DmManager::new_in_memory().unwrap();
    mgr.load_default_folder(&root).unwrap();

    // Schemas from both sub-folders are loaded.
    assert!(mgr
        .param_schema("Device.DeviceInfo.SoftwareVersion")
        .is_some());
    assert!(mgr
        .param_schema("Device.Bridging.Bridge.{i}.Enable")
        .is_some());

    // JS handler is live.
    let nums = mgr.instances("Device.Bridging.Bridge.").unwrap();
    assert_eq!(nums, vec![1, 2]);

    fs::remove_dir_all(&root).ok();
}

#[test]
fn dm_init_called_once_per_subfolder() {
    let root = make_default_folder("init");
    let sub = root.join("sub");
    fs::create_dir_all(sub.join("handlers")).unwrap();
    fs::write(
        sub.join("schema.json"),
        r#"[
            {
                "object": "Device.DeviceInfo.",
                "access": "readOnly",
                "parameters": [
                    {"name": "SoftwareVersion", "access": "readOnly", "dataType": "string"}
                ]
            }
        ]"#,
    )
    .unwrap();
    fs::write(
        sub.join("handlers/init.js"),
        "let init_count = 0;\n\
         function DM_Init() { init_count += 1; }\n\
         function DM_Getter_Device_DeviceInfo_SoftwareVersion() { return String(init_count); }\n",
    )
    .unwrap();

    let mut mgr = DmManager::new_in_memory().unwrap();

    // First load: DM_Init runs, init_count becomes 1.
    mgr.load_default_folder(&root).unwrap();
    let p = mgr.get("Device.DeviceInfo.SoftwareVersion").unwrap();
    assert_eq!(p.value, Some("1".to_string()));

    // Second load of the same folder: DM_Init must NOT run again.
    mgr.load_default_folder(&root).unwrap();
    let p = mgr.get("Device.DeviceInfo.SoftwareVersion").unwrap();
    assert_eq!(p.value, Some("1".to_string()));

    fs::remove_dir_all(&root).ok();
}

#[test]
fn missing_handlers_dir_is_fine() {
    let root = make_default_folder("no_handlers");
    let sub = root.join("plain");
    fs::create_dir_all(&sub).unwrap();
    fs::write(
        sub.join("schema.json"),
        r#"[
            {
                "object": "Device.DeviceInfo.",
                "access": "readOnly",
                "parameters": [
                    {"name": "SoftwareVersion", "access": "readOnly", "dataType": "string"}
                ]
            }
        ]"#,
    )
    .unwrap();

    let mut mgr = DmManager::new_in_memory().unwrap();
    mgr.load_default_folder(&root).unwrap();
    // No JS context created when no handlers present.
    assert!(mgr.js().is_none());

    fs::remove_dir_all(&root).ok();
}
