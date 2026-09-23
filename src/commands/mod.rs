pub mod check;
pub mod generate;
pub mod init;

use crate::config::SqltypeConfig;
use crate::ts_scanner::{is_query_file, is_ts_js_file};
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Atomically writes content to `path` using `tempfile::NamedTempFile` to avoid corrupted partial writes.
pub fn atomic_write(path: &Path, content: &str) -> std::io::Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let mut temp_file = tempfile::NamedTempFile::new_in(parent)?;
    temp_file.write_all(content.as_bytes())?;
    temp_file.persist(path).map_err(|e| e.error)?;
    Ok(())
}

/// Resolves migrations directory from CLI override or configuration.
pub fn resolve_migrations_dir(
    root: &Path,
    config: &SqltypeConfig,
    cli_migrations: Option<&Path>,
) -> PathBuf {
    if let Some(m) = cli_migrations {
        if m.is_relative() {
            root.join(m)
        } else {
            m.to_path_buf()
        }
    } else {
        let dir = Path::new(&config.migrations.directory);
        if dir.is_relative() {
            root.join(dir)
        } else {
            dir.to_path_buf()
        }
    }
}

/// Discovers query files (.sql and inline TS/JS) based on CLI override or configuration.
pub fn discover_query_files(
    root: &Path,
    config: &SqltypeConfig,
    cli_queries: Option<&Path>,
) -> Result<Vec<PathBuf>, String> {
    if let Some(queries_path) = cli_queries {
        let target = if queries_path.is_relative() {
            root.join(queries_path)
        } else {
            queries_path.to_path_buf()
        };

        if !target.exists() {
            return Err(format!("Query path does not exist: {}", target.display()));
        }

        if target.is_file() {
            return Ok(vec![target]);
        }

        let mut files = Vec::new();
        for entry in WalkDir::new(&target)
            .follow_links(true)
            .into_iter()
            .filter_entry(|e| !is_ignored_entry(e))
        {
            let entry = entry.map_err(|e| e.to_string())?;
            let path = entry.path();
            if path.is_file() && is_query_file(path) {
                if !config.queries.inline_ts && is_ts_js_file(path) {
                    continue;
                }
                files.push(path.to_path_buf());
            }
        }
        files.sort();
        files.dedup();
        return Ok(files);
    }

    let mut files = Vec::new();

    // Check each pattern/path from config.queries.sql_files
    for pattern in &config.queries.sql_files {
        let base_dir = extract_base_dir(pattern);
        let target_dir = root.join(base_dir);
        if target_dir.exists() {
            if target_dir.is_file() {
                if is_query_file(&target_dir) {
                    files.push(target_dir);
                }
            } else {
                for entry in WalkDir::new(&target_dir)
                    .follow_links(true)
                    .into_iter()
                    .filter_entry(|e| !is_ignored_entry(e))
                    .flatten()
                {
                    let path = entry.path();
                    if path.is_file() && is_query_file(path) {
                        files.push(path.to_path_buf());
                    }
                }
            }
        }
    }

    // If inline_ts is enabled, also scan for TypeScript/JavaScript files in queries/ and src/
    if config.queries.inline_ts {
        let mut searched_specific = false;
        for candidate_sub in &["src", "queries"] {
            let candidate_dir = root.join(candidate_sub);
            if candidate_dir.exists() {
                searched_specific = true;
                for entry in WalkDir::new(&candidate_dir)
                    .follow_links(true)
                    .into_iter()
                    .filter_entry(|e| !is_ignored_entry(e))
                    .flatten()
                {
                    let path = entry.path();
                    if path.is_file() && is_ts_js_file(path) && is_query_file(path) {
                        files.push(path.to_path_buf());
                    }
                }
            }
        }
        if !searched_specific {
            for entry in WalkDir::new(root)
                .follow_links(true)
                .into_iter()
                .filter_entry(|e| !is_ignored_entry(e))
                .flatten()
            {
                let path = entry.path();
                if path.is_file() && is_ts_js_file(path) && is_query_file(path) {
                    files.push(path.to_path_buf());
                }
            }
        }
    }

    files.sort();
    files.dedup();
    Ok(files)
}

fn is_ignored_entry(entry: &walkdir::DirEntry) -> bool {
    if entry.depth() > 0 && entry.file_type().is_dir() {
        let name = entry.file_name().to_string_lossy();
        name == "node_modules"
            || name == "target"
            || name == "dist"
            || name == ".git"
            || name == ".turbo"
            || name == ".next"
    } else {
        false
    }
}

fn extract_base_dir(pattern: &str) -> PathBuf {
    let mut parts = Vec::new();
    for part in pattern.split(['/', '\\']) {
        if part.contains('*') || part.contains('?') {
            break;
        }
        parts.push(part);
    }
    if parts.is_empty() {
        PathBuf::from(".")
    } else {
        parts.iter().collect()
    }
}
