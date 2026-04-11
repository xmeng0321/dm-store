# dm-store

A TR-181 data model storage and management system written in Rust. Provides persistent storage with hash-accelerated lookups, schema-aware validation, and transactional session support for CPE device data models.

## Overview

TR-181 is a Broadband Forum standard for device data models used in CPE (Customer Premises Equipment) management. It uses hierarchical dot-separated paths to organize device configuration and status data:

```
Device.WiFi.Radio.1.Enable = true
Device.WiFi.Radio.1.Channel = 6
Device.WiFi.SSID.1.SSID = "MyNetwork"
```

This workspace provides two layers for working with these data models:

- **dm-store** -- A high-performance SQLite-backed storage engine with hash-accelerated lookups, optional in-memory caching, and transactional sessions.
- **dm-manager** -- A schema-aware management layer that adds JSON schema loading, path/value validation, read/write separation, and hook-based extensibility on top of dm-store.

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
            |   dm-store-lib    |   SQLite persistence + cache + sessions
            +-------------------+
                     |
                  [SQLite]
```

**dm-store-lib** owns the database. It manages the SQLite schema (4 tables separating instance data from schema templates), provides FNV-1a hash-accelerated lookups, an optional in-memory cache for O(1) reads, and SAVEPOINT-based transactional sessions.

**dm-manager-lib** sits on top. It loads TR-181 schemas from JSON files into an in-memory `DmSchema`, validates every get/set against the schema, enforces type constraints and access modes, and provides hooks for read-only parameters and dynamic instance enumeration. Writable parameters are persisted through dm-store; read-only parameters are served from hooks or schema defaults.

Both layers have corresponding CLI crates (`dm-store-cli`, `dm-manager-cli`) that expose the full API as command-line tools with interactive REPL shells.

## Workspace Structure

```
dm-store/
  Cargo.toml               # Workspace root
  dm-store-lib/             # Storage engine library
  dm-store-cli/             # Storage engine CLI + REPL
  dm-manager-lib/           # Schema-aware manager library
  dm-manager-cli/           # Manager CLI + REPL with tab completion
```

See each crate's README for detailed API documentation and usage.

## Data Model Concepts

### Objects vs Parameters

- **Objects** define the tree structure. Their paths end with `.` (e.g., `Device.WiFi.Radio.1.`)
- **Parameters** hold values and belong to objects. Their paths do not end with `.` (e.g., `Device.WiFi.Radio.1.Enable`)

### Multi-Instance Objects

Multi-instance objects (tables) can have numbered instances created via `add` and removed via `delete`:

```
Schema templates:                    Concrete data:
  Device.WiFi.Radio.{i}.               Device.WiFi.Radio. [multi]
  Device.WiFi.Radio.{i}.Enable         Device.WiFi.Radio.1.
  Device.WiFi.Radio.{i}.Channel        Device.WiFi.Radio.1.Enable
                                        Device.WiFi.Radio.1.Channel
                                        Device.WiFi.Radio.2.
                                        Device.WiFi.Radio.2.Enable
                                        Device.WiFi.Radio.2.Channel
```

The `add` operation resolves `{i}` placeholders from schema templates and instantiates concrete rows in data tables. The `delete` operation cascades, removing all child objects and parameters.

### Path Conventions

| Pattern | Meaning | Example |
|---------|---------|---------|
| Ends with `.` | Object path | `Device.WiFi.Radio.1.` |
| No trailing `.` | Parameter path | `Device.WiFi.Radio.1.Enable` |
| Contains `{i}` | Template path | `Device.WiFi.Radio.{i}.Enable` |
| Trailing number + `.` | Instance path | `Device.WiFi.Radio.1.` |

## Getting Started

### Prerequisites

- Rust toolchain (install via [rustup](https://rustup.rs/))

### Build

```bash
cargo build --release
```

Binaries are at `target/release/dm-store` and `target/release/dm-manager`.

## License

This project is provided as-is for internal use.
