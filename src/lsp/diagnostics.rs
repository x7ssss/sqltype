use lsp_types::{Diagnostic, DiagnosticSeverity, PublishDiagnosticsParams, Range, Uri};
use pg_query::NodeRef;
use std::collections::HashSet;
use std::ffi::{CStr, CString, c_char, c_int};

use crate::catalog::Catalog;
use crate::lsp::document::{Document, byte_offset_to_position, find_token_end};

#[repr(C)]
struct PgQueryError {
    message: *mut c_char,
    funcname: *mut c_char,
    filename: *mut c_char,
    lineno: c_int,
    cursorpos: c_int,
    context: *mut c_char,
}

#[repr(C)]
struct PgQueryProtobuf {
    len: usize,
    data: *mut c_char,
}

#[repr(C)]
struct PgQueryProtobufParseResult {
    parse_tree: PgQueryProtobuf,
    stderr_buffer: *mut c_char,
    error: *mut PgQueryError,
}

unsafe extern "C" {
    fn pg_query_parse_protobuf(input: *const c_char) -> PgQueryProtobufParseResult;
    fn pg_query_free_protobuf_parse_result(result: PgQueryProtobufParseResult);
}

/// Raw parse error containing error message and 1-based character position.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawParseError {
    pub message: String,
    pub cursorpos: i32,
}

/// Parses SQL text using PostgreSQL's C raw parser to extract exact syntax error position.
pub fn parse_sql_raw(text: &str) -> Result<(), RawParseError> {
    let c_str = match CString::new(text) {
        Ok(s) => s,
        Err(_) => {
            return Err(RawParseError {
                message: "String contains embedded null byte".to_string(),
                cursorpos: 0,
            });
        }
    };

    let res = unsafe { pg_query_parse_protobuf(c_str.as_ptr()) };
    if !res.error.is_null() {
        let (message, cursorpos) = unsafe {
            let err = &*res.error;
            let msg = if !err.message.is_null() {
                CStr::from_ptr(err.message).to_string_lossy().to_string()
            } else {
                "Syntax error".to_string()
            };
            (msg, err.cursorpos)
        };
        unsafe { pg_query_free_protobuf_parse_result(res) };
        return Err(RawParseError { message, cursorpos });
    }
    unsafe { pg_query_free_protobuf_parse_result(res) };
    Ok(())
}

/// Validates raw SQL text against the schema catalog, returning LSP diagnostics.
pub fn validate_sql(text: &str, catalog: &Catalog) -> Vec<Diagnostic> {
    // 1. Check for parse / syntax errors using libpg_query raw parser
    if let Err(err) = parse_sql_raw(text) {
        let start_offset = if err.cursorpos > 0 {
            (err.cursorpos as usize).saturating_sub(1).min(text.len())
        } else {
            0
        };
        let end_offset = find_token_end(text, start_offset);
        let start = byte_offset_to_position(text, start_offset);
        let end = byte_offset_to_position(text, end_offset);

        return vec![Diagnostic {
            range: Range { start, end },
            severity: Some(DiagnosticSeverity::ERROR),
            source: Some("sqltype".to_string()),
            message: err.message,
            ..Default::default()
        }];
    }

    // 2. Parse AST to validate referenced tables / relations
    let parsed = match pg_query::parse(text) {
        Ok(p) => p,
        Err(_) => return Vec::new(),
    };

    let mut cte_names = HashSet::new();
    for (node, _, _, _) in parsed.protobuf.nodes() {
        if let NodeRef::CommonTableExpr(cte) = node {
            cte_names.insert(cte.ctename.clone());
        }
    }

    let mut diagnostics = Vec::new();
    for (node, _, _, _) in parsed.protobuf.nodes() {
        if let NodeRef::RangeVar(rv) = node {
            let schema = if rv.schemaname.is_empty() {
                None
            } else {
                Some(rv.schemaname.as_str())
            };
            if !cte_names.contains(&rv.relname)
                && catalog.get_table_qualified(schema, &rv.relname).is_none()
            {
                let loc = if rv.location >= 0 {
                    rv.location as usize
                } else {
                    0
                };
                let token_len = if rv.schemaname.is_empty() {
                    rv.relname.len()
                } else {
                    rv.schemaname.len() + 1 + rv.relname.len()
                };
                let display_name = if rv.schemaname.is_empty() {
                    rv.relname.clone()
                } else {
                    format!("{}.{}", rv.schemaname, rv.relname)
                };
                let start = byte_offset_to_position(text, loc);
                let end = byte_offset_to_position(text, loc + token_len);
                diagnostics.push(Diagnostic {
                    range: Range { start, end },
                    severity: Some(DiagnosticSeverity::ERROR),
                    source: Some("sqltype".to_string()),
                    message: format!(
                        "Table \"{}\" does not exist in schema catalog",
                        display_name
                    ),
                    ..Default::default()
                });
            }
        }
    }

    diagnostics
}

/// Validates an open document, mapping diagnostics to host coordinates for TypeScript files.
pub fn validate_document(doc: &Document, catalog: &Catalog) -> Vec<Diagnostic> {
    if !doc.is_ts {
        return validate_sql(&doc.text(), catalog);
    }

    let mut diagnostics = Vec::new();

    for query in doc.queries() {
        // 1. Check syntax errors in extracted query
        if let Err(err) = parse_sql_raw(&query.sql) {
            let sql_start = if err.cursorpos > 0 {
                (err.cursorpos as usize)
                    .saturating_sub(1)
                    .min(query.sql.len())
            } else {
                0
            };
            let sql_end = find_token_end(&query.sql, sql_start);
            let host_range = query.sql_to_host_range(sql_start..sql_end);
            let start = doc.byte_to_position(host_range.start);
            let end = doc.byte_to_position(host_range.end);

            diagnostics.push(Diagnostic {
                range: Range { start, end },
                severity: Some(DiagnosticSeverity::ERROR),
                source: Some("sqltype".to_string()),
                message: err.message,
                ..Default::default()
            });
            continue;
        }

        // 2. Parse AST to validate tables against schema catalog
        let parsed = match pg_query::parse(&query.sql) {
            Ok(p) => p,
            Err(_) => continue,
        };

        let mut cte_names = HashSet::new();
        for (node, _, _, _) in parsed.protobuf.nodes() {
            if let NodeRef::CommonTableExpr(cte) = node {
                cte_names.insert(cte.ctename.clone());
            }
        }

        for (node, _, _, _) in parsed.protobuf.nodes() {
            if let NodeRef::RangeVar(rv) = node {
                let schema = if rv.schemaname.is_empty() {
                    None
                } else {
                    Some(rv.schemaname.as_str())
                };
                if !cte_names.contains(&rv.relname)
                    && catalog.get_table_qualified(schema, &rv.relname).is_none()
                {
                    let loc = if rv.location >= 0 {
                        rv.location as usize
                    } else {
                        0
                    };
                    let token_len = if rv.schemaname.is_empty() {
                        rv.relname.len()
                    } else {
                        rv.schemaname.len() + 1 + rv.relname.len()
                    };
                    let display_name = if rv.schemaname.is_empty() {
                        rv.relname.clone()
                    } else {
                        format!("{}.{}", rv.schemaname, rv.relname)
                    };
                    let sql_range = loc..loc + token_len;
                    let host_range = query.sql_to_host_range(sql_range);
                    let start = doc.byte_to_position(host_range.start);
                    let end = doc.byte_to_position(host_range.end);

                    diagnostics.push(Diagnostic {
                        range: Range { start, end },
                        severity: Some(DiagnosticSeverity::ERROR),
                        source: Some("sqltype".to_string()),
                        message: format!(
                            "Table \"{}\" does not exist in schema catalog",
                            display_name
                        ),
                        ..Default::default()
                    });
                }
            }
        }
    }

    diagnostics
}

/// Creates publication parameters to clear all diagnostics when a document is closed.
pub fn clear_document_diagnostics(uri: &Uri) -> PublishDiagnosticsParams {
    PublishDiagnosticsParams {
        uri: uri.clone(),
        diagnostics: Vec::new(),
        version: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::TableMetadata;

    #[test]
    fn test_validate_sql_syntax_error() {
        let catalog = Catalog::default();
        let sql = "SELECT FROM;";
        let diags = validate_sql(sql, &catalog);
        assert_eq!(diags.len(), 1);
        assert!(diags[0].message.contains("syntax error"));
        assert_eq!(diags[0].range.start.line, 0);
        assert_eq!(diags[0].range.start.character, 11);
    }

    #[test]
    fn test_validate_sql_missing_table() {
        let catalog = Catalog::default();
        let sql = "SELECT id FROM nonexistent_table WHERE 1=1;";
        let diags = validate_sql(sql, &catalog);
        assert_eq!(diags.len(), 1);
        assert!(
            diags[0]
                .message
                .contains("Table \"nonexistent_table\" does not exist in schema catalog")
        );
        assert_eq!(diags[0].range.start.character, 15);
        assert_eq!(diags[0].range.end.character, 32);
    }

    #[test]
    fn test_validate_sql_valid_with_cte() {
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
                    name: "active".to_string(),
                    pg_type: "bool".to_string(),
                    ts_type: "boolean".to_string(),
                    is_nullable: false,
                    has_default: false,
                    is_primary_key: false,
                },
            ],
            primary_keys: vec!["id".to_string()],
        };
        catalog.tables.insert("users".to_string(), users_table);

        let sql = "WITH active_users AS (SELECT id FROM users WHERE active = true) SELECT au.id FROM active_users au;";
        let diags = validate_sql(sql, &catalog);
        assert!(diags.is_empty());
    }

    #[test]
    fn test_diagnostics_postgresql_syntax_error_in_ts_file_accurate_line() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql("CREATE TABLE users (id UUID PRIMARY KEY, email TEXT NOT NULL);")
            .unwrap();

        let ts = r#"import { sql } from 'bun';

export const brokenQuery = sql`
  SELECT id, email
  FROM;
`;
"#;
        let doc = Document::new("file:///queries.ts".to_string(), 1, ts);
        let diags = validate_document(&doc, &catalog);

        assert_eq!(diags.len(), 1, "Expected exactly 1 syntax error diagnostic");
        let diag = &diags[0];

        assert_eq!(diag.severity, Some(DiagnosticSeverity::ERROR));
        assert!(
            diag.message.to_lowercase().contains("syntax error"),
            "Diagnostic message must indicate syntax error, got: {}",
            diag.message
        );

        // The syntax error is at "FROM;" on line 4 (0-indexed)
        assert_eq!(
            diag.range.start.line, 4,
            "Diagnostic start line must map to host document line 4, got {}",
            diag.range.start.line
        );
        assert!(
            diag.range.start.line != 0,
            "Must NOT report syntax error on line 0 (TypeScript import)"
        );
    }

    #[test]
    fn test_diagnostics_syntax_error_with_interpolations_offset_accuracy() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql("CREATE TABLE users (id UUID PRIMARY KEY, email TEXT NOT NULL);")
            .unwrap();

        let ts = r#"import { sql } from 'bun';

export const query = sql`
  SELECT id FROM users
  WHERE id = ${superLongInterpolatedExpression}
    AND ORDER BY;
`;
"#;
        let doc = Document::new("file:///queries.ts".to_string(), 1, ts);
        let diags = validate_document(&doc, &catalog);

        assert_eq!(diags.len(), 1);
        let diag = &diags[0];

        // Syntax error is at "ORDER BY;" on line 5 (0-indexed)
        assert_eq!(
            diag.range.start.line, 5,
            "Diagnostic start line must be line 5 where ORDER BY is located"
        );
        assert!(
            diag.message.contains("syntax error"),
            "Expected syntax error message"
        );
    }

    #[test]
    fn test_diagnostics_catalog_table_mismatch_mapped_to_exact_token() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql("CREATE TABLE accounts (id UUID PRIMARY KEY, balance INT NOT NULL);")
            .unwrap();

        let ts = r#"import { sql } from 'bun';

export const query = sql`
  SELECT id FROM non_existent_orders
  WHERE id = ${orderId};
`;
"#;
        let doc = Document::new("file:///orders.ts".to_string(), 1, ts);
        let diags = validate_document(&doc, &catalog);

        assert_eq!(diags.len(), 1);
        let diag = &diags[0];

        assert!(
            diag.message
                .contains("Table \"non_existent_orders\" does not exist in schema catalog"),
            "Message must report missing table name, got: {}",
            diag.message
        );
        assert_eq!(diag.range.start.line, 3);

        // Verify token span in host text
        let line_str = doc.rope.line(3).to_string();
        let token_span =
            &line_str[diag.range.start.character as usize..diag.range.end.character as usize];
        assert_eq!(token_span, "non_existent_orders");
    }

    #[test]
    fn test_diagnostics_valid_query_with_interpolations_zero_diagnostics() {
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
        let diags = validate_document(&doc, &catalog);

        assert!(
            diags.is_empty(),
            "Valid query with interpolations must produce 0 diagnostics, got: {:?}",
            diags
        );
    }

    #[test]
    fn test_diagnostics_clear_on_did_close() {
        let uri: Uri = "file:///temp.ts".parse().unwrap();
        let params = clear_document_diagnostics(&uri);
        assert_eq!(params.uri, uri);
        assert!(
            params.diagnostics.is_empty(),
            "clear_document_diagnostics must publish empty vec to clear editor squigglies"
        );
    }

    #[test]
    fn test_diagnostics_schema_qualified_table_valid_and_missing() {
        let mut catalog = Catalog::default();
        catalog
            .apply_sql("CREATE SCHEMA custom; CREATE TABLE custom.orders (id UUID PRIMARY KEY);")
            .unwrap();

        // 1. Valid schema-qualified table produces 0 diagnostics
        let ts_valid = r#"import { sql } from 'bun';
export const q = sql`SELECT id FROM custom.orders;`;
"#;
        let doc_valid = Document::new("file:///valid.ts".to_string(), 1, ts_valid);
        let diags_valid = validate_document(&doc_valid, &catalog);
        assert!(
            diags_valid.is_empty(),
            "Valid custom.orders must produce 0 diagnostics, got: {:?}",
            diags_valid
        );

        // 2. Missing schema-qualified table produces error diagnostic with qualified name
        let ts_invalid = r#"import { sql } from 'bun';
export const q = sql`SELECT id FROM custom.non_existent;`;
"#;
        let doc_invalid = Document::new("file:///invalid.ts".to_string(), 1, ts_invalid);
        let diags_invalid = validate_document(&doc_invalid, &catalog);
        assert_eq!(diags_invalid.len(), 1);
        assert!(diags_invalid[0].message.contains("custom.non_existent"));
    }
}
