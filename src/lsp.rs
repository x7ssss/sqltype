use arc_swap::ArcSwap;
use crossbeam_channel::unbounded;
use lsp_server::{Connection, Message, Notification, Request, RequestId, Response};
use lsp_types::{
    Diagnostic, DiagnosticSeverity, DidChangeTextDocumentParams, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, Hover, HoverContents, HoverParams, HoverProviderCapability,
    InitializeResult, MarkupContent, MarkupKind, Position, PublishDiagnosticsParams, Range,
    ServerCapabilities, ServerInfo, TextDocumentSyncCapability, TextDocumentSyncKind, Uri,
};
use notify::{RecursiveMode, Watcher};
use pg_query::{NodeEnum, NodeRef};
use std::collections::{HashMap, HashSet};
use std::ffi::{c_char, c_int, CStr, CString};
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use crate::analyzer::analyze_query;
use crate::catalog::Catalog;

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawParseError {
    pub message: String,
    pub cursorpos: i32,
}

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

/// Converts a 0-indexed byte offset into an LSP `Position` (0-indexed line and UTF-16 character).
pub fn byte_offset_to_position(text: &str, byte_offset: usize) -> Position {
    let offset = byte_offset.min(text.len());
    let mut safe_offset = offset;
    while safe_offset > 0 && !text.is_char_boundary(safe_offset) {
        safe_offset -= 1;
    }
    let prefix = &text[..safe_offset];
    let line = prefix.chars().filter(|&c| c == '\n').count() as u32;
    let last_newline_pos = prefix.rfind('\n').map(|p| p + 1).unwrap_or(0);
    let line_str = &prefix[last_newline_pos..];
    let character = line_str.encode_utf16().count() as u32;
    Position { line, character }
}

/// Converts an LSP `Position` (0-indexed line and UTF-16 character) into a 0-indexed byte offset.
pub fn position_to_byte_offset(text: &str, pos: Position) -> usize {
    let mut current_offset: usize = 0;
    for (current_line, line) in (0_u32..).zip(text.split_inclusive('\n')) {
        if current_line == pos.line {
            let mut utf16_count: u32 = 0;
            for (idx, ch) in line.char_indices() {
                if utf16_count >= pos.character {
                    return current_offset + idx;
                }
                utf16_count += ch.len_utf16() as u32;
            }
            return current_offset + line.len();
        }
        current_offset += line.len();
    }
    text.len()
}

/// Finds the end byte offset of the token starting at `start_offset`.
pub fn find_token_end(text: &str, start_offset: usize) -> usize {
    if start_offset >= text.len() {
        return text.len();
    }
    let remainder = &text[start_offset..];
    let mut chars = remainder.chars();
    let first = match chars.next() {
        Some(c) => c,
        None => return text.len(),
    };

    if first.is_alphanumeric() || first == '_' {
        let len = remainder
            .chars()
            .take_while(|c| c.is_alphanumeric() || *c == '_')
            .map(|c| c.len_utf8())
            .sum::<usize>();
        start_offset + len.max(1)
    } else {
        start_offset + first.len_utf8()
    }
}

/// Validates SQL text against the schema catalog, returning LSP diagnostics.
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
        if let NodeRef::RangeVar(rv) = node
            && !cte_names.contains(&rv.relname)
            && catalog.get_table(&rv.relname).is_none()
        {
            let loc = if rv.location >= 0 {
                rv.location as usize
            } else {
                0
            };
            let start = byte_offset_to_position(text, loc);
            let end = byte_offset_to_position(text, loc + rv.relname.len());
            diagnostics.push(Diagnostic {
                range: Range { start, end },
                severity: Some(DiagnosticSeverity::ERROR),
                source: Some("sqltype".to_string()),
                message: format!("Table \"{}\" does not exist in schema catalog", rv.relname),
                ..Default::default()
            });
        }
    }

    diagnostics
}

fn format_column_hover(parts: &[String], catalog: &Catalog) -> String {
    if parts.len() >= 2 {
        let table_or_alias = &parts[0];
        let col_name = &parts[1];
        if let Some(table) = catalog.get_table(table_or_alias)
            && let Some(col) = table.get_column(col_name)
        {
            return format!(
                "### Column `{}.{}`\n- **PostgreSQL Type**: `{}`\n- **TypeScript Type**: `{}`\n- **Nullable**: `{}`",
                table_or_alias, col_name, col.pg_type, col.ts_type, col.is_nullable
            );
        }
    }

    let col_name = parts.last().cloned().unwrap_or_default();
    for table in catalog.tables.values() {
        if let Some(col) = table.get_column(&col_name) {
            return format!(
                "### Column `{}.{}`\n- **PostgreSQL Type**: `{}`\n- **TypeScript Type**: `{}`\n- **Nullable**: `{}`",
                table.name, col_name, col.pg_type, col.ts_type, col.is_nullable
            );
        }
    }

    format!("### Column `{}`", parts.join("."))
}

/// Resolves hover information at the given position in the SQL text.
pub fn resolve_hover(text: &str, pos: Position, catalog: &Catalog) -> Option<Hover> {
    let byte_offset = position_to_byte_offset(text, pos);
    let parsed = pg_query::parse(text).ok()?;
    let analyzed = analyze_query(text, catalog, None).ok();

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

                let md = if let Some(ref query) = analyzed {
                    if let Some(param) = query.params.iter().find(|p| p.index == pr.number as usize) {
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
            if byte_offset >= loc && byte_offset <= loc + rv.relname.len() {
                let start = byte_offset_to_position(text, loc);
                let end = byte_offset_to_position(text, loc + rv.relname.len());

                let md = if let Some(table) = catalog.get_table(&rv.relname) {
                    let mut cols = Vec::new();
                    for col in &table.columns {
                        let null_str = if col.is_nullable { "nullable" } else { "non-null" };
                        cols.push(format!("- `{}`: `{}` ({})", col.name, col.pg_type, null_str));
                    }
                    format!("### Table `{}`\n**Columns**:\n{}", table.name, cols.join("\n"))
                } else {
                    format!("### Table `{}`\n*(Not found in schema catalog)*", rv.relname)
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

                let md = if let Some(ref query) = analyzed {
                    let col_name = parts.last().unwrap();
                    if let Some(field) = query.fields.iter().find(|f| &f.name == col_name) {
                        format!(
                            "### Column `{}`\n- **TypeScript Type**: `{}`",
                            col_name, field.ts_type
                        )
                    } else {
                        format_column_hover(&parts, catalog)
                    }
                } else {
                    format_column_hover(&parts, catalog)
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

#[derive(Debug, Clone)]
struct AnalysisJob {
    uri: Uri,
    version: i32,
    text: String,
    job_id: u64,
}

fn cast_req<P>(req: Request) -> Result<(RequestId, P), Box<dyn std::error::Error>>
where
    P: serde::de::DeserializeOwned,
{
    let p = serde_json::from_value(req.params)?;
    Ok((req.id, p))
}

fn cast_notif<P>(notif: Notification) -> Result<P, Box<dyn std::error::Error>>
where
    P: serde::de::DeserializeOwned,
{
    let p = serde_json::from_value(notif.params)?;
    Ok(p)
}

/// Runs the LSP server over stdio.
pub fn run_lsp_server<P: AsRef<Path>>(migrations_dir: P) -> Result<(), Box<dyn std::error::Error>> {
    let migrations_path = migrations_dir.as_ref().to_path_buf();

    let (connection, io_threads) = Connection::stdio();

    let init_result = InitializeResult {
        capabilities: ServerCapabilities {
            text_document_sync: Some(TextDocumentSyncCapability::Kind(TextDocumentSyncKind::FULL)),
            hover_provider: Some(HoverProviderCapability::Simple(true)),
            ..Default::default()
        },
        server_info: Some(ServerInfo {
            name: "sqltype-lsp".to_string(),
            version: Some(env!("CARGO_PKG_VERSION").to_string()),
        }),
    };

    let _init_params = connection.initialize(serde_json::to_value(init_result)?)?;

    // In-memory VFS mapping uri_string -> (version, text)
    let vfs: Arc<RwLock<HashMap<String, (i32, String)>>> = Arc::new(RwLock::new(HashMap::new()));

    // Schema catalog snapshot with lock-free atomic swap
    let initial_catalog = Catalog::load_from_dir(&migrations_path).unwrap_or_default();
    let catalog = Arc::new(ArcSwap::from_pointee(initial_catalog));

    // Channels and job counter for background analysis worker
    let (job_sender, job_receiver) = unbounded::<AnalysisJob>();
    let latest_job_counter = Arc::new(AtomicU64::new(0));
    let uri_latest_jobs: Arc<Mutex<HashMap<String, u64>>> = Arc::new(Mutex::new(HashMap::new()));

    // Spawn analysis worker thread
    {
        let catalog = Arc::clone(&catalog);
        let uri_latest_jobs = Arc::clone(&uri_latest_jobs);
        let msg_sender = connection.sender.clone();

        std::thread::spawn(move || {
            while let Ok(job) = job_receiver.recv() {
                // Superseded job cancellation check
                {
                    let guard = uri_latest_jobs.lock().unwrap();
                    if let Some(&latest) = guard.get(job.uri.as_str())
                        && job.job_id < latest
                    {
                        continue;
                    }
                }

                let catalog_snapshot = catalog.load();
                let diagnostics = validate_sql(&job.text, &catalog_snapshot);

                let notif = Notification {
                    method: "textDocument/publishDiagnostics".to_string(),
                    params: serde_json::to_value(PublishDiagnosticsParams {
                        uri: job.uri,
                        diagnostics,
                        version: Some(job.version),
                    })
                    .unwrap(),
                };
                let _ = msg_sender.send(Message::Notification(notif));
            }
        });
    }

    // Spawn migration file watcher thread
    {
        let catalog = Arc::clone(&catalog);
        let vfs = Arc::clone(&vfs);
        let job_sender = job_sender.clone();
        let latest_job_counter = Arc::clone(&latest_job_counter);
        let uri_latest_jobs = Arc::clone(&uri_latest_jobs);
        let migrations_path_clone = migrations_path.clone();

        std::thread::spawn(move || {
            if !migrations_path_clone.exists() {
                return;
            }

            let (tx, rx) = std::sync::mpsc::channel();
            let mut watcher = match notify::recommended_watcher(tx) {
                Ok(w) => w,
                Err(_) => return,
            };

            if watcher
                .watch(&migrations_path_clone, RecursiveMode::Recursive)
                .is_err()
            {
                return;
            }

            while let Ok(res) = rx.recv() {
                if let Ok(event) = res {
                    let has_sql = event.paths.iter().any(|p| {
                        p.extension()
                            .map(|e| e.eq_ignore_ascii_case("sql"))
                            .unwrap_or(false)
                    });

                    if has_sql
                        && let Ok(new_cat) = Catalog::load_from_dir(&migrations_path_clone)
                    {
                        catalog.store(Arc::new(new_cat));

                        // Re-trigger analysis on all active open buffers
                        let vfs_docs = vfs.read().unwrap().clone();
                        for (uri_str, (version, text)) in vfs_docs {
                            if let Ok(uri) = uri_str.parse::<Uri>() {
                                let job_id = latest_job_counter.fetch_add(1, Ordering::SeqCst) + 1;
                                uri_latest_jobs.lock().unwrap().insert(uri_str, job_id);
                                let _ = job_sender.send(AnalysisJob {
                                    uri,
                                    version,
                                    text,
                                    job_id,
                                });
                            }
                        }
                    }
                }
            }
        });
    }

    // Main LSP server event loop
    for msg in &connection.receiver {
        match msg {
            Message::Request(req) => {
                if connection.handle_shutdown(&req)? {
                    return Ok(());
                }

                if req.method == "textDocument/hover" {
                    let (id, params) = cast_req::<HoverParams>(req)?;
                    let vfs_guard = vfs.read().unwrap();
                    let hover_res = if let Some((_, text)) =
                        vfs_guard.get(params.text_document_position_params.text_document.uri.as_str())
                    {
                        let cat_snapshot = catalog.load();
                        resolve_hover(
                            text,
                            params.text_document_position_params.position,
                            &cat_snapshot,
                        )
                    } else {
                        None
                    };

                    let resp = Response {
                        id,
                        result: Some(serde_json::to_value(hover_res)?),
                        error: None,
                    };
                    connection.sender.send(Message::Response(resp))?;
                }
            }
            Message::Notification(notif) => {
                match notif.method.as_str() {
                    "textDocument/didOpen" => {
                        if let Ok(params) = cast_notif::<DidOpenTextDocumentParams>(notif) {
                            let uri = params.text_document.uri;
                            let uri_str = uri.to_string();
                            let version = params.text_document.version;
                            let text = params.text_document.text;

                            vfs.write().unwrap().insert(uri_str.clone(), (version, text.clone()));
                            let job_id = latest_job_counter.fetch_add(1, Ordering::SeqCst) + 1;
                            uri_latest_jobs.lock().unwrap().insert(uri_str, job_id);
                            let _ = job_sender.send(AnalysisJob {
                                uri,
                                version,
                                text,
                                job_id,
                            });
                        }
                    }
                    "textDocument/didChange" => {
                        if let Ok(params) = cast_notif::<DidChangeTextDocumentParams>(notif) {
                            let uri = params.text_document.uri;
                            let uri_str = uri.to_string();
                            let version = params.text_document.version;
                            if let Some(change) = params.content_changes.into_iter().last() {
                                let text = change.text;
                                vfs.write().unwrap().insert(uri_str.clone(), (version, text.clone()));
                                let job_id = latest_job_counter.fetch_add(1, Ordering::SeqCst) + 1;
                                uri_latest_jobs.lock().unwrap().insert(uri_str, job_id);
                                let _ = job_sender.send(AnalysisJob {
                                    uri,
                                    version,
                                    text,
                                    job_id,
                                });
                            }
                        }
                    }
                    "textDocument/didClose" => {
                        if let Ok(params) = cast_notif::<DidCloseTextDocumentParams>(notif) {
                            let uri_str = params.text_document.uri.to_string();
                            vfs.write().unwrap().remove(&uri_str);
                            uri_latest_jobs.lock().unwrap().remove(&uri_str);
                        }
                    }
                    _ => {}
                }
            }
            Message::Response(_) => {}
        }
    }

    io_threads.join()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::TableMetadata;

    #[test]
    fn test_byte_offset_to_position_and_back() {
        let text = "SELECT\n  id,\n  email\nFROM users;";
        // 'SELECT' -> 0..6
        // '\n' -> 6
        // '  id,' -> 7..12
        // '\n' -> 12
        // '  email' -> 13..20
        let pos_id = byte_offset_to_position(text, 9);
        assert_eq!(pos_id.line, 1);
        assert_eq!(pos_id.character, 2);

        let offset_back = position_to_byte_offset(text, pos_id);
        assert_eq!(offset_back, 9);

        // Beginning of document
        assert_eq!(
            byte_offset_to_position(text, 0),
            Position {
                line: 0,
                character: 0
            }
        );
        assert_eq!(
            position_to_byte_offset(
                text,
                Position {
                    line: 0,
                    character: 0
                }
            ),
            0
        );
    }

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
        assert!(diags[0]
            .message
            .contains("Table \"nonexistent_table\" does not exist in schema catalog"));
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
                },
                crate::catalog::ColumnMetadata {
                    name: "active".to_string(),
                    pg_type: "bool".to_string(),
                    ts_type: "boolean".to_string(),
                    is_nullable: false,
                    has_default: false,
                },
            ],
        };
        catalog.tables.insert("users".to_string(), users_table);

        let sql = "WITH active_users AS (SELECT id FROM users WHERE active = true) SELECT au.id FROM active_users au;";
        let diags = validate_sql(sql, &catalog);
        assert!(diags.is_empty());
    }

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
                },
                crate::catalog::ColumnMetadata {
                    name: "name".to_string(),
                    pg_type: "text".to_string(),
                    ts_type: "string".to_string(),
                    is_nullable: true,
                    has_default: false,
                },
            ],
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
}
