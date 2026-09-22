use clap::{Parser, Subcommand};
use sqltype::analyzer::analyze_query;
use sqltype::catalog::{Catalog, DriverTarget};
use sqltype::codegen::{CodegenOptions, generate_file_ts_with_options};
use sqltype::ts_scanner::{is_query_file, is_ts_js_file, scan_ts_queries};
use std::path::{Path, PathBuf};
use std::process;
use walkdir::WalkDir;

#[derive(Parser, Debug)]
#[command(
    name = "sqltype",
    about = "Ultra-fast, local-first SQL-to-TypeScript compiler CLI in Rust",
    version
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Validates all queries against the migration schema and exits with code 1 on type/column mismatch
    Check {
        /// Directory containing PostgreSQL migration SQL files
        #[arg(long, short = 'm')]
        migrations: PathBuf,

        /// Directory containing SQL query files
        #[arg(long, short = 'q')]
        queries: PathBuf,

        /// Driver target profile (postgres, pg, bun)
        #[arg(long, short = 'd', value_enum, default_value_t = DriverTarget::Postgres)]
        driver: DriverTarget,

        /// Generate type-safe query execution wrappers
        #[arg(long, short = 'w', default_value_t = false)]
        wrappers: bool,
    },
    /// Emits .ts files for all valid queries
    Generate {
        /// Directory containing PostgreSQL migration SQL files
        #[arg(long, short = 'm')]
        migrations: PathBuf,

        /// Directory containing SQL query files
        #[arg(long, short = 'q')]
        queries: PathBuf,

        /// Directory where .ts files will be emitted
        #[arg(long, short = 'o')]
        out: Option<PathBuf>,

        /// Watch for file changes and re-generate TypeScript types incrementally
        #[arg(long, short = 'W')]
        watch: bool,

        /// Driver target profile (postgres, pg, bun)
        #[arg(long, short = 'd', value_enum, default_value_t = DriverTarget::Postgres)]
        driver: DriverTarget,

        /// Generate type-safe query execution wrappers
        #[arg(long, short = 'w', default_value_t = false)]
        wrappers: bool,
    },
    /// Starts the Language Server Protocol (LSP) server for real-time diagnostics and hover inspection
    Lsp {
        /// Directory containing PostgreSQL migration SQL files
        #[arg(long, short = 'm', default_value = "./migrations")]
        migrations: PathBuf,
    },
}

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

    // Sort deterministically
    files.sort();
    Ok(files)
}

fn run_check(
    migrations_dir: &Path,
    queries_dir: &Path,
    driver: DriverTarget,
    wrappers: bool,
) -> Result<(), ()> {
    let catalog = match Catalog::load_from_dir_with_driver(migrations_dir, driver) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[Error] Failed to load migrations: {}", e);
            return Err(());
        }
    };

    let query_files = match discover_query_files(queries_dir) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[Error] Failed to discover queries: {}", e);
            return Err(());
        }
    };

    if query_files.is_empty() {
        println!("No query files found in {}", queries_dir.display());
        return Ok(());
    }

    let mut has_errors = false;
    let mut total_queries = 0;

    for file in &query_files {
        let content = match std::fs::read_to_string(file) {
            Ok(c) => c,
            Err(e) => {
                eprintln!(
                    "[Error] Failed to read query file {}: {}",
                    file.display(),
                    e
                );
                has_errors = true;
                continue;
            }
        };

        if is_ts_js_file(file) {
            let extracted = scan_ts_queries(&content);
            if extracted.is_empty() {
                continue;
            }
            for q in extracted {
                total_queries += 1;
                let fallback = q
                    .name
                    .as_deref()
                    .or_else(|| file.file_stem().and_then(|s| s.to_str()));
                match analyze_query(&q.sql, &catalog, fallback) {
                    Ok(analyzed) => {
                        if wrappers {
                            let options = CodegenOptions::new(driver, true);
                            let _ = generate_file_ts_with_options(
                                std::slice::from_ref(&analyzed),
                                &options,
                            );
                        }
                        println!(
                            "  ✓ [{}] {}:{}:{} ({} params, {} fields{})",
                            analyzed.name,
                            file.display(),
                            q.line,
                            q.column,
                            analyzed.params.len(),
                            analyzed.fields.len(),
                            if wrappers { ", wrapper enabled" } else { "" }
                        );
                    }
                    Err(e) => {
                        eprintln!(
                            "  ✗ [Error] {}:{}:{}: in query '{}': {}",
                            file.display(),
                            q.line,
                            q.column,
                            fallback.unwrap_or("Query"),
                            e
                        );
                        has_errors = true;
                    }
                }
            }
        } else {
            total_queries += 1;
            let filename = file
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or("query.sql");

            match analyze_query(&content, &catalog, Some(filename)) {
                Ok(analyzed) => {
                    if wrappers {
                        let options = CodegenOptions::new(driver, true);
                        let _ = generate_file_ts_with_options(
                            std::slice::from_ref(&analyzed),
                            &options,
                        );
                    }
                    println!(
                        "  ✓ [{}] {} ({} params, {} fields{})",
                        analyzed.name,
                        file.display(),
                        analyzed.params.len(),
                        analyzed.fields.len(),
                        if wrappers { ", wrapper enabled" } else { "" }
                    );
                }
                Err(e) => {
                    eprintln!("  ✗ [Error] {}: {}", file.display(), e);
                    has_errors = true;
                }
            }
        }
    }

    if has_errors {
        eprintln!("\nValidation failed: one or more queries had errors.");
        Err(())
    } else {
        println!(
            "\n✓ All {} queries across {} files validated successfully against migration schema.",
            total_queries,
            query_files.len()
        );
        Ok(())
    }
}

fn run_generate(
    migrations_dir: &Path,
    queries_dir: &Path,
    out_dir: Option<&Path>,
    driver: DriverTarget,
    wrappers: bool,
) -> Result<(), ()> {
    let catalog = match Catalog::load_from_dir_with_driver(migrations_dir, driver) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[Error] Failed to load migrations: {}", e);
            return Err(());
        }
    };

    let query_files = match discover_query_files(queries_dir) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[Error] Failed to discover queries: {}", e);
            return Err(());
        }
    };

    if query_files.is_empty() {
        println!("No query files found in {}", queries_dir.display());
        return Ok(());
    }

    let mut file_analyzed_queries: Vec<(PathBuf, Vec<sqltype::analyzer::AnalyzedQuery>)> =
        Vec::new();
    let mut has_errors = false;
    let mut total_query_count = 0;

    for file in &query_files {
        let content = match std::fs::read_to_string(file) {
            Ok(c) => c,
            Err(e) => {
                eprintln!(
                    "[Error] Failed to read query file {}: {}",
                    file.display(),
                    e
                );
                has_errors = true;
                continue;
            }
        };

        if is_ts_js_file(file) {
            let extracted = scan_ts_queries(&content);
            if extracted.is_empty() {
                continue;
            }
            let mut analyzed_list = Vec::new();
            for q in extracted {
                let fallback = q
                    .name
                    .as_deref()
                    .or_else(|| file.file_stem().and_then(|s| s.to_str()));
                match analyze_query(&q.sql, &catalog, fallback) {
                    Ok(analyzed) => {
                        analyzed_list.push(analyzed);
                    }
                    Err(e) => {
                        eprintln!(
                            "  ✗ [Error] {}:{}:{}: in query '{}': {}",
                            file.display(),
                            q.line,
                            q.column,
                            fallback.unwrap_or("Query"),
                            e
                        );
                        has_errors = true;
                    }
                }
            }
            if !analyzed_list.is_empty() {
                total_query_count += analyzed_list.len();
                file_analyzed_queries.push((file.clone(), analyzed_list));
            }
        } else {
            let filename = file
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or("query.sql");

            match analyze_query(&content, &catalog, Some(filename)) {
                Ok(analyzed) => {
                    total_query_count += 1;
                    file_analyzed_queries.push((file.clone(), vec![analyzed]));
                }
                Err(e) => {
                    eprintln!("  ✗ [Error] {}: {}", file.display(), e);
                    has_errors = true;
                }
            }
        }
    }

    if has_errors {
        eprintln!("\nGeneration aborted: validation failed on one or more queries.");
        return Err(());
    }

    let options = CodegenOptions::new(driver, wrappers);
    let mut generated_file_count = 0;

    // Write generated .ts files
    for (file, queries) in &file_analyzed_queries {
        let ts_code = generate_file_ts_with_options(queries, &options);

        if is_ts_js_file(file) {
            // Sibling declaration file (e.g. users.ts -> users.sqltype.ts)
            let sibling_path = file.with_extension("sqltype.ts");
            if let Err(e) = std::fs::write(&sibling_path, &ts_code) {
                eprintln!(
                    "[Error] Failed to write TypeScript file {}: {}",
                    sibling_path.display(),
                    e
                );
                return Err(());
            }
            println!("  Generated: {}", sibling_path.display());
            generated_file_count += 1;

            // If out_dir is specified and not the same directory, also write to out_dir
            if let Some(out) = out_dir {
                let rel_path = file.strip_prefix(queries_dir).unwrap_or(file);
                let out_file_path = out.join(rel_path).with_extension("sqltype.ts");
                if out_file_path != sibling_path {
                    if let Some(parent) = out_file_path.parent()
                        && let Err(e) = std::fs::create_dir_all(parent)
                    {
                        eprintln!(
                            "[Error] Failed to create output directory {}: {}",
                            parent.display(),
                            e
                        );
                        return Err(());
                    }
                    let _ = std::fs::write(&out_file_path, &ts_code);
                }
            }
        } else {
            let out_file_path = if let Some(out) = out_dir {
                let rel_path = file.strip_prefix(queries_dir).unwrap_or(file);
                out.join(rel_path).with_extension("ts")
            } else {
                file.with_extension("ts")
            };

            if let Some(parent) = out_file_path.parent()
                && let Err(e) = std::fs::create_dir_all(parent)
            {
                eprintln!(
                    "[Error] Failed to create output directory {}: {}",
                    parent.display(),
                    e
                );
                return Err(());
            }

            if let Err(e) = std::fs::write(&out_file_path, &ts_code) {
                eprintln!(
                    "[Error] Failed to write TypeScript file {}: {}",
                    out_file_path.display(),
                    e
                );
                return Err(());
            }
            println!("  Generated: {}", out_file_path.display());
            generated_file_count += 1;
        }
    }

    println!(
        "\n✓ Successfully generated {} queries across {} files.",
        total_query_count, generated_file_count
    );
    Ok(())
}

fn main() {
    let cli = Cli::parse();

    let res = match cli.command {
        Commands::Check {
            migrations,
            queries,
            driver,
            wrappers,
        } => run_check(&migrations, &queries, driver, wrappers),
        Commands::Generate {
            migrations,
            queries,
            out,
            watch,
            driver,
            wrappers,
        } => {
            let res = run_generate(&migrations, &queries, out.as_deref(), driver, wrappers);
            if res.is_ok() && watch {
                let watch_out = out.as_deref().unwrap_or(&queries);
                if let Err(e) =
                    sqltype::watcher::run_watch(&migrations, &queries, watch_out, driver, wrappers)
                {
                    eprintln!("[Watch Error] {}", e);
                    Err(())
                } else {
                    Ok(())
                }
            } else {
                res
            }
        }
        Commands::Lsp { migrations } => {
            if let Err(e) = sqltype::lsp::run_lsp_server(migrations) {
                eprintln!("[LSP Error] {}", e);
                Err(())
            } else {
                Ok(())
            }
        }
    };

    if res.is_err() {
        process::exit(1);
    }
}
