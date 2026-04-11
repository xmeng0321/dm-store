# dm-store

A high-performance TR-181 data model store backed by SQLite, written in Rust. Manages hierarchical data model parameters with hash-accelerated lookups, optional in-memory caching, and transactional session support.

## Overview

TR-181 is a Broadband Forum standard for device data models used in CPE (Customer Premises Equipment) management. It uses hierarchical dot-separated paths to organize device configuration and status data:

```
Device.WiFi.Radio.1.Enable = true
Device.WiFi.Radio.1.Channel = 6
Device.WiFi.SSID.1.SSID = "MyNetwork"
```

**dm-store** provides a fast, persistent storage engine for these data models with:

- **SQLite persistence** with WAL mode for concurrent performance
- **Hash-accelerated lookups** using FNV-1a 64-bit hashing on path columns
- **Optional in-memory HashMap cache** for O(1) exact-path reads (enabled by default)
- **Transactional sessions** using SQLite SAVEPOINTs with commit/abort semantics
- **Library API** (`dm-store-lib`) for programmatic integration
- **CLI tool** (`dm-store`) for interactive testing and administration

## Architecture

### Project Structure

```
dm-store/
  Cargo.toml               # Workspace root
  dm-store-lib/             # Library crate
    src/
      lib.rs                # Public API re-exports
      error.rs              # DmStoreError enum
      types.rs              # ParamType, Parameter, Object, AddResult, DmStoreConfig
      path.rs               # TR-181 path parsing, validation, FNV-1a hashing
      schema.rs             # SQLite schema initialization and PRAGMAs
      store.rs              # DmStore — main entry point, connection, cache management
      session.rs            # Session — transactional get/set/add/delete
  dm-store-cli/             # CLI binary crate
    src/
      main.rs               # clap-based CLI + interactive REPL shell
```

### SQLite Schema

Four tables cleanly separate **instance data** from **schema templates**:

**Data tables** — contain only concrete paths (e.g., `Device.WiFi.Radio.1.Enable`):

```
dm_object                          dm_param
+-----------+--------+             +-----------+-----------+
| path      | TEXT   | UNIQUE      | path      | TEXT      | UNIQUE
| path_hash | INT    | INDEXED     | path_hash | INT       | INDEXED
| parent_path| TEXT  | INDEXED     | object_path| TEXT     | INDEXED (FK)
| is_multi  | INT    |             | name      | TEXT      |
+-----------+--------+             | value     | TEXT      |
                                   | param_type| TEXT      |
                                   | writable  | INT       |
                                   +-----------+-----------+
```

**Schema tables** — contain template definitions with `{i}` placeholders:

```
dm_schema_object                   dm_schema_param
+-----------+--------+             +-----------+-----------+
| path      | TEXT   | UNIQUE      | path      | TEXT      | UNIQUE
| path_hash | INT    | INDEXED     | path_hash | INT       | INDEXED
| parent_path| TEXT  | INDEXED     | object_path| TEXT     |
| is_multi  | INT    |             | name      | TEXT      |
+-----------+--------+             | value     | TEXT      |
                                   | param_type| TEXT      |
                                   | writable  | INT       |
                                   +-----------+-----------+
```

This separation ensures data tables never contain `{i}` template paths, making queries simple and efficient. Template definitions in the schema tables are used by `add` to instantiate new objects with the correct structure and default values.

### Two-Tier Lookup Strategy

All lookups are exact-match (no wildcard/prefix queries), enabling two acceleration layers:

1. **SQLite hash index** — `path_hash INTEGER` column with a B-tree index. Queries use `WHERE path_hash = ? AND path = ?`. The integer index narrows to ~1 row; the path equality handles hash collisions.

2. **In-memory HashMap** (togglable, ON by default) — On startup, all parameters are loaded into a `HashMap<i64, Vec<Parameter>>` keyed by path hash. Exact reads are O(1) without hitting SQLite. The cache is updated on set/add/delete operations.

```
get("Device.WiFi.Radio.1.Enable")
  │
  ├─ Cache ON?  ──yes──> HashMap lookup (O(1))
  │
  └─ Cache OFF? ──────> SQLite: WHERE path_hash=? AND path=? (indexed)
```

## Getting Started

### Prerequisites

- Rust toolchain (install via [rustup](https://rustup.rs/))

### Build

```bash
cargo build --release
```

The binary is at `target/release/dm-store`.

### Quick Start

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

## Library API

### Opening a Store

```rust
use dm_store_lib::{DmStore, DmStoreConfig, ParamType};

// Open with default config (cache ON)
let mut store = DmStore::open("my-datamodel.db")?;

// Open with cache disabled
let config = DmStoreConfig { use_cache: false };
let mut store = DmStore::open_with_config("my-datamodel.db", config)?;

// In-memory database (for testing)
let mut store = DmStore::open_in_memory()?;
```

### Defining Objects and Parameters

Objects form the tree structure. Parameters hold values and belong to objects.

`define_object` and `define_parameter` automatically route paths to the correct tables:
- Paths containing `{i}` → stored in **schema tables** (`dm_schema_object` / `dm_schema_param`)
- Concrete paths → stored in **data tables** (`dm_object` / `dm_param`)

When a template is defined and instances already exist, the new definition is **automatically propagated** to all existing instances.

```rust
// Single-instance objects (→ dm_object)
store.define_object("Device.", false)?;
store.define_object("Device.WiFi.", false)?;

// Multi-instance object table (→ dm_object)
store.define_object("Device.WiFi.Radio.", true)?;

// Template object — defines the schema for instances (→ dm_schema_object)
store.define_object("Device.WiFi.Radio.{i}.", false)?;

// Template parameters — copied to new instances on add() (→ dm_schema_param)
store.define_parameter(
    "Device.WiFi.Radio.{i}.Enable",
    ParamType::Boolean,
    true,           // writable
    Some("true"),   // default value
)?;
store.define_parameter(
    "Device.WiFi.Radio.{i}.Channel",
    ParamType::UnsignedInt,
    true,
    Some("0"),
)?;

// Read-only parameter (→ dm_schema_param)
store.define_parameter(
    "Device.WiFi.Radio.{i}.Status",
    ParamType::String,
    false,          // read-only
    Some("Down"),
)?;
```

### Session-Based Operations

All mutating operations go through sessions for transactional safety:

```rust
// Get (no session needed for reads)
let param = store.get("Device.WiFi.Radio.1.Enable")?;
println!("{} = {:?}", param.path, param.value);

// Get all params of an object
let params = store.get_object("Device.WiFi.Radio.1.")?;

// Add a new instance
let mut session = store.session()?;
let result = session.add("Device.WiFi.Radio.")?;
println!("Created {}", result.path); // e.g., "Device.WiFi.Radio.1."
session.commit()?;

// Set a value
let mut session = store.session()?;
session.set("Device.WiFi.Radio.1.Enable", "false")?;
session.set("Device.WiFi.Radio.1.Channel", "6")?;
session.commit()?;

// Abort a session (rollback all changes)
let mut session = store.session()?;
session.set("Device.WiFi.Radio.1.Enable", "true")?;
session.abort()?;  // Changes are discarded

// Delete an instance
let mut session = store.session()?;
session.delete("Device.WiFi.Radio.2.")?;  // Cascades to child params
session.commit()?;
```

### Parameter Types

Supported TR-181 parameter types:

| Type | Rust Enum | Example Value |
|------|-----------|---------------|
| `string` | `ParamType::String` | `"hello"` |
| `int` | `ParamType::Int` | `"-42"` |
| `unsignedInt` | `ParamType::UnsignedInt` | `"42"` |
| `long` | `ParamType::Long` | `"-1000000"` |
| `unsignedLong` | `ParamType::UnsignedLong` | `"1000000"` |
| `boolean` | `ParamType::Boolean` | `"true"` / `"false"` |
| `dateTime` | `ParamType::DateTime` | `"2024-01-01T00:00:00Z"` |
| `hexBinary` | `ParamType::HexBinary` | `"0A1B2C"` |
| `base64` | `ParamType::Base64` | `"SGVsbG8="` |

All values are stored as text strings in the database, consistent with TR-181 transport semantics.

## CLI Reference

### Global Options

```
dm-store [OPTIONS] <COMMAND>

Options:
  -d, --db <DB>    Path to SQLite database file [default: dm-store.db]
  --no-cache       Disable in-memory cache
  -h, --help       Print help
```

### Commands

#### `get <path>` — Get a parameter by exact path

```bash
dm-store get "Device.WiFi.Radio.1.Enable"
# Output: Device.WiFi.Radio.1.Enable = true (boolean, writable)
```

#### `get-object <path>` — Get all parameters of an object

```bash
dm-store get-object "Device.WiFi.Radio.1."
# Output:
# Device.WiFi.Radio.1.Enable = true (boolean, writable)
# Device.WiFi.Radio.1.Channel = 0 (unsignedInt, writable)
```

#### `set <path> <value>` — Set a parameter value

```bash
dm-store set "Device.WiFi.Radio.1.Enable" "false"
# Output: OK
```

#### `add <path>` — Add a new instance to a multi-instance object

```bash
dm-store add "Device.WiFi.Radio."
# Output: Added instance 1 at Device.WiFi.Radio.1.
```

#### `del <path>` — Delete an instance

```bash
dm-store del "Device.WiFi.Radio.2."
# Output: Deleted Device.WiFi.Radio.2.
```

#### `define-object <path> [--multi]` — Define an object

```bash
dm-store define-object "Device.WiFi.Radio." --multi
# Output: Defined multi-instance object: Device.WiFi.Radio.
```

#### `define-param <path> [--type T] [--default V] [--readonly]` — Define a parameter

```bash
dm-store define-param "Device.WiFi.Radio.{i}.Enable" --type boolean --default true
# Output: Defined parameter: Device.WiFi.Radio.{i}.Enable (boolean)
```

#### `dump` — Dump all objects and parameters

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
#   [Object] Device.WiFi.Radio.{i}.
#   [Param]  Device.WiFi.Radio.{i}.Channel = 0 (unsignedInt, rw)
#   [Param]  Device.WiFi.Radio.{i}.Enable = true (boolean, rw)
```

#### `shell` — Start interactive REPL

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
| `define-object <path> [--multi]` | Define an object |
| `define-param <path> [options]` | Define a parameter |
| `dump` | Dump all data |
| `begin` | Start a session |
| `commit` | Commit current session |
| `abort` | Abort current session (rollback) |
| `help` | Show help |
| `quit` | Exit |

## Performance

### SQLite Optimizations

- **WAL mode** — Readers never block writers; writes append to WAL instead of rewriting the main database
- **`synchronous = NORMAL`** — Safe with WAL, avoids fsync on every commit
- **8 MB page cache** (`cache_size = -8000`) — Reduces disk I/O for large data models
- **Prepared statement caching** — `prepare_cached()` avoids re-parsing SQL for hot queries

### Hash-Accelerated Lookups

Path hashing uses FNV-1a 64-bit, a fast non-cryptographic hash with good distribution:

- **O(log n)** lookups through SQLite integer B-tree index on `path_hash`
- Integer comparisons in B-tree traversal are faster than variable-length string comparisons
- Collisions handled by secondary `path = ?` equality check

### In-Memory Cache

When enabled (default), all parameters are loaded into a `HashMap` at startup:

- **O(1) amortized** exact-path reads without hitting SQLite
- Cache updated synchronously on `set`, `add`, `delete` operations
- For 20,000 parameters: ~1-2 MB memory footprint
- Disable with `DmStoreConfig { use_cache: false }` or `--no-cache` CLI flag

### Schema/Data Separation

Template definitions (`{i}` paths) are stored in dedicated schema tables (`dm_schema_object`, `dm_schema_param`), completely separate from instance data (`dm_object`, `dm_param`). This architecture:

- Keeps data tables clean — no template rows to filter out on queries
- Makes `dump` output clear — data and schema are displayed in separate sections
- Supports **automatic propagation** — when a new template is defined, existing instances automatically receive the new parameter/object
- Enables `add` to look up templates from schema tables and instantiate concrete rows in data tables

## Data Model Concepts

### Objects vs Parameters

- **Objects** define the tree structure. Their paths end with `.` (e.g., `Device.WiFi.Radio.1.`)
- **Parameters** hold values and belong to objects. Their paths do not end with `.` (e.g., `Device.WiFi.Radio.1.Enable`)

### Multi-Instance Objects

Multi-instance objects (tables) can have numbered instances:

```
Schema tables (dm_schema_*):         Data tables (dm_object/dm_param):
  Device.WiFi.Radio.{i}.               Device.WiFi.Radio. [multi]
  Device.WiFi.Radio.{i}.Enable         Device.WiFi.Radio.1.
  Device.WiFi.Radio.{i}.Channel        Device.WiFi.Radio.1.Enable
                                        Device.WiFi.Radio.1.Channel
                                        Device.WiFi.Radio.2.
                                        Device.WiFi.Radio.2.Enable
                                        Device.WiFi.Radio.2.Channel
```

The `add` operation:
1. Computes the canonical template path (e.g., `Device.WiFi.Radio.{i}.`)
2. Finds the next available instance number from existing children
3. Reads template parameters and child objects from **schema tables**
4. Resolves `{i}` placeholders to the instance number
5. Inserts concrete rows into **data tables**
6. Recursively creates child objects and their parameters from child templates

The `delete` operation cascades: deleting an instance removes all its parameters and child objects via BFS traversal of the subtree.

### Path Conventions

| Pattern | Meaning | Example |
|---------|---------|---------|
| Ends with `.` | Object path | `Device.WiFi.Radio.1.` |
| No trailing `.` | Parameter path | `Device.WiFi.Radio.1.Enable` |
| Contains `{i}` | Template path | `Device.WiFi.Radio.{i}.Enable` |
| Trailing number + `.` | Instance path | `Device.WiFi.Radio.1.` |

## License

This project is provided as-is for internal use.
