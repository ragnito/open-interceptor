//! Configuration persistence service.
//!
//! Handles I/O: reading YAML from disk, expanding `${ENV_VAR}` placeholders,
//! serializing back to YAML, and delegating validation to the domain layer.
//!
//! The domain types (`domain::config::*`) are pure — no I/O, no YAML parsing.

use std::path::{Path, PathBuf};

use crate::domain::config::{Config, ConfigValidationError};

/// Errors that can occur when loading or saving configuration.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },

    #[error("env var expansion failed: {0}")]
    EnvExpansion(String),

    #[error("YAML parse error: {0}")]
    Parse(#[from] serde_yml::Error),

    #[error("{0}")]
    Validation(#[from] ConfigValidationError),
}

/// Service for loading and saving configuration from/to disk.
pub struct ConfigService;

impl ConfigService {
    /// Read and validate a config file from disk.
    ///
    /// 1. Read the file as UTF-8.
    /// 2. Parse as YAML into a generic `Value` tree.
    /// 3. Walk that tree and expand `${ENV_VAR}` placeholders only inside
    ///    string scalars. This deliberately skips comments (already
    ///    stripped by the parser) and YAML keys, so e.g. a comment
    ///    documenting "use `${VAR}` syntax for api keys" doesn't break
    ///    the loader.
    /// 4. Deserialize the (now-expanded) tree into `Config`.
    /// 5. Validate that every `route.provider` references a known
    ///    provider in `providers` and that no route has empty patterns.
    pub fn load(path: &Path) -> Result<Config, ConfigError> {
        let raw = std::fs::read_to_string(path).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;

        let mut value: serde_yml::Value = serde_yml::from_str(&raw)?;
        expand_env_in_strings(&mut value)?;

        let config: Config = serde_yml::from_value(value)?;
        config.validate()?;
        Ok(config)
    }

    /// Serialize a `Config` to YAML and write it to disk.
    #[allow(dead_code)]
    pub fn save(config: &Config, path: &Path) -> Result<(), ConfigError> {
        let yaml = serde_yml::to_string(config).map_err(|e| ConfigError::EnvExpansion(format!("serialization failed: {e}")))?;
        let dir = path.parent().expect("config path must have a parent");
        std::fs::create_dir_all(dir).map_err(|source| ConfigError::Io {
            path: dir.to_path_buf(),
            source,
        })?;
        std::fs::write(path, yaml.as_bytes()).map_err(|source| ConfigError::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Ok(())
    }

    /// Validate a config without loading or saving.
    #[allow(dead_code)]
    pub fn validate(path: &Path) -> Result<Config, ConfigError> {
        Self::load(path)
    }
}

/// Walk a YAML `Value` tree and expand `${ENV_VAR}` inside every string
/// scalar (mapping values, sequence items, nested structures). Mapping
/// keys are intentionally left alone — they're identifiers, not config.
fn expand_env_in_strings(value: &mut serde_yml::Value) -> Result<(), ConfigError> {
    use serde_yml::Value;
    match value {
        Value::String(s) => {
            let expanded =
                shellexpand::env(s).map_err(|e| ConfigError::EnvExpansion(e.to_string()))?;
            // Avoid allocating when nothing changed.
            if expanded.as_ref() != s.as_str() {
                *s = expanded.into_owned();
            }
        }
        Value::Sequence(items) => {
            for item in items {
                expand_env_in_strings(item)?;
            }
        }
        Value::Mapping(map) => {
            for (_k, v) in map.iter_mut() {
                expand_env_in_strings(v)?;
            }
        }
        // Null, Bool, Number, Tagged — no strings to expand.
        _ => {}
    }
    Ok(())
}

