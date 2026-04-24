// Static handlers for Device.Hosts.* from Hosts.json.
//
// The entire Hosts tree is readOnly / externally-managed, so nothing is
// persisted to the store. Instance enumeration is served by DM_Instances_*
// handlers and every leaf value by a DM_Getter_*, both backed by the
// canned HOSTS table below.

const HOSTS = [
    {
        PhysAddress: 'AA:BB:CC:00:00:01',
        IPAddress: '192.168.1.100',
        HostName: 'laptop-xmeng',
        Active: true,
        InterfaceType: 'Wi-Fi',
        Layer1Interface: 'Device.WiFi.SSID.1.',
        Layer3Interface: 'Device.IP.Interface.1.',
        AssociatedDevice: 'Device.WiFi.AccessPoint.1.AssociatedDevice.1.',
        DHCPClient: 'Device.DHCPv4.Client.1.',
        IPv4Address: ['192.168.1.100'],
        IPv6Address: ['fe80::aabb:ccff:fe00:0001']
    },
    {
        PhysAddress: 'AA:BB:CC:00:00:02',
        IPAddress: '192.168.1.101',
        HostName: 'desktop-lab',
        Active: true,
        InterfaceType: 'Ethernet',
        Layer1Interface: 'Device.Ethernet.Interface.1.',
        Layer3Interface: 'Device.IP.Interface.1.',
        AssociatedDevice: '',
        DHCPClient: 'Device.DHCPv4.Client.2.',
        IPv4Address: ['192.168.1.101'],
        IPv6Address: []
    },
    {
        PhysAddress: 'AA:BB:CC:00:00:03',
        IPAddress: '192.168.1.102',
        HostName: 'phone-guest',
        Active: false,
        InterfaceType: 'Wi-Fi',
        Layer1Interface: 'Device.WiFi.SSID.2.',
        Layer3Interface: 'Device.IP.Interface.1.',
        AssociatedDevice: 'Device.WiFi.AccessPoint.2.AssociatedDevice.1.',
        DHCPClient: 'Device.DHCPv4.Client.3.',
        IPv4Address: ['192.168.1.102'],
        IPv6Address: ['fe80::aabb:ccff:fe00:0003', '2001:db8::3']
    }
];

function hostAt(i) {
    return HOSTS[i - 1];
}

// --- Instance enumeration --------------------------------------------------

function DM_Instances_Device_Hosts_Host() {
    return HOSTS.map(function (_, i) { return i + 1; });
}

function DM_Instances_Device_Hosts_Host_i_IPv4Address(parents) {
    return hostAt(parents[0]).IPv4Address.map(function (_, i) { return i + 1; });
}

function DM_Instances_Device_Hosts_Host_i_IPv6Address(parents) {
    return hostAt(parents[0]).IPv6Address.map(function (_, i) { return i + 1; });
}

// --- Device.Hosts counts ----------------------------------------------------

function DM_Getter_Device_Hosts_HostNumberOfEntries() {
    return HOSTS.length;
}

// --- Device.Hosts.Host.{i}. -------------------------------------------------

function DM_Getter_Device_Hosts_Host_i_PhysAddress(ins)      { return hostAt(ins[0]).PhysAddress; }
function DM_Getter_Device_Hosts_Host_i_IPAddress(ins)        { return hostAt(ins[0]).IPAddress; }
function DM_Getter_Device_Hosts_Host_i_DHCPClient(ins)       { return hostAt(ins[0]).DHCPClient; }
function DM_Getter_Device_Hosts_Host_i_AssociatedDevice(ins) { return hostAt(ins[0]).AssociatedDevice; }
function DM_Getter_Device_Hosts_Host_i_Layer1Interface(ins)  { return hostAt(ins[0]).Layer1Interface; }
function DM_Getter_Device_Hosts_Host_i_Layer3Interface(ins)  { return hostAt(ins[0]).Layer3Interface; }
function DM_Getter_Device_Hosts_Host_i_InterfaceType(ins)    { return hostAt(ins[0]).InterfaceType; }
function DM_Getter_Device_Hosts_Host_i_HostName(ins)         { return hostAt(ins[0]).HostName; }
function DM_Getter_Device_Hosts_Host_i_Active(ins)           { return hostAt(ins[0]).Active; }

function DM_Getter_Device_Hosts_Host_i_IPv4AddressNumberOfEntries(ins) {
    return hostAt(ins[0]).IPv4Address.length;
}

function DM_Getter_Device_Hosts_Host_i_IPv6AddressNumberOfEntries(ins) {
    return hostAt(ins[0]).IPv6Address.length;
}

// --- Nested IPv4/IPv6 address tables ---------------------------------------

function DM_Getter_Device_Hosts_Host_i_IPv4Address_i_IPAddress(ins) {
    return hostAt(ins[0]).IPv4Address[ins[1] - 1];
}

function DM_Getter_Device_Hosts_Host_i_IPv6Address_i_IPAddress(ins) {
    return hostAt(ins[0]).IPv6Address[ins[1] - 1];
}
