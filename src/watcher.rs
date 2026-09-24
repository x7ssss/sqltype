use crate::analyzer::analyze_query;
use crate::catalog::{Catalog, DriverTarget};
use crate::codegen::{CodegenOptions, generate_file_ts_with_options};
use crate::ts_scanner::{is_query_file, is_ts_js_file, scan_ts_queries};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::channel;
use std::time::{Duration, Instant};
use walkdir::WalkDir;

fn discover_query_files<P: AsRef<Path>>(dir: P) -> Result<Vec<PathBuf>, String> {
    let dir_path = dir.as_ref();
    if !dir_path.exists() {
        return Err(format!("Directory does not exist: {}", dir_path.display()));
    }

    let mut files = Vec::new();
    for entry in WalkDir::new(dir_path).follow_links(true) {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_file() && is_query_file(path) {
            files.push(path.to_path_buf());
        }
    }

    files.sort();
    Ok(files)
}

fn process_and_generate_query_file(
    q_path: &Path,
    abs_queries: &Path,
    abs_out: &Path,
    catalog: &Catalog,
    options: &CodegenOptions,
) -> Result<usize, String> {
    let content = std::fs::read_to_string(q_path)
        .map_err(|e| format!("Failed to read {}: {}", q_path.display(), e))?;

    let (analyzed_queries, is_ts) = if is_ts_js_file(q_path) {
        let extracted = scan_ts_queries(&content);
        if extracted.is_empty() {
            return Ok(0);
        }
        let mut list = Vec::new();
        for q in extracted {
            let fallback = q
                .name
                .as_deref()
                .or_else(|| q_path.file_stem().and_then(|s| s.to_str()));
            let analyzed = analyze_query(&q.sql, catalog, fallback).map_err(|e| {
                format!(
                    "{}:{}:{}: in query '{}': {}",
                    q_path.display(),
                    q.line,
                    q.column,
                    fallback.unwrap_or("Query"),
                    e
                )
            })?;
            list.push(analyzed);
        }
        (list, true)
    } else {
        let filename = q_path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("query.sql");
        let analyzed = analyze_query(&content, catalog, Some(filename))
            .map_err(|e| format!("{}: {}", q_path.display(), e))?;
        (vec![analyzed], false)
    };

    let count = analyzed_queries.len();
    let ts_code = generate_file_ts_with_options(&analyzed_queries, options);

    if is_ts {
        let sibling = q_path.with_extension("sqltype.ts");
        std::fs::write(&sibling, &ts_code)
            .map_err(|e| format!("Failed to write {}: {}", sibling.display(), e))?;

        let rel = q_path.strip_prefix(abs_queries).unwrap_or(q_path);
        let out_file = abs_out.join(rel).with_extension("sqltype.ts");
        if out_file != sibling {
            if let Some(parent) = out_file.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&out_file, &ts_code);
        }
    } else {
        let rel = q_path.strip_prefix(abs_queries).unwrap_or(q_path);
        let out_file = abs_out.join(rel).with_extension("ts");
        if let Some(parent) = out_file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&out_file, &ts_code)
            .map_err(|e| format!("Failed to write {}: {}", out_file.display(), e))?;
    }

    Ok(count)
}

/// Runs watch mode on migrations and queries directories with sub-5ms incremental re-generation.
pub fn run_watch(
    migrations_dir: &Path,
    queries_dir: &Path,
    out_dir: &Path,
    driver: DriverTarget,
    wrappers: bool,
) -> Result<(), String> {
    // Canonicalize paths for robust comparison
    let abs_migrations = migrations_dir
        .canonicalize()
        .map_err(|e| format!("Failed to canonicalize migrations dir: {}", e))?;
    let abs_queries = queries_dir
        .canonicalize()
        .map_err(|e| format!("Failed to canonicalize queries dir: {}", e))?;
    let abs_out = if out_dir.exists() {
        out_dir
            .canonicalize()
            .unwrap_or_else(|_| out_dir.to_path_buf())
    } else {
        std::fs::create_dir_all(out_dir).map_err(|e| e.to_string())?;
        out_dir
            .canonicalize()
            .unwrap_or_else(|_| out_dir.to_path_buf())
    };

    println!("\n👀 Watching for changes in:");
    println!("   Migrations: {}", abs_migrations.display());
    println!("   Queries:    {}", abs_queries.display());
    println!("   Output:     {}", abs_out.display());
    println!("   Driver:     {}", driver);
    println!("   (Press Ctrl+C to stop)\n");

    let mut catalog = Catalog::load_from_dir_with_driver(&abs_migrations, driver)?;

    let (tx, rx) = channel();
    let mut watcher = RecommendedWatcher::new(tx, Config::default()).map_err(|e| e.to_string())?;

    watcher
        .watch(&abs_migrations, RecursiveMode::Recursive)
        .map_err(|e| format!("Failed to watch migrations: {}", e))?;
    watcher
        .watch(&abs_queries, RecursiveMode::Recursive)
        .map_err(|e| format!("Failed to watch queries: {}", e))?;

    while let Ok(res) = rx.recv() {
        // Collect and debounce events within a small 15ms window
        let mut events: Vec<Event> = Vec::new();
        if let Ok(event) = res {
            events.push(event);
        }

        std::thread::sleep(Duration::from_millis(15));
        while let Ok(Ok(event)) = rx.try_recv() {
            events.push(event);
        }

        let mut changed_migrations = false;
        let mut changed_query_paths: HashSet<PathBuf> = HashSet::new();

        for event in events {
            match event.kind {
                EventKind::Create(_) | EventKind::Modify(_) => {
                    for path in event.paths {
                        if path.starts_with(&abs_migrations) {
                            if let Some(ext) = path.extension()
                                && ext.eq_ignore_ascii_case("sql")
                            {
                                changed_migrations = true;
                            }
                        } else if path.starts_with(&abs_queries) && is_query_file(&path) {
                            changed_query_paths.insert(path);
                        }
                    }
                }
                _ => {}
            }
        }

        // If a migration changed, reload catalog and re-generate all queries
        if changed_migrations {
            let start = Instant::now();
            println!("⚡ [Watch] Migration change detected. Rebuilding schema catalog...");
            match Catalog::load_from_dir_with_driver(&abs_migrations, driver) {
                Ok(new_catalog) => {
                    catalog = new_catalog;
                    let query_files = discover_query_files(&abs_queries).unwrap_or_default();
                    let mut success_count = 0;
                    let options = CodegenOptions::new(driver, wrappers);
                    for q_path in &query_files {
                        if let Ok(count) = process_and_generate_query_file(
                            q_path,
                            &abs_queries,
                            &abs_out,
                            &catalog,
                            &options,
                        ) {
                            success_count += count;
                        }
                    }
                    let elapsed = start.elapsed();
                    println!(
                        "  ✓ Catalog rebuilt and {} queries re-generated in {:.2}ms\n",
                        success_count,
                        elapsed.as_secs_f64() * 1000.0
                    );
                }
                Err(e) => {
                    eprintln!("  ✗ [Error] Failed to rebuild migrations: {}\n", e);
                }
            }
        } else if !changed_query_paths.is_empty() {
            // Incremental sub-5ms query re-generation
            let options = CodegenOptions::new(driver, wrappers);
            for q_path in changed_query_paths {
                let start = Instant::now();
                if !q_path.exists() {
                    continue;
                }

                match process_and_generate_query_file(
                    &q_path,
                    &abs_queries,
                    &abs_out,
                    &catalog,
                    &options,
                ) {
                    Ok(count) => {
                        let elapsed = start.elapsed();
                        println!(
                            "⚡ [Watch] {} queries from {} updated in {:.2}ms",
                            count,
                            q_path.display(),
                            elapsed.as_secs_f64() * 1000.0
                        );
                    }
                    Err(e) => {
                        eprintln!("  ✗ [Error] {}", e);
                    }
                }
            }
        }
    }

    Ok(())
}
