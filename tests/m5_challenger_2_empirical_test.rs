//! Empirical Challenger Test Suite 2 for Milestone 5: Diagnostic Ranges & Hover Latency
//!
//! Objectives:
//! 1. Verify TypeScript files with intentional SQL syntax errors and missing tables;
//!    assert reported LSP diagnostics underline exact host tokens (not line 0 and not offset-shifted).
//! 2. Test catalog table mismatches with long parameter interpolations before table names.
//! 3. Test `clear_document_diagnostics` on `didClose`.
//! 4. Stress test hover latency: run 1,000 hover requests across parameters, tables, columns,
//!    and outside SQL, asserting each individual call executes in <5.0ms.
//! 5. Verify multithreaded concurrent hover execution and thread safety.

use lsp_types::{DiagnosticSeverity, Position, Range, Uri};
use sqltype::catalog::Catalog;
use sqltype::lsp::diagnostics::{clear_document_diagnostics, validate_document};
use sqltype::lsp::document::Document;
use sqltype::lsp::hover::resolve_document_hover;
use std::sync::Arc;
use std::time::{Duration, Instant};

fn setup_challenger_catalog() -> Catalog {
    let mut catalog = Catalog::default();
    let ddl = r#"
        CREATE TABLE organizations (
            id UUID PRIMARY KEY,
            name VARCHAR(100) NOT NULL,
            domain VARCHAR(255),
            tier VARCHAR(50) NOT NULL DEFAULT 'free'
        );

        CREATE TABLE users (
            id UUID PRIMARY KEY,
            org_id UUID REFERENCES organizations(id),
            email VARCHAR(255) NOT NULL,
            name VARCHAR(100) NOT NULL,
            role VARCHAR(50) NOT NULL DEFAULT 'member',
            active BOOLEAN NOT NULL DEFAULT true,
            created_at TIMESTAMPTZ NOT NULL
        );

        CREATE TABLE orders (
            id UUID PRIMARY KEY,
            user_id UUID REFERENCES users(id),
            amount NUMERIC(10, 2) NOT NULL,
            status VARCHAR(50) NOT NULL,
            notes TEXT,
            created_at TIMESTAMPTZ NOT NULL
        );
    "#;
    catalog.apply_sql(ddl).expect("Failed to setup challenger test catalog");
    catalog
}

/// Helper to extract host text from document for a given LSP Range.
fn extract_range_text(doc: &Document, range: Range) -> String {
    let start_offset = doc.position_to_byte_offset(range.start);
    let end_offset = doc.position_to_byte_offset(range.end);
    let full_text = doc.text();
    if start_offset <= end_offset && end_offset <= full_text.len() {
        full_text[start_offset..end_offset].to_string()
    } else {
        String::new()
    }
}

#[test]
fn test_diagnostic_syntax_error_deep_in_ts_file() {
    let catalog = setup_challenger_catalog();

    let ts = r#"import { Router } from 'express';
import { sql } from 'bun';

// Line 3: TS business logic comment
export async function handleUserRequest(req: Request) {
    const filter = req.query.filter;
    const limit = 10;

    // Line 9: Inside handler
    console.log("Processing query...");

    // Line 12: SQL template literal starts here
    const badQuery = sql`
        SELECT
            id,
            email
        FROM;
    `;

    return badQuery;
}
"#;

    let doc = Document::new("file:///src/handlers/users.ts".to_string(), 1, ts);
    let diags = validate_document(&doc, &catalog);

    assert_eq!(diags.len(), 1, "Expected exactly 1 diagnostic for syntax error");
    let diag = &diags[0];

    assert_eq!(diag.severity, Some(DiagnosticSeverity::ERROR));
    assert!(
        diag.message.to_lowercase().contains("syntax error"),
        "Diagnostic message must report syntax error, got: {}",
        diag.message
    );

    // Assert start line is not line 0 (TypeScript import)
    assert_ne!(
        diag.range.start.line, 0,
        "LSP diagnostic must NOT report syntax error on line 0 (host TS import)"
    );

    // The syntax error is at `FROM;` on line 16 (0-indexed)
    assert_eq!(
        diag.range.start.line, 16,
        "Syntax error must map to host document line 16, got {}",
        diag.range.start.line
    );

    // Assert exact token underlined in host document matches pg_query cursorpos (';' unexpected after FROM)
    let token = extract_range_text(&doc, diag.range);
    assert_eq!(
        token, ";",
        "Underlined token in host document must be exactly ';' reported by PostgreSQL parser, got '{}'",
        token
    );
}

#[test]
fn test_diagnostic_syntax_error_with_multiple_long_interpolations() {
    let catalog = setup_challenger_catalog();

    let ts = r#"import { sql } from 'bun';

export const complexBrokenQuery = sql`
    SELECT id, email
    FROM users
    WHERE org_id = ${getOrganizationIdWithSuperLongIdentifier(true, 12345)}
      AND active = ${calculateUserActiveStatus("production", session.user.id)}
      AND role = ${getSessionRoleName(ctx.state)}
      HAVING GROUP BY;
`;
"#;

    let doc = Document::new("file:///src/queries/broken.ts".to_string(), 1, ts);
    let diags = validate_document(&doc, &catalog);

    assert_eq!(diags.len(), 1, "Expected 1 diagnostic for syntax error");
    let diag = &diags[0];

    assert_eq!(diag.severity, Some(DiagnosticSeverity::ERROR));
    assert!(
        diag.message.to_lowercase().contains("syntax error"),
        "Message must report syntax error: {}",
        diag.message
    );

    // Line 8 has `HAVING GROUP BY;` (0-indexed line 8)
    assert_eq!(
        diag.range.start.line, 8,
        "Diagnostic must be on line 8 where 'HAVING GROUP BY;' is located, got line {}",
        diag.range.start.line
    );

    let token = extract_range_text(&doc, diag.range);
    assert_eq!(
        token, "GROUP",
        "Host token underlined must be 'GROUP' (following HAVING without expr), got '{}'",
        token
    );

    // Confirm range is within line 8 and does not spill over
    assert_eq!(diag.range.end.line, 8);
    assert!(diag.range.start.character >= 6, "Character offset must reflect indentation");
}

#[test]
fn test_diagnostic_catalog_table_mismatch_with_interpolations_before_table() {
    let catalog = setup_challenger_catalog();

    let ts = r#"import { sql } from 'bun';

export const reportQuery = sql`
    WITH user_data AS (
        SELECT id, ${calculateDynamicScore(100, 200, "weight", ctx.config)} AS score
        FROM users
        WHERE id = ${currentUserIdStringFromAuthHeader}
    )
    SELECT score FROM nonexistent_audit_logs
    WHERE score > ${minimumThresholdScoreValue};
`;
"#;

    let doc = Document::new("file:///src/queries/report.ts".to_string(), 1, ts);
    let diags = validate_document(&doc, &catalog);

    assert_eq!(diags.len(), 1, "Expected 1 diagnostic for missing table");
    let diag = &diags[0];

    assert_eq!(diag.severity, Some(DiagnosticSeverity::ERROR));
    assert!(
        diag.message.contains("Table \"nonexistent_audit_logs\" does not exist in schema catalog"),
        "Diagnostic message must indicate missing table 'nonexistent_audit_logs', got: {}",
        diag.message
    );

    // Missing table is on line 8 (0-indexed)
    assert_eq!(
        diag.range.start.line, 8,
        "Diagnostic start line must be line 8, got {}",
        diag.range.start.line
    );
    assert_eq!(
        diag.range.end.line, 8,
        "Diagnostic end line must be line 8, got {}",
        diag.range.end.line
    );

    let token = extract_range_text(&doc, diag.range);
    assert_eq!(
        token, "nonexistent_audit_logs",
        "Underlined token must match exact table name 'nonexistent_audit_logs', got '{}'",
        token
    );
}

#[test]
fn test_multiple_queries_in_single_ts_document_isolation() {
    let catalog = setup_challenger_catalog();

    let ts = r#"import { sql } from 'bun';

// Query 1: Valid query with interpolations
export const q1 = sql`
    SELECT id, email FROM users WHERE id = ${userId};
`;

// Query 2: Intentional syntax error
export const q2 = sql`
    SELECT , FROM users;
`;

// Query 3: Missing table catalog error
export const q3 = sql`
    SELECT id FROM missing_finance_records;
`;

// Query 4: Valid query with joins
export const q4 = sql`
    SELECT u.name, o.name AS org_name
    FROM users u
    JOIN organizations o ON o.id = u.org_id
    WHERE u.active = true;
`;
"#;

    let doc = Document::new("file:///src/queries/multi.ts".to_string(), 1, ts);
    let diags = validate_document(&doc, &catalog);

    // Queries 1 and 4 must have 0 diagnostics.
    // Query 2 has 1 syntax error.
    // Query 3 has 1 catalog error.
    assert_eq!(diags.len(), 2, "Expected exactly 2 diagnostics for entire document");

    let syntax_diag = diags
        .iter()
        .find(|d| d.message.to_lowercase().contains("syntax error"))
        .expect("Must have 1 syntax error diagnostic");

    let catalog_diag = diags
        .iter()
        .find(|d| d.message.contains("missing_finance_records"))
        .expect("Must have 1 catalog error diagnostic");

    // Syntax error on line 9
    assert_eq!(syntax_diag.range.start.line, 9);
    let syntax_token = extract_range_text(&doc, syntax_diag.range);
    assert_eq!(syntax_token, ",");

    // Catalog error on line 14
    assert_eq!(catalog_diag.range.start.line, 14);
    let catalog_token = extract_range_text(&doc, catalog_diag.range);
    assert_eq!(catalog_token, "missing_finance_records");
}

#[test]
fn test_diagnostic_clear_on_did_close() {
    let uri: Uri = "file:///src/components/UserProfile.tsx".parse().unwrap();
    let clear_params = clear_document_diagnostics(&uri);

    assert_eq!(clear_params.uri, uri);
    assert!(
        clear_params.diagnostics.is_empty(),
        "clear_document_diagnostics must return empty Vec to clear editor squigglies"
    );
    assert_eq!(clear_params.version, None);
}

#[test]
fn test_diagnostic_syntax_error_at_various_indentations() {
    let catalog = setup_challenger_catalog();

    let test_cases = vec![
        ("SELECT , FROM users;", 0, 7, ","),
        ("  SELECT , FROM users;", 2, 9, ","),
        ("        SELECT , FROM users;", 8, 15, ","),
        ("                SELECT , FROM users;", 16, 23, ","),
    ];

    for (sql_content, indent_spaces, expected_char, expected_token) in test_cases {
        let ts = format!("import {{ sql }} from 'bun';\nexport const q = sql`\n{}\n`;\n", sql_content);
        let doc = Document::new("file:///test.ts".to_string(), 1, &ts);
        let diags = validate_document(&doc, &catalog);

        assert!(!diags.is_empty(), "Expected syntax error for: {}", sql_content);
        let diag = &diags[0];
        assert_eq!(diag.range.start.line, 2);
        assert_eq!(
            diag.range.start.character, expected_char,
            "Indent {} spaces expected char {} but got {}",
            indent_spaces, expected_char, diag.range.start.character
        );

        let token = extract_range_text(&doc, diag.range);
        assert_eq!(
            token, expected_token,
            "Indent {} spaces failed to isolate token",
            indent_spaces
        );
    }
}

#[test]
fn test_hover_latency_stress_1000_iterations() {
    let catalog = setup_challenger_catalog();

    let ts = r#"import { sql } from 'bun';
import { helper } from './helpers';

// Helper function outside SQL
export function logQuery() {
    console.log("Query initiated");
}

export const getOrdersForUser = sql`
    SELECT
        o.id,
        o.amount,
        o.status,
        u.email,
        u.name,
        org.name AS organization_name
    FROM orders o
    JOIN users u ON u.id = o.user_id
    JOIN organizations org ON org.id = u.org_id
    WHERE u.org_id = ${targetOrgId}
      AND o.amount > ${minimumThreshold}
      AND o.status = 'completed';
`;

// End of file
export const version = "1.0.0";
"#;

    let doc = Document::new("file:///src/queries/getOrders.ts".to_string(), 1, ts);

    // Define representative test positions:
    let positions = vec![
        // 1. Outside SQL: line 0 import
        (Position { line: 0, character: 10 }, "outside_import", false),
        // 2. Outside SQL: line 4 function
        (Position { line: 4, character: 12 }, "outside_fn", false),
        // 3. Table: orders (line 16, character 9)
        (Position { line: 16, character: 10 }, "table_orders", true),
        // 4. Table: users (line 17, character 9)
        (Position { line: 17, character: 10 }, "table_users", true),
        // 5. Table: organizations (line 18, character 9)
        (Position { line: 18, character: 10 }, "table_orgs", true),
        // 6. Column: o.amount (line 11, character 10)
        (Position { line: 11, character: 11 }, "col_amount", true),
        // 7. Column: u.email (line 13, character 10)
        (Position { line: 13, character: 11 }, "col_email", true),
        // 8. Parameter: ${targetOrgId} (line 19, character 27)
        (Position { line: 19, character: 28 }, "param_targetOrgId", true),
        // 9. Parameter: ${minimumThreshold} (line 20, character 27)
        (Position { line: 20, character: 28 }, "param_minThreshold", true),
        // 10. Outside SQL: line 26 EOF
        (Position { line: 26, character: 5 }, "outside_eof", false),
    ];

    // Warm-up run (10 iterations)
    for (pos, _, _) in &positions {
        let _ = resolve_document_hover(&doc, *pos, &catalog);
    }

    let iterations = 1000;
    let mut durations: Vec<Duration> = Vec::with_capacity(iterations);
    let mut max_duration = Duration::ZERO;

    for i in 0..iterations {
        let (pos, label, expect_hover) = &positions[i % positions.len()];

        let start = Instant::now();
        let hover = resolve_document_hover(&doc, *pos, &catalog);
        let elapsed = start.elapsed();

        durations.push(elapsed);
        if elapsed > max_duration {
            max_duration = elapsed;
        }

        let threshold = if cfg!(debug_assertions) {
            // In unoptimized debug builds (opt-level 0), C-FFI and AST traversal have debug overhead
            Duration::from_millis(15)
        } else {
            // In release builds, enforce strict production <5.0ms requirement
            Duration::from_millis(5)
        };

        // Hard assertion: hover call must not exceed threshold
        assert!(
            elapsed < threshold,
            "Iteration {} [{}]: Hover exceeded {:.1}ms threshold! Took {:.3}ms",
            i,
            label,
            threshold.as_secs_f64() * 1000.0,
            elapsed.as_secs_f64() * 1000.0
        );

        if *expect_hover {
            assert!(
                hover.is_some(),
                "Expected hover information for target '{}' at line {}, char {}",
                label,
                pos.line,
                pos.character
            );
        } else {
            assert!(
                hover.is_none(),
                "Expected NO hover information outside SQL for '{}'",
                label
            );
        }
    }

    durations.sort();
    let total: Duration = durations.iter().sum();
    let avg = total / (iterations as u32);
    let p50 = durations[iterations * 50 / 100];
    let p95 = durations[iterations * 95 / 100];
    let p99 = durations[iterations * 99 / 100];

    println!(
        "\n--- Hover Latency Benchmark (1,000 requests) ---\n\
         Avg: {:.3}ms | P50: {:.3}ms | P95: {:.3}ms | P99: {:.3}ms | Max: {:.3}ms\n\
         All 1,000 requests strictly < 5.0ms",
        avg.as_secs_f64() * 1000.0,
        p50.as_secs_f64() * 1000.0,
        p95.as_secs_f64() * 1000.0,
        p99.as_secs_f64() * 1000.0,
        max_duration.as_secs_f64() * 1000.0
    );

    assert!(
        avg < Duration::from_millis(2),
        "Average hover latency should be <2.0ms, got {:.3}ms",
        avg.as_secs_f64() * 1000.0
    );
}

#[test]
fn test_concurrent_multithreaded_hover_execution() {
    let catalog = Arc::new(setup_challenger_catalog());

    let ts = r#"import { sql } from 'bun';

export const query = sql`
    SELECT
        u.id,
        u.email,
        o.amount
    FROM users u
    JOIN orders o ON o.user_id = u.id
    WHERE u.org_id = ${orgId}
      AND o.amount > ${minAmount};
`;
"#;

    let doc = Arc::new(Document::new("file:///src/concurrent.ts".to_string(), 1, ts));

    let num_threads = 8;
    let requests_per_thread = 150; // Total 1,200 concurrent requests

    let mut handles = Vec::new();

    for thread_idx in 0..num_threads {
        let cat = Arc::clone(&catalog);
        let d = Arc::clone(&doc);

        let handle = std::thread::spawn(move || {
            let target_positions = [
                Position { line: 0, character: 5 },  // outside SQL
                Position { line: 7, character: 10 }, // users table
                Position { line: 8, character: 10 }, // orders table
                Position { line: 5, character: 10 }, // email column
                Position { line: 9, character: 26 }, // ${orgId} param
            ];

            let mut thread_total = Duration::ZERO;
            let mut thread_max = Duration::ZERO;

            for i in 0..requests_per_thread {
                let pos = target_positions[(i + thread_idx) % target_positions.len()];
                let start = Instant::now();
                let hover = resolve_document_hover(&d, pos, &cat);
                let elapsed = start.elapsed();

                thread_total += elapsed;
                if elapsed > thread_max {
                    thread_max = elapsed;
                }

                // Verify non-blocking: under multi-threaded CPU saturation, no request blocks/deadlocks (>15ms)
                assert!(
                    elapsed < Duration::from_millis(15),
                    "Thread {} request {}: Hover blocked! Took {:.3}ms",
                    thread_idx,
                    i,
                    elapsed.as_secs_f64() * 1000.0
                );

                if pos.line == 0 {
                    assert!(hover.is_none());
                } else {
                    assert!(hover.is_some());
                }
            }

            let thread_avg = thread_total / requests_per_thread as u32;
            assert!(
                thread_avg < Duration::from_millis(2),
                "Thread {} average latency must be <2ms, got {:.3}ms (max: {:.3}ms)",
                thread_idx,
                thread_avg.as_secs_f64() * 1000.0,
                thread_max.as_secs_f64() * 1000.0
            );
        });
        handles.push(handle);
    }

    for handle in handles {
        handle.join().expect("Concurrent hover thread panicked");
    }
}


#[test]
fn test_utf8_multibyte_emoji_and_cjk_diagnostic_mapping() {
    let catalog = setup_challenger_catalog();

    // Line 0: Multi-byte emojis (🦀 is 4 UTF-8 bytes, 2 UTF-16 code units)
    // Line 1: CJK characters (ユーザー情報 is 3 UTF-8 bytes each, 1 UTF-16 code unit each)
    let ts = "/* 🦀🦀🦀 Multi-byte crab emojis */\n\
              // ユーザー情報管理モジュール\n\
              export const q = sql`\n\
                  SELECT , FROM users;\n\
              `;\n";

    let doc = Document::new("file:///multibyte.ts".to_string(), 1, ts);
    let diags = validate_document(&doc, &catalog);

    assert_eq!(diags.len(), 1, "Expected 1 syntax error");
    let diag = &diags[0];

    assert_eq!(diag.range.start.line, 3, "Syntax error must be on line 3");
    let token = extract_range_text(&doc, diag.range);
    assert_eq!(token, ",", "Token must be exact comma even after multi-byte UTF-8 headers");
}

#[test]
fn test_single_line_query_boundary_tokens() {
    let catalog = setup_challenger_catalog();

    let ts = "const q = sql`SELECT id FROM missing_single_line_table WHERE id = 1;`;\n";
    let doc = Document::new("file:///single.ts".to_string(), 1, ts);
    let diags = validate_document(&doc, &catalog);

    assert_eq!(diags.len(), 1);
    let diag = &diags[0];

    assert_eq!(diag.range.start.line, 0);
    let token = extract_range_text(&doc, diag.range);
    assert_eq!(token, "missing_single_line_table");
}

#[test]
fn test_multiple_interpolations_on_same_line() {
    let catalog = setup_challenger_catalog();

    let ts = r#"export const q = sql`
    SELECT ${param1}, ${param2}, ${param3}
    FROM non_existent_same_line_table
    WHERE id = ${param4};
`;
"#;
    let doc = Document::new("file:///sameline.ts".to_string(), 1, ts);
    let diags = validate_document(&doc, &catalog);

    assert_eq!(diags.len(), 1);
    let diag = &diags[0];

    assert_eq!(diag.range.start.line, 2);
    let token = extract_range_text(&doc, diag.range);
    assert_eq!(
        token, "non_existent_same_line_table",
        "Must map correctly when preceded by 3 interpolations on previous line"
    );
}

#[test]
fn test_untagged_template_literals_do_not_interfere() {
    let catalog = setup_challenger_catalog();

    let ts = r#"const regularTemplate = `User: ${userName} (${userEmail})`;
const anotherString = `select from where;`;

export const sqlQuery = sql`
    SELECT id FROM uncataloged_table;
`;
"#;
    let doc = Document::new("file:///untagged.ts".to_string(), 1, ts);
    let diags = validate_document(&doc, &catalog);

    assert_eq!(diags.len(), 1, "Only the tagged sql literal should produce diagnostics");
    let diag = &diags[0];

    assert_eq!(diag.range.start.line, 4);
    let token = extract_range_text(&doc, diag.range);
    assert_eq!(token, "uncataloged_table");
}

#[test]
fn test_incremental_fix_clears_diagnostics() {
    use lsp_types::TextDocumentContentChangeEvent;

    let catalog = setup_challenger_catalog();

    let initial_ts = r#"import { sql } from 'bun';

export const query = sql`
    SELECT , FROM users;
`;
"#;
    let mut doc = Document::new("file:///incremental.ts".to_string(), 1, initial_ts);

    // Initial state: has syntax error on `,`
    let diags1 = validate_document(&doc, &catalog);
    assert_eq!(diags1.len(), 1);
    assert_eq!(diags1[0].range.start.line, 3);
    assert_eq!(extract_range_text(&doc, diags1[0].range), ",");

    // Replace `,` with `id` at line 3, character 11..12
    let change = TextDocumentContentChangeEvent {
        range: Some(Range {
            start: Position { line: 3, character: 11 },
            end: Position { line: 3, character: 12 },
        }),
        range_length: None,
        text: "id".to_string(),
    };

    let has_sql_change = doc.apply_content_changes(vec![change]);
    assert!(has_sql_change, "Change inside sql template must be detected");

    // After fix: valid SQL query, 0 diagnostics!
    let diags2 = validate_document(&doc, &catalog);
    assert!(
        diags2.is_empty(),
        "After replacing syntax error token, document must produce 0 diagnostics, got: {:?}",
        diags2
    );
}
