use lsp_types::{Position, TextDocumentContentChangeEvent};
use ropey::Rope;
use std::path::Path;

use crate::ts_scanner::{ExtractedQuery, is_ts_js_file, scan_ts_queries};

/// Represents an in-memory document managed by the Language Server, backed by `ropey::Rope`.
#[derive(Debug, Clone)]
pub struct Document {
    pub uri: String,
    pub version: i32,
    pub rope: Rope,
    pub queries: Vec<ExtractedQuery>,
    pub is_ts: bool,
}

impl Document {
    /// Creates a new `Document` instance from text and URI.
    pub fn new(uri: String, version: i32, text: &str) -> Self {
        let is_ts = is_ts_document(&uri);
        let queries = if is_ts {
            scan_ts_queries(text)
        } else {
            Vec::new()
        };

        Self {
            uri,
            version,
            rope: Rope::from_str(text),
            queries,
            is_ts,
        }
    }

    /// Returns the full text of the document.
    pub fn text(&self) -> String {
        self.rope.to_string()
    }

    /// Returns the extracted queries for this document.
    pub fn queries(&self) -> &[ExtractedQuery] {
        &self.queries
    }

    /// Converts a 0-indexed LSP `Position` (line and UTF-16 character) into a character index in the rope.
    pub fn position_to_char_idx(&self, pos: Position) -> usize {
        let line_idx = pos.line as usize;
        if line_idx >= self.rope.len_lines() {
            return self.rope.len_chars();
        }

        let line_slice = self.rope.line(line_idx);
        let mut utf16_count: u32 = 0;
        let mut char_offset_in_line: usize = 0;

        for ch in line_slice.chars() {
            if utf16_count >= pos.character {
                break;
            }
            utf16_count += ch.len_utf16() as u32;
            char_offset_in_line += 1;
        }

        let line_start_char = self.rope.line_to_char(line_idx);
        line_start_char + char_offset_in_line
    }

    /// Converts a character index in the rope into an LSP `Position`.
    pub fn char_idx_to_position(&self, char_idx: usize) -> Position {
        let safe_char_idx = char_idx.min(self.rope.len_chars());
        let line_idx = self.rope.char_to_line(safe_char_idx);
        let line_start_char = self.rope.line_to_char(line_idx);
        let line_slice = self.rope.line(line_idx);
        let chars_to_count = safe_char_idx.saturating_sub(line_start_char);

        let mut utf16_count: u32 = 0;
        for ch in line_slice.chars().take(chars_to_count) {
            utf16_count += ch.len_utf16() as u32;
        }

        Position {
            line: line_idx as u32,
            character: utf16_count,
        }
    }

    /// Converts an LSP `Position` into a 0-indexed byte offset in the document.
    pub fn position_to_byte_offset(&self, pos: Position) -> usize {
        let char_idx = self.position_to_char_idx(pos);
        self.rope.char_to_byte(char_idx)
    }

    /// Converts a 0-indexed byte offset in the document into an LSP `Position`.
    pub fn byte_to_position(&self, byte_offset: usize) -> Position {
        let safe_byte_offset = byte_offset.min(self.rope.len_bytes());
        let char_idx = self.rope.byte_to_char(safe_byte_offset);
        self.char_idx_to_position(char_idx)
    }

    /// Applies incremental or full content changes to the rope buffer.
    /// Returns `true` if any SQL query template was affected, or `false` if changes were
    /// completely outside SQL templates (allowing the server to skip re-parsing).
    pub fn apply_content_changes(&mut self, changes: Vec<TextDocumentContentChangeEvent>) -> bool {
        let mut has_sql_change = false;
        let old_queries = if self.is_ts {
            self.queries.clone()
        } else {
            Vec::new()
        };

        for change in changes {
            if let Some(range) = change.range {
                let start_byte = self.position_to_byte_offset(range.start);
                let end_byte = self.position_to_byte_offset(range.end);

                let touches_sql = if self.is_ts {
                    old_queries.iter().any(|q| {
                        if start_byte == end_byte {
                            q.host_span.contains(&start_byte)
                                || start_byte == q.host_span.start
                                || start_byte == q.host_span.end
                        } else {
                            start_byte < q.host_span.end && end_byte > q.host_span.start
                        }
                    })
                } else {
                    true
                };

                if touches_sql {
                    has_sql_change = true;
                }

                let start_char = self.position_to_char_idx(range.start);
                let end_char = self.position_to_char_idx(range.end);
                self.rope.remove(start_char..end_char);
                self.rope.insert(start_char, &change.text);
            } else {
                // Full content replacement
                has_sql_change = true;
                self.rope = Rope::from_str(&change.text);
            }
        }

        if self.is_ts {
            let new_text = self.text();
            self.queries = scan_ts_queries(&new_text);

            if !has_sql_change
                && (old_queries.len() != self.queries.len()
                    || old_queries
                        .iter()
                        .zip(&self.queries)
                        .any(|(old_q, new_q)| old_q.sql != new_q.sql))
            {
                has_sql_change = true;
            }
        }

        has_sql_change
    }
}

/// Helper to determine if a URI points to a TypeScript or JavaScript file.
fn is_ts_document(uri: &str) -> bool {
    let path_str = uri.strip_prefix("file://").unwrap_or(uri);
    let path = Path::new(path_str);
    is_ts_js_file(path)
}

/// Standalone helper converting a byte offset to `Position` for a raw string slice.
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

/// Standalone helper converting `Position` to a byte offset for a raw string slice.
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

#[cfg(test)]
mod tests {
    use super::*;
    use lsp_types::Range;

    #[test]
    fn test_byte_offset_to_position_and_back() {
        let text = "SELECT\n  id,\n  email\nFROM users;";
        let pos_id = byte_offset_to_position(text, 9);
        assert_eq!(pos_id.line, 1);
        assert_eq!(pos_id.character, 2);

        let offset_back = position_to_byte_offset(text, pos_id);
        assert_eq!(offset_back, 9);

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
    fn test_incremental_single_char_insertion_and_deletion() {
        let initial_ts = "const q = sql`SELECT id FROM users;`;";
        let mut doc = Document::new("file:///test.ts".to_string(), 1, initial_ts);

        assert_eq!(doc.queries().len(), 1);
        assert_eq!(doc.queries()[0].sql, "SELECT id FROM users;");

        // 1. Insert 'x' after 'id' (line 0, col 23): "SELECT idx FROM users;"
        let insert_event = TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position {
                    line: 0,
                    character: 23,
                },
                end: Position {
                    line: 0,
                    character: 23,
                },
            }),
            range_length: None,
            text: "x".to_string(),
        };

        let has_sql_change = doc.apply_content_changes(vec![insert_event]);
        assert!(
            has_sql_change,
            "Edit inside template must report has_sql_change = true"
        );
        assert_eq!(doc.text(), "const q = sql`SELECT idx FROM users;`;");
        assert_eq!(doc.queries().len(), 1);
        assert_eq!(doc.queries()[0].sql, "SELECT idx FROM users;");

        // 2. Delete 'x' (backspace: line 0, col 23..24): back to "SELECT id FROM users;"
        let delete_event = TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position {
                    line: 0,
                    character: 23,
                },
                end: Position {
                    line: 0,
                    character: 24,
                },
            }),
            range_length: None,
            text: String::new(),
        };

        let has_sql_change = doc.apply_content_changes(vec![delete_event]);
        assert!(has_sql_change);
        assert_eq!(doc.text(), initial_ts);
        assert_eq!(doc.queries()[0].sql, "SELECT id FROM users;");
    }

    #[test]
    fn test_multi_char_and_multiline_replacement() {
        let initial_ts = "const q = sql`\n  SELECT id\n  FROM users;\n`;";
        let mut doc = Document::new("file:///test.ts".to_string(), 1, initial_ts);

        // Replace "users" (line 2, col 7..12) with "organizations"
        let replace_event = TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position {
                    line: 2,
                    character: 7,
                },
                end: Position {
                    line: 2,
                    character: 12,
                },
            }),
            range_length: None,
            text: "organizations".to_string(),
        };

        let has_sql_change = doc.apply_content_changes(vec![replace_event]);
        assert!(has_sql_change);
        assert!(doc.text().contains("FROM organizations;"));
        assert!(doc.queries()[0].sql.contains("FROM organizations;"));

        // Multi-line replacement: replace lines 1 and 2 with "  SELECT 1;"
        let multiline_event = TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position {
                    line: 1,
                    character: 2,
                },
                end: Position {
                    line: 2,
                    character: 21,
                },
            }),
            range_length: None,
            text: "SELECT 1;".to_string(),
        };

        doc.apply_content_changes(vec![multiline_event]);
        assert!(doc.text().contains("SELECT 1;"));
        assert_eq!(doc.queries()[0].sql.trim(), "SELECT 1;");
    }

    #[test]
    fn test_utf16_surrogate_pairs_and_position_translation() {
        let text = "const icon = '🦀';\nconst drink = 'café';";
        let doc = Document::new("file:///unicode.ts".to_string(), 1, text);

        let pos_after_emoji = Position {
            line: 0,
            character: 16,
        };
        let offset_after_emoji = doc.position_to_byte_offset(pos_after_emoji);
        assert_eq!(&text[..offset_after_emoji], "const icon = '🦀");

        let pos_roundtrip = doc.byte_to_position(offset_after_emoji);
        assert_eq!(pos_roundtrip, pos_after_emoji);

        let pos_after_cafe = Position {
            line: 1,
            character: 19,
        };
        let offset_after_cafe = doc.position_to_byte_offset(pos_after_cafe);
        assert_eq!(
            &text[text.find("const drink").unwrap()..offset_after_cafe],
            "const drink = 'café"
        );

        let pos_cafe_roundtrip = doc.byte_to_position(offset_after_cafe);
        assert_eq!(pos_cafe_roundtrip, pos_after_cafe);
    }

    #[test]
    fn test_detection_of_edits_inside_vs_outside_sql_templates() {
        let source = r#"import { sql } from 'bun';

const helperValue = 42;

export const myQuery = sql`
  SELECT id FROM users WHERE id = $1;
`;

export function compute() {
  return helperValue * 2;
}
"#;
        let mut doc = Document::new("file:///app.ts".to_string(), 1, source);
        assert_eq!(doc.queries().len(), 1);

        // Edit 1: Modify helperValue outside template (line 2: 42 -> 99)
        let outside_edit_1 = TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position {
                    line: 2,
                    character: 20,
                },
                end: Position {
                    line: 2,
                    character: 22,
                },
            }),
            range_length: None,
            text: "99".to_string(),
        };

        let sql_changed = doc.apply_content_changes(vec![outside_edit_1]);
        assert!(
            !sql_changed,
            "Edit outside template literal must return has_sql_change = false"
        );
        assert_eq!(doc.queries().len(), 1);
        assert!(doc.text().contains("const helperValue = 99;"));

        // Edit 2: Modify compute() function below template (line 10)
        let outside_edit_2 = TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position {
                    line: 10,
                    character: 23,
                },
                end: Position {
                    line: 10,
                    character: 24,
                },
            }),
            range_length: None,
            text: "5".to_string(),
        };

        let sql_changed = doc.apply_content_changes(vec![outside_edit_2]);
        assert!(
            !sql_changed,
            "Edit below template literal must return has_sql_change = false"
        );

        // Edit 3: Modify inside SQL template (line 5: 'users' -> 'customers')
        let inside_edit = TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position {
                    line: 5,
                    character: 17,
                },
                end: Position {
                    line: 5,
                    character: 22,
                },
            }),
            range_length: None,
            text: "customers".to_string(),
        };

        let sql_changed = doc.apply_content_changes(vec![inside_edit]);
        assert!(
            sql_changed,
            "Edit inside template literal must return has_sql_change = true"
        );
        assert!(doc.queries()[0].sql.contains("FROM customers"));
    }

    #[test]
    fn test_apply_content_changes_empty_document_insertion() {
        let mut doc = Document::new("file:///empty.ts".to_string(), 1, "");
        assert_eq!(doc.queries().len(), 0);

        let insert_query = TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position { line: 0, character: 0 },
                end: Position { line: 0, character: 0 },
            }),
            range_length: None,
            text: "export const q = sql`SELECT id FROM users`;\n".to_string(),
        };

        let changed = doc.apply_content_changes(vec![insert_query]);
        assert!(changed, "Inserting first query into empty document must trigger has_sql_change = true");
        assert_eq!(doc.queries().len(), 1);
        assert_eq!(doc.queries()[0].sql, "SELECT id FROM users");
    }

    #[test]
    fn test_apply_content_changes_append_second_query() {
        let initial_ts = "export const q1 = sql`SELECT id FROM users`;\n";
        let mut doc = Document::new("file:///queries.ts".to_string(), 1, initial_ts);
        assert_eq!(doc.queries().len(), 1);

        let append_edit = TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position { line: 1, character: 0 },
                end: Position { line: 1, character: 0 },
            }),
            range_length: None,
            text: "export const q2 = sql`SELECT name FROM products`;\n".to_string(),
        };

        let changed = doc.apply_content_changes(vec![append_edit]);
        assert!(changed, "Appending a second query must trigger has_sql_change = true");
        assert_eq!(doc.queries().len(), 2);
    }

    #[test]
    fn test_apply_content_changes_outside_edit_bypasses() {
        let initial_ts = "import { sql } from 'bun';\n// comment\nexport const q = sql`SELECT id FROM users`;\n";
        let mut doc = Document::new("file:///test.ts".to_string(), 1, initial_ts);
        assert_eq!(doc.queries().len(), 1);

        // Edit comment line
        let edit_comment = TextDocumentContentChangeEvent {
            range: Some(Range {
                start: Position { line: 1, character: 10 },
                end: Position { line: 1, character: 10 },
            }),
            range_length: None,
            text: " extra info".to_string(),
        };

        let changed = doc.apply_content_changes(vec![edit_comment]);
        assert!(!changed, "Editing comment outside SQL query must return has_sql_change = false");
        assert_eq!(doc.queries().len(), 1);
    }
}
