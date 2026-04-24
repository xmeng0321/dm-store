//! Exercise the `default/bridge/` sub-folder shipped at the workspace root.

use std::path::PathBuf;

use dm_manager_lib::DmManager;

fn default_dir() -> PathBuf {
    // CARGO_MANIFEST_DIR is .../dm-store/dm-manager-lib; the workspace root
    // holds `default/`.
    let mut p = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    p.pop();
    p.push("default");
    p
}

#[test]
fn loads_bridge_subfolder_and_dispatches_handlers() {
    let mut mgr = DmManager::new_in_memory().unwrap();
    mgr.load_default_folder(default_dir()).unwrap();

    // Schema from VLANBridge.json is present.
    assert!(mgr
        .param_schema("Device.Bridging.Bridge.{i}.Enable")
        .is_some());
    assert!(mgr
        .param_schema("Device.Bridging.Bridge.{i}.Port.{i}.Stats.BytesSent")
        .is_some());

    // DM_Init seeded 2 instances of each writable multi-instance table.
    assert_eq!(
        mgr.store().instances("Device.Bridging.Bridge.").unwrap(),
        vec![1, 2]
    );
    assert_eq!(
        mgr.store()
            .instances("Device.Bridging.Bridge.1.Port.")
            .unwrap(),
        vec![1, 2]
    );
    assert_eq!(
        mgr.store()
            .instances("Device.Bridging.Bridge.1.VLAN.")
            .unwrap(),
        vec![1, 2]
    );
    assert_eq!(
        mgr.store()
            .instances("Device.Bridging.Bridge.1.VLANPort.")
            .unwrap(),
        vec![1, 2]
    );
    assert_eq!(
        mgr.store()
            .instances("Device.Bridging.Bridge.2.Port.")
            .unwrap(),
        vec![1, 2]
    );
    assert_eq!(
        mgr.store()
            .instances("Device.Bridging.ProviderBridge.")
            .unwrap(),
        vec![1, 2]
    );

    // BridgeNumberOfEntries handler returns 0 (stub).
    let p = mgr.get("Device.Bridging.BridgeNumberOfEntries").unwrap();
    assert_eq!(p.value, Some("0".to_string()));

    // Synthesised names use the instance numbers.
    let name = mgr.get("Device.Bridging.Bridge.1.Name").unwrap();
    assert_eq!(name.value, Some("br1".to_string()));
    let port_name = mgr.get("Device.Bridging.Bridge.1.Port.1.Name").unwrap();
    assert_eq!(port_name.value, Some("port1".to_string()));

    // Status handler returns sensible default.
    let st = mgr.get("Device.Bridging.Bridge.1.Port.1.Status").unwrap();
    assert_eq!(st.value, Some("Down".to_string()));

    // Stats counter default.
    let bs = mgr
        .get("Device.Bridging.Bridge.1.Port.1.Stats.BytesSent")
        .unwrap();
    assert_eq!(bs.value, Some("0".to_string()));

    // DM_Init uses DM.update to seed writable defaults as well.
    assert_eq!(
        mgr.get("Device.Bridging.Bridge.1.Enable").unwrap().value,
        Some("1".to_string())
    );
    assert_eq!(
        mgr.get("Device.Bridging.Bridge.1.Alias").unwrap().value,
        Some("br1".to_string())
    );
    assert_eq!(
        mgr.get("Device.Bridging.Bridge.1.STP.Protocol").unwrap().value,
        Some("RSTP".to_string())
    );
    assert_eq!(
        mgr.get("Device.Bridging.Bridge.1.VLAN.1.VLANID")
            .unwrap()
            .value,
        Some("100".to_string())
    );
    assert_eq!(
        mgr.get("Device.Bridging.Bridge.2.Enable").unwrap().value,
        Some("0".to_string())
    );
    assert_eq!(
        mgr.get("Device.Bridging.ProviderBridge.1.Type")
            .unwrap()
            .value,
        Some("S-VLAN".to_string())
    );
}
