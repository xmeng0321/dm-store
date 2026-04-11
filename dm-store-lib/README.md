# dm-store-lib

A high-performance TR-181 data model storage engine backed by SQLite. Manages hierarchical data model parameters with hash-accelerated lookups, optional in-memory caching, and transactional session support.

## Features

- **SQLite persistence** with WAL mode for concurrent performance
- **Hash-accelerated lookups** using FNV-1a 64-bit hashing on path columns
- **Optional in-memory HashMap cache** for O(1) exact-path reads (enabled by default)
- **Transactional sessions** using SQLite SAVEPOINTs with commit/abort semantics
- **Schema/data separation** -- template definitions (`{i}` paths) stored in dedicated schema tables

## SQLite Schema

Four tables cleanly separate **instance data** from **schema templates**:

**Data tables** -- contain only concrete paths (e.g., `Device.WiFi.Radio.1.Enable`):

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

**Schema tables** -- contain template definitions with `{i}` placeholders:

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

## Two-Tier Lookup Strategy

All lookups are exact-match (no wildcard/prefix queries), enabling two acceleration layers:

1. **SQLite hash index** -- `path_hash INTEGER` column with a B-tree index. Queries use `WHERE path_hash = ? AND path = ?`. The integer index narrows to ~1 row; the path equality handles hash collisions.

2. **In-memory HashMap** (togglable, ON by default) -- On startup, all parameters are loaded into a `HashMap<i64, Vec<Parameter>>` keyed by path hash. Exact reads are O(1) without hitting SQLite. The cache is updated on set/add/delete operations.

```
get("Device.WiFi.Radio.1.Enable")
  |
  +-- Cache ON?  --yes--> HashMap lookup (O(1))
  |
  +-- Cache OFF? -------> SQLite: WHERE path_hash=? AND path=? (indexed)
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
- Paths containing `{i}` -> stored in **schema tables** (`dm_schema_object` / `dm_schema_param`)
- Concrete paths -> stored in **data tables** (`dm_object` / `dm_param`)

When a template is defined and instances already exist, the new definition is **automatically propagated** to all existing instances.

```rust
// Single-instance objects
store.define_object("Device.", false)?;
store.define_object("Device.WiFi.", false)?;

// Multi-instance object table
store.define_object("Device.WiFi.Radio.", true)?;

// Template object -- defines the schema for instances
store.define_object("Device.WiFi.Radio.{i}.", false)?;

// Template parameters -- copied to new instances on add()
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
println!("Created {}", result.path);
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

// Delete an instance (cascades to child params)
let mut session = store.session()?;
session.delete("Device.WiFi.Radio.2.")?;
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

## Performance

### SQLite Optimizations

- **WAL mode** -- Readers never block writers; writes append to WAL instead of rewriting the main database
- **`synchronous = NORMAL`** -- Safe with WAL, avoids fsync on every commit
- **8 MB page cache** (`cache_size = -8000`) -- Reduces disk I/O for large data models
- **Prepared statement caching** -- `prepare_cached()` avoids re-parsing SQL for hot queries

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

## Source Layout

```
src/
  lib.rs       Public API re-exports
  error.rs     DmStoreError enum
  types.rs     ParamType, Parameter, Object, AddResult, DmStoreConfig
  path.rs      TR-181 path parsing, validation, FNV-1a hashing
  schema.rs    SQLite schema initialization and PRAGMAs
  store.rs     DmStore -- main entry point, connection, cache management
  session.rs   Session -- transactional get/set/add/delete
```
