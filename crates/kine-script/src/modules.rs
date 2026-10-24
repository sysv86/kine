//! Ready-made scene modules loaded from a project's `Modules/` directory.
//!
//! A module is a directory with metadata and a scene fragment:
//!
//! ```text
//! Modules/
//!   lower-third/
//!     module.json
//!     scene.ts
//! ```
//!
//! `module.json` describes the module (`kind`, `description`, `params`);
//! `scene.ts` is TypeScript that must evaluate to a factory
//! `(props) => SceneNode`. Scenes call it as `module("lower-third", { ... })`.
//!
//! Discovery is deliberately filesystem-only: the library is whatever exists in
//! `Modules/`, so a module can be dropped in without touching the engine.

use crate::authoring::{NodeDef, SceneDef};
use crate::runtime::{eval_module_node_ts, eval_scene_def_ts_with_modules, ModuleSource};
use crate::ScriptError;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Directory name every project uses for its module library.
pub const MODULES_DIR: &str = "Modules";

/// Scanned module directory names are restricted to a safe character set; a
/// scene may only reference a name, never a path.
const MODULE_NAME_CHARS: &str = "-_.abcdefghijklmnopqrstuvwxyz0123456789";

#[derive(Debug, thiserror::Error)]
pub enum ModuleError {
    #[error("module {module:?}: {reason}")]
    Invalid { module: String, reason: String },
    #[error("modules directory {path:?}: {reason}")]
    Io { path: PathBuf, reason: String },
    #[error("module {module:?}: {reason}")]
    Script { module: String, reason: String },
}

/// Metadata from `Modules/<name>/module.json`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModuleMeta {
    /// Module name. Optional in the file; must match the directory name.
    #[serde(default)]
    pub name: String,
    /// Free-form category: `subtitle`, `widget`, `transition`, `animation`, ...
    #[serde(default = "default_kind")]
    pub kind: String,
    #[serde(default)]
    pub description: String,
    /// Default props, surfaced by `kine modules`; a scene may override any of them.
    #[serde(default)]
    pub params: serde_json::Value,
}

fn default_kind() -> String {
    "widget".to_string()
}

impl Default for ModuleMeta {
    fn default() -> Self {
        Self {
            name: String::new(),
            kind: default_kind(),
            description: String::new(),
            params: serde_json::Value::Null,
        }
    }
}

/// One module on disk.
#[derive(Debug, Clone)]
pub struct Module {
    pub meta: ModuleMeta,
    pub scene_path: PathBuf,
    /// TypeScript source of the module factory.
    pub source: String,
}

/// All modules discovered under one `Modules/` directory.
#[derive(Debug, Clone, Default)]
pub struct ModuleRegistry {
    root: Option<PathBuf>,
    modules: BTreeMap<String, Module>,
}

impl ModuleRegistry {
    /// An empty library, used when a project has no `Modules/` directory.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Discover `<dir>/Modules`. A missing directory yields an empty registry:
    /// modules are optional, a broken module is not.
    pub fn discover(dir: &Path) -> Result<Self, ModuleError> {
        let root = dir.join(MODULES_DIR);
        if !root.exists() {
            return Ok(Self::empty());
        }
        if !root.is_dir() {
            return Err(ModuleError::Io {
                path: root,
                reason: "not a directory".to_string(),
            });
        }

        let entries = std::fs::read_dir(&root).map_err(|e| ModuleError::Io {
            path: root.clone(),
            reason: e.to_string(),
        })?;

        let mut modules = BTreeMap::new();
        for entry in entries {
            let entry = entry.map_err(|e| ModuleError::Io {
                path: root.clone(),
                reason: e.to_string(),
            })?;
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            validate_name(&name)?;
            let module = load_module(&path, &name)?;
            modules.insert(name, module);
        }

        Ok(Self {
            root: Some(root),
            modules,
        })
    }

    /// The `Modules/` directory this registry was built from, if it exists.
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    pub fn is_empty(&self) -> bool {
        self.modules.is_empty()
    }

    pub fn len(&self) -> usize {
        self.modules.len()
    }

    pub fn get(&self, name: &str) -> Option<&Module> {
        self.modules.get(name)
    }

    /// Modules in stable name order.
    pub fn iter(&self) -> impl Iterator<Item = (&str, &Module)> {
        self.modules
            .iter()
            .map(|(name, module)| (name.as_str(), module))
    }

    pub fn names(&self) -> Vec<&str> {
        self.modules.keys().map(String::as_str).collect()
    }
}

impl ModuleRegistry {
    /// Evaluate a scene script with this library available through `module()`.
    pub fn eval_scene_ts(&self, ts_src: &str) -> Result<SceneDef, ScriptError> {
        let modules: Vec<ModuleSource> = self
            .iter()
            .map(|(name, module)| ModuleSource {
                name: name.to_string(),
                source_ts: module.source.clone(),
                defaults: module.meta.params.clone(),
            })
            .collect();
        eval_scene_def_ts_with_modules(ts_src, &modules)
    }

    /// Instantiate one module into a single [`NodeDef`], merging its defaults
    /// under the call-site props.
    pub fn instantiate(
        &self,
        name: &str,
        props: serde_json::Value,
    ) -> Result<NodeDef, ModuleError> {
        let module = self.get(name).ok_or_else(|| ModuleError::Invalid {
            module: name.to_string(),
            reason: format!(
                "unknown module (available: {})",
                if self.is_empty() {
                    "none".to_string()
                } else {
                    self.names().join(", ")
                }
            ),
        })?;

        let merged = match (module.meta.params.as_object(), props) {
            (Some(defaults), serde_json::Value::Object(overrides)) => {
                let mut merged = defaults.clone();
                merged.extend(overrides);
                serde_json::Value::Object(merged)
            }
            (Some(defaults), serde_json::Value::Null) => {
                serde_json::Value::Object(defaults.clone())
            }
            (_, value) => value,
        };

        eval_module_node_ts(&module.source, merged).map_err(|e| ModuleError::Script {
            module: name.to_string(),
            reason: e.to_string(),
        })
    }
}

fn load_module(path: &Path, name: &str) -> Result<Module, ModuleError> {
    let meta_path = path.join("module.json");
    let mut meta = if meta_path.exists() {
        let raw = std::fs::read_to_string(&meta_path).map_err(|e| ModuleError::Invalid {
            module: name.to_string(),
            reason: format!("module.json: {e}"),
        })?;
        serde_json::from_str::<ModuleMeta>(&raw).map_err(|e| ModuleError::Invalid {
            module: name.to_string(),
            reason: format!("module.json: {e}"),
        })?
    } else {
        ModuleMeta::default()
    };

    if !meta.name.is_empty() && meta.name != name {
        return Err(ModuleError::Invalid {
            module: name.to_string(),
            reason: format!(
                "module.json declares name {:?}, which does not match its directory",
                meta.name
            ),
        });
    }
    meta.name = name.to_string();

    let scene_path = ["scene.ts", "module.ts", "index.ts"]
        .iter()
        .map(|file| path.join(file))
        .find(|candidate| candidate.exists())
        .ok_or_else(|| ModuleError::Invalid {
            module: name.to_string(),
            reason: "missing scene.ts".to_string(),
        })?;

    let source = std::fs::read_to_string(&scene_path).map_err(|e| ModuleError::Invalid {
        module: name.to_string(),
        reason: format!("{}: {e}", scene_path.display()),
    })?;

    Ok(Module {
        meta,
        scene_path,
        source,
    })
}

fn validate_name(name: &str) -> Result<(), ModuleError> {
    let valid = !name.is_empty()
        && name
            .chars()
            .all(|c| MODULE_NAME_CHARS.contains(c.to_ascii_lowercase()))
        && !name.starts_with('.');
    if valid {
        Ok(())
    } else {
        Err(ModuleError::Invalid {
            module: name.to_string(),
            reason: "module names may only contain letters, digits, '-', '_' and '.'".to_string(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn scratch(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("kine-modules-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("scratch");
        dir
    }

    #[test]
    fn missing_modules_directory_is_an_empty_library() {
        let dir = scratch("missing");
        let registry = ModuleRegistry::discover(&dir).expect("discover");
        assert!(registry.is_empty());
        assert!(registry.root().is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn discovers_metadata_and_factory_source() {
        let dir = scratch("discover");
        let module = dir.join(MODULES_DIR).join("lower-third");
        fs::create_dir_all(&module).unwrap();
        fs::write(
            module.join("module.json"),
            r#"{"kind":"subtitle","description":"Lower third","params":{"title":"HELLO"}}"#,
        )
        .unwrap();
        fs::write(
            module.join("scene.ts"),
            "return (props) => container({}, []);",
        )
        .unwrap();

        let registry = ModuleRegistry::discover(&dir).expect("discover");
        assert_eq!(registry.len(), 1);
        let (name, found) = registry.iter().next().unwrap();
        assert_eq!(name, "lower-third");
        assert_eq!(found.meta.kind, "subtitle");
        assert_eq!(found.meta.params["title"], "HELLO");
        assert!(found.source.contains("return (props)"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_module_without_scene_is_rejected() {
        let dir = scratch("broken");
        let module = dir.join(MODULES_DIR).join("broken");
        fs::create_dir_all(&module).unwrap();
        fs::write(module.join("module.json"), "{}").unwrap();

        let err = ModuleRegistry::discover(&dir).expect_err("must fail");
        assert!(err.to_string().contains("missing scene.ts"), "{err}");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn instantiates_a_module_with_merged_defaults() {
        let dir = scratch("instantiate");
        let module = dir.join(MODULES_DIR).join("badge");
        fs::create_dir_all(&module).unwrap();
        fs::write(
            module.join("module.json"),
            r#"{"kind":"widget","params":{"title":"DEFAULT"}}"#,
        )
        .unwrap();
        fs::write(
            module.join("scene.ts"),
            r#"return (props) => text(props.title || "NONE", { fontSize: 12 });"#,
        )
        .unwrap();

        let registry = ModuleRegistry::discover(&dir).expect("discover");
        let node = registry
            .instantiate("badge", serde_json::json!({"title": "CALLER"}))
            .expect("instantiate");
        assert_eq!(node.content.as_deref(), Some("CALLER"));

        let defaulted = registry
            .instantiate("badge", serde_json::json!({}))
            .expect("defaults");
        assert_eq!(defaulted.content.as_deref(), Some("DEFAULT"));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn rejects_names_outside_the_safe_character_set() {
        assert!(validate_name("lower-third").is_ok());
        assert!(validate_name("3d_badge.v2").is_ok());
        assert!(validate_name("../escape").is_err());
        assert!(validate_name("with space").is_err());
    }
}
