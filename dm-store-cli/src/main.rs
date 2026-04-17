use clap::{Parser, Subcommand};
use dm_store_lib::{DmStore, DmStoreConfig, DmStoreError, ParamType};
use rustyline::error::ReadlineError;
use rustyline::DefaultEditor;

#[derive(Parser)]
#[command(name = "dm-store", about = "TR-181 data model store CLI")]
struct Cli {
    /// Path to the SQLite database file
    #[arg(short, long, default_value = "dm-store.db")]
    db: String,

    /// Disable in-memory cache
    #[arg(long)]
    no_cache: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Clone)]
enum Commands {
    /// Get a parameter by exact path
    Get {
        /// Parameter path (e.g., Device.WiFi.Radio.1.Enable)
        path: String,
    },
    /// Get all parameters of an object
    GetObject {
        /// Object path ending with '.' (e.g., Device.WiFi.Radio.1.)
        path: String,
    },
    /// Set a parameter value
    Set {
        /// Parameter path
        path: String,
        /// New value
        value: String,
    },
    /// Add a new instance to a multi-instance object
    Add {
        /// Multi-instance object path (e.g., Device.WiFi.Radio.)
        path: String,
    },
    /// Delete an instance
    Del {
        /// Instance path (e.g., Device.WiFi.Radio.3.)
        path: String,
    },
    /// List instance numbers of a multi-instance object
    Instances {
        /// Multi-instance object path (e.g., Device.WiFi.Radio.)
        path: String,
    },
    /// Define an object in the data model
    DefineObject {
        /// Object path ending with '.'
        path: String,
        /// Mark as multi-instance object
        #[arg(long)]
        multi: bool,
    },
    /// Define a parameter in the data model
    DefineParam {
        /// Parameter path
        path: String,
        /// Parameter type
        #[arg(long, default_value = "string")]
        r#type: String,
        /// Read-only parameter
        #[arg(long)]
        readonly: bool,
        /// Default value
        #[arg(long)]
        default: Option<String>,
    },
    /// Dump all objects and parameters
    Dump,
    /// Start interactive REPL shell
    Shell,
}

fn main() {
    let cli = Cli::parse();

    let config = DmStoreConfig {
        use_cache: !cli.no_cache,
    };

    let mut store = match DmStore::open_with_config(&cli.db, config) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error opening database: {}", e);
            std::process::exit(1);
        }
    };

    match cli.command {
        Some(cmd) => {
            if let Err(e) = execute_command(&mut store, &cmd) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        None => {
            // No subcommand: show help
            eprintln!("No command specified. Use --help for usage.");
            std::process::exit(1);
        }
    }
}

fn execute_command(store: &mut DmStore, cmd: &Commands) -> Result<(), DmStoreError> {
    match cmd {
        Commands::Get { path } => {
            let param = store.get(path)?;
            println!("{}", param);
        }
        Commands::GetObject { path } => {
            let params = store.get_object(path)?;
            if params.is_empty() {
                println!("No parameters found for {}", path);
            } else {
                for p in &params {
                    println!("{}", p);
                }
            }
        }
        Commands::Set { path, value } => {
            let mut session = store.session()?;
            session.set(path, value)?;
            session.commit()?;
            println!("OK");
        }
        Commands::Add { path } => {
            let mut session = store.session()?;
            let result = session.add(path)?;
            session.commit()?;
            dm_store_lib::render::print_add_result(&result);
        }
        Commands::Del { path } => {
            let mut session = store.session()?;
            session.delete(path)?;
            session.commit()?;
            dm_store_lib::render::print_deleted(path);
        }
        Commands::Instances { path } => {
            let nums = store.instances(path)?;
            dm_store_lib::render::print_instances(path, &nums);
        }
        Commands::DefineObject { path, multi } => {
            store.define_object(path, *multi)?;
            let kind = if *multi { "multi-instance" } else { "single" };
            println!("Defined {} object: {}", kind, path);
        }
        Commands::DefineParam {
            path,
            r#type,
            readonly,
            default,
        } => {
            let param_type =
                ParamType::parse_name(r#type).ok_or_else(|| DmStoreError::InvalidPath {
                    path: path.clone(),
                    reason: format!("unknown type: {}", r#type),
                })?;
            store.define_parameter(path, param_type, !readonly, default.as_deref())?;
            println!("Defined parameter: {} ({})", path, param_type);
        }
        Commands::Dump => {
            dump_all(store)?;
        }
        Commands::Shell => {
            run_repl(store)?;
        }
    }
    Ok(())
}

fn dump_all(store: &DmStore) -> Result<(), DmStoreError> {
    let dump = store.dump()?;

    println!("=== Objects ===");
    for obj in &dump.objects {
        let multi_str = if obj.is_multi { " [multi]" } else { "" };
        let canonical = dm_store_lib::path::canonicalize(&obj.path);
        let inst_str = if canonical != obj.path {
            format!(" (schema: {})", canonical)
        } else {
            String::new()
        };
        println!("  {}{}{}", obj.path, multi_str, inst_str);
    }

    println!("\n=== Parameters ===");
    for p in &dump.params {
        let val = p.value.as_deref().unwrap_or("(empty)");
        let rw = if p.writable { "rw" } else { "ro" };
        println!("  {} = {} ({}, {})", p.path, val, p.param_type, rw);
    }

    if !dump.schema_objects.is_empty() || !dump.schema_params.is_empty() {
        println!("\n=== Schema Templates ===");
        for obj in &dump.schema_objects {
            let multi_str = if obj.is_multi { " [multi]" } else { "" };
            println!("  {} [template]{}", obj.path, multi_str);
        }
        for p in &dump.schema_params {
            let rw = if p.writable { "rw" } else { "ro" };
            println!("  {} ({}, {})", p.path, p.param_type, rw);
        }
    }

    Ok(())
}

fn run_repl(store: &mut DmStore) -> Result<(), DmStoreError> {
    let mut rl = DefaultEditor::new().map_err(|e| DmStoreError::Schema(e.to_string()))?;
    let history_path = dirs_hint();

    if let Some(ref path) = history_path {
        let _ = rl.load_history(path);
    }

    println!("dm-store interactive shell. Type 'help' for commands, 'quit' to exit.");

    let mut in_session = false;
    const REPL_SAVEPOINT: &str = "repl_session";

    // The REPL wraps its batch scope in an outer savepoint managed via
    // DmStore::begin/release/rollback_savepoint. Each set/add/del still goes
    // through a nested Session, so cache coherency is driven by the Session
    // abstraction, not by raw SQL here.
    loop {
        let prompt = if in_session { "dm(session)> " } else { "dm> " };
        match rl.readline(prompt) {
            Ok(line) => {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let _ = rl.add_history_entry(line);

                let parts: Vec<&str> = line.split_whitespace().collect();
                let cmd = parts[0].to_lowercase();

                match cmd.as_str() {
                    "help" => print_repl_help(),
                    "quit" | "exit" => {
                        if in_session {
                            let _ = store.rollback_savepoint(REPL_SAVEPOINT);
                            println!("Session aborted.");
                        }
                        break;
                    }
                    "begin" => {
                        if in_session {
                            println!(
                                "Error: session already active. Use 'commit' or 'abort' first."
                            );
                        } else {
                            store.begin_savepoint(REPL_SAVEPOINT)?;
                            in_session = true;
                            println!("Session started.");
                        }
                    }
                    "commit" => {
                        if !in_session {
                            println!("Error: no active session.");
                        } else {
                            store.release_savepoint(REPL_SAVEPOINT)?;
                            in_session = false;
                            println!("Session committed.");
                        }
                    }
                    "abort" => {
                        if !in_session {
                            println!("Error: no active session.");
                        } else {
                            store.rollback_savepoint(REPL_SAVEPOINT)?;
                            in_session = false;
                            println!("Session aborted.");
                        }
                    }
                    "get" => {
                        if parts.len() < 2 {
                            println!("Usage: get <path>");
                        } else {
                            match store.get(parts[1]) {
                                Ok(p) => println!("{}", p),
                                Err(e) => println!("Error: {}", e),
                            }
                        }
                    }
                    "get-object" => {
                        if parts.len() < 2 {
                            println!("Usage: get-object <object_path>");
                        } else {
                            match store.get_object(parts[1]) {
                                Ok(params) => {
                                    if params.is_empty() {
                                        println!("No parameters found.");
                                    } else {
                                        for p in &params {
                                            println!("{}", p);
                                        }
                                    }
                                }
                                Err(e) => println!("Error: {}", e),
                            }
                        }
                    }
                    "set" => {
                        if parts.len() < 3 {
                            println!("Usage: set <path> <value>");
                        } else {
                            let path = parts[1];
                            let value = parts[2..].join(" ");
                            match repl_set(store, path, &value) {
                                Ok(()) => println!("OK"),
                                Err(e) => println!("Error: {}", e),
                            }
                        }
                    }
                    "add" => {
                        if parts.len() < 2 {
                            println!("Usage: add <table_path>");
                        } else {
                            let mut session = store.session()?;
                            match session.add(parts[1]) {
                                Ok(r) => {
                                    session.commit()?;
                                    dm_store_lib::render::print_add_result(&r);
                                }
                                Err(e) => {
                                    println!("Error: {}", e);
                                }
                            }
                        }
                    }
                    "del" => {
                        if parts.len() < 2 {
                            println!("Usage: del <instance_path>");
                        } else {
                            let mut session = store.session()?;
                            match session.delete(parts[1]) {
                                Ok(()) => {
                                    session.commit()?;
                                    dm_store_lib::render::print_deleted(parts[1]);
                                }
                                Err(e) => {
                                    println!("Error: {}", e);
                                }
                            }
                        }
                    }
                    "instances" => {
                        if parts.len() < 2 {
                            println!("Usage: instances <table_path>");
                        } else {
                            match store.instances(parts[1]) {
                                Ok(nums) => {
                                    dm_store_lib::render::print_instances(parts[1], &nums);
                                }
                                Err(e) => println!("Error: {}", e),
                            }
                        }
                    }
                    "define-object" => {
                        if parts.len() < 2 {
                            println!("Usage: define-object <path> [--multi]");
                        } else {
                            let is_multi = parts.get(2).is_some_and(|s| *s == "--multi");
                            match store.define_object(parts[1], is_multi) {
                                Ok(()) => {
                                    let kind = if is_multi { "multi-instance" } else { "single" };
                                    println!("Defined {} object: {}", kind, parts[1]);
                                }
                                Err(e) => println!("Error: {}", e),
                            }
                        }
                    }
                    "define-param" => {
                        if parts.len() < 2 {
                            println!("Usage: define-param <path> [type=string] [default=value] [readonly]");
                        } else {
                            let path = parts[1];
                            let opts = &parts[2..];

                            let mut ptype = ParamType::String;
                            let mut writable = true;
                            let mut default_val: Option<String> = None;

                            for opt in opts {
                                if let Some(t) = opt.strip_prefix("type=") {
                                    if let Some(pt) = ParamType::parse_name(t) {
                                        ptype = pt;
                                    } else {
                                        println!("Unknown type: {}", t);
                                        continue;
                                    }
                                } else if let Some(d) = opt.strip_prefix("default=") {
                                    default_val = Some(d.to_string());
                                } else if *opt == "readonly" {
                                    writable = false;
                                }
                            }

                            match store.define_parameter(
                                path,
                                ptype,
                                writable,
                                default_val.as_deref(),
                            ) {
                                Ok(()) => println!("Defined parameter: {} ({})", path, ptype),
                                Err(e) => println!("Error: {}", e),
                            }
                        }
                    }
                    "dump" => {
                        if let Err(e) = dump_all(store) {
                            println!("Error: {}", e);
                        }
                    }
                    _ => {
                        println!("Unknown command: {}. Type 'help' for commands.", cmd);
                    }
                }
            }
            Err(ReadlineError::Interrupted) | Err(ReadlineError::Eof) => {
                if in_session {
                    let _ = store.rollback_savepoint(REPL_SAVEPOINT);
                    println!("\nSession aborted.");
                }
                break;
            }
            Err(e) => {
                eprintln!("Error: {}", e);
                break;
            }
        }
    }

    if let Some(ref path) = history_path {
        let _ = rl.save_history(path);
    }

    println!("Bye.");
    Ok(())
}

fn repl_set(store: &mut DmStore, path: &str, value: &str) -> Result<(), DmStoreError> {
    let mut session = store.session()?;
    session.set(path, value)?;
    session.commit()?;
    Ok(())
}

fn print_repl_help() {
    println!(
        "Commands:
  get <path>                    Get a parameter by exact path
  get-object <object_path>      Get all parameters of an object
  set <path> <value>            Set a parameter value
  add <table_path>              Add instance to multi-instance object
  del <instance_path>           Delete an instance
  instances <table_path>        List instance numbers of a multi-instance object
  define-object <path> [--multi]  Define an object
  define-param <path> [type=T] [default=V] [readonly]  Define a parameter
  dump                          Dump all objects and parameters
  begin                         Start a session
  commit                        Commit current session
  abort                         Abort current session
  help                          Show this help
  quit                          Exit"
    );
}

fn dirs_hint() -> Option<String> {
    std::env::var("HOME")
        .ok()
        .map(|h| format!("{}/.dm-store-history", h))
}
