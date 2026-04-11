# dm-store-cli

Command-line interface and interactive REPL for dm-store-lib. Provides direct access to the TR-181 data model storage engine for schema definition, data manipulation, and interactive testing.

## Quick Start

```bash
# Create a data model schema
dm-store define-object "Device."
dm-store define-object "Device.WiFi."
dm-store define-object "Device.WiFi.Radio." --multi
dm-store define-object "Device.WiFi.Radio.{i}."
dm-store define-param "Device.WiFi.Radio.{i}.Enable" --type boolean --default true
dm-store define-param "Device.WiFi.Radio.{i}.Channel" --type unsignedInt --default 0

# Add instances
dm-store add "Device.WiFi.Radio."    # Creates Device.WiFi.Radio.1.
dm-store add "Device.WiFi.Radio."    # Creates Device.WiFi.Radio.2.

# Read and write
dm-store get "Device.WiFi.Radio.1.Enable"
dm-store set "Device.WiFi.Radio.1.Enable" "false"
dm-store get-object "Device.WiFi.Radio.1."

# Delete an instance
dm-store del "Device.WiFi.Radio.2."

# Dump everything
dm-store dump
```

## Global Options

```
dm-store [OPTIONS] <COMMAND>

Options:
  -d, --db <DB>    Path to SQLite database file [default: dm-store.db]
  --no-cache       Disable in-memory cache
  -h, --help       Print help
```

## Commands

### `get <path>` -- Get a parameter by exact path

```bash
dm-store get "Device.WiFi.Radio.1.Enable"
# Output: Device.WiFi.Radio.1.Enable = true (boolean, writable)
```

### `get-object <path>` -- Get all parameters of an object

```bash
dm-store get-object "Device.WiFi.Radio.1."
# Output:
# Device.WiFi.Radio.1.Enable = true (boolean, writable)
# Device.WiFi.Radio.1.Channel = 0 (unsignedInt, writable)
```

### `set <path> <value>` -- Set a parameter value

```bash
dm-store set "Device.WiFi.Radio.1.Enable" "false"
# Output: OK
```

### `add <path>` -- Add a new instance to a multi-instance object

```bash
dm-store add "Device.WiFi.Radio."
# Output: Added instance 1 at Device.WiFi.Radio.1.
```

### `del <path>` -- Delete an instance

```bash
dm-store del "Device.WiFi.Radio.2."
# Output: Deleted Device.WiFi.Radio.2.
```

### `instances <path>` -- List instance numbers

```bash
dm-store instances "Device.WiFi.Radio."
```

### `define-object <path> [--multi]` -- Define an object

```bash
dm-store define-object "Device.WiFi.Radio." --multi
# Output: Defined multi-instance object: Device.WiFi.Radio.
```

### `define-param <path> [--type T] [--default V] [--readonly]` -- Define a parameter

```bash
dm-store define-param "Device.WiFi.Radio.{i}.Enable" --type boolean --default true
# Output: Defined parameter: Device.WiFi.Radio.{i}.Enable (boolean)
```

### `dump` -- Dump all objects and parameters

```bash
dm-store dump
# Output:
# === Objects ===
#   Device.
#   Device.WiFi.
#   Device.WiFi.Radio. [multi]
#   Device.WiFi.Radio.1.
#
# === Parameters ===
#   Device.WiFi.Radio.1.Channel = 0 (unsignedInt, rw)
#   Device.WiFi.Radio.1.Enable = false (boolean, rw)
#
# === Schema Templates ===
#   Device.WiFi.Radio.{i}.
#   Device.WiFi.Radio.{i}.Channel (unsignedInt, rw)
#   Device.WiFi.Radio.{i}.Enable (boolean, rw)
```

### `shell` -- Start interactive REPL

```bash
dm-store shell
```

## Interactive REPL

The REPL provides all operations with explicit session management:

```
dm> get Device.WiFi.Radio.1.Enable
Device.WiFi.Radio.1.Enable = true (boolean, writable)

dm> begin
Session started.
dm(session)> set Device.WiFi.Radio.1.Enable false
OK
dm(session)> set Device.WiFi.Radio.1.Channel 6
OK
dm(session)> commit
Session committed.

dm> begin
Session started.
dm(session)> set Device.WiFi.Radio.1.Enable true
OK
dm(session)> abort
Session aborted.

dm> get Device.WiFi.Radio.1.Enable
Device.WiFi.Radio.1.Enable = false (boolean, writable)
```

### REPL Commands

| Command | Description |
|---------|-------------|
| `get <path>` | Get parameter by exact path |
| `get-object <path>` | Get all parameters of an object |
| `set <path> <value>` | Set parameter value |
| `add <path>` | Add instance to multi-instance object |
| `del <path>` | Delete an instance |
| `instances <path>` | List instance numbers |
| `define-object <path> [--multi]` | Define an object |
| `define-param <path> [options]` | Define a parameter |
| `dump` | Dump all data |
| `begin` | Start a session |
| `commit` | Commit current session |
| `abort` | Abort current session (rollback) |
| `help` | Show help |
| `quit` | Exit |
