use crate::catalog::DriverTarget;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Default, Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct SqltypeConfig {
    pub migrations: MigrationsConfig,
    pub queries: QueriesConfig,
    pub driver: DriverConfig,
    pub output: OutputConfig,
    pub types: TypesConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct MigrationsConfig {
    pub directory: String,
    pub pattern: String,
}

impl Default for MigrationsConfig {
    fn default() -> Self {
        Self {
            directory: "migrations".to_string(),
            pattern: "*.sql".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct QueriesConfig {
    pub sql_files: Vec<String>,
    pub inline_ts: bool,
}

impl Default for QueriesConfig {
    fn default() -> Self {
        Self {
            sql_files: vec!["queries/**/*.sql".to_string()],
            inline_ts: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct DriverConfig {
    pub name: DriverName,
}

impl Default for DriverConfig {
    fn default() -> Self {
        Self {
            name: DriverName::PostgresJs,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DriverName {
    #[serde(rename = "postgres.js", alias = "postgres", alias = "postgresjs")]
    PostgresJs,
    #[serde(rename = "pg", alias = "node-pg", alias = "nodepg")]
    NodePg,
    #[serde(rename = "bun:sql", alias = "bun", alias = "bunsql")]
    BunSql,
}

impl fmt::Display for DriverName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DriverName::PostgresJs => write!(f, "postgres.js"),
            DriverName::NodePg => write!(f, "pg"),
            DriverName::BunSql => write!(f, "bun:sql"),
        }
    }
}

impl From<DriverName> for DriverTarget {
    fn from(name: DriverName) -> Self {
        match name {
            DriverName::PostgresJs => DriverTarget::Postgres,
            DriverName::NodePg => DriverTarget::Pg,
            DriverName::BunSql => DriverTarget::Bun,
        }
    }
}

impl From<DriverTarget> for DriverName {
    fn from(target: DriverTarget) -> Self {
        match target {
            DriverTarget::Postgres => DriverName::PostgresJs,
            DriverTarget::Pg => DriverName::NodePg,
            DriverTarget::Bun => DriverName::BunSql,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct OutputConfig {
    pub mode: OutputMode,
    pub file_path: Option<String>,
    pub declaration_only: bool,
}

impl Default for OutputConfig {
    fn default() -> Self {
        Self {
            mode: OutputMode::Companion,
            file_path: None,
            declaration_only: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum OutputMode {
    Companion,
    Centralized,
}

impl fmt::Display for OutputMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            OutputMode::Companion => write!(f, "companion"),
            OutputMode::Centralized => write!(f, "centralized"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct TypesConfig {
    pub emit_enums: bool,
    pub strict_null_checks: bool,
    pub overrides: HashMap<String, String>,
}

impl Default for TypesConfig {
    fn default() -> Self {
        Self {
            emit_enums: true,
            strict_null_checks: true,
            overrides: HashMap::new(),
        }
    }
}

#[derive(Debug)]
pub enum ConfigError {
    NotFound(PathBuf),
    Io(std::io::Error),
    Toml(toml::de::Error),
    Json(serde_json::Error),
}

impl fmt::Display for ConfigError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ConfigError::NotFound(p) => {
                write!(f, "Configuration file not found in {}", p.display())
            }
            ConfigError::Io(e) => write!(f, "IO error reading config: {}", e),
            ConfigError::Toml(e) => write!(f, "TOML error parsing config: {}", e),
            ConfigError::Json(e) => write!(f, "JSON error parsing config: {}", e),
        }
    }
}

impl std::error::Error for ConfigError {}

impl From<std::io::Error> for ConfigError {
    fn from(e: std::io::Error) -> Self {
        ConfigError::Io(e)
    }
}

impl From<toml::de::Error> for ConfigError {
    fn from(e: toml::de::Error) -> Self {
        ConfigError::Toml(e)
    }
}

impl From<serde_json::Error> for ConfigError {
    fn from(e: serde_json::Error) -> Self {
        ConfigError::Json(e)
    }
}

impl SqltypeConfig {
    pub fn discover(root: &Path, explicit: Option<&Path>) -> Result<(Self, PathBuf), ConfigError> {
        if let Some(explicit_path) = explicit {
            let path = if explicit_path.is_relative() {
                root.join(explicit_path)
            } else {
                explicit_path.to_path_buf()
            };
            if !path.exists() {
                return Err(ConfigError::NotFound(path));
            }
            let content = fs::read_to_string(&path)?;
            if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
                let config: Self = serde_json::from_str(&content)?;
                return Ok((config, path));
            } else {
                let config: Self = toml::from_str(&content)?;
                return Ok((config, path));
            }
        }

        let toml_path = root.join("sqltype.toml");
        if toml_path.is_file() {
            let content = fs::read_to_string(&toml_path)?;
            let config: Self = toml::from_str(&content)?;
            return Ok((config, toml_path));
        }

        let json_path = root.join("sqltype.json");
        if json_path.is_file() {
            let content = fs::read_to_string(&json_path)?;
            let config: Self = serde_json::from_str(&content)?;
            return Ok((config, json_path));
        }

        Err(ConfigError::NotFound(root.to_path_buf()))
    }

    pub fn discover_or_default(root: &Path, explicit: Option<&Path>) -> (Self, Option<PathBuf>) {
        match Self::discover(root, explicit) {
            Ok((config, path)) => (config, Some(path)),
            Err(_) => (Self::default(), None),
        }
    }

    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string_pretty(self)
    }

    pub fn to_json(&self) -> Result<String, serde_json::Error> {
        serde_json::to_string_pretty(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_config_defaults_and_serialization() {
        let config = SqltypeConfig::default();
        let toml_str = config.to_toml().unwrap();
        assert!(toml_str.contains("directory = \"migrations\""));
        assert!(toml_str.contains("name = \"postgres.js\""));

        let deserialized_toml: SqltypeConfig = toml::from_str(&toml_str).unwrap();
        assert_eq!(config, deserialized_toml);

        let json_str = config.to_json().unwrap();
        assert!(json_str.contains("\"directory\": \"migrations\""));
        let deserialized_json: SqltypeConfig = serde_json::from_str(&json_str).unwrap();
        assert_eq!(config, deserialized_json);
    }

    #[test]
    fn test_driver_name_aliases() {
        let toml1 = r#"
            [driver]
            name = "postgres"
        "#;
        let c1: SqltypeConfig = toml::from_str(toml1).unwrap();
        assert_eq!(c1.driver.name, DriverName::PostgresJs);

        let toml2 = r#"
            [driver]
            name = "pg"
        "#;
        let c2: SqltypeConfig = toml::from_str(toml2).unwrap();
        assert_eq!(c2.driver.name, DriverName::NodePg);

        let toml3 = r#"
            [driver]
            name = "bun:sql"
        "#;
        let c3: SqltypeConfig = toml::from_str(toml3).unwrap();
        assert_eq!(c3.driver.name, DriverName::BunSql);
    }

    #[test]
    fn test_config_discovery() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        // 1. None found
        assert!(SqltypeConfig::discover(root, None).is_err());

        // 2. Discover sqltype.toml
        let toml_path = root.join("sqltype.toml");
        fs::write(&toml_path, "[migrations]\ndirectory = \"custom_mig\"\n").unwrap();
        let (discovered_toml, p) = SqltypeConfig::discover(root, None).unwrap();
        assert_eq!(p, toml_path);
        assert_eq!(discovered_toml.migrations.directory, "custom_mig");

        // 3. Discover sqltype.json if toml absent
        fs::remove_file(&toml_path).unwrap();
        let json_path = root.join("sqltype.json");
        fs::write(&json_path, r#"{"migrations": {"directory": "json_mig"}}"#).unwrap();
        let (discovered_json, p2) = SqltypeConfig::discover(root, None).unwrap();
        assert_eq!(p2, json_path);
        assert_eq!(discovered_json.migrations.directory, "json_mig");

        // 4. Explicit path
        let explicit = root.join("custom.config.toml");
        fs::write(&explicit, "[migrations]\ndirectory = \"explicit_mig\"\n").unwrap();
        let (discovered_exp, p3) = SqltypeConfig::discover(root, Some(&explicit)).unwrap();
        assert_eq!(p3, explicit);
        assert_eq!(discovered_exp.migrations.directory, "explicit_mig");
    }
}
