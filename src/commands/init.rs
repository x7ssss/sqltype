use crate::cli::InitArgs;
use crate::commands::atomic_write;
use crate::config::{DriverConfig, DriverName, SqltypeConfig};
use colored::Colorize;
use std::fs;
use std::path::Path;

/// Detects the target PostgreSQL driver by inspecting package.json or lockfiles.
pub fn detect_driver(root: &Path) -> DriverName {
    let pkg_path = root.join("package.json");
    if let Some(val) = fs::read_to_string(&pkg_path)
        .ok()
        .and_then(|c| serde_json::from_str::<serde_json::Value>(&c).ok())
    {
        let mut all_deps = Vec::new();
        if let Some(deps) = val.get("dependencies").and_then(|d| d.as_object()) {
            all_deps.extend(deps.keys().cloned());
        }
        if let Some(dev_deps) = val.get("devDependencies").and_then(|d| d.as_object()) {
            all_deps.extend(dev_deps.keys().cloned());
        }

        if all_deps.iter().any(|d| d == "postgres") {
            return DriverName::PostgresJs;
        }
        if all_deps.iter().any(|d| d == "pg") {
            return DriverName::NodePg;
        }
        if all_deps
            .iter()
            .any(|d| d == "@types/bun" || d == "bun-types")
        {
            return DriverName::BunSql;
        }
    }

    if root.join("bun.lockb").exists() || root.join("bun.lock").exists() {
        return DriverName::BunSql;
    }

    DriverName::PostgresJs
}

pub fn run_init(args: &InitArgs) -> Result<(), String> {
    let root = &args.path;
    fs::create_dir_all(root)
        .map_err(|e| format!("Failed to create directory {}: {}", root.display(), e))?;

    let driver = if let Some(d) = &args.driver {
        match d.to_ascii_lowercase().as_str() {
            "postgres.js" | "postgres" | "postgresjs" => DriverName::PostgresJs,
            "pg" | "node-pg" | "nodepg" => DriverName::NodePg,
            "bun:sql" | "bun" | "bunsql" => DriverName::BunSql,
            other => {
                return Err(format!(
                    "Unknown driver '{}'. Supported drivers: postgres.js, pg, bun:sql",
                    other
                ));
            }
        }
    } else {
        detect_driver(root)
    };

    let is_json = args.format.eq_ignore_ascii_case("json");
    let config_filename = if is_json {
        "sqltype.json"
    } else {
        "sqltype.toml"
    };
    let config_path = root.join(config_filename);

    if config_path.exists() && !args.force {
        return Err(format!(
            "Configuration file already exists at {}. Use --force to overwrite.",
            config_path.display()
        ));
    }

    let config = SqltypeConfig {
        driver: DriverConfig { name: driver },
        ..Default::default()
    };

    let config_content = if is_json {
        config.to_json().map_err(|e| e.to_string())?
    } else {
        config.to_toml().map_err(|e| e.to_string())?
    };

    atomic_write(&config_path, &config_content)
        .map_err(|e| format!("Failed to write configuration file: {}", e))?;

    // Scaffold migrations
    let migrations_dir = root.join(&config.migrations.directory);
    fs::create_dir_all(&migrations_dir)
        .map_err(|e| format!("Failed to create migrations directory: {}", e))?;
    let demo_migration = migrations_dir.join("001_init.sql");
    if args.force || !demo_migration.exists() {
        let migration_sql = r#"-- Initial schema migration
CREATE TABLE users (
    id UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    email TEXT NOT NULL UNIQUE,
    name TEXT NOT NULL,
    tags TEXT[] NOT NULL DEFAULT '{}',
    created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
);
"#;
        atomic_write(&demo_migration, migration_sql)
            .map_err(|e| format!("Failed to write demo migration: {}", e))?;
    }

    // Scaffold queries
    let queries_dir = root.join("queries");
    fs::create_dir_all(&queries_dir)
        .map_err(|e| format!("Failed to create queries directory: {}", e))?;
    let demo_query = queries_dir.join("find_users_by_tag.sql");
    if args.force || !demo_query.exists() {
        let query_sql = r#"-- name: FindUsersByTag
SELECT id, email, name, tags, created_at
FROM users
WHERE tags && $1
ORDER BY created_at DESC;
"#;
        atomic_write(&demo_query, query_sql)
            .map_err(|e| format!("Failed to write demo query: {}", e))?;
    }

    println!(
        "{} Initialized sqltype project in {}:",
        "✓".green().bold(),
        root.display()
    );
    println!(
        "  {} Created {} (driver: {})",
        "✓".green(),
        config_filename,
        driver
    );
    println!("  {} Created {}", "✓".green(), demo_migration.display());
    println!("  {} Created {}", "✓".green(), demo_query.display());

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn test_detect_driver_from_package_json() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        // No package.json -> default to PostgresJs
        assert_eq!(detect_driver(root), DriverName::PostgresJs);

        // pg dependency
        fs::write(
            root.join("package.json"),
            r#"{"dependencies": {"pg": "^8.0.0"}}"#,
        )
        .unwrap();
        assert_eq!(detect_driver(root), DriverName::NodePg);

        // bun types in devDependencies
        fs::write(
            root.join("package.json"),
            r#"{"devDependencies": {"@types/bun": "latest"}}"#,
        )
        .unwrap();
        assert_eq!(detect_driver(root), DriverName::BunSql);

        // postgres dependency
        fs::write(
            root.join("package.json"),
            r#"{"dependencies": {"postgres": "^3.4.0"}}"#,
        )
        .unwrap();
        assert_eq!(detect_driver(root), DriverName::PostgresJs);
    }

    #[test]
    fn test_init_scaffolding() {
        let dir = tempdir().unwrap();
        let root = dir.path();

        let args = InitArgs {
            path: root.to_path_buf(),
            format: "toml".to_string(),
            driver: Some("pg".to_string()),
            force: false,
        };

        run_init(&args).unwrap();

        assert!(root.join("sqltype.toml").exists());
        assert!(root.join("migrations/001_init.sql").exists());
        assert!(root.join("queries/find_users_by_tag.sql").exists());

        let toml_content = fs::read_to_string(root.join("sqltype.toml")).unwrap();
        assert!(toml_content.contains("name = \"pg\""));

        // Running again without force should fail
        assert!(run_init(&args).is_err());

        // Running with force should succeed
        let mut force_args = args.clone();
        force_args.force = true;
        force_args.format = "json".to_string();
        run_init(&force_args).unwrap();
        assert!(root.join("sqltype.json").exists());
    }
}
