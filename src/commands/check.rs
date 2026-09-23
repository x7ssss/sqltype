use crate::analyzer::analyze_query;
use crate::catalog::Catalog;
use crate::cli::CheckArgs;
use crate::codegen::{CodegenOptions, generate_file_ts_with_options};
use crate::commands::{discover_query_files, resolve_migrations_dir};
use crate::config::SqltypeConfig;
use crate::ts_scanner::{is_ts_js_file, scan_ts_queries};
use colored::Colorize;
use serde::Serialize;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize)]
pub struct CheckDiagnostic {
    pub severity: String,
    pub file: String,
    pub line: usize,
    pub column: usize,
    pub query_name: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct CheckReport {
    pub success: bool,
    pub total_queries: usize,
    pub error_count: usize,
    pub diagnostics: Vec<CheckDiagnostic>,
}

pub fn print_rustc_diagnostic(
    file: &Path,
    line: usize,
    col: usize,
    query_name: Option<&str>,
    message: &str,
    source_content: &str,
) {
    let q_header = if let Some(q) = query_name {
        format!("in query '{}': ", q)
    } else {
        String::new()
    };

    eprintln!("{}: {}{}", "error".red().bold(), q_header.bold(), message);
    eprintln!(
        " {} {}:{}:{}",
        "-->".blue().bold(),
        file.display(),
        line,
        col
    );

    let line_content = source_content.lines().nth(line.saturating_sub(1));
    if let Some(src_line) = line_content {
        let line_num_str = format!("{}", line);
        let padding = " ".repeat(line_num_str.len());
        eprintln!(" {} {}", padding, "|".blue().bold());
        eprintln!(
            "{} {} {}",
            line_num_str.blue().bold(),
            "|".blue().bold(),
            src_line
        );
        let col_indent = " ".repeat(col.saturating_sub(1));
        eprintln!(
            " {} {} {}{}",
            padding,
            "|".blue().bold(),
            col_indent,
            "^".red().bold()
        );
    }
    eprintln!();
}

pub fn run_check(
    args: &CheckArgs,
    root: &Path,
    explicit_config: Option<&Path>,
) -> Result<(), String> {
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
        if args.json {
            let report = CheckReport {
                success: true,
                total_queries: 0,
                error_count: 0,
                diagnostics: Vec::new(),
            };
            println!("{}", serde_json::to_string_pretty(&report).unwrap());
        } else {
            println!("No query files found.");
        }
        return Ok(());
    }

    let mut diagnostics = Vec::new();
    let mut total_queries = 0;
    let mut should_stop = false;

    for file in &query_files {
        if should_stop {
            break;
        }

        let content = match fs::read_to_string(file) {
            Ok(c) => c,
            Err(e) => {
                let diag = CheckDiagnostic {
                    severity: "error".to_string(),
                    file: file.display().to_string(),
                    line: 1,
                    column: 1,
                    query_name: None,
                    message: format!("Failed to read file: {}", e),
                };
                if !args.json {
                    eprintln!(
                        "{}: Failed to read query file {}: {}",
                        "error".red().bold(),
                        file.display(),
                        e
                    );
                }
                diagnostics.push(diag);
                if args.fail_fast {
                    break;
                }
                continue;
            }
        };

        if is_ts_js_file(file) {
            let extracted = scan_ts_queries(&content);
            for q in extracted {
                total_queries += 1;
                let fallback = q
                    .name
                    .as_deref()
                    .or_else(|| file.file_stem().and_then(|s| s.to_str()));

                match analyze_query(&q.sql, &catalog, fallback) {
                    Ok(analyzed) => {
                        if args.wrappers {
                            let options = CodegenOptions::new(driver, true);
                            let _ = generate_file_ts_with_options(
                                std::slice::from_ref(&analyzed),
                                &options,
                            );
                        }
                        if !args.json {
                            println!(
                                "  {} [{}] {}:{}:{} ({} params, {} fields{})",
                                "✓".green(),
                                analyzed.name,
                                file.display(),
                                q.line,
                                q.column,
                                analyzed.params.len(),
                                analyzed.fields.len(),
                                if args.wrappers {
                                    ", wrapper enabled"
                                } else {
                                    ""
                                }
                            );
                        }
                    }
                    Err(e) => {
                        let diag = CheckDiagnostic {
                            severity: "error".to_string(),
                            file: file.display().to_string(),
                            line: q.line,
                            column: q.column,
                            query_name: fallback.map(|s| s.to_string()),
                            message: e.clone(),
                        };
                        diagnostics.push(diag);

                        if !args.json {
                            print_rustc_diagnostic(file, q.line, q.column, fallback, &e, &content);
                        }

                        if args.fail_fast {
                            should_stop = true;
                            break;
                        }
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
                    if args.wrappers {
                        let options = CodegenOptions::new(driver, true);
                        let _ = generate_file_ts_with_options(
                            std::slice::from_ref(&analyzed),
                            &options,
                        );
                    }
                    if !args.json {
                        println!(
                            "  {} [{}] {} ({} params, {} fields{})",
                            "✓".green(),
                            analyzed.name,
                            file.display(),
                            analyzed.params.len(),
                            analyzed.fields.len(),
                            if args.wrappers {
                                ", wrapper enabled"
                            } else {
                                ""
                            }
                        );
                    }
                }
                Err(e) => {
                    let diag = CheckDiagnostic {
                        severity: "error".to_string(),
                        file: file.display().to_string(),
                        line: 1,
                        column: 1,
                        query_name: Some(filename.to_string()),
                        message: e.clone(),
                    };
                    diagnostics.push(diag);

                    if !args.json {
                        print_rustc_diagnostic(file, 1, 1, Some(filename), &e, &content);
                    }

                    if args.fail_fast {
                        should_stop = true;
                    }
                }
            }
        }
    }

    let error_count = diagnostics.len();
    if args.json {
        let report = CheckReport {
            success: error_count == 0,
            total_queries,
            error_count,
            diagnostics,
        };
        println!("{}", serde_json::to_string_pretty(&report).unwrap());
    }

    if error_count > 0 {
        if !args.json {
            eprintln!("\nValidation failed: one or more queries had errors.");
        }
        Err(format!("Check failed with {} error(s)", error_count))
    } else {
        if !args.json {
            println!(
                "\n{} All {} queries across {} files validated successfully against migration schema.",
                "✓".green().bold(),
                total_queries,
                query_files.len()
            );
        }
        Ok(())
    }
}
