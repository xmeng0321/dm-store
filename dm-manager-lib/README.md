# dm-manager-lib

A TR-181 data model manager library built on top of [dm-store-lib](../dm-store-lib). Loads data model schemas from JSON files, validates paths and values, and provides hooks for read-only parameters and dynamic instances.

## Overview

`dm-manager` adds a schema-aware management layer over the raw dm-store storage engine:

- **JSON schema loading** -- define your data model in JSON files and load them at startup
- **Path validation** -- every get/set is checked against the loaded schema
- **Value validation** -- type checking, range constraints, enum enforcement, string length limits
- **Read/write separation** -- only writable parameters are persisted in dm-store; read-only parameters are served via hooks or schema defaults
- **Hooks** -- register callbacks for read-only parameters and dynamic instance enumeration
- **Default callbacks** -- read-only parameters automatically return their schema default, const value, or empty string without requiring explicit hook registration

## Architecture

```
            +-------------------+
            |    Application    |
            +-------------------+
                     |
            +-------------------+
            |   dm-manager-lib  |   Schema + validation + hooks
            +-------------------+
                     |
            +-------------------+
            |   dm-store-lib    |   SQLite persistence + sessions
            +-------------------+
                     |
                  [SQLite]
```

**Schema flow:** JSON file -> parser -> in-memory `DmSchema` (HashMap by template path) + register writable items in dm-store.

**Get flow:**
1. Canonicalize path to template form (e.g., `Device.Bridging.Bridge.1.Enable` -> `Device.Bridging.Bridge.{i}.Enable`)
2. Look up `ParamSchema` -- if not found, return `NotInSchema` error
3. If param has `const` value -> return it directly
4. If param is writable -> query dm-store; fall back to schema default
5. If param is read-only -> call registered hook; fall back to schema default/empty

**Set flow:**
1. Validate path exists in schema
2. Reject if read-only
3. Validate value against type and constraints
4. Delegate to dm-store session

## JSON Schema Format

The JSON format is a flat array of object definitions, matching TR-181 conventions:

```json
[
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
                "name": "BridgePriority",
                "access": "readWrite",
                "dataType": "unsignedInt(0:61440)",
                "default": "32768"
            }
        ]
    }
]
```

### Supported Data Types

| JSON `dataType` | Mapped Type | Constraint |
|---|---|---|
| `string` | String | -- |
| `string(:64)` | String | max length 64 |
| `boolean` | Boolean | true/false/1/0 |
| `int` | Int | -- |
| `int(1:4094)` | Int | range 1..4094 |
| `unsignedInt` | UnsignedInt | -- |
| `unsignedInt(0:61440)` | UnsignedInt | range 0..61440 |
| `long` | Long | -- |
| `unsignedLong` | UnsignedLong | -- |
| `dateTime` | DateTime | -- |
| `hexBinary` | HexBinary | even length, hex chars |
| `base64` | Base64 | valid base64 chars |
| `enum` | String | values from `"enum"` array |
| `pathRef` / `pathRef[]` | String | -- |
| `StatsCounter32` | UnsignedInt | -- |
| `StatsCounter64` | UnsignedLong | -- |

List types (e.g., `unsignedInt(0:7)[]`) are recognized via the `[]` suffix.

### Parameter Fields

| Field | Required | Description |
|---|---|---|
| `name` | yes | Parameter name (leaf) |
| `access` | no | `"readWrite"` or `"readOnly"` (default: `"readOnly"`) |
| `dataType` | yes | Type with optional constraints |
| `default` | no | Default value |
| `const` | no | Immutable constant value |
| `enum` | no | Array of allowed string values (for `"dataType": "enum"`) |

Extra fields (e.g., `uci`, `db`, `flags`, `js-value`, `set_on_create`) are silently ignored.

## Library API

### Creating a Manager

```rust
use dm_manager_lib::DmManager;
use dm_store_lib::DmStore;

// With file-backed store
let store = DmStore::open("datamodel.db")?;
let mut mgr = DmManager::new(store);

// With in-memory store (for testing)
let mut mgr = DmManager::new_in_memory()?;
```

### Loading Schemas

```rust
// From a file
mgr.load_schema_file("VLANBridge.json")?;

// From a string (for testing or embedded schemas)
mgr.load_schema_str(r#"[{"object": "Device.", "access": "readOnly", "parameters": []}]"#)?;

// Multiple files can be loaded additively
mgr.load_schema_file("WiFi.json")?;
mgr.load_schema_file("Bridging.json")?;
```

### Querying the Schema

```rust
// Get schema info for a parameter
if let Some(ps) = mgr.param_schema("Device.Bridging.Bridge.{i}.Enable") {
    println!("Type: {}, Access: {}", ps.param_type, ps.access);
}

// Get schema info for an object
if let Some(os) = mgr.object_schema("Device.Bridging.Bridge.{i}.") {
    println!("UniqueKeys: {:?}", os.unique_keys);
    println!("Parameters: {:?}", os.param_names);
}

// List all schema paths
for path in mgr.schema_paths() {
    println!("{}", path);
}
```

### Reading Values

```rust
// Get a single parameter -- path is validated against schema
let param = mgr.get("Device.Bridging.Bridge.1.Enable")?;
println!("{} = {:?}", param.path, param.value);

// Get all parameters of an object
let params = mgr.get_object("Device.Bridging.Bridge.1.")?;
for p in &params {
    println!("{}", p);
}

// List instances
let nums = mgr.instances("Device.Bridging.Bridge.")?;
println!("Instances: {:?}", nums);
```

### Writing Values (Sessions)

```rust
// Add an instance
let mut session = mgr.session()?;
let result = session.add("Device.Bridging.Bridge.")?;
println!("Created {}", result.path); // "Device.Bridging.Bridge.1."
session.commit()?;

// Set a value -- validates path, access mode, and value constraints
let mut session = mgr.session()?;
session.set("Device.Bridging.Bridge.1.Enable", "true")?;
session.commit()?;

// Abort (rollback)
let mut session = mgr.session()?;
session.set("Device.Bridging.Bridge.1.Enable", "false")?;
session.abort()?; // changes discarded
```

### Hooks

```rust
// Read hook: override the value for a read-only parameter
mgr.register_read_hook(
    "Device.Bridging.Bridge.{i}.Status",
    |concrete_path| {
        // concrete_path is e.g. "Device.Bridging.Bridge.1.Status"
        Ok("Enabled".to_string())
    },
);

// Instance hook: return dynamic instance numbers
mgr.register_instance_hook(
    "Device.Bridging.Bridge.",
    |_table_path| Ok(vec![1, 2, 5]),
);

// When an instance hook is registered, add/delete are blocked
// for that object (instances are externally managed)
```

### Default Callbacks

Read-only parameters work without explicit hook registration:

| Scenario | Returned Value |
|---|---|
| `const` defined in schema | The const value |
| `default` defined in schema | The default value |
| Hook registered | Hook return value |
| None of the above | Empty string `""` |


## Source Layout

```
src/
  lib.rs          Public API re-exports
  error.rs        DmManagerError enum
  schema.rs       DmSchema, ObjectSchema, ParamSchema, Access, ValueConstraint
  parser.rs       JSON deserialization + data type parsing
  loader.rs       Load JSON -> DmSchema + register writable items in DmStore
  validate.rs     Value validation against type/constraints
  manager.rs      DmManager struct: get/set/instances, hooks, sessions
```
