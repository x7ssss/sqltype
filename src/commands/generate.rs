use crate::analyzer::{AnalyzedQuery, analyze_query};
use crate::catalog::Catalog;
use crate::cli::GenerateArgs;
use crate::codegen::{CodegenOptions, generate_file_ts_with_options};
use crate::commands::check::print_rustc_diagnostic;
use crate::commands::{atomic_write, discover_query_files, resolve_migrations_dir};
use crate::config::{OutputMode, SqltypeConfig};
use crate::ts_scanner::{is_ts_js_file, scan_ts_queries};
use rayon::prelude::*;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

pub fn run_generate(
    args: &GenerateArgs,
    root: &Path,
    explicit_config: Option<&Path>,
) -> Result<(), String> {
    let start_time = Instant::now();

    let (config, _) = SqltypeConfig::discover_or_default(root, explicit_config);
    let migrations_dir = resolve_migrations_dir(root, &config, args.migrations.as_deref());
    let driver = args.driver.unwrap_or_else(|| config.driver.name.into());

    let catalog = Catalog::load_from_dir_with_driver(&migrations_dir, driver).map_err(|e| {
        format!(
            "Failed to load migrations from {}: {}",
            migrations_dir.display(),
            e
        )
    })?;

    let query_files = discover_query_files(root, &config, args.queries.as_deref())?;
    if query_files.is_empty() {
        println!("No query files found.");
        return Ok(());
    }

    // Parallel query analysis across CPU threads with rayon
    type AnalyzeError = (PathBuf, usize, usize, Option<String>, String, String);
    let analyze_file = |file: &PathBuf| -> Result<(PathBuf, Vec<AnalyzedQuery>), AnalyzeError> {
        let content = fs::read_to_string(file).map_err(|e| {
            (
                file.clone(),
                1,
                1,
                None,
                format!("Failed to read file: {}", e),
                String::new(),
            )
        })?;

        if is_ts_js_file(file) {
            let extracted = scan_ts_queries(&content);
            let mut analyzed_list = Vec::new();
            for q in extracted {
                let fallback = q
                    .name
                    .as_deref()
                    .or_else(|| file.file_stem().and_then(|s| s.to_str()));
                match analyze_query(&q.sql, &catalog, fallback) {
                    Ok(analyzed) => analyzed_list.push(analyzed),
                    Err(e) => {
                        return Err((
                            file.clone(),
                            q.line,
                            q.column,
                            fallback.map(|s| s.to_string()),
                            e,
                            content,
                        ));
                    }
                }
            }
            Ok((file.clone(), analyzed_list))
        } else {
            let filename = file
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or("query.sql");
            match analyze_query(&content, &catalog, Some(filename)) {
                Ok(analyzed) => Ok((file.clone(), vec![analyzed])),
                Err(e) => Err((file.clone(), 1, 1, Some(filename.to_string()), e, content)),
            }
        }
    };

    let results: Vec<Result<(PathBuf, Vec<AnalyzedQuery>), AnalyzeError>> =
        if let Some(threads) = args.threads {
            let pool = rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .map_err(|e| format!("Failed to initialize rayon thread pool: {}", e))?;
            pool.install(|| query_files.par_iter().map(analyze_file).collect())
        } else {
            query_files.par_iter().map(analyze_file).collect()
        };

    let mut has_errors = false;
    let mut file_analyzed_queries = Vec::new();

    for res in results {
        match res {
            Ok((file, queries)) => {
                if !queries.is_empty() {
                    file_analyzed_queries.push((file, queries));
                }
            }
            Err((file, line, col, query_name, err, content)) => {
                has_errors = true;
                print_rustc_diagnostic(&file, line, col, query_name.as_deref(), &err, &content);
            }
        }
    }

    if has_errors {
        return Err("Generation aborted: validation failed on one or more queries.".to_string());
    }

    let declaration_only = args.declaration_only || config.output.declaration_only;
    let options = CodegenOptions::new(driver, args.wrappers);
    let mut generated_file_count = 0;
    let mut total_query_count = 0;

    match config.output.mode {
        OutputMode::Centralized => {
            let mut all_queries = Vec::new();
            for (_, queries) in &file_analyzed_queries {
                all_queries.extend(queries.clone());
            }
            total_query_count = all_queries.len();

            let target_path = if let Some(out) = &args.out {
                if out.extension().is_some() {
                    out.clone()
                } else {
                    out.join(if declaration_only {
                        "sqltype.d.ts"
                    } else {
                        "sqltype.ts"
                    })
                }
            } else if let Some(cfg_path) = &config.output.file_path {
                root.join(cfg_path)
            } else {
                root.join(if declaration_only {
                    "src/generated/sqltype.d.ts"
                } else {
                    "src/generated/sqltype.ts"
                })
            };

            let ts_code = generate_file_ts_with_options(&all_queries, &options);
            atomic_write(&target_path, &ts_code).map_err(|e| {
                format!(
                    "Failed to write centralized TypeScript file {}: {}",
                    target_path.display(),
                    e
                )
            })?;

            println!("  Generated: {}", target_path.display());
            generated_file_count = 1;
        }
        OutputMode::Companion => {
            for (file, queries) in &file_analyzed_queries {
                total_query_count += queries.len();
                let ts_code = generate_file_ts_with_options(queries, &options);

                if is_ts_js_file(file) {
                    let sibling_ext = if declaration_only {
                        "sqltype.d.ts"
                    } else {
                        "sqltype.ts"
                    };
                    let sibling_path = file.with_extension(sibling_ext);

                    atomic_write(&sibling_path, &ts_code).map_err(|e| {
                        format!(
                            "Failed to write companion file {}: {}",
                            sibling_path.display(),
                            e
                        )
                    })?;
                    println!("  Generated: {}", sibling_path.display());
                    generated_file_count += 1;

                    // If explicit out directory provided and distinct from parent, write duplicate
                    if let Some(out) = &args.out {
                        let queries_base = args.queries.as_deref().unwrap_or(Path::new("queries"));
                        let rel_path = file.strip_prefix(queries_base).unwrap_or(file);
                        let out_file_path = out.join(rel_path).with_extension(sibling_ext);
                        if out_file_path != sibling_path {
                            let _ = atomic_write(&out_file_path, &ts_code);
                        }
                    }
                } else {
                    let out_file_path = if let Some(out) = &args.out {
                        let queries_base = args.queries.as_deref().unwrap_or(Path::new("queries"));
                        let rel_path = file.strip_prefix(queries_base).unwrap_or(file);
                        let ext = if declaration_only { "d.ts" } else { "ts" };
                        out.join(rel_path).with_extension(ext)
                    } else {
                        let ext = if declaration_only { "d.ts" } else { "ts" };
                        file.with_extension(ext)
                    };

                    atomic_write(&out_file_path, &ts_code).map_err(|e| {
                        format!(
                            "Failed to write TypeScript file {}: {}",
                            out_file_path.display(),
                            e
                        )
                    })?;
                    println!("  Generated: {}", out_file_path.display());

                    // Also emit companion .sql.ts if not declaration-only
                    if !declaration_only {
                        let sql_ts_path = if let Some(out) = &args.out {
                            let queries_base =
                                args.queries.as_deref().unwrap_or(Path::new("queries"));
                            let rel_path = file.strip_prefix(queries_base).unwrap_or(file);
                            out.join(rel_path).with_extension("sql.ts")
                        } else {
                            file.with_extension("sql.ts")
                        };
                        if sql_ts_path != out_file_path {
                            let _ = atomic_write(&sql_ts_path, &ts_code);
                        }
                    }

                    generated_file_count += 1;
                }
            }
        }
    }

    let elapsed = start_time.elapsed();
    println!(
        "\nProcessed migrations, generated {} query types across {} file(s) in {}ms.",
        total_query_count,
        generated_file_count,
        elapsed.as_millis()
    );

    if args.watch {
        return crate::commands::watch::execute_watch(args, root, explicit_config);
    }

    Ok(())
}
