//! Load schemas and JS handlers from a folder hierarchy.
//!
//! Expected layout:
//!
//! ```text
//! <default_folder>/
//!   <sub>/
//!     *.json          # schema files (loaded first)
//!     handlers/
//!       *.js          # handler scripts (optional)
//! ```
//!
//! For every immediate sub-folder under the default folder the manager:
//! 1. Loads every `*.json` file as a schema (in sorted order for determinism).
//! 2. If a `handlers/` directory exists, evaluates every `*.js` file in the
//!    shared QuickJS context.
//! 3. On the first registration of a given sub-folder name (per process),
//!    calls `DM_Init()` if such a function is defined after the JS evals.

use std::fs;
use std::path::{Path, PathBuf};

use crate::error::DmManagerError;
use crate::manager::DmManager;

impl DmManager {
    /// Load every immediate sub-folder beneath `folder_path`.
    pub fn load_default_folder<P: AsRef<Path>>(
        &mut self,
        folder_path: P,
    ) -> Result<(), DmManagerError> {
        let root = folder_path.as_ref();
        if !root.is_dir() {
            return Err(DmManagerError::Schema(format!(
                "not a directory: {}",
                root.display()
            )));
        }

        let mut subdirs: Vec<PathBuf> = fs::read_dir(root)?
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|p| p.is_dir())
            .collect();
        subdirs.sort();

        for sub in subdirs {
            self.load_subfolder(&sub)?;
        }
        Ok(())
    }

    /// Load a single sub-folder (schemas + handlers + optional `DM_Init`).
    ///
    /// Reloading a sub-folder that's already been registered in this process
    /// is a no-op — handler scripts are not re-evaluated (doing so would
    /// fail for modules that declare `let`/`const` bindings) and `DM_Init`
    /// is not re-invoked.
    pub fn load_subfolder<P: AsRef<Path>>(&mut self, dir: P) -> Result<(), DmManagerError> {
        let dir = dir.as_ref();
        let folder_name = dir
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| {
                DmManagerError::Schema(format!("invalid folder name: {}", dir.display()))
            })?
            .to_string();

        if self.is_folder_registered(&folder_name) {
            return Ok(());
        }

        let mut schema_files: Vec<PathBuf> = fs::read_dir(dir)?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| {
                p.is_file() && p.extension().and_then(|s| s.to_str()) == Some("json")
            })
            .collect();
        schema_files.sort();

        for schema_path in schema_files {
            let as_str = schema_path.to_str().ok_or_else(|| {
                DmManagerError::Schema(format!(
                    "non-utf8 schema path: {}",
                    schema_path.display()
                ))
            })?;
            self.load_schema_file(as_str)?;
        }

        let handlers_dir = dir.join("handlers");
        if handlers_dir.is_dir() {
            let mut js_files: Vec<PathBuf> = fs::read_dir(&handlers_dir)?
                .filter_map(|e| e.ok())
                .map(|e| e.path())
                .filter(|p| {
                    p.is_file() && p.extension().and_then(|s| s.to_str()) == Some("js")
                })
                .collect();
            js_files.sort();

            if !js_files.is_empty() {
                self.ensure_js()?;
                for js_path in js_files {
                    self.js_mut().unwrap().eval_file(&js_path)?;
                }

                self.run_init_with_write_session()?;
            }
        }

        self.mark_folder_registered(&folder_name);
        Ok(())
    }
}
