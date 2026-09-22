use clap::{Parser, Subcommand};
use sqltype::analyzer::analyze_query;
use sqltype::catalog::{Catalog, DriverTarget};
use sqltype::codegen::{CodegenOptions, generate_file_ts_with_options};
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
        out: PathBuf,

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

fn discover_sql_files<P: AsRef<Path>>(dir: P) -> Result<Vec<PathBuf>, String> {
    let dir_path = dir.as_ref();
    if !dir_path.exists() {
        return Err(format!("Directory does not exist: {}", dir_path.display()));
    }

    let mut files = Vec::new();
    for entry in WalkDir::new(dir_path).follow_links(true) {
        let entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path();
        if path.is_file()
            && let Some(ext) = path.extension()
            && ext.eq_ignore_ascii_case("sql")
        {
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

    let query_files = match discover_sql_files(queries_dir) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[Error] Failed to discover queries: {}", e);
            return Err(());
        }
    };

    if query_files.is_empty() {
        println!("No query .sql files found in {}", queries_dir.display());
        return Ok(());
    }

    let mut has_errors = false;
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

        let filename = file
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("query.sql");

        match analyze_query(&content, &catalog, Some(filename)) {
            Ok(analyzed) => {
                if wrappers {
                    let options = CodegenOptions::new(driver, true);
                    let _ =
                        generate_file_ts_with_options(std::slice::from_ref(&analyzed), &options);
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

    if has_errors {
        eprintln!("\nValidation failed: one or more queries had errors.");
        Err(())
    } else {
        println!(
            "\n✓ All {} queries validated successfully against migration schema.",
            query_files.len()
        );
        Ok(())
    }
}

fn run_generate(
    migrations_dir: &Path,
    queries_dir: &Path,
    out_dir: &Path,
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

    let query_files = match discover_sql_files(queries_dir) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("[Error] Failed to discover queries: {}", e);
            return Err(());
        }
    };

    if query_files.is_empty() {
        println!("No query .sql files found in {}", queries_dir.display());
        return Ok(());
    }

    let mut analyzed_queries = Vec::new();
    let mut has_errors = false;

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

        let filename = file
            .file_name()
            .and_then(|f| f.to_str())
            .unwrap_or("query.sql");

        match analyze_query(&content, &catalog, Some(filename)) {
            Ok(analyzed) => {
                analyzed_queries.push((file.clone(), analyzed));
            }
            Err(e) => {
                eprintln!("  ✗ [Error] {}: {}", file.display(), e);
                has_errors = true;
            }
        }
    }

    if has_errors {
        eprintln!("\nGeneration aborted: validation failed on one or more queries.");
        return Err(());
    }

    let options = CodegenOptions::new(driver, wrappers);

    // Write generated .ts files
    for (file, analyzed) in &analyzed_queries {
        let rel_path = file.strip_prefix(queries_dir).unwrap_or(file);
        let out_file_path = out_dir.join(rel_path).with_extension("ts");

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

        let ts_code = generate_file_ts_with_options(std::slice::from_ref(analyzed), &options);
        if let Err(e) = std::fs::write(&out_file_path, ts_code) {
            eprintln!(
                "[Error] Failed to write TypeScript file {}: {}",
                out_file_path.display(),
                e
            );
            return Err(());
        }
        println!("  Generated: {}", out_file_path.display());
    }

    println!(
        "\n✓ Successfully generated {} TypeScript files in {}.",
        analyzed_queries.len(),
        out_dir.display()
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
            let res = run_generate(&migrations, &queries, &out, driver, wrappers);
            if res.is_ok() && watch {
                if let Err(e) =
                    sqltype::watcher::run_watch(&migrations, &queries, &out, driver, wrappers)
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
