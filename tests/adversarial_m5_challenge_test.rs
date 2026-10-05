//! Adversarial Empirical Challenge Suite for Milestone 5 (Incremental LSP & TS Scanner)
//!
//! Stress-tests:
//! 1. `ts_scanner.rs`: multi-byte Unicode strings (emojis, CJK, Cyrillic, accents), mixed quotes,
//!    escaped backticks, complex nested object/array/arrow interpolations, and source map accuracy.
//! 2. `lsp::document::Document`: rapid keystroke bursts, backspaces, multiline paste/cut,
//!    strict bypass (`has_sql_change == false`) for edits outside SQL templates, and UTF-16 position translations.
//! 3. `lsp::hover` & `lsp::diagnostics`: coordinate mapping accuracy under severe offset shifts and <5ms latency.

use lsp_types::{DiagnosticSeverity, Position, Range, TextDocumentContentChangeEvent};
use std::time::{Duration, Instant};

use sqltype::catalog::Catalog;
use sqltype::lsp::diagnostics::{clear_document_diagnostics, validate_document};
use sqltype::lsp::document::{Document, byte_offset_to_position, position_to_byte_offset};
use sqltype::lsp::hover::resolve_document_hover;
use sqltype::ts_scanner::scan_ts_queries;

fn setup_test_catalog() -> Catalog {
    let mut catalog = Catalog::default();
    catalog
        .apply_sql(
            r#"
            CREATE TABLE users (
                id UUID PRIMARY KEY,
                username VARCHAR(50) NOT NULL,
                email TEXT NOT NULL,
                bio TEXT,
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            );

            CREATE TABLE orders (
                id BIGSERIAL PRIMARY KEY,
                user_id UUID NOT NULL REFERENCES users(id),
                total_cents INT NOT NULL,
                status VARCHAR(20) NOT NULL
            );
            "#,
        )
        .expect("Setup schema DDL must succeed");
    catalog
}

// =========================================================================
// 1. TS Scanner Adversarial Tests
// =========================================================================

#[test]
fn test_adv_ts_scanner_multibyte_unicode_and_emojis() {
    // Test multi-byte UTF-8 across emojis (4 bytes), CJK (3 bytes), Cyrillic (2 bytes), and accented chars (2 bytes)
    let ts_source = r#"
        import { sql } from 'bun';

        // 🦀 Crab comment: データベース & Привет мир
        export const complexUnicodeQuery = sql`
            SELECT id, '🦀' AS crab, '你好世界' AS cjk, 'München' AS accent
            FROM users
            WHERE bio LIKE ${`%${pattern}%`}
              AND username = ${userName};
        `;
    "#;

    let queries = scan_ts_queries(ts_source);
    assert_eq!(
        queries.len(),
        1,
        "Should find 1 query with unicode characters"
    );
    let q = &queries[0];

    assert_eq!(q.name.as_deref(), Some("ComplexUnicodeQuery"));
    assert!(
        q.sql.contains("'🦀' AS crab"),
        "Emoji literal must be preserved verbatim"
    );
    assert!(
        q.sql.contains("'你好世界' AS cjk"),
        "CJK literal must be preserved verbatim"
    );
    assert!(
        q.sql.contains("'München' AS accent"),
        "Accented literal must be preserved verbatim"
    );

    // Check parameter replacement: $1 and $2
    assert!(
        q.sql.contains("WHERE bio LIKE $1"),
        "First interpolation must be mapped to $1"
    );
    assert!(
        q.sql.contains("AND username = $2"),
        "Second interpolation must be mapped to $2"
    );

    assert_eq!(q.source_map.interpolations.len(), 2);
    assert_eq!(q.source_map.interpolations[0].index, 1);
    assert_eq!(q.source_map.interpolations[1].index, 2);
    assert_eq!(q.source_map.interpolations[1].expr, "userName");

    // Ensure host span encompasses backticks exactly
    assert!(q.host_span.start < q.host_span.end);
    let backtick_slice = &ts_source[q.host_span.clone()];
    assert!(backtick_slice.starts_with('`'));
    assert!(backtick_slice.ends_with('`'));
}

#[test]
fn test_adv_ts_scanner_mixed_quotes_and_escaped_backticks() {
    let ts_source = r#"
        export const escapedQuery = sql`
            SELECT id, 'single \' quote', "double \" quote", \`backtick\`
            FROM users
            WHERE email = ${`user_${domain}`} AND bio = ${"simple"};
        `;
    "#;

    let queries = scan_ts_queries(ts_source);
    assert_eq!(queries.len(), 1);
    let q = &queries[0];

    // Escaped backticks in template should become single backticks in SQL content
    assert!(
        q.sql.contains("`backtick`"),
        "Escaped backticks should be preserved in content: {}",
        q.sql
    );
    assert!(q.sql.contains("$1"));
    assert!(q.sql.contains("$2"));

    // Verify source map round-trip on escaped backtick
    let backtick_pos_in_sql = q.sql.find("`backtick`").unwrap();
    let host_offset = q.source_map.sql_to_host_offset(backtick_pos_in_sql);
    assert!(host_offset < ts_source.len());
    let host_slice = &ts_source[host_offset..host_offset + 2];
    assert_eq!(
        host_slice, "\\`",
        "Mapped offset must point to the escaped backtick in host"
    );
}

#[test]
fn test_adv_ts_scanner_complex_nested_interpolations() {
    let ts_source = r#"
        export const nestedExprQuery = sql`
            SELECT id, email
            FROM users
            WHERE id = ${{ a: { b: 1, c: { d: [1, 2, 3] } } }.a.c.d[0]}
              AND username = ${((x: string) => ({ name: x }))("alice").name}
              AND created_at > ${[100, 200, 300].map(n => ({ val: n * 2 }))[1].val};
        `;
    "#;

    let queries = scan_ts_queries(ts_source);
    assert_eq!(queries.len(), 1);
    let q = &queries[0];

    assert_eq!(q.source_map.interpolations.len(), 3);
    assert_eq!(q.source_map.interpolations[0].index, 1);
    assert_eq!(q.source_map.interpolations[1].index, 2);
    assert_eq!(q.source_map.interpolations[2].index, 3);

    // Verify expressions extracted cleanly without trailing/leading braces
    assert_eq!(
        q.source_map.interpolations[0].expr,
        "{ a: { b: 1, c: { d: [1, 2, 3] } } }.a.c.d[0]"
    );
    assert!(
        q.source_map.interpolations[1]
            .expr
            .contains("((x: string) => ({ name: x }))")
    );
    assert!(
        q.source_map.interpolations[2]
            .expr
            .contains("[100, 200, 300].map(n => ({ val: n * 2 }))[1].val")
    );

    // Verify SQL parameter replacements
    assert!(q.sql.contains("WHERE id = $1"));
    assert!(q.sql.contains("AND username = $2"));
    assert!(q.sql.contains("AND created_at > $3"));
}

#[test]
fn test_adv_ts_scanner_source_mapping_adversarial_shifts() {
    // Construct massive length divergence:
    // Param 1 expression is 72 chars long, replaced by 2 chars ($1) -> shift of -70 chars.
    // Param 2 is 1 char long (x), replaced by 2 chars ($2) -> shift of +1 char.
    // Param 3 is 65 chars long, replaced by 2 chars ($3) -> shift of -63 chars.
    let ts_source = r#"
const q = sql`SELECT id FROM users WHERE id = ${superLongInterpolatedExpressionWithLotsOfWordsAndNumbers123456789} AND active = ${x} AND email = ${anotherExtremelyLongIdentifierThatDivergesDrasticallyInCharacterCount};`;
"#;

    let queries = scan_ts_queries(ts_source);
    assert_eq!(queries.len(), 1);
    let q = &queries[0];
    let map = &q.source_map;

    // Verify $1, $2, $3 exist in SQL
    let p1_sql = q.sql.find("$1").expect("Must contain $1");
    let p2_sql = q.sql.find("$2").expect("Must contain $2");
    let p3_sql = q.sql.find("$3").expect("Must contain $3");

    // 1. Map $1 back to host
    let p1_host_range = map.sql_to_host_range(p1_sql..p1_sql + 2);
    let p1_host_text = &ts_source[p1_host_range];
    assert!(
        p1_host_text
            .starts_with("${superLongInterpolatedExpressionWithLotsOfWordsAndNumbers123456789}")
    );

    // 2. Map $2 back to host
    let p2_host_range = map.sql_to_host_range(p2_sql..p2_sql + 2);
    let p2_host_text = &ts_source[p2_host_range];
    assert_eq!(p2_host_text, "${x}");

    // 3. Map $3 back to host
    let p3_host_range = map.sql_to_host_range(p3_sql..p3_sql + 2);
    let p3_host_text = &ts_source[p3_host_range];
    assert!(
        p3_host_text.starts_with(
            "${anotherExtremelyLongIdentifierThatDivergesDrasticallyInCharacterCount}"
        )
    );

    // 4. Map SQL keyword after $1 (" AND active = ")
    let and_active_sql = q.sql.find("AND active = ").unwrap();
    let and_active_host = map.sql_to_host_offset(and_active_sql);
    assert_eq!(
        &ts_source[and_active_host..and_active_host + 13],
        "AND active = "
    );

    // 5. Reverse host_to_sql_offset
    let host_active = ts_source.find("AND active = ").unwrap();
    let sql_active = map
        .host_to_sql_offset(host_active)
        .expect("Reverse offset must exist");
    assert_eq!(&q.sql[sql_active..sql_active + 13], "AND active = ");

    // 6. Param lookup by offset
    let inside_p1 = ts_source.find("superLong").unwrap();
    let found_p1 = map
        .find_interpolated_param(inside_p1)
        .expect("Should find param 1");
    assert_eq!(found_p1.index, 1);

    let inside_p2 = ts_source.find("${x}").unwrap() + 2;
    let found_p2 = map
        .find_interpolated_param(inside_p2)
        .expect("Should find param 2");
    assert_eq!(found_p2.index, 2);
}

// =========================================================================
// 2. LSP Document Incremental Sync & Bypass Stress Tests
// =========================================================================

#[test]
fn test_adv_lsp_document_single_char_keystroke_bursts() {
    let initial_ts = "const q = sql``;";
    let mut doc = Document::new("file:///app.ts".to_string(), 1, initial_ts);

    assert_eq!(doc.queries().len(), 1);
    assert_eq!(doc.queries()[0].sql, "");

    // User types "SELECT 1;" keystroke by keystroke inside the empty backticks
    let typing_sequence = "SELECT 1;";
    let backtick_pos = initial_ts.find('`').unwrap();
    let mut cur_col = (backtick_pos + 1) as u32;

    for (byte_idx, ch) in typing_sequence.char_indices() {
        let insert_event = TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position {
                    line: 0,
                    character: cur_col,
                },
                end: Position {
                    line: 0,
                    character: cur_col,
                },
            }),
            range_length: None,
            text: ch.to_string(),
        };

        let has_sql_change = doc.apply_content_changes(vec![insert_event]);
        assert!(
            has_sql_change,
            "Keystroke '{}' inside backticks must set has_sql_change = true",
            ch
        );

        cur_col += 1;
        assert_eq!(doc.queries().len(), 1);
        assert_eq!(
            doc.queries()[0].sql,
            &typing_sequence[..byte_idx + ch.len_utf8()]
        );
    }

    assert_eq!(doc.text(), "const q = sql`SELECT 1;`;");
    assert_eq!(doc.queries()[0].sql, "SELECT 1;");

    // Now user hits backspace 9 times to delete "SELECT 1;"
    for (byte_idx, _) in typing_sequence.char_indices().rev() {
        let delete_event = TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position {
                    line: 0,
                    character: cur_col - 1,
                },
                end: Position {
                    line: 0,
                    character: cur_col,
                },
            }),
            range_length: None,
            text: String::new(),
        };

        let has_sql_change = doc.apply_content_changes(vec![delete_event]);
        assert!(
            has_sql_change,
            "Backspace inside backticks must set has_sql_change = true"
        );

        cur_col -= 1;
        assert_eq!(doc.queries().len(), 1);
        assert_eq!(doc.queries()[0].sql, &typing_sequence[..byte_idx]);
    }

    assert_eq!(doc.text(), initial_ts);
    assert_eq!(doc.queries()[0].sql, "");
}

#[test]
fn test_adv_lsp_document_edits_strictly_outside_skip_sql_reparse() {
    let source = r#"import { sql } from 'bun';

// TypeScript helper types and functions
export interface AppConfig {
    port: number;
    host: string;
}

export function createServer(config: AppConfig) {
    return { running: true };
}

export const findUser = sql`
    SELECT id, username, email FROM users WHERE id = $1;
`;

export function cleanup() {
    console.log("Shutting down");
}
"#;

    let mut doc = Document::new("file:///service.ts".to_string(), 1, source);
    assert_eq!(doc.queries().len(), 1);
    assert_eq!(doc.queries()[0].name.as_deref(), Some("FindUser"));

    // 1. Single-character edits in import header (line 0, col 0): insert comment '// '
    let edit_import = TextDocumentContentChangeEvent {
        range: Some(Range {
            start: Position {
                line: 0,
                character: 0,
            },
            end: Position {
                line: 0,
                character: 0,
            },
        }),
        range_length: None,
        text: "// ".to_string(),
    };
    let has_sql = doc.apply_content_changes(vec![edit_import]);
    assert!(
        !has_sql,
        "Edit at line 0 (imports) must strictly return has_sql_change = false"
    );

    // 2. Burst of edits in AppConfig interface (line 4)
    for ch in "  debug: boolean;\n".chars() {
        let edit_type = TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position {
                    line: 4,
                    character: 4,
                },
                end: Position {
                    line: 4,
                    character: 4,
                },
            }),
            range_length: None,
            text: ch.to_string(),
        };
        let has_sql = doc.apply_content_changes(vec![edit_type]);
        assert!(
            !has_sql,
            "Edit inside interface must strictly return has_sql_change = false"
        );
    }

    // 3. Rapid typing inside cleanup() function BELOW the template (around line 18)
    let cleanup_pos = doc.text().find("console.log").unwrap();
    let cleanup_lsp_pos = doc.byte_to_position(cleanup_pos);
    let edit_fn = TextDocumentContentChangeEvent {
        range: Some(Range {
            start: cleanup_lsp_pos,
            end: cleanup_lsp_pos,
        }),
        range_length: None,
        text: "/* logging */ ".to_string(),
    };
    let has_sql = doc.apply_content_changes(vec![edit_fn]);
    assert!(
        !has_sql,
        "Edit below template must strictly return has_sql_change = false"
    );

    // Verify query was not damaged or altered
    assert_eq!(doc.queries().len(), 1);
    assert!(
        doc.queries()[0]
            .sql
            .contains("SELECT id, username, email FROM users")
    );

    // 4. Now perform an edit INSIDE the template: change 'users' to 'customers'
    let users_offset = doc.text().find("users").unwrap();
    let users_start_pos = doc.byte_to_position(users_offset);
    let users_end_pos = doc.byte_to_position(users_offset + 5);

    let edit_inside = TextDocumentContentChangeEvent {
        range: Some(Range {
            start: users_start_pos,
            end: users_end_pos,
        }),
        range_length: None,
        text: "customers".to_string(),
    };
    let has_sql = doc.apply_content_changes(vec![edit_inside]);
    assert!(
        has_sql,
        "Edit inside template must return has_sql_change = true"
    );
    assert!(doc.queries()[0].sql.contains("FROM customers"));
}

#[test]
fn test_adv_lsp_document_multiline_paste_and_selection_deletion() {
    let source = r#"
export const q = sql`
    SELECT 1;
`;
"#;
    let mut doc = Document::new("file:///multiline.ts".to_string(), 1, source);

    // Multiline paste replacing "    SELECT 1;\n" with a 20-line complex CTE statement
    let complex_sql = r#"    WITH active_orders AS (
        SELECT id, user_id, total_cents
        FROM orders
        WHERE status = 'active'
    )
    SELECT u.id, u.username, o.total_cents
    FROM users u
    JOIN active_orders o ON o.user_id = u.id
    WHERE u.created_at > NOW() - INTERVAL '30 days';"#;

    let select_start = doc.text().find("    SELECT 1;").unwrap();
    let start_pos = doc.byte_to_position(select_start);
    let end_pos = doc.byte_to_position(select_start + "    SELECT 1;".len());

    let paste_event = TextDocumentContentChangeEvent {
        range: Some(Range {
            start: start_pos,
            end: end_pos,
        }),
        range_length: None,
        text: complex_sql.to_string(),
    };

    let has_sql = doc.apply_content_changes(vec![paste_event]);
    assert!(
        has_sql,
        "Multiline paste inside SQL template must return has_sql_change = true"
    );
    assert!(doc.queries()[0].sql.contains("WITH active_orders AS"));
    assert!(
        doc.queries()[0]
            .sql
            .contains("JOIN active_orders o ON o.user_id = u.id")
    );

    // Selection deletion: select CTE and replace with empty string
    let with_offset = doc.text().find("WITH active_orders AS").unwrap();
    let select_u_offset = doc.text().find("SELECT u.id").unwrap();
    let del_start = doc.byte_to_position(with_offset);
    let del_end = doc.byte_to_position(select_u_offset);

    let delete_selection_event = TextDocumentContentChangeEvent {
        range: Some(Range {
            start: del_start,
            end: del_end,
        }),
        range_length: None,
        text: String::new(),
    };

    let has_sql = doc.apply_content_changes(vec![delete_selection_event]);
    assert!(
        has_sql,
        "Block deletion inside SQL template must return has_sql_change = true"
    );
    assert!(!doc.queries()[0].sql.contains("WITH active_orders AS"));
    assert!(doc.queries()[0].sql.contains("SELECT u.id"));
}

#[test]
fn test_adv_lsp_document_utf16_surrogate_pairs_and_boundary_conversions() {
    // Construct text with multi-byte characters and 4-byte surrogate emojis
    let text = "const crab = '🦀';\nconst cjk = '你好世界';\nconst mixed = 'A🦀B';";
    let doc = Document::new("file:///unicode_math.ts".to_string(), 1, text);

    // Line 0: "const crab = '🦀';"
    // 'const crab = \'' has 14 ASCII characters (14 UTF-16 code units, 14 bytes)
    // '🦀' has 1 Unicode char, 2 UTF-16 code units (surrogate pair), 4 bytes
    // ';' has 1 UTF-16 code unit
    let pos_before_crab = Position {
        line: 0,
        character: 14,
    };
    let pos_after_crab = Position {
        line: 0,
        character: 16,
    };
    let pos_end_line0 = Position {
        line: 0,
        character: 18,
    };

    assert_eq!(doc.position_to_byte_offset(pos_before_crab), 14);
    assert_eq!(doc.position_to_byte_offset(pos_after_crab), 18); // 14 + 4 bytes
    assert_eq!(doc.position_to_byte_offset(pos_end_line0), 20); // 18 + 2 bytes for "';"

    // Round-trip byte -> position -> byte
    assert_eq!(doc.byte_to_position(14), pos_before_crab);
    assert_eq!(doc.byte_to_position(18), pos_after_crab);
    assert_eq!(doc.byte_to_position(20), pos_end_line0);

    // Test standalone helpers
    assert_eq!(byte_offset_to_position(text, 14), pos_before_crab);
    assert_eq!(byte_offset_to_position(text, 18), pos_after_crab);
    assert_eq!(position_to_byte_offset(text, pos_before_crab), 14);
    assert_eq!(position_to_byte_offset(text, pos_after_crab), 18);

    // Line 2: "const mixed = 'A🦀B';"
    // Before '🦀': character 16
    // After '🦀': character 18
    let pos_line2_before = Position {
        line: 2,
        character: 16,
    };
    let pos_line2_after = Position {
        line: 2,
        character: 18,
    };
    let offset_before = doc.position_to_byte_offset(pos_line2_before);
    let offset_after = doc.position_to_byte_offset(pos_line2_after);
    assert_eq!(
        offset_after - offset_before,
        4,
        "Crab emoji must span exactly 4 UTF-8 bytes"
    );

    // Test editing: insert '!' immediately after crab emoji in Line 2
    let mut doc_edit = doc.clone();
    let insert_after_crab = TextDocumentContentChangeEvent {
        range: Some(Range {
            start: pos_line2_after,
            end: pos_line2_after,
        }),
        range_length: None,
        text: "!".to_string(),
    };
    doc_edit.apply_content_changes(vec![insert_after_crab]);
    assert!(
        doc_edit.text().contains("const mixed = 'A🦀!B';"),
        "Editing after surrogate pair must preserve emoji and insert at correct position: {}",
        doc_edit.text()
    );

    // Test deleting emoji: delete character 16..18
    let mut doc_del = doc.clone();
    let delete_crab = TextDocumentContentChangeEvent {
        range: Some(Range {
            start: pos_line2_before,
            end: pos_line2_after,
        }),
        range_length: None,
        text: String::new(),
    };
    doc_del.apply_content_changes(vec![delete_crab]);
    assert!(
        doc_del.text().contains("const mixed = 'AB';"),
        "Deleting surrogate pair must remove emoji cleanly: {}",
        doc_del.text()
    );
}

// =========================================================================
// 3. LSP Hover & Diagnostics Integration Stress Tests
// =========================================================================

#[test]
fn test_adv_lsp_diagnostics_syntax_error_with_unicode_preceding_offset() {
    let catalog = setup_test_catalog();

    // Line 0 has Cyrillic comments and emojis
    let ts = r#"// Привет мир! 🦀🚀
import { sql } from 'bun';

export const brokenQuery = sql`
    SELECT id, email
    FROM;
`;
"#;
    let doc = Document::new("file:///unicode_err.ts".to_string(), 1, ts);
    let diags = validate_document(&doc, &catalog);

    assert_eq!(
        diags.len(),
        1,
        "Must emit exactly 1 syntax error diagnostic"
    );
    let diag = &diags[0];

    assert_eq!(diag.severity, Some(DiagnosticSeverity::ERROR));
    assert!(diag.message.to_lowercase().contains("syntax error"));

    // Syntax error is on "FROM;" at line 5 (0-indexed)
    assert_eq!(
        diag.range.start.line, 5,
        "Diagnostic must map to line 5, got {}",
        diag.range.start.line
    );
    assert_ne!(
        diag.range.start.line, 0,
        "Must not map to line 0 (comments/imports)"
    );

    // Token highlighted must be ";" (unexpected token in PostgreSQL parser)
    let line_str = doc.rope.line(5).to_string();
    let start_col = diag.range.start.character as usize;
    let end_col = diag.range.end.character as usize;
    assert_eq!(&line_str[start_col..end_col], ";");
}

#[test]
fn test_adv_lsp_diagnostics_missing_table_with_interpolations_shift() {
    let catalog = setup_test_catalog();

    let ts = r#"import { sql } from 'bun';

export const testQuery = sql`
    SELECT u.id
    FROM non_existent_table u
    WHERE u.id = ${superLongInterpolatedExpressionWithManyVariables};
`;
"#;
    let doc = Document::new("file:///missing_table.ts".to_string(), 1, ts);
    let diags = validate_document(&doc, &catalog);

    assert_eq!(diags.len(), 1);
    let diag = &diags[0];

    assert!(
        diag.message
            .contains("Table \"non_existent_table\" does not exist in schema catalog"),
        "Expected missing table diagnostic: {}",
        diag.message
    );
    assert_eq!(diag.range.start.line, 4);

    let line_str = doc.rope.line(4).to_string();
    let token = &line_str[diag.range.start.character as usize..diag.range.end.character as usize];
    assert_eq!(token, "non_existent_table");
}

#[test]
fn test_adv_lsp_hover_latency_benchmark_under_adversarial_queries() {
    let catalog = setup_test_catalog();

    let ts = r#"import { sql } from 'bun';

export const complexHoverQuery = sql`
    SELECT
        u.id,
        u.username,
        u.bio,
        o.total_cents
    FROM users u
    JOIN orders o ON o.user_id = u.id
    WHERE u.id = ${userId} AND o.total_cents > ${minCents};
`;
"#;
    let doc = Document::new("file:///hover_bench.ts".to_string(), 1, ts);

    // Warm up
    let users_offset = doc.text().find("users u").unwrap();
    let _ = resolve_document_hover(&doc, doc.byte_to_position(users_offset), &catalog);

    // Hover targets computed dynamically from actual token byte offsets:
    // 1. users table
    // 2. orders table
    // 3. bio column
    // 4. ${userId} param
    // 5. Outside template
    let orders_offset = doc.text().find("orders o").unwrap();
    let bio_offset = doc.text().find("bio").unwrap();
    let user_id_offset = doc.text().find("${userId}").unwrap();
    let outside_offset = doc.text().find("import").unwrap();

    let positions = [
        doc.byte_to_position(users_offset),
        doc.byte_to_position(orders_offset),
        doc.byte_to_position(bio_offset),
        doc.byte_to_position(user_id_offset),
        doc.byte_to_position(outside_offset),
    ];

    const BENCHMARK_ROUNDS: usize = 200;
    let mut max_time = Duration::ZERO;
    let mut total_time = Duration::ZERO;
    let mut latencies = Vec::with_capacity(BENCHMARK_ROUNDS);

    for i in 0..BENCHMARK_ROUNDS {
        let pos = positions[i % positions.len()];
        let start = Instant::now();
        let hover = resolve_document_hover(&doc, pos, &catalog);
        let elapsed = start.elapsed();

        if elapsed > max_time {
            max_time = elapsed;
        }
        total_time += elapsed;

        if i % positions.len() == 4 {
            assert!(hover.is_none(), "Hover outside template must be None");
        } else {
            assert!(
                hover.is_some(),
                "Hover inside template at {:?} must resolve",
                pos
            );
        }

        latencies.push(elapsed);
    }

    latencies.sort();
    let p95_latency = latencies[(BENCHMARK_ROUNDS as f64 * 0.95) as usize];
    let avg_time = total_time / BENCHMARK_ROUNDS as u32;

    assert!(
        avg_time < Duration::from_millis(2),
        "Average hover latency must be <2ms, got: {:?}",
        avg_time
    );
    assert!(
        p95_latency < Duration::from_millis(5),
        "p95 hover latency must strictly be <5ms, got: {:?}",
        p95_latency
    );
}

#[test]
fn test_adv_lsp_clear_diagnostics_on_close() {
    let uri: lsp_types::Uri = "file:///workspace/src/queries.ts".parse().unwrap();
    let cleared = clear_document_diagnostics(&uri);

    assert_eq!(cleared.uri, uri);
    assert!(
        cleared.diagnostics.is_empty(),
        "Published diagnostics must be empty to clear editor squigglies"
    );
}

#[test]
fn test_adv_multiple_queries_in_single_file_isolation_and_edits() {
    let ts = r#"import { sql } from 'bun';

export const queryA = sql`
    SELECT id FROM users WHERE id = ${userId};
`;

// Helper code between queries
export const midValue = 123;

export const queryB = sql`
    SELECT total_cents FROM orders WHERE status = ${status};
`;
"#;
    let mut doc = Document::new("file:///multi.ts".to_string(), 1, ts);
    assert_eq!(doc.queries().len(), 2);
    assert_eq!(doc.queries()[0].name.as_deref(), Some("QueryA"));
    assert_eq!(doc.queries()[1].name.as_deref(), Some("QueryB"));

    // Edit 1: Edit between queryA and queryB (line 8: 123 -> 456)
    let mid_offset = doc.text().find("123").unwrap();
    let mid_start = doc.byte_to_position(mid_offset);
    let mid_end = doc.byte_to_position(mid_offset + 3);

    let edit_mid = TextDocumentContentChangeEvent {
        range: Some(Range {
            start: mid_start,
            end: mid_end,
        }),
        range_length: None,
        text: "456".to_string(),
    };
    let has_sql = doc.apply_content_changes(vec![edit_mid]);
    assert!(
        !has_sql,
        "Edit between query blocks must strictly return has_sql_change = false"
    );
    assert_eq!(doc.queries().len(), 2);

    // Edit 2: Edit queryB (change status to order_status)
    let status_offset = doc.text().find("status = ").unwrap();
    let status_start = doc.byte_to_position(status_offset);
    let status_end = doc.byte_to_position(status_offset + 6);

    let edit_qb = TextDocumentContentChangeEvent {
        range: Some(Range {
            start: status_start,
            end: status_end,
        }),
        range_length: None,
        text: "order_status".to_string(),
    };
    let has_sql = doc.apply_content_changes(vec![edit_qb]);
    assert!(
        has_sql,
        "Edit inside queryB must return has_sql_change = true"
    );
    assert!(doc.queries()[1].sql.contains("order_status = $1"));
    assert_eq!(
        doc.queries()[0].sql.trim(),
        "SELECT id FROM users WHERE id = $1;"
    );
}

#[test]
fn test_adv_ts_scanner_unterminated_and_empty_interpolations() {
    // 1. Unterminated template literal
    let unterminated = "export const unclosed = sql`SELECT id FROM users";
    let queries = scan_ts_queries(unterminated);
    assert_eq!(
        queries.len(),
        1,
        "Should cleanly extract unclosed template without panicking"
    );
    assert_eq!(queries[0].sql, "SELECT id FROM users");

    // 2. Empty interpolation `${}`
    let empty_interp = "export const empty = sql`SELECT ${} FROM users;`;";
    let queries_empty = scan_ts_queries(empty_interp);
    assert_eq!(queries_empty.len(), 1);
    assert_eq!(queries_empty[0].sql, "SELECT $1 FROM users;");
    assert_eq!(queries_empty[0].source_map.interpolations[0].expr, "");
}

#[test]
fn test_adv_ts_scanner_comments_with_braces_inside_interpolation() {
    let source = r#"
export const q = sql`
    SELECT id FROM users
    WHERE id = ${/* block comment with { curly } braces */ userId}
      AND email = ${// line comment with { braces }
        userEmail};
`;
"#;
    let queries = scan_ts_queries(source);
    assert_eq!(queries.len(), 1);
    let q = &queries[0];
    assert_eq!(q.source_map.interpolations.len(), 2);
    assert!(q.source_map.interpolations[0].expr.contains("userId"));
    assert!(q.source_map.interpolations[1].expr.contains("userEmail"));
    assert!(q.sql.contains("WHERE id = $1"));
    assert!(q.sql.contains("AND email = $2"));
}

#[test]
fn test_adv_lsp_document_crlf_windows_line_endings_position_fidelity() {
    let crlf_text =
        "const a = 1;\r\nconst b = 2;\r\nconst q = sql`\r\n  SELECT id\r\n  FROM users;\r\n`;\r\n";
    let mut doc = Document::new("file:///crlf.ts".to_string(), 1, crlf_text);

    assert_eq!(doc.queries().len(), 1);
    assert!(doc.queries()[0].sql.contains("SELECT id"));

    // Verify position conversions for CRLF lines
    let line1_pos = Position {
        line: 1,
        character: 6,
    }; // 'b' in const b
    let byte_offset = doc.position_to_byte_offset(line1_pos);
    let pos_roundtrip = doc.byte_to_position(byte_offset);
    assert_eq!(pos_roundtrip, line1_pos);

    // Edit outside CRLF template (line 0)
    let edit_crlf_outside = TextDocumentContentChangeEvent {
        range: Some(Range {
            start: Position {
                line: 0,
                character: 10,
            },
            end: Position {
                line: 0,
                character: 11,
            },
        }),
        range_length: None,
        text: "99".to_string(),
    };
    let has_sql = doc.apply_content_changes(vec![edit_crlf_outside]);
    assert!(
        !has_sql,
        "Edit on CRLF outside template must return has_sql_change = false"
    );
    assert!(doc.text().contains("const a = 99;"));

    // Edit inside CRLF template (change 'users' to 'members')
    let users_offset = doc.text().find("users").unwrap();
    let users_start = doc.byte_to_position(users_offset);
    let users_end = doc.byte_to_position(users_offset + 5);

    let edit_crlf_inside = TextDocumentContentChangeEvent {
        range: Some(Range {
            start: users_start,
            end: users_end,
        }),
        range_length: None,
        text: "members".to_string(),
    };
    let has_sql_inside = doc.apply_content_changes(vec![edit_crlf_inside]);
    assert!(
        has_sql_inside,
        "Edit on CRLF inside template must return has_sql_change = true"
    );
    assert!(doc.queries()[0].sql.contains("FROM members;"));
}

#[test]
fn test_adv_fuzz_edit_bursts_inside_and_outside_invariance() {
    let initial = r#"import { sql } from 'bun';

// TypeScript code
let counter = 0;

export const myQuery = sql`
    SELECT id, username FROM users WHERE active = true;
`;

export function getCounter() {
    return counter;
}
"#;
    let mut doc = Document::new("file:///fuzz.ts".to_string(), 1, initial);

    // 50 sequential single-character edits strictly outside the template (line 3, col 4)
    for i in 0..50 {
        let ch = ((b'a' + (i % 26) as u8) as char).to_string();
        let edit = TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position {
                    line: 3,
                    character: 4 + i as u32,
                },
                end: Position {
                    line: 3,
                    character: 4 + i as u32,
                },
            }),
            range_length: None,
            text: ch,
        };
        let has_sql = doc.apply_content_changes(vec![edit]);
        assert!(
            !has_sql,
            "Fuzz iteration {} outside template must never trigger SQL re-parse",
            i
        );
    }
    assert_eq!(doc.queries().len(), 1);
    assert_eq!(
        doc.queries()[0].sql.trim(),
        "SELECT id, username FROM users WHERE active = true;"
    );

    // 50 sequential single-character edits strictly inside the template (after "SELECT ")
    let initial_select_offset = doc.text().find("SELECT ").unwrap() + 7;
    for (i, select_offset) in (0..50).zip(initial_select_offset..) {
        let ch = ((b'0' + (i % 10) as u8) as char).to_string();
        let pos = doc.byte_to_position(select_offset);
        let edit = TextDocumentContentChangeEvent {
            range: Some(Range {
                start: pos,
                end: pos,
            }),
            range_length: None,
            text: ch,
        };
        let has_sql = doc.apply_content_changes(vec![edit]);
        assert!(
            has_sql,
            "Fuzz iteration {} inside template must always report has_sql_change = true",
            i
        );
    }
    assert_eq!(doc.queries().len(), 1);
    assert!(
        doc.queries()[0]
            .sql
            .contains("FROM users WHERE active = true;")
    );
}
