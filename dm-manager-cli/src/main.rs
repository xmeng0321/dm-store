use clap::{Parser, Subcommand};
use dm_manager_lib::{Access, DmManager, DmManagerError};
use dm_store_lib::{DmStore, DmStoreConfig};
use rustyline::completion::{Completer, Pair};
use rustyline::error::ReadlineError;
use rustyline::highlight::Highlighter;
use rustyline::hint::Hinter;
use rustyline::validate::Validator;
use rustyline::{Context, Editor, Helper};

#[derive(Parser)]
#[command(name = "dm-manager", about = "TR-181 data model manager CLI")]
struct Cli {
    /// Path to the SQLite database file
    #[arg(short, long, default_value = "dm-store.db")]
    db: String,

    /// JSON schema files to load before executing the command
    #[arg(short, long)]
    schema: Vec<String>,

    /// Disable in-memory cache
    #[arg(long)]
    no_cache: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand, Clone)]
enum Commands {
    /// Load a JSON schema file
    Load {
        /// Path to JSON schema file
        path: String,
    },
    /// Get a parameter value or all parameters of an object (path ending with '.')
    Get {
        /// Parameter or object path
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
        /// Multi-instance object path (e.g., Device.Bridging.Bridge.)
        path: String,
    },
    /// Delete an instance
    Del {
        /// Instance path (e.g., Device.Bridging.Bridge.3.)
        path: String,
    },
    /// List instance numbers of a multi-instance object
    Instances {
        /// Multi-instance object path (e.g., Device.Bridging.Bridge.)
        path: String,
    },
    /// Show schema info for a path
    Schema {
        /// Object or parameter path
        path: String,
    },
    /// List all paths in the schema
    ListSchema,
    /// Dump all data (schema + stored values)
    Dump,
    /// Start interactive REPL shell
    Shell,
}

fn main() {
    let cli = Cli::parse();

    let config = DmStoreConfig {
        use_cache: !cli.no_cache,
    };

    let store = match DmStore::open_with_config(&cli.db, config) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error opening database: {}", e);
            std::process::exit(1);
        }
    };

    let mut mgr = DmManager::new(store);

    // Load schema files specified with --schema
    for schema_path in &cli.schema {
        if let Err(e) = mgr.load_schema_file(schema_path) {
            eprintln!("Error loading schema {}: {}", schema_path, e);
            std::process::exit(1);
        }
    }

    // Register default read hooks for read-only params without const/default
    if !cli.schema.is_empty() {
        mgr.register_default_read_hooks();
    }

    match cli.command {
        Some(cmd) => {
            if let Err(e) = execute_command(&mut mgr, &cmd) {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        None => {
            eprintln!("No command specified. Use --help for usage.");
            std::process::exit(1);
        }
    }
}

fn do_get(mgr: &DmManager, path: &str) -> Result<(), DmManagerError> {
    if path.ends_with('.') {
        let params = mgr.get_object(path)?;
        if params.is_empty() {
            println!("No parameters found for {}", path);
        } else {
            for p in &params {
                println!("{}", p);
            }
        }
    } else {
        let p = mgr.get(path)?;
        println!("{}", p);
    }
    Ok(())
}

fn execute_command(mgr: &mut DmManager, cmd: &Commands) -> Result<(), DmManagerError> {
    match cmd {
        Commands::Load { path } => {
            mgr.load_schema_file(path)?;
            println!("Schema loaded from {}", path);
        }
        Commands::Get { path } => {
            do_get(mgr, path)?;
        }
        Commands::Set { path, value } => {
            let mut session = mgr.session()?;
            session.set(path, value)?;
            session.commit()?;
            println!("OK");
        }
        Commands::Add { path } => {
            let mut session = mgr.session()?;
            let result = session.add(path)?;
            session.commit()?;
            println!(
                "Added instance {} at {}",
                result.instance_number, result.path
            );
        }
        Commands::Del { path } => {
            let mut session = mgr.session()?;
            session.delete(path)?;
            session.commit()?;
            println!("Deleted {}", path);
        }
        Commands::Instances { path } => {
            let nums = mgr.instances(path)?;
            if nums.is_empty() {
                println!("No instances for {}", path);
            } else {
                for n in &nums {
                    println!("{}", n);
                }
            }
        }
        Commands::Schema { path } => {
            show_schema(mgr, path);
        }
        Commands::ListSchema => {
            let paths = mgr.schema_paths();
            for p in paths {
                println!("{}", p);
            }
        }
        Commands::Dump => {
            dump_all(mgr)?;
        }
        Commands::Shell => {
            run_repl(mgr)?;
        }
    }
    Ok(())
}

fn show_schema(mgr: &DmManager, path: &str) {
    if let Some(ps) = mgr.param_schema(path) {
        println!("Parameter: {}", ps.path);
        println!("  Type:       {} ({})", ps.param_type, ps.data_type_raw);
        println!("  Access:     {}", ps.access);
        if let Some(ref d) = ps.default {
            println!("  Default:    {}", d);
        }
        if let Some(ref c) = ps.const_value {
            println!("  Const:      {}", c);
        }
        if ps.is_list {
            println!("  List:       yes");
        }
        match &ps.constraint {
            dm_manager_lib::ValueConstraint::SignedRange { min, max } => {
                println!(
                    "  Range:      {}..{}",
                    min.map_or("".to_string(), |v| v.to_string()),
                    max.map_or("".to_string(), |v| v.to_string())
                );
            }
            dm_manager_lib::ValueConstraint::UnsignedRange { min, max } => {
                println!(
                    "  Range:      {}..{}",
                    min.map_or("".to_string(), |v| v.to_string()),
                    max.map_or("".to_string(), |v| v.to_string())
                );
            }
            dm_manager_lib::ValueConstraint::Length { max } => {
                if let Some(m) = max {
                    println!("  MaxLength:  {}", m);
                }
            }
            dm_manager_lib::ValueConstraint::Enum(variants) => {
                println!("  Enum:       {:?}", variants);
            }
            dm_manager_lib::ValueConstraint::None => {}
        }
        if !ps.path_ref.is_empty() {
            println!("  PathRef:");
            for r in &ps.path_ref {
                println!("    {}", r);
            }
        }
        println!("  Object:     {}", ps.object_path);
    } else if let Some(os) = mgr.object_schema(path) {
        println!("Object: {}", os.path);
        println!("  Access:     {}", os.access);
        println!("  Template:   {}", os.is_template);
        println!("  Multi:      {}", os.is_multi);
        if let Some(ref p) = os.parent_path {
            println!("  Parent:     {}", p);
        }
        if !os.unique_keys.is_empty() {
            println!("  UniqueKeys: {}", os.unique_keys.join(", "));
        }
        if !os.param_names.is_empty() {
            println!("  Parameters: {}", os.param_names.join(", "));
        }
        if !os.child_object_paths.is_empty() {
            println!("  Children:");
            for c in &os.child_object_paths {
                println!("    {}", c);
            }
        }
    } else {
        println!("Not found in schema: {}", path);
    }
}

fn dump_all(mgr: &DmManager) -> Result<(), DmManagerError> {
    println!("=== Schema Objects ===");
    for path in mgr.schema().object_paths() {
        let Some(os) = mgr.object_schema(path) else {
            log::warn!("schema object listed but not resolvable: {}", path);
            continue;
        };
        let flags: Vec<&str> = [
            if os.is_template {
                Some("template")
            } else {
                None
            },
            if os.is_multi { Some("multi") } else { None },
            if os.access == Access::ReadWrite {
                Some("rw")
            } else {
                Some("ro")
            },
        ]
        .iter()
        .filter_map(|f| *f)
        .collect();
        println!("  {} [{}]", path, flags.join(", "));
    }

    println!("\n=== Schema Parameters ===");
    for path in mgr.schema().param_paths() {
        let Some(ps) = mgr.param_schema(path) else {
            log::warn!("schema parameter listed but not resolvable: {}", path);
            continue;
        };
        let rw = if ps.access == Access::ReadWrite {
            "rw"
        } else {
            "ro"
        };
        let val = ps
            .const_value
            .as_deref()
            .or(ps.default.as_deref())
            .unwrap_or("");
        println!("  {} ({}, {}) = {}", path, ps.data_type_raw, rw, val);
    }

    let dump = mgr.store().dump().map_err(DmManagerError::Store)?;

    println!("\n=== Stored Objects ===");
    for obj in &dump.objects {
        let multi_str = if obj.is_multi { " [multi]" } else { "" };
        println!("  {}{}", obj.path, multi_str);
    }

    println!("\n=== Stored Schema Templates ===");
    for obj in &dump.schema_objects {
        let multi_str = if obj.is_multi { " [multi]" } else { "" };
        println!("  {}{}", obj.path, multi_str);
    }
    for p in &dump.schema_params {
        let val = p.value.as_deref().unwrap_or("(empty)");
        let rw = if p.writable { "rw" } else { "ro" };
        println!("  {} = {} ({}, {})", p.path, val, p.param_type, rw);
    }

    println!("\n=== Stored Parameters ===");
    for p in &dump.params {
        let val = p.value.as_deref().unwrap_or("(empty)");
        let rw = if p.writable { "rw" } else { "ro" };
        println!("  {} = {} ({}, {})", p.path, val, p.param_type, rw);
    }

    Ok(())
}

// --- REPL with auto-completion ---

struct DmHelper {
    /// Sorted list of all schema template paths for completion.
    schema_paths: Vec<String>,
    /// Expanded concrete paths (template paths with {i} replaced by real instance numbers).
    concrete_paths: Vec<String>,
    /// REPL command names.
    commands: Vec<String>,
}

impl DmHelper {
    fn new() -> Self {
        DmHelper {
            schema_paths: Vec::new(),
            concrete_paths: Vec::new(),
            commands: vec![
                "load".to_string(),
                "get".to_string(),
                "set".to_string(),
                "add".to_string(),
                "del".to_string(),
                "instances".to_string(),
                "schema".to_string(),
                "list-schema".to_string(),
                "dump".to_string(),
                "begin".to_string(),
                "commit".to_string(),
                "abort".to_string(),
                "help".to_string(),
                "quit".to_string(),
            ],
        }
    }

    fn update_schema_paths(&mut self, paths: Vec<String>) {
        self.schema_paths = paths;
        self.schema_paths.sort();
    }

    /// Rebuild concrete paths by expanding {i} in schema paths using real instance numbers.
    fn refresh_concrete_paths(&mut self, mgr: &DmManager) {
        let mut concrete = Vec::new();
        for tmpl in &self.schema_paths {
            if !tmpl.contains("{i}") {
                concrete.push(tmpl.clone());
            } else {
                expand_template(mgr, tmpl, &mut concrete);
            }
        }
        concrete.sort();
        concrete.dedup();
        self.concrete_paths = concrete;
    }
}

/// Recursively expand {i} placeholders in a template path using real instances.
fn expand_template(mgr: &DmManager, template: &str, out: &mut Vec<String>) {
    // Find the first {i} and get the table path up to it
    let Some(pos) = template.find("{i}") else {
        out.push(template.to_string());
        return;
    };

    let table_path = &template[..pos]; // e.g. "Device.Bridging.Bridge."
    let after = &template[pos + 3..]; // e.g. ".Enable" or ".Port.{i}.Status"

    // table_path might itself contain {i} that was already expanded in a previous call,
    // so query instances directly
    let nums = match mgr.instances(table_path) {
        Ok(n) => n,
        Err(_) => return,
    };

    for num in nums {
        let expanded = format!("{}{}{}", table_path, num, after);
        if expanded.contains("{i}") {
            expand_template(mgr, &expanded, out);
        } else {
            out.push(expanded);
        }
    }
}

impl Completer for DmHelper {
    type Candidate = Pair;

    fn complete(
        &self,
        line: &str,
        pos: usize,
        _ctx: &Context<'_>,
    ) -> rustyline::Result<(usize, Vec<Pair>)> {
        let line_up_to = &line[..pos];
        let parts: Vec<&str> = line_up_to.split_whitespace().collect();

        if parts.is_empty() || (parts.len() == 1 && !line_up_to.ends_with(' ')) {
            // Completing command name
            let prefix = parts.first().copied().unwrap_or("");
            let matches: Vec<Pair> = self
                .commands
                .iter()
                .filter(|c| c.starts_with(prefix))
                .map(|c| Pair {
                    display: c.clone(),
                    replacement: c.clone(),
                })
                .collect();
            let start = pos - prefix.len();
            return Ok((start, matches));
        }

        // Completing path argument (second word onwards)
        let last_word = if line_up_to.ends_with(' ') {
            ""
        } else {
            parts.last().copied().unwrap_or("")
        };

        // Check if user input contains digits (concrete path) or is pure template
        let has_digit = last_word.chars().any(|c| c.is_ascii_digit());

        let candidates = if has_digit {
            &self.concrete_paths
        } else {
            &self.schema_paths
        };

        let matches: Vec<Pair> = candidates
            .iter()
            .filter(|p| p.starts_with(last_word))
            .map(|p| Pair {
                display: p.clone(),
                replacement: p.clone(),
            })
            .collect();

        let start = pos - last_word.len();
        Ok((start, matches))
    }
}

impl Hinter for DmHelper {
    type Hint = String;
}

impl Highlighter for DmHelper {}
impl Validator for DmHelper {}
impl Helper for DmHelper {}

fn run_repl(mgr: &mut DmManager) -> Result<(), DmManagerError> {
    let mut helper = DmHelper::new();
    // Populate schema paths if schema is already loaded
    helper.update_schema_paths(mgr.schema_paths().iter().map(|s| s.to_string()).collect());

    helper.refresh_concrete_paths(mgr);

    let mut rl = Editor::new().map_err(|e| DmManagerError::Schema(e.to_string()))?;
    rl.set_helper(Some(helper));

    let history_path = dirs_hint();
    if let Some(ref path) = history_path {
        let _ = rl.load_history(path);
    }

    println!("dm-manager interactive shell. Type 'help' for commands, 'quit' to exit.");

    let mut in_session = false;

    loop {
        let prompt = if in_session {
            "dm-mgr(session)> "
        } else {
            "dm-mgr> "
        };

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
                            let _ = mgr.store().connection().execute_batch(
                                "ROLLBACK TO SAVEPOINT repl_session; RELEASE SAVEPOINT repl_session;",
                            );
                            let _ = mgr.store_mut().reload_cache();
                            println!("Session aborted.");
                        }
                        break;
                    }
                    "load" => {
                        if parts.len() < 2 {
                            println!("Usage: load <json_file>");
                        } else {
                            match mgr.load_schema_file(parts[1]) {
                                Ok(()) => {
                                    println!("Schema loaded from {}", parts[1]);
                                    mgr.register_default_read_hooks();
                                    // Update completion paths
                                    if let Some(h) = rl.helper_mut() {
                                        h.update_schema_paths(
                                            mgr.schema_paths()
                                                .iter()
                                                .map(|s| s.to_string())
                                                .collect(),
                                        );
                                        h.refresh_concrete_paths(mgr);
                                    }
                                }
                                Err(e) => println!("Error: {}", e),
                            }
                        }
                    }
                    "get" => {
                        if parts.len() < 2 {
                            println!("Usage: get <path>");
                        } else {
                            if let Err(e) = do_get(mgr, parts[1]) {
                                println!("Error: {}", e);
                            }
                        }
                    }
                    "set" => {
                        if parts.len() < 3 {
                            println!("Usage: set <path> <value>");
                        } else {
                            let path = parts[1];
                            let value = parts[2..].join(" ");
                            if in_session {
                                match repl_set_in_session(mgr, path, &value) {
                                    Ok(()) => println!("OK"),
                                    Err(e) => println!("Error: {}", e),
                                }
                            } else {
                                match repl_set(mgr, path, &value) {
                                    Ok(()) => println!("OK"),
                                    Err(e) => println!("Error: {}", e),
                                }
                            }
                        }
                    }
                    "add" => {
                        if parts.len() < 2 {
                            println!("Usage: add <table_path>");
                        } else {
                            let mut session = match mgr.session() {
                                Ok(s) => s,
                                Err(e) => {
                                    println!("Error: {}", e);
                                    continue;
                                }
                            };
                            match session.add(parts[1]) {
                                Ok(r) => {
                                    if let Err(e) = session.commit() {
                                        println!("Error committing: {}", e);
                                    } else {
                                        println!(
                                            "Added instance {} at {}",
                                            r.instance_number, r.path
                                        );
                                        if let Some(h) = rl.helper_mut() {
                                            h.refresh_concrete_paths(mgr);
                                        }
                                    }
                                }
                                Err(e) => println!("Error: {}", e),
                            }
                        }
                    }
                    "del" => {
                        if parts.len() < 2 {
                            println!("Usage: del <instance_path>");
                        } else {
                            let mut session = match mgr.session() {
                                Ok(s) => s,
                                Err(e) => {
                                    println!("Error: {}", e);
                                    continue;
                                }
                            };
                            match session.delete(parts[1]) {
                                Ok(()) => {
                                    if let Err(e) = session.commit() {
                                        println!("Error committing: {}", e);
                                    } else {
                                        let _ = mgr.store_mut().reload_cache();
                                        println!("Deleted {}", parts[1]);
                                        if let Some(h) = rl.helper_mut() {
                                            h.refresh_concrete_paths(mgr);
                                        }
                                    }
                                }
                                Err(e) => println!("Error: {}", e),
                            }
                        }
                    }
                    "instances" => {
                        if parts.len() < 2 {
                            println!("Usage: instances <table_path>");
                        } else {
                            match mgr.instances(parts[1]) {
                                Ok(nums) => {
                                    if nums.is_empty() {
                                        println!("No instances for {}", parts[1]);
                                    } else {
                                        for n in &nums {
                                            println!("{}", n);
                                        }
                                    }
                                }
                                Err(e) => println!("Error: {}", e),
                            }
                        }
                    }
                    "schema" => {
                        if parts.len() < 2 {
                            println!("Usage: schema <path>");
                        } else {
                            show_schema(mgr, parts[1]);
                        }
                    }
                    "list-schema" => {
                        let paths = mgr.schema_paths();
                        for p in paths {
                            println!("{}", p);
                        }
                    }
                    "dump" => {
                        if let Err(e) = dump_all(mgr) {
                            println!("Error: {}", e);
                        }
                    }
                    "begin" => {
                        if in_session {
                            println!(
                                "Error: session already active. Use 'commit' or 'abort' first."
                            );
                        } else {
                            match mgr
                                .store()
                                .connection()
                                .execute_batch("SAVEPOINT repl_session")
                            {
                                Ok(()) => {
                                    in_session = true;
                                    println!("Session started.");
                                }
                                Err(e) => println!("Error: {}", e),
                            }
                        }
                    }
                    "commit" => {
                        if !in_session {
                            println!("Error: no active session.");
                        } else {
                            match mgr
                                .store()
                                .connection()
                                .execute_batch("RELEASE SAVEPOINT repl_session")
                            {
                                Ok(()) => {
                                    in_session = false;
                                    println!("Session committed.");
                                }
                                Err(e) => println!("Error: {}", e),
                            }
                        }
                    }
                    "abort" => {
                        if !in_session {
                            println!("Error: no active session.");
                        } else {
                            match mgr.store().connection().execute_batch(
                                "ROLLBACK TO SAVEPOINT repl_session; RELEASE SAVEPOINT repl_session;",
                            ) {
                                Ok(()) => {
                                    let _ = mgr.store_mut().reload_cache();
                                    in_session = false;
                                    println!("Session aborted.");
                                }
                                Err(e) => println!("Error: {}", e),
                            }
                        }
                    }
                    _ => {
                        println!("Unknown command: {}. Type 'help' for commands.", cmd);
                    }
                }
            }
            Err(ReadlineError::Interrupted) => {
                // Ctrl+C: clear current line, continue
                println!();
                continue;
            }
            Err(ReadlineError::Eof) => {
                // Ctrl+D: exit
                if in_session {
                    let _ = mgr.store().connection().execute_batch(
                        "ROLLBACK TO SAVEPOINT repl_session; RELEASE SAVEPOINT repl_session;",
                    );
                    let _ = mgr.store_mut().reload_cache();
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

fn repl_set(mgr: &mut DmManager, path: &str, value: &str) -> Result<(), DmManagerError> {
    let mut session = mgr.session()?;
    session.set(path, value)?;
    session.commit()?;
    Ok(())
}

fn repl_set_in_session(mgr: &mut DmManager, path: &str, value: &str) -> Result<(), DmManagerError> {
    // In a REPL session, we use a nested session for each set
    let mut session = mgr.session()?;
    session.set(path, value)?;
    session.commit()?;
    Ok(())
}

fn print_repl_help() {
    println!(
        "Commands:
  load <json_file>              Load a JSON schema file
  get <path>                    Get parameter or object (path ending with '.')
  set <path> <value>            Set a parameter value
  add <table_path>              Add instance to multi-instance object
  del <instance_path>           Delete an instance
  instances <table_path>        List instance numbers
  schema <path>                 Show schema info for a path
  list-schema                   List all schema paths
  dump                          Dump all data
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
        .map(|h| format!("{}/.dm-manager-history", h))
}
