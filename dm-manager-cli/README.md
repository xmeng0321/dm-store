# dm-manager-cli

Command-line interface and interactive REPL for dm-manager-lib. Provides schema-aware access to the TR-181 data model with path/value validation and tab completion.

## Global Options

```
dm-manager [OPTIONS] <COMMAND>

Options:
  -d, --db <DB>          Path to SQLite database file [default: dm-store.db]
  -s, --schema <FILE>    JSON schema files to load (repeatable)
  --no-cache             Disable in-memory cache
  -h, --help             Print help
```

## Commands

```bash
# Load a schema and query it
dm-manager -s VLANBridge.json list-schema
dm-manager -s VLANBridge.json schema "Device.Bridging.Bridge.{i}."

# Add instances and set values
dm-manager -s VLANBridge.json add "Device.Bridging.Bridge."
dm-manager -s VLANBridge.json set "Device.Bridging.Bridge.1.Enable" "true"
dm-manager -s VLANBridge.json get "Device.Bridging.Bridge.1.Enable"

# Get all parameters of an object
dm-manager -s VLANBridge.json get-object "Device.Bridging.Bridge.1."

# List instances
dm-manager -s VLANBridge.json instances "Device.Bridging.Bridge."

# Dump everything (schema + stored data)
dm-manager -s VLANBridge.json dump

# Delete an instance
dm-manager -s VLANBridge.json del "Device.Bridging.Bridge.1."
```

## Interactive REPL

```bash
dm-manager -s VLANBridge.json shell
```

The REPL provides tab completion for commands and schema paths. Concrete paths (with instance numbers) and template paths (with `{i}`) are both completed based on context.

```
dm-mgr> get Device.Bridging.MaxBridgeEntries
Device.Bridging.MaxBridgeEntries = 20 (unsignedInt, read-only)

dm-mgr> add Device.Bridging.Bridge.
Added instance 1 at Device.Bridging.Bridge.1.

dm-mgr> set Device.Bridging.Bridge.1.Enable true
OK

dm-mgr> get-object Device.Bridging.Bridge.1.
Device.Bridging.Bridge.1.Enable = true (boolean, writable)
Device.Bridging.Bridge.1.Name =  (string, read-only)
Device.Bridging.Bridge.1.Alias = (empty) (string, writable)
Device.Bridging.Bridge.1.Status = Disabled (string, read-only)
Device.Bridging.Bridge.1.Standard = 802.1Q-2011 (string, writable)
...

dm-mgr> begin
Session started.
dm-mgr(session)> set Device.Bridging.Bridge.1.Enable false
OK
dm-mgr(session)> abort
Session aborted.
```

### REPL Commands

| Command | Description |
|---------|-------------|
| `load <file>` | Load a JSON schema file |
| `get <path>` | Get parameter value (or object if path ends with `.`) |
| `set <path> <value>` | Set parameter value |
| `add <path>` | Add instance to multi-instance object |
| `del <path>` | Delete an instance |
| `instances <path>` | List instance numbers |
| `schema <path>` | Show schema info for a path |
| `list-schema` | List all schema paths |
| `dump` | Dump all data |
| `begin` | Start a session |
| `commit` | Commit current session |
| `abort` | Abort current session |
| `help` | Show help |
| `quit` | Exit |

## Validation

The `set` command validates both path and value against the loaded schema:

```
dm-mgr> set Device.Bridging.Bridge.1.Enable invalid
Error: invalid value for Device.Bridging.Bridge.{i}.Enable: expected boolean (true/false/1/0), got 'invalid'

dm-mgr> set Device.Bridging.Bridge.1.Status Enabled
Error: parameter is read-only: Device.Bridging.Bridge.1.Status

dm-mgr> set Device.Bridging.Bridge.1.Standard BadValue
Error: invalid value for Device.Bridging.Bridge.{i}.Standard: value 'BadValue' not in allowed values: ["802.1D-2004", "802.1Q-2005", "802.1Q-2011"]

dm-mgr> get Device.NonExistent.Param
Error: path not found in schema: Device.NonExistent.Param
```
