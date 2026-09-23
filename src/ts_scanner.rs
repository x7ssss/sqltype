use crate::analyzer::to_pascal_case;
use std::path::Path;

/// An inline SQL query extracted from a TypeScript or JavaScript file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedQuery {
    /// Inferred or explicit query name (e.g. from `const findActiveUsers = sql`...`` or `-- name: Foo`).
    pub name: Option<String>,
    /// The raw inner SQL string.
    pub sql: String,
    /// 1-based start line of the template in the source file.
    pub line: usize,
    /// 1-based start column of the template in the source file.
    pub column: usize,
    /// 0-based byte offset where the SQL string begins in the source file.
    pub byte_offset: usize,
}

/// Checks whether a given path is an accepted query file (.sql, .ts, .tsx, .js),
/// explicitly ignoring generated files like *.sqltype.ts, *.generated.ts, and *.d.ts.
pub fn is_query_file(path: &Path) -> bool {
    let file_name = match path.file_name().and_then(|f| f.to_str()) {
        Some(name) => name.to_ascii_lowercase(),
        None => return false,
    };

    if file_name.ends_with(".sqltype.ts")
        || file_name.ends_with(".sql.ts")
        || file_name.ends_with(".generated.ts")
        || file_name.ends_with(".d.ts")
    {
        return false;
    }

    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let ext_lower = ext.to_ascii_lowercase();
        matches!(ext_lower.as_str(), "sql" | "ts" | "tsx" | "js")
    } else {
        false
    }
}

/// Checks whether a path is a TypeScript or JavaScript file (.ts, .tsx, .js).
pub fn is_ts_js_file(path: &Path) -> bool {
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        let ext_lower = ext.to_ascii_lowercase();
        matches!(ext_lower.as_str(), "ts" | "tsx" | "js")
    } else {
        false
    }
}

/// Extracts any explicit query name defined via leading SQL comment `-- name: <Name>`.
fn extract_comment_name(sql: &str) -> Option<String> {
    for line in sql.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("--") {
            let after_dashes = trimmed.trim_start_matches('-').trim();
            if let Some(name_part) = after_dashes.strip_prefix("name:") {
                let name = name_part.trim();
                let query_name: String = name
                    .chars()
                    .take_while(|c| c.is_alphanumeric() || *c == '_')
                    .collect();
                if !query_name.is_empty() {
                    return Some(query_name);
                }
            }
        } else if !trimmed.is_empty() {
            break;
        }
    }
    None
}

/// High-performance zero-allocation streaming scanner for extracting `sql` tagged template literals
/// and function calls from TypeScript/JavaScript sources.
pub struct TsScanner<'a> {
    src: &'a str,
    bytes: &'a [u8],
    len: usize,
    pos: usize,
    line: usize,
    col: usize,
}

impl<'a> TsScanner<'a> {
    pub fn new(src: &'a str) -> Self {
        Self {
            src,
            bytes: src.as_bytes(),
            len: src.len(),
            pos: 0,
            line: 1,
            col: 1,
        }
    }

    #[inline]
    fn peek(&self) -> Option<u8> {
        if self.pos < self.len {
            Some(self.bytes[self.pos])
        } else {
            None
        }
    }

    #[inline]
    fn peek_at(&self, offset: usize) -> Option<u8> {
        if self.pos + offset < self.len {
            Some(self.bytes[self.pos + offset])
        } else {
            None
        }
    }

    fn advance(&mut self) -> Option<u8> {
        if self.pos >= self.len {
            return None;
        }
        let b = self.bytes[self.pos];
        self.pos += 1;
        if b == b'\n' {
            self.line += 1;
            self.col = 1;
        } else {
            self.col += 1;
        }
        Some(b)
    }

    fn skip_whitespace(&mut self) {
        while let Some(b) = self.peek() {
            if b == b' ' || b == b'\t' || b == b'\r' || b == b'\n' {
                self.advance();
            } else {
                break;
            }
        }
    }

    fn skip_single_line_comment(&mut self) {
        while let Some(b) = self.advance() {
            if b == b'\n' {
                break;
            }
        }
    }

    fn skip_multi_line_comment(&mut self) {
        while let Some(b) = self.advance() {
            if b == b'*' && self.peek() == Some(b'/') {
                self.advance();
                break;
            }
        }
    }

    fn skip_string_literal(&mut self, quote: u8) {
        while let Some(b) = self.advance() {
            if b == b'\\' {
                self.advance();
            } else if b == quote || b == b'\n' {
                break;
            }
        }
    }

    fn skip_template_literal(&mut self) {
        while let Some(b) = self.advance() {
            if b == b'\\' {
                self.advance();
            } else if b == b'$' && self.peek() == Some(b'{') {
                self.advance();
                self.skip_balanced_braces();
            } else if b == b'`' {
                break;
            }
        }
    }

    fn skip_balanced_braces(&mut self) {
        let mut depth = 1;
        while depth > 0 && self.pos < self.len {
            match self.peek() {
                Some(b'{') => {
                    self.advance();
                    depth += 1;
                }
                Some(b'}') => {
                    self.advance();
                    depth -= 1;
                }
                Some(b'\'') => {
                    self.advance();
                    self.skip_string_literal(b'\'');
                }
                Some(b'"') => {
                    self.advance();
                    self.skip_string_literal(b'"');
                }
                Some(b'`') => {
                    self.advance();
                    self.skip_template_literal();
                }
                Some(b'/') if self.peek_at(1) == Some(b'/') => {
                    self.advance();
                    self.advance();
                    self.skip_single_line_comment();
                }
                Some(b'/') if self.peek_at(1) == Some(b'*') => {
                    self.advance();
                    self.advance();
                    self.skip_multi_line_comment();
                }
                _ => {
                    self.advance();
                }
            }
        }
    }

    fn skip_balanced_angles(&mut self) {
        let mut depth = 1;
        while depth > 0 && self.pos < self.len {
            match self.peek() {
                Some(b'<') => {
                    self.advance();
                    depth += 1;
                }
                Some(b'>') => {
                    self.advance();
                    depth -= 1;
                }
                Some(b'`') | Some(b';') | Some(b'\n') => {
                    break;
                }
                _ => {
                    self.advance();
                }
            }
        }
    }

    fn read_identifier(&mut self) -> String {
        let start = self.pos;
        while let Some(b) = self.peek() {
            if b.is_ascii_alphanumeric() || b == b'_' || b == b'$' {
                self.advance();
            } else {
                break;
            }
        }
        self.src[start..self.pos].to_string()
    }

    /// Scans the entire source and extracts all inline `sql` queries.
    pub fn scan(&mut self) -> Vec<ExtractedQuery> {
        let mut queries = Vec::new();
        let mut pending_var_name: Option<String> = None;

        while self.pos < self.len {
            self.skip_whitespace();
            if self.pos >= self.len {
                break;
            }

            // Check comments
            if self.peek() == Some(b'/') {
                if self.peek_at(1) == Some(b'/') {
                    self.advance();
                    self.advance();
                    self.skip_single_line_comment();
                    continue;
                } else if self.peek_at(1) == Some(b'*') {
                    self.advance();
                    self.advance();
                    self.skip_multi_line_comment();
                    continue;
                }
            }

            // Check strings
            if self.peek() == Some(b'\'') {
                self.advance();
                self.skip_string_literal(b'\'');
                continue;
            }
            if self.peek() == Some(b'"') {
                self.advance();
                self.skip_string_literal(b'"');
                continue;
            }

            // Check untagged template literal
            if self.peek() == Some(b'`') {
                self.advance();
                self.skip_template_literal();
                pending_var_name = None;
                continue;
            }

            // Semicolon or comma clears pending variable binding
            if self.peek() == Some(b';') || self.peek() == Some(b',') {
                self.advance();
                pending_var_name = None;
                continue;
            }

            // Check identifier
            let first_byte = self.peek().unwrap();
            if first_byte.is_ascii_alphabetic() || first_byte == b'_' || first_byte == b'$' {
                let ident = self.read_identifier();

                // Check variable declarations: const, let, var
                if matches!(ident.as_str(), "const" | "let" | "var") {
                    self.skip_whitespace();
                    if let Some(next_byte) = self.peek()
                        && (next_byte.is_ascii_alphabetic()
                            || next_byte == b'_'
                            || next_byte == b'$')
                    {
                        let var_name = self.read_identifier();
                        self.skip_whitespace();
                        // Skip optional TypeScript type annotation e.g. : Type
                        if self.peek() == Some(b':') {
                            self.advance();
                            while let Some(b) = self.peek() {
                                if b == b'=' || b == b';' || b == b'\n' {
                                    break;
                                }
                                self.advance();
                            }
                        }
                        self.skip_whitespace();
                        if self.peek() == Some(b'=') {
                            self.advance();
                            pending_var_name = Some(var_name);
                        }
                    }
                    continue;
                }

                // Check object property assignment: `findUser: sql`...``
                let checkpoint = (self.pos, self.line, self.col);
                self.skip_whitespace();
                if self.peek() == Some(b':') {
                    self.advance();
                    pending_var_name = Some(ident.clone());
                    continue;
                } else {
                    // Revert back
                    self.pos = checkpoint.0;
                    self.line = checkpoint.1;
                    self.col = checkpoint.2;
                }

                // Check if ident is `sql` or `SQL`
                if ident == "sql" || ident == "SQL" {
                    // Check optional method chaining: `sql.unsafe` or `sql.raw`
                    if self.peek() == Some(b'.') {
                        self.advance();
                        let _method = self.read_identifier();
                    }

                    self.skip_whitespace();

                    // Check optional generic type arguments: `sql<UserRow>`
                    if self.peek() == Some(b'<') {
                        self.advance();
                        self.skip_balanced_angles();
                        self.skip_whitespace();
                    }

                    // Check for tagged template backtick `
                    if self.peek() == Some(b'`') {
                        self.advance();

                        let start_pos = self.pos;
                        let start_line = self.line;
                        let start_col = self.col;

                        let mut content = String::new();
                        while let Some(b) = self.advance() {
                            if b == b'\\' {
                                if let Some(escaped) = self.advance() {
                                    if escaped == b'`' {
                                        content.push('`');
                                    } else {
                                        content.push('\\');
                                        content.push(escaped as char);
                                    }
                                }
                            } else if b == b'$' && self.peek() == Some(b'{') {
                                self.advance();
                                self.skip_balanced_braces();
                            } else if b == b'`' {
                                break;
                            } else {
                                content.push(b as char);
                            }
                        }

                        let explicit_name = extract_comment_name(&content);
                        let final_name = explicit_name
                            .or_else(|| pending_var_name.take().map(|v| to_pascal_case(&v)));

                        queries.push(ExtractedQuery {
                            name: final_name,
                            sql: content,
                            line: start_line,
                            column: start_col,
                            byte_offset: start_pos,
                        });
                        continue;
                    }

                    // Check for function call: `sql("...")` or `sql('...')`
                    if self.peek() == Some(b'(') {
                        self.advance();
                        self.skip_whitespace();
                        if let Some(quote @ (b'\'' | b'"')) = self.peek() {
                            self.advance();
                            let start_pos = self.pos;
                            let start_line = self.line;
                            let start_col = self.col;

                            let mut content = String::new();
                            while let Some(b) = self.advance() {
                                if b == b'\\' {
                                    if let Some(escaped) = self.advance() {
                                        content.push(escaped as char);
                                    }
                                } else if b == quote {
                                    break;
                                } else {
                                    content.push(b as char);
                                }
                            }

                            let explicit_name = extract_comment_name(&content);
                            let final_name = explicit_name
                                .or_else(|| pending_var_name.take().map(|v| to_pascal_case(&v)));

                            queries.push(ExtractedQuery {
                                name: final_name,
                                sql: content,
                                line: start_line,
                                column: start_col,
                                byte_offset: start_pos,
                            });
                            continue;
                        }
                    }
                }
            } else {
                self.advance();
            }
        }

        queries
    }
}

/// Convenience function to scan a TypeScript/JavaScript source code string and extract all inline queries.
pub fn scan_ts_queries(src: &str) -> Vec<ExtractedQuery> {
    TsScanner::new(src).scan()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_scan_basic_template_literal() {
        let ts = r#"
            import { sql } from 'bun';

            export const findActiveUsers = sql`
                SELECT id, email FROM users WHERE status = 'active' AND id = $1;
            `;
        "#;

        let queries = scan_ts_queries(ts);
        assert_eq!(queries.len(), 1);
        let q = &queries[0];
        assert_eq!(q.name.as_deref(), Some("FindActiveUsers"));
        assert!(
            q.sql
                .contains("SELECT id, email FROM users WHERE status = 'active' AND id = $1;")
        );
        assert_eq!(q.line, 4);
    }

    #[test]
    fn test_scan_generic_and_method_call() {
        let ts = r#"
            const getUser = sql<UserRow>`SELECT id FROM users WHERE id = $1;`;
            const deletePost = sql.unsafe`DELETE FROM posts WHERE id = $1;`;
            const rawQuery = sql.raw`SELECT 1;`;
        "#;

        let queries = scan_ts_queries(ts);
        assert_eq!(queries.len(), 3);
        assert_eq!(queries[0].name.as_deref(), Some("GetUser"));
        assert_eq!(queries[1].name.as_deref(), Some("DeletePost"));
        assert_eq!(queries[2].name.as_deref(), Some("RawQuery"));
    }

    #[test]
    fn test_scan_ignores_comments_and_strings() {
        let ts = r#"
            // const fake1 = sql`SELECT * FROM fake1;`;
            /*
               const fake2 = sql`SELECT * FROM fake2;`;
            */
            const fakeStr = "sql`SELECT * FROM fake3;`";
            const realQuery = sql`SELECT id FROM real_table;`;
        "#;

        let queries = scan_ts_queries(ts);
        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].name.as_deref(), Some("RealQuery"));
        assert!(queries[0].sql.contains("real_table"));
    }

    #[test]
    fn test_scan_with_explicit_comment_name() {
        let ts = r#"
            export const query = sql`
                -- name: CustomFetchUsers
                SELECT id FROM users;
            `;
        "#;

        let queries = scan_ts_queries(ts);
        assert_eq!(queries.len(), 1);
        assert_eq!(queries[0].name.as_deref(), Some("CustomFetchUsers"));
    }

    #[test]
    fn test_scan_escaped_backticks() {
        let ts = r#"
            const q = sql`SELECT 'it\`s working' AS val;`;
        "#;

        let queries = scan_ts_queries(ts);
        assert_eq!(queries.len(), 1);
        assert!(queries[0].sql.contains("'it`s working'"));
    }

    #[test]
    fn test_is_query_file() {
        assert!(is_query_file(Path::new("queries/users.sql")));
        assert!(is_query_file(Path::new("src/queries/users.ts")));
        assert!(is_query_file(Path::new("src/components/List.tsx")));
        assert!(is_query_file(Path::new("src/db/queries.js")));

        // Ignored extensions
        assert!(!is_query_file(Path::new("src/queries/users.sqltype.ts")));
        assert!(!is_query_file(Path::new("src/queries/users.generated.ts")));
        assert!(!is_query_file(Path::new("src/types.d.ts")));
        assert!(!is_query_file(Path::new("README.md")));
    }
}
