use crate::analyzer::analyze_query;
use crate::catalog::{Catalog, DriverTarget, SchemaCatalog};
use crate::cli::GenerateArgs;
use crate::codegen::{CodegenOptions, generate_file_ts_with_options};
use crate::commands::check::print_rustc_diagnostic;
use crate::commands::{atomic_write, discover_query_files, resolve_migrations_dir};
use crate::config::SqltypeConfig;
use crate::ts_scanner::{is_ts_js_file, scan_ts_queries};
use colored::Colorize;
use notify::{EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::hash_map::DefaultHasher;
use std::collections::{HashMap, HashSet};
use std::fs;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

/// Incremental compilation state holding the thread-safe schema catalog and 64-bit query hashes.
pub struct IncrementalState {
    pub catalog: Arc<RwLock<SchemaCatalog>>,
    pub query_cache: HashMap<PathBuf, u64>,
}

impl IncrementalState {
    pub fn new(catalog: SchemaCatalog) -> Self {
        Self {
            catalog: Arc::new(RwLock::new(catalog)),
            query_cache: HashMap::new(),
        }
    }
}

/// Fast 64-bit content hashing to skip redundant codegen when files are saved without content changes.
pub fn compute_hash(content: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    content.hash(&mut hasher);
    hasher.finish()
}

/// Filters out `.tmp` files, generated `.sql.ts` / companion files, and metadata-only files.
pub fn should_filter_path(path: &Path) -> bool {
    let file_name = match path.file_name().and_then(|f| f.to_str()) {
        Some(name) => name.to_ascii_lowercase(),
        None => return true,
    };

    // Filter out .tmp files and temporary editor swap files
    if file_name.ends_with(".tmp")
        || file_name.starts_with(".tmp")
        || file_name.contains(".tmp.")
        || file_name.ends_with('~')
        || file_name.starts_with('~')
    {
        return true;
    }

    if let Some(ext) = path.extension().and_then(|e| e.to_str())
        && ext.eq_ignore_ascii_case("tmp")
    {
        return true;
    }

    // Filter out generated companion files (.sql.ts, .sqltype.ts, .generated.ts, .d.ts)
    if file_name.ends_with(".sql.ts")
        || file_name.ends_with(".sqltype.ts")
        || file_name.ends_with(".generated.ts")
        || file_name.ends_with(".d.ts")
    {
        return true;
    }

    false
}

/// Classification of file system changes for incremental compilation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileChange {
    /// Changes inside migrations directory: rebuilds schema and triggers full query sweep.
    FullRebuild,
    /// Changes to .sql, .ts, or .tsx query files: recompiles only that specific query.
    PartialQuery(PathBuf),
    /// Non-SQL, non-TS files.
    Ignored,
}

/// Classifies a path based on its location and file extension.
pub fn classify_path(path: &Path, migrations_dir: &Path) -> FileChange {
    if path.starts_with(migrations_dir) {
        if let Some(ext) = path.extension().and_then(|e| e.to_str())
            && ext.eq_ignore_ascii_case("sql")
        {
            return FileChange::FullRebuild;
        }
        return FileChange::Ignored;
    }

    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let ext_lower = ext.to_ascii_lowercase();
        if ext_lower == "sql" || ext_lower == "ts" || ext_lower == "tsx" {
            return FileChange::PartialQuery(path.to_path_buf());
        }
    }

    FileChange::Ignored
}

/// Normalizes path by stripping Windows UNC prefix (`\\?\`) for robust path comparisons.
pub fn normalize_path(path: &Path) -> PathBuf {
    if let Ok(c) = path.canonicalize() {
        let s = c.to_string_lossy();
        if let Some(stripped) = s.strip_prefix(r"\\?\") {
            PathBuf::from(stripped)
        } else {
            c
        }
    } else {
        let s = path.to_string_lossy();
        if let Some(stripped) = s.strip_prefix(r"\\?\") {
            PathBuf::from(stripped)
        } else {
            path.to_path_buf()
        }
    }
}

pub fn current_time_str() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

/// Recompiles a single query file and updates its companion `.sql.ts`.
fn recompile_single_query(
    q_path: &Path,
    state: &mut IncrementalState,
    _config: &SqltypeConfig,
    driver: DriverTarget,
    wrappers: bool,
    root: &Path,
) {
    let companion = q_path.with_extension("sql.ts");

    if !q_path.exists() {
        state.query_cache.remove(q_path);
        if companion.exists() {
            let _ = fs::remove_file(&companion);
        }
        if is_ts_js_file(q_path) {
            let sibling = q_path.with_extension("sqltype.ts");
            if sibling.exists() {
                let _ = fs::remove_file(&sibling);
            }
        }
        let ts = current_time_str();
        let rel_display = q_path
            .strip_prefix(root)
            .unwrap_or(q_path)
            .to_string_lossy()
            .replace('\\', "/");
        println!("[{}] Removed companion for {}", ts, rel_display);
        return;
    }

    let content = match fs::read_to_string(q_path) {
        Ok(c) => c,
        Err(e) => {
            let ts = current_time_str();
            eprintln!(
                "[{}] {}: {}",
                ts,
                "Failed to read query file".red().bold(),
                e
            );
            return;
        }
    };

    let hash = compute_hash(&content);
    if state.query_cache.get(q_path).copied() == Some(hash) {
        // Redundant save without content changes: skip codegen
        return;
    }

    let start = Instant::now();
    let cat_guard = match state.catalog.read() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };

    let analyzed_queries = if is_ts_js_file(q_path) {
        let extracted = scan_ts_queries(&content);
        if extracted.is_empty() {
            state.query_cache.insert(q_path.to_path_buf(), hash);
            return;
        }
        let mut list = Vec::new();
        let mut has_err = false;
        for q in extracted {
            let fallback = q
                .name
                .as_deref()
                .or_else(|| q_path.file_stem().and_then(|s| s.to_str()));
            match analyze_query(&q.sql, &cat_guard, fallback) {
                Ok(analyzed) => list.push(analyzed),
                Err(e) => {
                    let ts = current_time_str();
                    eprintln!("[{}] {}", ts, "error: in inline query".red().bold());
                    print_rustc_diagnostic(q_path, q.line, q.column, fallback, &e, &content);
                    has_err = true;
                }
            }
        }
        if has_err {
            return;
        }
        list
    } else {
        let filename = q_path
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("query.sql");
        match analyze_query(&content, &cat_guard, Some(filename)) {
            Ok(analyzed) => vec![analyzed],
            Err(e) => {
                let ts = current_time_str();
                eprintln!("[{}] {}", ts, "error: in query".red().bold());
                print_rustc_diagnostic(q_path, 1, 1, Some(filename), &e, &content);
                return;
            }
        }
    };

    let options = CodegenOptions::new(driver, wrappers);
    let ts_code = generate_file_ts_with_options(&analyzed_queries, &options);

    if let Err(e) = atomic_write(&companion, &ts_code) {
        let ts = current_time_str();
        eprintln!(
            "[{}] {}: {}",
            ts,
            format!("Failed to write companion file {}", companion.display())
                .red()
                .bold(),
            e
        );
        return;
    }

    if is_ts_js_file(q_path) {
        let sibling = q_path.with_extension("sqltype.ts");
        let _ = atomic_write(&sibling, &ts_code);
    } else {
        // Also update standard .ts companion if it exists
        let ts_file = q_path.with_extension("ts");
        if ts_file.exists() && ts_file != companion {
            let _ = atomic_write(&ts_file, &ts_code);
        }
    }

    state.query_cache.insert(q_path.to_path_buf(), hash);

    let elapsed = start.elapsed().as_secs_f64() * 1000.0;
    let ts = current_time_str();
    let rel_companion = companion
        .strip_prefix(root)
        .unwrap_or(&companion)
        .to_string_lossy()
        .replace('\\', "/");

    println!(
        "[{}] Updated {} in {:.2}ms",
        ts.cyan(),
        rel_companion.bold(),
        elapsed
    );
}

/// Rebuilds in-memory schema catalog from migrations and triggers full query sweep.
fn execute_full_rebuild(
    abs_migrations: &Path,
    driver: DriverTarget,
    wrappers: bool,
    state: &mut IncrementalState,
    root: &Path,
    config: &SqltypeConfig,
    args: &GenerateArgs,
) {
    let start = Instant::now();
    match Catalog::load_from_dir_with_driver(abs_migrations, driver) {
        Ok(new_catalog) => {
            {
                let mut cat_lock = match state.catalog.write() {
                    Ok(g) => g,
                    Err(poisoned) => poisoned.into_inner(),
                };
                *cat_lock = new_catalog;
            }

            let elapsed = start.elapsed().as_secs_f64() * 1000.0;
            let ts = current_time_str();
            println!(
                "[{}] Rebuilt schema from migrations in {:.2}ms",
                ts.cyan(),
                elapsed
            );

            // Invalidate cache to recompile queries against the new schema
            state.query_cache.clear();

            let query_files =
                discover_query_files(root, config, args.queries.as_deref()).unwrap_or_default();
            for q_path in query_files {
                recompile_single_query(&q_path, state, config, driver, wrappers, root);
            }
        }
        Err(e) => {
            let ts = current_time_str();
            eprintln!(
                "[{}] {}: {}",
                ts.red().bold(),
                "Failed to rebuild schema from migrations".red().bold(),
                e
            );
        }
    }
}

/// Core watch loop with 75ms sliding window debouncing and graceful shutdown via `running` flag.
pub fn run_watch_loop(
    args: &GenerateArgs,
    root: &Path,
    explicit_config: Option<&Path>,
    running: Arc<AtomicBool>,
) -> Result<(), String> {
    let (config, _) = SqltypeConfig::discover_or_default(root, explicit_config);
    let migrations_dir = resolve_migrations_dir(root, &config, args.migrations.as_deref());
    let abs_migrations = normalize_path(&migrations_dir);

    let driver = args.driver.unwrap_or_else(|| config.driver.name.into());
    let wrappers = args.wrappers;

    let catalog = Catalog::load_from_dir_with_driver(&abs_migrations, driver).map_err(|e| {
        format!(
            "Failed to load migrations from {}: {}",
            abs_migrations.display(),
            e
        )
    })?;

    let mut state = IncrementalState::new(catalog);

    // Initial compilation sweep
    let initial_queries =
        discover_query_files(root, &config, args.queries.as_deref()).unwrap_or_default();
    for q_path in &initial_queries {
        recompile_single_query(q_path, &mut state, &config, driver, wrappers, root);
    }

    println!("\n👀 Watching for changes in:");
    println!("   Migrations: {}", abs_migrations.display());
    if let Some(q) = args.queries.as_deref() {
        println!("   Queries:    {}", q.display());
    } else {
        println!("   Queries:    {}", root.join("queries").display());
    }
    println!("   (Press Ctrl+C to stop)\n");

    let (tx, rx) = std::sync::mpsc::channel();
    let mut watcher = RecommendedWatcher::new(tx, notify::Config::default())
        .map_err(|e| format!("Failed to initialize file watcher: {}", e))?;

    if abs_migrations.exists() {
        watcher
            .watch(&abs_migrations, RecursiveMode::Recursive)
            .map_err(|e| format!("Failed to watch migrations directory: {}", e))?;
    }

    let queries_dir = if let Some(q) = args.queries.as_deref() {
        if q.is_relative() {
            root.join(q)
        } else {
            q.to_path_buf()
        }
    } else {
        root.join("queries")
    };

    if queries_dir.exists() {
        let abs_q = normalize_path(&queries_dir);
        let _ = watcher.watch(&abs_q, RecursiveMode::Recursive);
    }

    let src_dir = root.join("src");
    if src_dir.exists() && src_dir != queries_dir && src_dir != abs_migrations {
        let abs_src = normalize_path(&src_dir);
        let _ = watcher.watch(&abs_src, RecursiveMode::Recursive);
    }

    let debounce_window = Duration::from_millis(75);

    while running.load(Ordering::SeqCst) {
        // Poll for incoming events with short timeout to stay responsive to `running` flag
        let first_res = match rx.recv_timeout(Duration::from_millis(50)) {
            Ok(res) => res,
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
        };

        let mut raw_events = vec![first_res];

        // 75ms sliding window debouncing loop
        loop {
            if !running.load(Ordering::SeqCst) {
                break;
            }
            match rx.recv_timeout(debounce_window) {
                Ok(res) => {
                    raw_events.push(res);
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // Sliding window elapsed with no new events: debounce complete
                    break;
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    break;
                }
            }
        }

        let mut accumulated_paths = HashSet::new();

        for res in raw_events {
            let event = match res {
                Ok(e) => e,
                Err(_) => continue,
            };

            // Filter out metadata-only modify events
            if let EventKind::Modify(notify::event::ModifyKind::Metadata(_)) = event.kind {
                continue;
            }

            match event.kind {
                EventKind::Create(_)
                | EventKind::Modify(_)
                | EventKind::Remove(_)
                | EventKind::Any => {
                    for path in event.paths {
                        let norm = normalize_path(&path);
                        if !should_filter_path(&norm) {
                            accumulated_paths.insert(norm);
                        }
                    }
                }
                _ => {}
            }
        }

        if accumulated_paths.is_empty() {
            continue;
        }

        let mut full_rebuild = false;
        let mut partial_queries = HashSet::new();

        for path in &accumulated_paths {
            match classify_path(path, &abs_migrations) {
                FileChange::FullRebuild => {
                    full_rebuild = true;
                }
                FileChange::PartialQuery(q) => {
                    partial_queries.insert(q);
                }
                FileChange::Ignored => {}
            }
        }

        if full_rebuild {
            execute_full_rebuild(
                &abs_migrations,
                driver,
                wrappers,
                &mut state,
                root,
                &config,
                args,
            );
        } else if !partial_queries.is_empty() {
            for q_path in partial_queries {
                recompile_single_query(&q_path, &mut state, &config, driver, wrappers, root);
            }
        }
    }

    Ok(())
}

/// Executes watch mode with graceful shutdown on Ctrl+C.
pub fn execute_watch(
    args: &GenerateArgs,
    root: &Path,
    explicit_config: Option<&Path>,
) -> Result<(), String> {
    let running = Arc::new(AtomicBool::new(true));
    let r = Arc::clone(&running);
    let _ = ctrlc::set_handler(move || {
        r.store(false, Ordering::SeqCst);
    });

    run_watch_loop(args, root, explicit_config, running)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_hash() {
        let content_a = "SELECT 1;";
        let content_b = "SELECT 1;";
        let content_c = "SELECT 2;";

        assert_eq!(compute_hash(content_a), compute_hash(content_b));
        assert_ne!(compute_hash(content_a), compute_hash(content_c));
    }

    #[test]
    fn test_should_filter_path() {
        assert!(should_filter_path(Path::new("queries/find_users.sql.ts")));
        assert!(should_filter_path(Path::new(
            "queries/find_users.sqltype.ts"
        )));
        assert!(should_filter_path(Path::new(
            "queries/find_users.generated.ts"
        )));
        assert!(should_filter_path(Path::new("queries/find_users.d.ts")));
        assert!(should_filter_path(Path::new(".tmp_file")));
        assert!(should_filter_path(Path::new("temp.tmp")));
        assert!(should_filter_path(Path::new("queries/.tmp123.sql")));

        assert!(!should_filter_path(Path::new("queries/find_users.sql")));
        assert!(!should_filter_path(Path::new("src/queries/users.ts")));
        assert!(!should_filter_path(Path::new(
            "src/components/UserList.tsx"
        )));
    }

    #[test]
    fn test_classify_path() {
        let migrations = Path::new("migrations");

        assert_eq!(
            classify_path(Path::new("migrations/001_init.sql"), migrations),
            FileChange::FullRebuild
        );
        assert_eq!(
            classify_path(Path::new("migrations/readme.md"), migrations),
            FileChange::Ignored
        );

        assert_eq!(
            classify_path(Path::new("queries/find_users.sql"), migrations),
            FileChange::PartialQuery(PathBuf::from("queries/find_users.sql"))
        );
        assert_eq!(
            classify_path(Path::new("src/users.ts"), migrations),
            FileChange::PartialQuery(PathBuf::from("src/users.ts"))
        );
        assert_eq!(
            classify_path(Path::new("src/users.tsx"), migrations),
            FileChange::PartialQuery(PathBuf::from("src/users.tsx"))
        );
        assert_eq!(
            classify_path(Path::new("package.json"), migrations),
            FileChange::Ignored
        );
    }
}
