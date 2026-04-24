//! Integration tests for JS handler dispatch through DmManager.

use dm_manager_lib::{DmManager, DmManagerError};

fn setup_manager() -> DmManager {
    let mut mgr = DmManager::new_in_memory().unwrap();
    mgr.load_schema_str(
        r#"[
            {
                "object": "Device.DeviceInfo.",
                "access": "readOnly",
                "parameters": [
                    {
                        "name": "SoftwareVersion",
                        "access": "readOnly",
                        "dataType": "string"
                    },
                    {
                        "name": "ProvisioningCode",
                        "access": "readWrite",
                        "dataType": "string(:64)"
                    }
                ]
            },
            {
                "object": "Device.Bridging.Bridge.{i}.",
                "access": "readWrite",
                "parameters": [
                    {
                        "name": "Enable",
                        "access": "readWrite",
                        "dataType": "boolean"
                    },
                    {
                        "name": "Alias",
                        "access": "readWrite",
                        "dataType": "string(:64)"
                    }
                ]
            },
            {
                "object": "Device.Bridging.Bridge.{i}.Port.{i}.",
                "access": "readWrite",
                "parameters": [
                    {
                        "name": "Name",
                        "access": "readWrite",
                        "dataType": "string(:64)"
                    }
                ]
            }
        ]"#,
    )
    .unwrap();
    mgr
}

#[test]
fn js_getter_used_instead_of_default_callback() {
    let mut mgr = setup_manager();
    mgr.ensure_js().unwrap();
    mgr.js_mut()
        .unwrap()
        .eval_source(
            "function DM_Getter_Device_DeviceInfo_SoftwareVersion() { return '1.2.3'; }",
            "<test>",
        )
        .unwrap();

    let p = mgr.get("Device.DeviceInfo.SoftwareVersion").unwrap();
    assert_eq!(p.value, Some("1.2.3".to_string()));
}

#[test]
fn js_getter_receives_instance_numbers() {
    let mut mgr = setup_manager();
    // Create instance 7 in the store.
    {
        let mut raw = mgr.store_mut().session().unwrap();
        for _ in 0..7 {
            raw.add("Device.Bridging.Bridge.").unwrap();
        }
        raw.commit().unwrap();
    }

    mgr.ensure_js().unwrap();
    mgr.js_mut()
        .unwrap()
        .eval_source(
            "function DM_Getter_Device_Bridging_Bridge_i_Enable(ins) {\
                return ins[0] === 7 ? 'true' : 'false';\
            }",
            "<test>",
        )
        .unwrap();

    let p = mgr.get("Device.Bridging.Bridge.7.Enable").unwrap();
    assert_eq!(p.value, Some("true".to_string()));
}

#[test]
fn js_getter_returning_undefined_is_error() {
    let mut mgr = setup_manager();
    mgr.ensure_js().unwrap();
    mgr.js_mut()
        .unwrap()
        .eval_source(
            "function DM_Getter_Device_DeviceInfo_SoftwareVersion() { return undefined; }",
            "<test>",
        )
        .unwrap();

    let err = mgr.get("Device.DeviceInfo.SoftwareVersion").unwrap_err();
    assert!(matches!(err, DmManagerError::HookError { .. }));
}

#[test]
fn js_setter_replaces_db_write() {
    let mut mgr = setup_manager();
    mgr.ensure_js().unwrap();
    mgr.js_mut()
        .unwrap()
        .eval_source(
            "let last_value = null;\
             function DM_Setter_Device_DeviceInfo_ProvisioningCode(ins, v) {\
                last_value = v;\
                return true;\
             }\
             function DM_Getter_Device_DeviceInfo_ProvisioningCode() { return last_value; }",
            "<test>",
        )
        .unwrap();

    // The setter is not required to write to the DB; a read should go through
    // the getter (which in turn uses the JS closure variable).
    let mut session = mgr.session().unwrap();
    session
        .set("Device.DeviceInfo.ProvisioningCode", "abc123")
        .unwrap();
    session.commit().unwrap();

    let p = mgr.get("Device.DeviceInfo.ProvisioningCode").unwrap();
    assert_eq!(p.value, Some("abc123".to_string()));
}

#[test]
fn js_setter_returning_false_produces_error() {
    let mut mgr = setup_manager();
    mgr.ensure_js().unwrap();
    mgr.js_mut()
        .unwrap()
        .eval_source(
            "function DM_Setter_Device_DeviceInfo_ProvisioningCode() { return false; }",
            "<test>",
        )
        .unwrap();

    let mut session = mgr.session().unwrap();
    let err = session
        .set("Device.DeviceInfo.ProvisioningCode", "xyz")
        .unwrap_err();
    assert!(matches!(err, DmManagerError::HookError { .. }));
}

#[test]
fn js_instances_overrides_store() {
    let mut mgr = setup_manager();
    mgr.ensure_js().unwrap();
    mgr.js_mut()
        .unwrap()
        .eval_source(
            "function DM_Instances_Device_Bridging_Bridge() { return [10, 20, 30]; }",
            "<test>",
        )
        .unwrap();

    let nums = mgr.instances("Device.Bridging.Bridge.").unwrap();
    assert_eq!(nums, vec![10, 20, 30]);
}

#[test]
fn js_instances_make_get_resolve_without_db_rows() {
    // DM_Instances alone (no DB-seeded rows) must let readOnly getters on
    // concrete instance paths resolve through the availability check.
    let mut mgr = setup_manager();
    mgr.ensure_js().unwrap();
    mgr.js_mut()
        .unwrap()
        .eval_source(
            "function DM_Instances_Device_Bridging_Bridge() { return [1, 2]; }\
             function DM_Instances_Device_Bridging_Bridge_i_Port(parents) {\
                return parents[0] === 1 ? [1] : [];\
             }\
             function DM_Getter_Device_Bridging_Bridge_i_Port_i_Name(ins) {\
                return 'p' + ins[0] + '_' + ins[1];\
             }",
            "<test>",
        )
        .unwrap();

    // Nested instance exposed purely via JS resolves.
    let p = mgr.get("Device.Bridging.Bridge.1.Port.1.Name").unwrap();
    assert_eq!(p.value, Some("p1_1".to_string()));

    // Instance not in the JS enumeration should still be NotInDb.
    let err = mgr.get("Device.Bridging.Bridge.1.Port.9.Name").unwrap_err();
    assert!(matches!(err, DmManagerError::NotInDb(_)));

    // Parent instance not in JS enumeration: NotInDb as well.
    let err = mgr.get("Device.Bridging.Bridge.7.Port.1.Name").unwrap_err();
    assert!(matches!(err, DmManagerError::NotInDb(_)));
}

#[test]
fn js_instances_blocks_add() {
    let mut mgr = setup_manager();
    mgr.ensure_js().unwrap();
    mgr.js_mut()
        .unwrap()
        .eval_source(
            "function DM_Instances_Device_Bridging_Bridge() { return [1]; }",
            "<test>",
        )
        .unwrap();

    let mut session = mgr.session().unwrap();
    let err = session.add("Device.Bridging.Bridge.").unwrap_err();
    assert!(matches!(err, DmManagerError::Schema(_)));
}

// --- DM.update -------------------------------------------------------------

/// Run a JS snippet inside a write-session setter so `DM.update` has access
/// to the bridge session. Returns after commit.
fn run_js_with_write_session(mgr: &mut DmManager, body: &str) {
    mgr.ensure_js().unwrap();
    let src = format!(
        "function DM_Setter_Device_DeviceInfo_ProvisioningCode(_, v) {{ {body}; return true; }}",
    );
    mgr.js_mut().unwrap().eval_source(&src, "<test>").unwrap();

    let mut session = mgr.session().unwrap();
    session
        .set("Device.DeviceInfo.ProvisioningCode", "go")
        .unwrap();
    session.commit().unwrap();
}

#[test]
fn dm_update_array_adds_instances_with_params() {
    let mut mgr = setup_manager();
    run_js_with_write_session(
        &mut mgr,
        "DM.update('Device.Bridging.', {\
            Bridge: [\
                { Enable: 1, Alias: 'br1' },\
                { Enable: 0, Alias: 'br2' }\
            ]\
        })",
    );

    let nums = mgr.instances("Device.Bridging.Bridge.").unwrap();
    assert_eq!(nums, vec![1, 2]);

    let p1 = mgr.get("Device.Bridging.Bridge.1.Enable").unwrap();
    assert_eq!(p1.value, Some("1".to_string()));
    let a1 = mgr.get("Device.Bridging.Bridge.1.Alias").unwrap();
    assert_eq!(a1.value, Some("br1".to_string()));

    let p2 = mgr.get("Device.Bridging.Bridge.2.Enable").unwrap();
    assert_eq!(p2.value, Some("0".to_string()));
    let a2 = mgr.get("Device.Bridging.Bridge.2.Alias").unwrap();
    assert_eq!(a2.value, Some("br2".to_string()));
}

#[test]
fn dm_update_instance_path_writes_only_params() {
    let mut mgr = setup_manager();
    {
        let mut session = mgr.session().unwrap();
        session.add("Device.Bridging.Bridge.").unwrap();
        session.commit().unwrap();
    }

    run_js_with_write_session(
        &mut mgr,
        "DM.update('Device.Bridging.Bridge.1.', { Enable: 1, Alias: 'br1' })",
    );

    // No new instance was added.
    assert_eq!(
        mgr.instances("Device.Bridging.Bridge.").unwrap(),
        vec![1]
    );
    let p = mgr.get("Device.Bridging.Bridge.1.Enable").unwrap();
    assert_eq!(p.value, Some("1".to_string()));
    let a = mgr.get("Device.Bridging.Bridge.1.Alias").unwrap();
    assert_eq!(a.value, Some("br1".to_string()));
}

#[test]
fn dm_update_recurses_through_nested_tables() {
    let mut mgr = setup_manager();

    run_js_with_write_session(
        &mut mgr,
        "DM.update('Device.Bridging.', {\
            Bridge: [\
                { Alias: 'br1', Port: [ { Name: 'eth0' }, { Name: 'eth1' } ] }\
            ]\
        })",
    );

    assert_eq!(
        mgr.instances("Device.Bridging.Bridge.").unwrap(),
        vec![1]
    );
    assert_eq!(
        mgr.instances("Device.Bridging.Bridge.1.Port.").unwrap(),
        vec![1, 2]
    );
    let n1 = mgr.get("Device.Bridging.Bridge.1.Port.1.Name").unwrap();
    assert_eq!(n1.value, Some("eth0".to_string()));
    let n2 = mgr.get("Device.Bridging.Bridge.1.Port.2.Name").unwrap();
    assert_eq!(n2.value, Some("eth1".to_string()));
}

#[test]
fn dm_update_trailing_dot_optional() {
    let mut mgr = setup_manager();
    run_js_with_write_session(
        &mut mgr,
        "DM.update('Device.Bridging', { Bridge: [ { Alias: 'x' } ] })",
    );
    assert_eq!(
        mgr.instances("Device.Bridging.Bridge.").unwrap(),
        vec![1]
    );
    let a = mgr.get("Device.Bridging.Bridge.1.Alias").unwrap();
    assert_eq!(a.value, Some("x".to_string()));
}

#[test]
fn dm_update_coerces_primitive_types() {
    let mut mgr = setup_manager();
    {
        let mut session = mgr.session().unwrap();
        session.add("Device.Bridging.Bridge.").unwrap();
        session.commit().unwrap();
    }

    // true -> "true", 0 -> "0", string passthrough
    run_js_with_write_session(
        &mut mgr,
        "DM.update('Device.Bridging.Bridge.1.', { Enable: true, Alias: 'abc' })",
    );
    assert_eq!(
        mgr.get("Device.Bridging.Bridge.1.Enable").unwrap().value,
        Some("true".to_string())
    );

    run_js_with_write_session(
        &mut mgr,
        "DM.update('Device.Bridging.Bridge.1.', { Enable: false })",
    );
    assert_eq!(
        mgr.get("Device.Bridging.Bridge.1.Enable").unwrap().value,
        Some("false".to_string())
    );
}

#[test]
fn dm_update_null_value_throws() {
    let mut mgr = setup_manager();
    mgr.ensure_js().unwrap();
    mgr.js_mut()
        .unwrap()
        .eval_source(
            "function DM_Setter_Device_DeviceInfo_ProvisioningCode(_, v) {\
                DM.update('Device.Bridging.Bridge.1.', { Enable: null });\
                return true;\
            }",
            "<test>",
        )
        .unwrap();

    let mut session = mgr.session().unwrap();
    let err = session
        .set("Device.DeviceInfo.ProvisioningCode", "go")
        .unwrap_err();
    assert!(matches!(err, DmManagerError::HookError { .. }));
}

#[test]
fn dm_update_without_write_session_throws() {
    let mut mgr = setup_manager();
    mgr.ensure_js().unwrap();
    // Call DM.update from a getter -> only a read-only Store bridge is installed.
    mgr.js_mut()
        .unwrap()
        .eval_source(
            "function DM_Getter_Device_DeviceInfo_SoftwareVersion() {\
                DM.update('Device.Bridging.', { Bridge: [ { Alias: 'x' } ] });\
                return 'v';\
            }",
            "<test>",
        )
        .unwrap();

    let err = mgr.get("Device.DeviceInfo.SoftwareVersion").unwrap_err();
    assert!(matches!(err, DmManagerError::HookError { .. }));
}
