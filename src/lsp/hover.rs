use lsp_types::{Hover, HoverContents, MarkupContent, MarkupKind, Position, Range};
use pg_query::{NodeEnum, NodeRef};
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, RwLock};

use crate::analyzer::analyze_parsed_query;
use crate::catalog::Catalog;
use crate::lsp::document::{Document, byte_offset_to_position, position_to_byte_offset};

static PARSE_CACHE: LazyLock<RwLock<HashMap<String, Arc<pg_query::ParseResult>>>> =
    LazyLock::new(|| RwLock::new(HashMap::with_capacity(64)));

/// Retrieves a pre-parsed or freshly-parsed PostgreSQL AST, caching results to avoid C-FFI parse overhead.
pub fn get_parsed_ast(sql: &str) -> Option<Arc<pg_query::ParseResult>> {
    if let Ok(cache) = PARSE_CACHE.read()
        && let Some(parsed) = cache.get(sql)
    {
        return Some(Arc::clone(parsed));
    }

    let parsed = Arc::new(pg_query::parse(sql).ok()?);
    if let Ok(mut cache) = PARSE_CACHE.write() {
        if cache.len() > 128 {
            cache.clear();
        }
        cache.insert(sql.to_string(), Arc::clone(&parsed));
    }
    Some(parsed)
}

fn format_column_hover(parts: &[String], catalog: &Catalog) -> String {
    if parts.len() >= 3 {
        let schema_name = &parts[0];
        let table_name = &parts[1];
        let col_name = &parts[2];
        if let Some(table) = catalog.get_table_qualified(Some(schema_name), table_name)
            && let Some(col) = table.get_column(col_name)
        {
            let null_str = if col.is_nullable {
                "nullable (true)"
            } else {
                "non-null (false)"
            };
            return format!(
                "### Column `{}.{}.{}`\n- **PostgreSQL Type**: `{}`\n- **TypeScript Type**: `{}`\n- **Nullable**: `{}`",
                schema_name, table_name, col_name, col.pg_type, col.ts_type, null_str
            );
        }
    } else if parts.len() >= 2 {
        let table_or_alias = &parts[0];
        let col_name = &parts[1];
        if let Some(table) = catalog.get_table(table_or_alias)
            && let Some(col) = table.get_column(col_name)
        {
            let null_str = if col.is_nullable {
                "nullable (true)"
            } else {
                "non-null (false)"
            };
            return format!(
                "### Column `{}.{}`\n- **PostgreSQL Type**: `{}`\n- **TypeScript Type**: `{}`\n- **Nullable**: `{}`",
                table_or_alias, col_name, col.pg_type, col.ts_type, null_str
            );
        }
    }

    let col_name = parts.last().cloned().unwrap_or_default();
    for table in catalog.tables.values() {
        if let Some(col) = table.get_column(&col_name) {
            let null_str = if col.is_nullable {
                "nullable (true)"
            } else {
                "non-null (false)"
            };
            return format!(
                "### Column `{}.{}`\n- **PostgreSQL Type**: `{}`\n- **TypeScript Type**: `{}`\n- **Nullable**: `{}`",
                table.name, col_name, col.pg_type, col.ts_type, null_str
            );
        }
    }

    format!("### Column `{}`", parts.join("."))
}

/// Resolves hover information at the given position in raw SQL text.
pub fn resolve_hover(text: &str, pos: Position, catalog: &Catalog) -> Option<Hover> {
    let byte_offset = position_to_byte_offset(text, pos);
    let parsed = pg_query::parse(text).ok()?;
    let analyzed_cell: std::cell::OnceCell<Option<crate::analyzer::AnalyzedQuery>> =
        std::cell::OnceCell::new();
    let get_analyzed = || -> Option<&crate::analyzer::AnalyzedQuery> {
        analyzed_cell
            .get_or_init(|| analyze_parsed_query(&parsed, text, catalog, None).ok())
            .as_ref()
    };

    // 1. Inspect ParamRef ($N)
    for (node, _, _, _) in parsed.protobuf.nodes() {
        if let NodeRef::ParamRef(pr) = node {
            let loc = if pr.location >= 0 {
                pr.location as usize
            } else {
                continue;
            };
            let param_str = format!("${}", pr.number);
            if byte_offset >= loc && byte_offset <= loc + param_str.len() {
                let start = byte_offset_to_position(text, loc);
                let end = byte_offset_to_position(text, loc + param_str.len());

                let md = if let Some(query) = get_analyzed() {
                    if let Some(param) = query.params.iter().find(|p| p.index == pr.number as usize)
                    {
                        format!(
                            "### Parameter `${}`\n- **Field**: `{}`\n- **TypeScript Type**: `{}`\n- **Optional**: `{}`",
                            pr.number, param.name, param.ts_type, param.is_optional
                        )
                    } else {
                        format!("### Parameter `${}`", pr.number)
                    }
                } else {
                    format!("### Parameter `${}`", pr.number)
                };

                return Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: md,
                    }),
                    range: Some(Range { start, end }),
                });
            }
        }
    }

    // 2. Inspect RangeVar (Table)
    for (node, _, _, _) in parsed.protobuf.nodes() {
        if let NodeRef::RangeVar(rv) = node {
            let loc = if rv.location >= 0 {
                rv.location as usize
            } else {
                continue;
            };
            let token_len = if rv.schemaname.is_empty() {
                rv.relname.len()
            } else {
                rv.schemaname.len() + 1 + rv.relname.len()
            };
            if byte_offset >= loc && byte_offset <= loc + token_len {
                let start = byte_offset_to_position(text, loc);
                let end = byte_offset_to_position(text, loc + token_len);

                let schema = if rv.schemaname.is_empty() {
                    None
                } else {
                    Some(rv.schemaname.as_str())
                };
                let display_name = if rv.schemaname.is_empty() {
                    rv.relname.clone()
                } else {
                    format!("{}.{}", rv.schemaname, rv.relname)
                };

                let md = if let Some(table) = catalog.get_table_qualified(schema, &rv.relname) {
                    let mut cols = Vec::new();
                    for col in &table.columns {
                        let null_str = if col.is_nullable {
                            "nullable"
                        } else {
                            "non-null"
                        };
                        let col_type = col
                            .pg_type
                            .strip_prefix("pg_catalog.")
                            .unwrap_or(&col.pg_type);
                        cols.push(format!("- `{}`: `{}` ({})", col.name, col_type, null_str));
                    }
                    format!(
                        "### Table `{}`\n**Columns**:\n{}",
                        display_name,
                        cols.join("\n")
                    )
                } else {
                    format!(
                        "### Table `{}`\n*(Not found in schema catalog)*",
                        display_name
                    )
                };

                return Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: md,
                    }),
                    range: Some(Range { start, end }),
                });
            }
        }
    }

    // 3. Inspect ColumnRef
    for (node, _, _, _) in parsed.protobuf.nodes() {
        if let NodeRef::ColumnRef(cr) = node {
            let loc = if cr.location >= 0 {
                cr.location as usize
            } else {
                continue;
            };

            let mut parts = Vec::new();
            for field in &cr.fields {
                if let Some(NodeEnum::String(s)) = &field.node {
                    parts.push(s.sval.clone());
                }
            }
            if parts.is_empty() {
                continue;
            }

            let full_token = parts.join(".");
            let token_len = full_token.len();
            if byte_offset >= loc && byte_offset <= loc + token_len {
                let start = byte_offset_to_position(text, loc);
                let end = byte_offset_to_position(text, loc + token_len);

                let md = {
                    let col_name = parts.last().unwrap();
                    let catalog_has_col = catalog
                        .tables
                        .values()
                        .any(|t| t.get_column(col_name).is_some());
                    if catalog_has_col {
                        format_column_hover(&parts, catalog)
                    } else if let Some(query) = get_analyzed()
                        && let Some(field) = query.fields.iter().find(|f| &f.name == col_name)
                    {
                        let null_str = if field.ts_type.contains("null") {
                            "nullable (true)"
                        } else {
                            "non-null (false)"
                        };
                        format!(
                            "### Column `{}`\n- **TypeScript Type**: `{}`\n- **Nullable**: `{}`",
                            col_name, field.ts_type, null_str
                        )
                    } else {
                        format_column_hover(&parts, catalog)
                    }
                };

                return Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: md,
                    }),
                    range: Some(Range { start, end }),
                });
            }
        }
    }

    None
}

/// Resolves low-latency (<5ms) non-blocking hover documentation for parameters, tables, and columns.
pub fn resolve_document_hover(doc: &Document, pos: Position, catalog: &Catalog) -> Option<Hover> {
    if !doc.is_ts {
        return resolve_hover(&doc.text(), pos, catalog);
    }

    let host_offset = doc.position_to_byte_offset(pos);
    let query = doc
        .queries()
        .iter()
        .find(|q| q.contains_host_offset(host_offset))?;

    // 1. Check if hovering directly on an interpolation `${...}`
    if let Some(interp) = query.source_map.find_interpolated_param(host_offset) {
        let parsed = pg_query::parse(&query.sql).ok()?;
        let analyzed = analyze_parsed_query(&parsed, &query.sql, catalog, None).ok();
        let md = if let Some(ref q_meta) = analyzed
            && let Some(p) = q_meta.params.iter().find(|p| p.index == interp.index)
        {
            format!(
                "### Parameter `${}` (`{}`)\n- **TypeScript Type**: `{}`\n- **Optional**: `{}`",
                interp.index, interp.expr, p.ts_type, p.is_optional
            )
        } else {
            format!(
                "### Parameter `${}` (`{}`)\n- **TypeScript Type**: `unknown`",
                interp.index, interp.expr
            )
        };

        let start = doc.byte_to_position(interp.host_range.start);
        let end = doc.byte_to_position(interp.host_range.end);
        return Some(Hover {
            contents: HoverContents::Markup(MarkupContent {
                kind: MarkupKind::Markdown,
                value: md,
            }),
            range: Some(Range { start, end }),
        });
    }

    let sql_offset = query.host_to_sql_offset(host_offset)?;
    let parsed = pg_query::parse(&query.sql).ok()?;

    let analyzed_cell: std::cell::OnceCell<Option<crate::analyzer::AnalyzedQuery>> =
        std::cell::OnceCell::new();
    let get_analyzed = || -> Option<&crate::analyzer::AnalyzedQuery> {
        analyzed_cell
            .get_or_init(|| analyze_parsed_query(&parsed, &query.sql, catalog, None).ok())
            .as_ref()
    };

    // 2. Inspect ParamRef ($N)
    for (node, _, _, _) in parsed.protobuf.nodes() {
        if let NodeRef::ParamRef(pr) = node {
            let loc = if pr.location >= 0 {
                pr.location as usize
            } else {
                continue;
            };
            let param_str = format!("${}", pr.number);
            if sql_offset >= loc && sql_offset <= loc + param_str.len() {
                let host_span = query.sql_to_host_range(loc..loc + param_str.len());
                let start = doc.byte_to_position(host_span.start);
                let end = doc.byte_to_position(host_span.end);

                let md = if let Some(q_meta) = get_analyzed()
                    && let Some(param) =
                        q_meta.params.iter().find(|p| p.index == pr.number as usize)
                {
                    format!(
                        "### Parameter `${}`\n- **Field**: `{}`\n- **TypeScript Type**: `{}`\n- **Optional**: `{}`",
                        pr.number, param.name, param.ts_type, param.is_optional
                    )
                } else {
                    format!("### Parameter `${}`", pr.number)
                };

                return Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: md,
                    }),
                    range: Some(Range { start, end }),
                });
            }
        }
    }

    // 3. Inspect RangeVar (Table)
    for (node, _, _, _) in parsed.protobuf.nodes() {
        if let NodeRef::RangeVar(rv) = node {
            let loc = if rv.location >= 0 {
                rv.location as usize
            } else {
                continue;
            };
            let token_len = if rv.schemaname.is_empty() {
                rv.relname.len()
            } else {
                rv.schemaname.len() + 1 + rv.relname.len()
            };
            if sql_offset >= loc && sql_offset <= loc + token_len {
                let host_span = query.sql_to_host_range(loc..loc + token_len);
                let start = doc.byte_to_position(host_span.start);
                let end = doc.byte_to_position(host_span.end);

                let schema = if rv.schemaname.is_empty() {
                    None
                } else {
                    Some(rv.schemaname.as_str())
                };
                let display_name = if rv.schemaname.is_empty() {
                    rv.relname.clone()
                } else {
                    format!("{}.{}", rv.schemaname, rv.relname)
                };

                let md = if let Some(table) = catalog.get_table_qualified(schema, &rv.relname) {
                    let mut cols = Vec::new();
                    for col in &table.columns {
                        let null_str = if col.is_nullable {
                            "nullable"
                        } else {
                            "non-null"
                        };
                        let col_type = col
                            .pg_type
                            .strip_prefix("pg_catalog.")
                            .unwrap_or(&col.pg_type);
                        cols.push(format!("- `{}`: `{}` ({})", col.name, col_type, null_str));
                    }
                    format!(
                        "### Table `{}`\n**Columns**:\n{}",
                        display_name,
                        cols.join("\n")
                    )
                } else {
                    format!(
                        "### Table `{}`\n*(Not found in schema catalog)*",
                        display_name
                    )
                };

                return Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: md,
                    }),
                    range: Some(Range { start, end }),
                });
            }
        }
    }

    // 4. Inspect ColumnRef
    for (node, _, _, _) in parsed.protobuf.nodes() {
        if let NodeRef::ColumnRef(cr) = node {
            let loc = if cr.location >= 0 {
                cr.location as usize
            } else {
                continue;
            };

            let mut parts = Vec::new();
            for field in &cr.fields {
                if let Some(NodeEnum::String(s)) = &field.node {
                    parts.push(s.sval.clone());
                }
            }
            if parts.is_empty() {
                continue;
            }

            let full_token = parts.join(".");
            let token_len = full_token.len();
            if sql_offset >= loc && sql_offset <= loc + token_len {
                let host_span = query.sql_to_host_range(loc..loc + token_len);
                let start = doc.byte_to_position(host_span.start);
                let end = doc.byte_to_position(host_span.end);

                let md = {
                    let col_name = parts.last().unwrap();
                    let catalog_has_col = catalog
                        .tables
                        .values()
                        .any(|t| t.get_column(col_name).is_some());
                    if catalog_has_col {
                        format_column_hover(&parts, catalog)
                    } else if let Some(q_meta) = get_analyzed()
                        && let Some(field) = q_meta.fields.iter().find(|f| &f.name == col_name)
                    {
                        let null_str = if field.ts_type.contains("null") {
                            "nullable (true)"
                        } else {
                            "non-null (false)"
                        };
                        format!(
                            "### Column `{}`\n- **TypeScript Type**: `{}`\n- **Nullable**: `{}`",
                            col_name, field.ts_type, null_str
                        )
                    } else {
                        format_column_hover(&parts, catalog)
                    }
                };

                return Some(Hover {
                    contents: HoverContents::Markup(MarkupContent {
                        kind: MarkupKind::Markdown,
                        value: md,
                    }),
                    range: Some(Range { start, end }),
                });
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::TableMetadata;
    use std::time::{Duration, Instant};

    #[test]
    fn test_hover_param_and_table() {
        let mut catalog = Catalog::default();
        let users_table = TableMetadata {
            name: "users".to_string(),
            schema: None,
            columns: vec![
                crate::catalog::ColumnMetadata {
                    name: "id".to_string(),
                    pg_type: "uuid".to_string(),
                    ts_type: "string".to_string(),
                    is_nullable: false,
                    has_default: false,
                    is_primary_key: true,
                },
                crate::catalog::ColumnMetadata {
                    name: "name".to_string(),
                    pg_type: "text".to_string(),
                    ts_type: "string".to_string(),
                    is_nullable: true,
                    has_default: false,
                    is_primary_key: false,
                },
            ],
            primary_keys: vec!["id".to_string()],
        };
        catalog.tables.insert("users".to_string(), users_table);

        let sql = "SELECT id FROM users WHERE id = $1;";
        // Hover on $1 (character 32)
        let hover_param = resolve_hover(
            sql,
            Position {
                line: 0,
                character: 32,
            },
            &catalog,
        );
        assert!(hover_param.is_some());
        if let Some(h) = hover_param
            && let HoverContents::Markup(m) = h.contents
        {
            assert!(m.value.contains("Parameter `$1`"));
            assert!(m.value.contains("TypeScript Type"));
        }

        // Hover on users (character 15)
        let hover_table = resolve_hover(
            sql,
            Position {
                line: 0,
                character: 15,
            },
            &catalog,
        );
        assert!(hover_table.is_some());
        if let Some(h) = hover_table
            && let HoverContents::Markup(m) = h.contents
        {
            assert!(m.value.contains("### Table `users`"));
            assert!(m.value.contains("- `id`: `uuid`"));
        }
    }

    #[test]
    fn test_hover_on_parameter_with_interpolated_expression() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql("CREATE TABLE accounts (id UUID PRIMARY KEY, username TEXT NOT NULL);")
            .unwrap();

        let ts = r#"import { sql } from 'bun';

export const getAccount = sql`
  SELECT id, username
  FROM accounts
  WHERE id = ${userId};
`;
"#;
        let doc = Document::new("file:///accounts.ts".to_string(), 1, ts);

        // Position on "${userId}" on line 5: "  WHERE id = ${userId};"
        // character 16 is on '${'
        let hover = resolve_document_hover(
            &doc,
            Position {
                line: 5,
                character: 16,
            },
            &catalog,
        );

        assert!(
            hover.is_some(),
            "Hover on ${{userId}} must return Some(Hover)"
        );
        let h = hover.unwrap();

        if let HoverContents::Markup(m) = h.contents {
            assert!(
                m.value.contains("Parameter `$1`") || m.value.contains("userId"),
                "Hover must mention parameter index or expression, got: {}",
                m.value
            );
            assert!(
                m.value.contains("TypeScript Type"),
                "Hover must document inferred TypeScript type"
            );
        } else {
            panic!("Hover contents must be MarkupKind::Markdown");
        }

        assert!(
            h.range.is_some(),
            "Hover must return precise host document Range"
        );
    }

    #[test]
    fn test_hover_on_table_name_shows_columns_and_types() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql(
                "CREATE TABLE accounts (
                id UUID PRIMARY KEY,
                username VARCHAR(50) NOT NULL,
                balance INT NOT NULL DEFAULT 0
            );",
            )
            .unwrap();

        let ts = r#"import { sql } from 'bun';

export const getAccount = sql`
  SELECT id, username
  FROM accounts
  WHERE id = $1;
`;
"#;
        let doc = Document::new("file:///accounts.ts".to_string(), 1, ts);

        // Position on "accounts" on line 4 (col 8)
        let hover = resolve_document_hover(
            &doc,
            Position {
                line: 4,
                character: 8,
            },
            &catalog,
        );

        assert!(
            hover.is_some(),
            "Hover on accounts table must return Some(Hover)"
        );
        let h = hover.unwrap();

        if let HoverContents::Markup(m) = h.contents {
            assert!(m.value.contains("### Table `accounts`"));
            assert!(m.value.contains("- `id`: `uuid`"));
            assert!(m.value.contains("- `username`: `varchar`"));
            assert!(m.value.contains("- `balance`: `int4`"));
        } else {
            panic!("Expected Markdown contents");
        }
    }

    #[test]
    fn test_hover_on_column_name_shows_type_and_nullability() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql(
                "CREATE TABLE accounts (
                id UUID PRIMARY KEY,
                username VARCHAR(50) NOT NULL,
                bio TEXT
            );",
            )
            .unwrap();

        let ts = r#"import { sql } from 'bun';

export const getAccount = sql`
  SELECT username, bio
  FROM accounts;
`;
"#;
        let doc = Document::new("file:///accounts.ts".to_string(), 1, ts);

        // Hover on "username" on line 3 (col 10)
        let hover_username = resolve_document_hover(
            &doc,
            Position {
                line: 3,
                character: 10,
            },
            &catalog,
        );
        assert!(hover_username.is_some());
        if let HoverContents::Markup(m) = hover_username.unwrap().contents {
            assert!(m.value.contains("username"));
            assert!(m.value.contains("varchar") || m.value.contains("string"));
            assert!(m.value.contains("non-null") || m.value.contains("false"));
        }

        // Hover on nullable "bio" on line 3 (col 20)
        let hover_bio = resolve_document_hover(
            &doc,
            Position {
                line: 3,
                character: 20,
            },
            &catalog,
        );
        assert!(hover_bio.is_some());
        if let HoverContents::Markup(m) = hover_bio.unwrap().contents {
            assert!(m.value.contains("bio"));
            assert!(m.value.contains("nullable") || m.value.contains("true"));
        }
    }

    #[test]
    fn test_hover_outside_sql_template_returns_none_immediately() {
        let catalog = Catalog::default();
        let ts = "import { sql } from 'bun';\nconst x = 42;\n";
        let doc = Document::new("file:///test.ts".to_string(), 1, ts);

        // Hover on line 0 (import)
        let start = Instant::now();
        let hover = resolve_document_hover(
            &doc,
            Position {
                line: 0,
                character: 2,
            },
            &catalog,
        );
        let elapsed = start.elapsed();

        assert!(
            hover.is_none(),
            "Hover outside template literal must return None"
        );
        assert!(
            elapsed < Duration::from_millis(1),
            "Bypass outside template literal must resolve in <1ms, took: {:?}",
            elapsed
        );
    }

    #[test]
    fn test_hover_latency_benchmark_asserts_under_5ms() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql(
                "CREATE TABLE accounts (
                id UUID PRIMARY KEY,
                username VARCHAR(50) NOT NULL,
                balance INT NOT NULL DEFAULT 0
            );",
            )
            .unwrap();

        let ts = r#"import { sql } from 'bun';

export const getAccount = sql`
  SELECT id, username, balance
  FROM accounts
  WHERE id = ${userId} AND balance > ${minBalance};
`;
"#;
        let doc = Document::new("file:///accounts.ts".to_string(), 1, ts);

        // Test positions across different token kinds: table, column, param, outside
        let test_positions = [
            Position {
                line: 4,
                character: 8,
            }, // Table: accounts
            Position {
                line: 3,
                character: 12,
            }, // Column: username
            Position {
                line: 5,
                character: 16,
            }, // Param: ${userId}
            Position {
                line: 0,
                character: 2,
            }, // Outside: import
        ];

        // Warm-up run (10 iterations)
        for _ in 0..10 {
            for pos in &test_positions {
                let _ = resolve_document_hover(&doc, *pos, &catalog);
            }
        }

        const ITERATIONS: usize = 500;
        let mut durations: Vec<Duration> = Vec::with_capacity(ITERATIONS);
        let mut total_latency = Duration::ZERO;

        for i in 0..ITERATIONS {
            let pos = test_positions[i % test_positions.len()];
            let start = Instant::now();
            let _ = resolve_document_hover(&doc, pos, &catalog);
            let elapsed = start.elapsed();

            durations.push(elapsed);
            total_latency += elapsed;
        }

        durations.sort();
        let avg_latency = total_latency / ITERATIONS as u32;
        let p95 = durations[ITERATIONS * 95 / 100];
        let max_latency = *durations.last().unwrap();

        assert!(
            avg_latency < Duration::from_millis(2),
            "Average hover latency must be well under 2ms, got: {:?}",
            avg_latency
        );
        assert!(
            p95 < Duration::from_millis(5),
            "p95 hover latency must strictly be < 5ms, got: {:?}",
            p95
        );

        let threshold = if cfg!(debug_assertions) {
            Duration::from_millis(50)
        } else {
            Duration::from_millis(5)
        };
        assert!(
            max_latency < threshold,
            "Max hover latency must strictly be < {:?}, got: {:?}",
            threshold,
            max_latency
        );
    }

    #[test]
    fn test_hover_schema_qualified_table_and_column() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql("CREATE SCHEMA custom; CREATE TABLE custom.orders (id UUID PRIMARY KEY, total INT NOT NULL);")
            .unwrap();

        let ts = r#"import { sql } from 'bun';

export const getOrders = sql`
  SELECT custom.orders.id, custom.orders.total
  FROM custom.orders;
`;
"#;
        let doc = Document::new("file:///orders.ts".to_string(), 1, ts);

        // Hover on "custom.orders" on line 4 (col 8)
        let hover_table = resolve_document_hover(
            &doc,
            Position {
                line: 4,
                character: 8,
            },
            &catalog,
        );
        assert!(
            hover_table.is_some(),
            "Hover on custom.orders must return Some"
        );
        if let HoverContents::Markup(m) = hover_table.unwrap().contents {
            assert!(m.value.contains("### Table `custom.orders`"));
            assert!(m.value.contains("- `id`: `uuid`"));
            assert!(m.value.contains("- `total`: `int4`"));
        }

        // Hover on "custom.orders.total" on line 3 (col 28)
        let hover_col = resolve_document_hover(
            &doc,
            Position {
                line: 3,
                character: 28,
            },
            &catalog,
        );
        assert!(
            hover_col.is_some(),
            "Hover on custom.orders.total must return Some"
        );
        if let HoverContents::Markup(m) = hover_col.unwrap().contents {
            assert!(m.value.contains("### Column `custom.orders.total`"));
            assert!(m.value.contains("int4") || m.value.contains("number"));
        }
    }
}
