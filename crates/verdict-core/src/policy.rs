//! Policy loading: built-in default, then the global `~/.verdict/policy.json`, then the project's
//! `<repo>/verdict.json` on top. Every file is validated against `policy/schema.json`
//! (embedded in the binary), and a missing file simply falls back to the layer below.

use serde_json::Value;
use std::fmt;
use std::path::{Path, PathBuf};

/// The schema every policy file must satisfy.
pub const SCHEMA_JSON: &str = include_str!("../../../policy/schema.json");
/// The built-in default policy, used when no file exists.
pub const DEFAULT_JSON: &str = include_str!("../../../policy/default.json");

/// Where a layer of the policy came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicySource {
    /// The default embedded in the binary.
    BuiltIn,
    /// `~/.verdict/policy.json`.
    Global(PathBuf),
    /// `<repo>/verdict.json`.
    Project(PathBuf),
    /// A file named on the command line for this one run (`vdt --policy <file>`).
    RunTime(PathBuf),
}

/// The file named by `--policy` for this process, if any. It is merged last, after the project
/// file, for this run only; nothing is written.
static RUNTIME_OVERRIDE: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// Set the one-run policy override (the `--policy` flag). Idempotent; the first value wins.
pub fn set_runtime_override(path: PathBuf) {
    let _ = RUNTIME_OVERRIDE.set(path);
}

/// The one-run policy override, if one was set.
pub fn runtime_override() -> Option<&'static std::path::Path> {
    RUNTIME_OVERRIDE.get().map(|p| p.as_path())
}

/// A policy file problem.
#[derive(Debug)]
pub enum PolicyError {
    /// A file could not be read.
    Io(PathBuf, std::io::Error),
    /// A file is not valid JSON.
    Json(PathBuf, serde_json::Error),
    /// A file does not satisfy the schema.
    Invalid(PathBuf, Vec<String>),
    /// The embedded schema or default is broken (a bug in this build).
    Internal(String),
}

impl fmt::Display for PolicyError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PolicyError::Io(p, e) => write!(f, "cannot read policy file {}: {e}", p.display()),
            PolicyError::Json(p, e) => write!(f, "policy file {} is not JSON: {e}", p.display()),
            PolicyError::Invalid(p, errs) => write!(
                f,
                "policy file {} does not match policy/schema.json: {}",
                p.display(),
                errs.join("; ")
            ),
            PolicyError::Internal(m) => write!(f, "built-in policy problem: {m}"),
        }
    }
}

impl std::error::Error for PolicyError {}

/// The effective policy.
#[derive(Debug, Clone, PartialEq)]
pub struct Policy {
    /// The merged policy document.
    pub value: Value,
    /// Its `version` field.
    pub version: u32,
    /// blake3 of the merged document in a canonical form (sorted keys).
    pub hash: String,
    /// The layers that contributed, lowest first.
    pub sources: Vec<PolicySource>,
}

impl Policy {
    /// The built-in default policy.
    pub fn builtin() -> Result<Policy, PolicyError> {
        let value: Value = serde_json::from_str(DEFAULT_JSON)
            .map_err(|e| PolicyError::Internal(format!("default.json: {e}")))?;
        validate(Path::new("policy/default.json"), &value)?;
        Ok(Policy::from_value(value, vec![PolicySource::BuiltIn]))
    }

    /// Load the effective policy for the project at `project_root`.
    ///
    /// `home` is the folder that holds `.verdict/policy.json`; pass `None` to use the user's home
    /// folder (`USERPROFILE`, then `HOME`).
    pub fn load(project_root: &Path, home: Option<&Path>) -> Result<Policy, PolicyError> {
        let mut policy = Policy::builtin()?;
        let home = home.map(Path::to_path_buf).or_else(home_dir);
        let mut layers: Vec<(PolicySource, PathBuf)> = Vec::new();
        if let Some(home) = home {
            let p = home.join(".verdict").join("policy.json");
            layers.push((PolicySource::Global(p.clone()), p));
        }
        let p = project_root.join("verdict.json");
        layers.push((PolicySource::Project(p.clone()), p));
        if let Some(p) = runtime_override() {
            layers.push((PolicySource::RunTime(p.to_path_buf()), p.to_path_buf()));
        }

        let mut value = policy.value.clone();
        let mut sources = policy.sources.clone();
        for (source, path) in layers {
            if !path.is_file() {
                continue;
            }
            let text =
                std::fs::read_to_string(&path).map_err(|e| PolicyError::Io(path.clone(), e))?;
            let layer: Value =
                serde_json::from_str(&text).map_err(|e| PolicyError::Json(path.clone(), e))?;
            validate(&path, &layer)?;
            merge(&mut value, layer);
            sources.push(source);
        }
        validate(Path::new("merged policy"), &value)?;
        policy = Policy::from_value(value, sources);
        Ok(policy)
    }

    fn from_value(value: Value, sources: Vec<PolicySource>) -> Policy {
        let version = value.get("version").and_then(Value::as_u64).unwrap_or(1) as u32;
        // serde_json's default map is ordered by key, so this is canonical.
        let canonical = serde_json::to_string(&value).unwrap_or_default();
        Policy {
            hash: blake3::hash(canonical.as_bytes()).to_hex().to_string(),
            value,
            version,
            sources,
        }
    }

    /// `gate.mode` (`shadow` unless the policy says `enforce`).
    pub fn gate_mode(&self) -> &str {
        self.value
            .pointer("/gate/mode")
            .and_then(Value::as_str)
            .unwrap_or("shadow")
    }
}

/// The user's home folder, from `USERPROFILE` (Windows) or `HOME`.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .filter(|v| !v.is_empty())
        .or_else(|| std::env::var_os("HOME").filter(|v| !v.is_empty()))
        .map(PathBuf::from)
}

/// Validate `doc` against the embedded schema.
pub fn validate(origin: &Path, doc: &Value) -> Result<(), PolicyError> {
    let schema: Value = serde_json::from_str(SCHEMA_JSON)
        .map_err(|e| PolicyError::Internal(format!("schema.json: {e}")))?;
    let validator = jsonschema::validator_for(&schema)
        .map_err(|e| PolicyError::Internal(format!("schema.json: {e}")))?;
    let errors: Vec<String> = validator
        .iter_errors(doc)
        .map(|e| format!("{e} (at {})", e.instance_path()))
        .collect();
    if errors.is_empty() {
        Ok(())
    } else {
        Err(PolicyError::Invalid(origin.to_path_buf(), errors))
    }
}

/// Objects merge key by key; anything else (arrays, scalars) is replaced by the upper layer.
fn merge(base: &mut Value, over: Value) {
    match (base, over) {
        (Value::Object(b), Value::Object(o)) => {
            for (k, v) in o {
                match b.get_mut(&k) {
                    Some(slot) => merge(slot, v),
                    None => {
                        b.insert(k, v);
                    }
                }
            }
        }
        (slot, v) => *slot = v,
    }
}
