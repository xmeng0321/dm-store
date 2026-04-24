// Default DM_* handlers for Device.Bridging.* from VLANBridge.json.
//
// These are stub implementations that return sensible defaults for read-only
// parameters that have neither a `const` nor a `default` value in the schema.
// In a real deployment these would be replaced with handlers that query the
// underlying system (bridges, ports, stats).

function DM_Init() {
    // Seed 2 bridges (each with 2 ports / VLANs / VLANPorts) and 2 provider
    // bridges, filling writable defaults for each so reads and dumps see
    // something useful out of the box.
    DM.update('Device.Bridging.', {
        Bridge: [
            {
                Enable: 1,
                Alias: 'br1',
                Standard: '802.1Q-2011',
                STP: { Enable: 1, Protocol: 'RSTP' },
                Port: [
                    { Enable: 1, Alias: 'port1', PVID: 1, Type: 'CustomerVLANPort' },
                    { Enable: 1, Alias: 'port2', PVID: 1, Type: 'CustomerVLANPort' }
                ],
                VLAN: [
                    { Enable: 1, Alias: 'vlan1', Name: 'vlan-100', VLANID: 100 },
                    { Enable: 1, Alias: 'vlan2', Name: 'vlan-200', VLANID: 200 }
                ],
                VLANPort: [
                    { Enable: 1, Alias: 'vp1', Untagged: 0 },
                    { Enable: 1, Alias: 'vp2', Untagged: 1 }
                ]
            },
            {
                Enable: 0,
                Alias: 'br2',
                Standard: '802.1Q-2011',
                STP: { Enable: 0, Protocol: 'STP' },
                Port: [
                    { Enable: 0, Alias: 'port1', PVID: 1, Type: 'CustomerVLANPort' },
                    { Enable: 0, Alias: 'port2', PVID: 1, Type: 'CustomerVLANPort' }
                ],
                VLAN: [
                    { Enable: 0, Alias: 'vlan1', Name: 'vlan-10', VLANID: 10 },
                    { Enable: 0, Alias: 'vlan2', Name: 'vlan-20', VLANID: 20 }
                ],
                VLANPort: [
                    { Enable: 0, Alias: 'vp1', Untagged: 1 },
                    { Enable: 0, Alias: 'vp2', Untagged: 0 }
                ]
            }
        ],
        ProviderBridge: [
            { Enable: 1, Alias: 'pb1', Type: 'S-VLAN' },
            { Enable: 0, Alias: 'pb2', Type: 'PE' }
        ]
    });
}

// --- Device.Bridging counts -------------------------------------------------

function DM_Getter_Device_Bridging_BridgeNumberOfEntries() {
    return 0;
}

function DM_Getter_Device_Bridging_Bridge_i_PortNumberOfEntries(ins) {
    return 0;
}

function DM_Getter_Device_Bridging_Bridge_i_VLANNumberOfEntries(ins) {
    return 0;
}

function DM_Getter_Device_Bridging_Bridge_i_VLANPortNumberOfEntries(ins) {
    return 0;
}

// --- Status ----------------------------------------------------------------

function DM_Getter_Device_Bridging_Bridge_i_Status(ins) {
    return 'Disabled';
}

function DM_Getter_Device_Bridging_Bridge_i_STP_Status(ins) {
    return 'Disabled';
}

function DM_Getter_Device_Bridging_Bridge_i_Port_i_Status(ins) {
    return 'Down';
}

function DM_Getter_Device_Bridging_ProviderBridge_i_Status(ins) {
    return 'Disabled';
}

// --- Read-only identity strings --------------------------------------------

function DM_Getter_Device_Bridging_Bridge_i_Name(ins) {
    return 'br' + ins[0];
}

function DM_Getter_Device_Bridging_Bridge_i_Port_i_Name(ins) {
    return 'port' + ins[1];
}

// --- Port stats (all default to 0) -----------------------------------------

function DM_Getter_Device_Bridging_Bridge_i_Port_i_Stats_BytesSent(ins)              { return 0; }
function DM_Getter_Device_Bridging_Bridge_i_Port_i_Stats_BytesReceived(ins)          { return 0; }
function DM_Getter_Device_Bridging_Bridge_i_Port_i_Stats_PacketsSent(ins)            { return 0; }
function DM_Getter_Device_Bridging_Bridge_i_Port_i_Stats_PacketsReceived(ins)        { return 0; }
function DM_Getter_Device_Bridging_Bridge_i_Port_i_Stats_ErrorsSent(ins)             { return 0; }
function DM_Getter_Device_Bridging_Bridge_i_Port_i_Stats_ErrorsReceived(ins)         { return 0; }
function DM_Getter_Device_Bridging_Bridge_i_Port_i_Stats_UnicastPacketsSent(ins)     { return 0; }
function DM_Getter_Device_Bridging_Bridge_i_Port_i_Stats_UnicastPacketsReceived(ins) { return 0; }
function DM_Getter_Device_Bridging_Bridge_i_Port_i_Stats_DiscardPacketsSent(ins)     { return 0; }
function DM_Getter_Device_Bridging_Bridge_i_Port_i_Stats_DiscardPacketsReceived(ins) { return 0; }
function DM_Getter_Device_Bridging_Bridge_i_Port_i_Stats_MulticastPacketsSent(ins)   { return 0; }
function DM_Getter_Device_Bridging_Bridge_i_Port_i_Stats_MulticastPacketsReceived(ins) { return 0; }
function DM_Getter_Device_Bridging_Bridge_i_Port_i_Stats_BroadcastPacketsSent(ins)   { return 0; }
function DM_Getter_Device_Bridging_Bridge_i_Port_i_Stats_BroadcastPacketsReceived(ins) { return 0; }
function DM_Getter_Device_Bridging_Bridge_i_Port_i_Stats_UnknownProtoPacketsReceived(ins) { return 0; }
