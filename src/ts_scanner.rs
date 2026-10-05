use crate::analyzer::to_pascal_case;
use std::path::Path;

/// Represents an interpolation expression in the host TypeScript document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InterpolatedParam {
    /// 1-based parameter index ($1 -> 1, $2 -> 2, etc.)
    pub index: usize,
    /// The raw expression text inside the braces, e.g. "userId" or "user.profile.id"
    pub expr: String,
    /// Byte range of the entire `${...}` in the host document: [start..end)
    pub host_range: std::ops::Range<usize>,
    /// Byte range of the `$n` placeholder in the generated SQL: [start..end)
    pub sql_range: std::ops::Range<usize>,
}

/// A contiguous segment mapping between the generated SQL string and the host document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceMappingSegment {
    /// Range in generated SQL: [sql_start..sql_end)
    pub sql_range: std::ops::Range<usize>,
    /// Range in host document: [host_start..host_end)
    pub host_range: std::ops::Range<usize>,
    /// Whether this segment corresponds to an interpolation `${...}` mapped to `$n`
    pub is_interpolation: bool,
    /// If interpolation, the parameter index (1 for $1, etc.)
    pub param_index: Option<usize>,
}

/// Bidirectional source map between generated SQL and host document.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SourceMap {
    pub segments: Vec<SourceMappingSegment>,
    pub interpolations: Vec<InterpolatedParam>,
}

impl SourceMap {
    pub fn new() -> Self {
        Self::default()
    }

    /// Maps a byte offset in the generated SQL string to the corresponding byte offset in the host document.
    pub fn sql_to_host_offset(&self, sql_offset: usize) -> usize {
        for seg in &self.segments {
            if sql_offset >= seg.sql_range.start && sql_offset < seg.sql_range.end {
                if seg.is_interpolation {
                    return seg.host_range.start;
                }
                let delta = sql_offset - seg.sql_range.start;
                return seg.host_range.start + delta;
            }
        }
        self.segments.last().map(|s| s.host_range.end).unwrap_or(0)
    }

    /// Maps a byte range in the generated SQL string to the corresponding byte range in the host document.
    pub fn sql_to_host_range(&self, sql_range: std::ops::Range<usize>) -> std::ops::Range<usize> {
        if let Some(interp) = self
            .interpolations
            .iter()
            .find(|i| sql_range.start >= i.sql_range.start && sql_range.end <= i.sql_range.end)
        {
            return interp.host_range.clone();
        }

        let host_start = self.sql_to_host_offset(sql_range.start);
        let host_end = self.sql_to_host_offset(sql_range.end);
        host_start..host_end.max(host_start)
    }

    /// Reverse mapping: maps a byte offset in the host document to the generated SQL string offset.
    pub fn host_to_sql_offset(&self, host_offset: usize) -> Option<usize> {
        for seg in &self.segments {
            if host_offset >= seg.host_range.start && host_offset < seg.host_range.end {
                if seg.is_interpolation {
                    return Some(seg.sql_range.start);
                }
                let delta = host_offset - seg.host_range.start;
                return Some(seg.sql_range.start + delta);
            }
        }
        None
    }

    /// Finds the interpolated parameter at the given host document byte offset, if any.
    pub fn find_interpolated_param(&self, host_offset: usize) -> Option<&InterpolatedParam> {
        self.interpolations
            .iter()
            .find(|i| host_offset >= i.host_range.start && host_offset < i.host_range.end)
    }

    /// Finds the interpolated parameter by 1-based parameter index ($1 -> 1).
    pub fn find_interpolated_param_by_index(&self, index: usize) -> Option<&InterpolatedParam> {
        self.interpolations.iter().find(|i| i.index == index)
    }
}

/// An inline SQL query extracted from a TypeScript or JavaScript file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExtractedQuery {
    /// Inferred or explicit query name (e.g. from `const findActiveUsers = sql`...`` or `-- name: Foo`).
    pub name: Option<String>,
    /// The raw inner SQL string with `${...}` replaced by `$1, $2, ...`.
    pub sql: String,
    /// 1-based start line of the template in the source file.
    pub line: usize,
    /// 1-based start column of the template in the source file.
    pub column: usize,
    /// 0-based byte offset where the SQL string begins in the source file.
    pub byte_offset: usize,
    /// Total byte span of the template in the host document: `[start_backtick..end_backtick_plus_1)`.
    pub host_span: std::ops::Range<usize>,
    /// Bidirectional source map between generated SQL and host document.
    pub source_map: SourceMap,
}

impl ExtractedQuery {
    /// Returns whether the given host byte offset falls within this query's host span.
    pub fn contains_host_offset(&self, host_offset: usize) -> bool {
        self.host_span.contains(&host_offset)
    }

    /// Translates a byte range in the generated SQL back to the host document range.
    pub fn sql_to_host_range(&self, sql_range: std::ops::Range<usize>) -> std::ops::Range<usize> {
        self.source_map.sql_to_host_range(sql_range)
    }

    /// Translates a host document byte offset into the generated SQL byte offset.
    pub fn host_to_sql_offset(&self, host_offset: usize) -> Option<usize> {
        self.source_map.host_to_sql_offset(host_offset)
    }
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
        } else if (b & 0xC0) != 0x80 {
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
                        let backtick_start = self.pos;
                        self.advance(); // consume opening `

                        let template_body_start = self.pos;
                        let start_line = self.line;
                        let start_col = self.col;

                        let mut content = String::new();
                        let mut segments = Vec::new();
                        let mut interpolations = Vec::new();
                        let mut param_counter: usize = 1;
                        let mut chunk_start = self.pos;
                        let mut terminated = false;

                        while self.pos < self.len {
                            let b = self.bytes[self.pos];
                            if b == b'`' {
                                // Flush literal chunk before closing `
                                if self.pos > chunk_start {
                                    let chunk_str = &self.src[chunk_start..self.pos];
                                    let sql_start = content.len();
                                    content.push_str(chunk_str);
                                    let sql_end = content.len();
                                    segments.push(SourceMappingSegment {
                                        sql_range: sql_start..sql_end,
                                        host_range: chunk_start..self.pos,
                                        is_interpolation: false,
                                        param_index: None,
                                    });
                                }
                                self.advance(); // consume closing `
                                terminated = true;
                                break;
                            } else if b == b'\\' {
                                let slash_pos = self.pos;
                                self.advance();
                                if let Some(escaped) = self.peek() {
                                    if escaped == b'`' {
                                        if slash_pos > chunk_start {
                                            let chunk_str = &self.src[chunk_start..slash_pos];
                                            let sql_start = content.len();
                                            content.push_str(chunk_str);
                                            let sql_end = content.len();
                                            segments.push(SourceMappingSegment {
                                                sql_range: sql_start..sql_end,
                                                host_range: chunk_start..slash_pos,
                                                is_interpolation: false,
                                                param_index: None,
                                            });
                                        }
                                        self.advance(); // consume `
                                        let sql_start = content.len();
                                        content.push('`');
                                        let sql_end = content.len();
                                        segments.push(SourceMappingSegment {
                                            sql_range: sql_start..sql_end,
                                            host_range: slash_pos..self.pos,
                                            is_interpolation: false,
                                            param_index: None,
                                        });
                                        chunk_start = self.pos;
                                    } else {
                                        self.advance();
                                    }
                                }
                            } else if b == b'$' && self.peek_at(1) == Some(b'{') {
                                let dollar_pos = self.pos;
                                if dollar_pos > chunk_start {
                                    let chunk_str = &self.src[chunk_start..dollar_pos];
                                    let sql_start = content.len();
                                    content.push_str(chunk_str);
                                    let sql_end = content.len();
                                    segments.push(SourceMappingSegment {
                                        sql_range: sql_start..sql_end,
                                        host_range: chunk_start..dollar_pos,
                                        is_interpolation: false,
                                        param_index: None,
                                    });
                                }
                                self.advance(); // consume `$`
                                self.advance(); // consume `{`
                                let expr_start = self.pos;
                                self.skip_balanced_braces();
                                let interp_end = self.pos;
                                let expr_end = interp_end.saturating_sub(1);
                                let expr = if expr_end >= expr_start {
                                    self.src[expr_start..expr_end].trim().to_string()
                                } else {
                                    String::new()
                                };

                                let param_str = format!("${}", param_counter);
                                let sql_start = content.len();
                                content.push_str(&param_str);
                                let sql_end = content.len();

                                let host_range = dollar_pos..interp_end;
                                let sql_range = sql_start..sql_end;

                                interpolations.push(InterpolatedParam {
                                    index: param_counter,
                                    expr,
                                    host_range: host_range.clone(),
                                    sql_range: sql_range.clone(),
                                });

                                segments.push(SourceMappingSegment {
                                    sql_range,
                                    host_range,
                                    is_interpolation: true,
                                    param_index: Some(param_counter),
                                });

                                param_counter += 1;
                                chunk_start = self.pos;
                            } else {
                                self.advance();
                            }
                        }

                        if terminated {
                            let host_end = self.pos;
                            let explicit_name = extract_comment_name(&content);
                            let final_name = explicit_name
                                .or_else(|| pending_var_name.take().map(|v| to_pascal_case(&v)));

                            queries.push(ExtractedQuery {
                                name: final_name,
                                sql: content,
                                line: start_line,
                                column: start_col,
                                byte_offset: template_body_start,
                                host_span: backtick_start..host_end,
                                source_map: SourceMap {
                                    segments,
                                    interpolations,
                                },
                            });
                        } else if chunk_start < self.len {
                            let chunk_str = &self.src[chunk_start..self.len];
                            let sql_start = content.len();
                            content.push_str(chunk_str);
                            let sql_end = content.len();
                            segments.push(SourceMappingSegment {
                                sql_range: sql_start..sql_end,
                                host_range: chunk_start..self.len,
                                is_interpolation: false,
                                param_index: None,
                            });

                            let explicit_name = extract_comment_name(&content);
                            let final_name = explicit_name
                                .or_else(|| pending_var_name.take().map(|v| to_pascal_case(&v)));

                            queries.push(ExtractedQuery {
                                name: final_name,
                                sql: content,
                                line: start_line,
                                column: start_col,
                                byte_offset: template_body_start,
                                host_span: backtick_start..self.len,
                                source_map: SourceMap {
                                    segments,
                                    interpolations,
                                },
                            });
                        }

                        continue;
                    }

                    // Check for function call: `sql("...")` or `sql('...')`
                    if self.peek() == Some(b'(') {
                        self.advance();
                        self.skip_whitespace();
                        if let Some(quote @ (b'\'' | b'"')) = self.peek() {
                            let quote_start = self.pos;
                            self.advance();
                            let start_pos = self.pos;
                            let start_line = self.line;
                            let start_col = self.col;

                            let mut content = String::new();
                            let mut segments = Vec::new();
                            let mut chunk_start = self.pos;
                            let mut terminated = false;

                            while self.pos < self.len {
                                let b = self.bytes[self.pos];
                                if b == quote {
                                    if self.pos > chunk_start {
                                        let chunk_str = &self.src[chunk_start..self.pos];
                                        let sql_start = content.len();
                                        content.push_str(chunk_str);
                                        let sql_end = content.len();
                                        segments.push(SourceMappingSegment {
                                            sql_range: sql_start..sql_end,
                                            host_range: chunk_start..self.pos,
                                            is_interpolation: false,
                                            param_index: None,
                                        });
                                    }
                                    self.advance(); // consume quote
                                    terminated = true;
                                    break;
                                } else if b == b'\\' {
                                    let slash_pos = self.pos;
                                    self.advance();
                                    if let Some(escaped) = self.peek() {
                                        if escaped == quote {
                                            if slash_pos > chunk_start {
                                                let chunk_str = &self.src[chunk_start..slash_pos];
                                                let sql_start = content.len();
                                                content.push_str(chunk_str);
                                                let sql_end = content.len();
                                                segments.push(SourceMappingSegment {
                                                    sql_range: sql_start..sql_end,
                                                    host_range: chunk_start..slash_pos,
                                                    is_interpolation: false,
                                                    param_index: None,
                                                });
                                            }
                                            self.advance();
                                            let sql_start = content.len();
                                            content.push(quote as char);
                                            let sql_end = content.len();
                                            segments.push(SourceMappingSegment {
                                                sql_range: sql_start..sql_end,
                                                host_range: slash_pos..self.pos,
                                                is_interpolation: false,
                                                param_index: None,
                                            });
                                            chunk_start = self.pos;
                                        } else {
                                            self.advance();
                                        }
                                    }
                                } else {
                                    self.advance();
                                }
                            }

                            if terminated || chunk_start < self.len {
                                if !terminated {
                                    let chunk_str = &self.src[chunk_start..self.len];
                                    let sql_start = content.len();
                                    content.push_str(chunk_str);
                                    let sql_end = content.len();
                                    segments.push(SourceMappingSegment {
                                        sql_range: sql_start..sql_end,
                                        host_range: chunk_start..self.len,
                                        is_interpolation: false,
                                        param_index: None,
                                    });
                                }

                                let explicit_name = extract_comment_name(&content);
                                let final_name = explicit_name.or_else(|| {
                                    pending_var_name.take().map(|v| to_pascal_case(&v))
                                });

                                queries.push(ExtractedQuery {
                                    name: final_name,
                                    sql: content,
                                    line: start_line,
                                    column: start_col,
                                    byte_offset: start_pos,
                                    host_span: quote_start..self.pos,
                                    source_map: SourceMap {
                                        segments,
                                        interpolations: Vec::new(),
                                    },
                                });
                            }
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

    #[test]
    fn test_scan_single_interpolation_mapped_to_dollar1() {
        let ts = r#"
            import { sql } from 'bun';

            export const findUser = sql`
                SELECT id, email, created_at
                FROM users
                WHERE id = ${userId};
            `;
        "#;

        let queries = scan_ts_queries(ts);
        assert_eq!(queries.len(), 1, "Expected 1 extracted query");
        let q = &queries[0];

        assert_eq!(q.name.as_deref(), Some("FindUser"));
        assert!(
            q.sql.contains("WHERE id = $1;"),
            "Expected ${{userId}} to be replaced with $1, got: {}",
            q.sql
        );
        assert_eq!(q.source_map.interpolations.len(), 1);

        let interp = &q.source_map.interpolations[0];
        assert_eq!(interp.index, 1);
        assert_eq!(interp.expr, "userId");

        // Verify host range matches verbatim "${userId}" in source
        let host_slice = &ts[interp.host_range.clone()];
        assert_eq!(host_slice, "${userId}");

        // Verify SQL range matches verbatim "$1" in transformed SQL
        let sql_slice = &q.sql[interp.sql_range.clone()];
        assert_eq!(sql_slice, "$1");

        // Verify SQL can be parsed by PostgreSQL parser
        assert!(
            pg_query::parse(&q.sql).is_ok(),
            "Transformed SQL must be valid PostgreSQL syntax"
        );
    }

    #[test]
    fn test_scan_multiple_interpolations_sequential_indexing() {
        let ts = r#"
            export const searchOrders = sql`
                SELECT id, total_amount, status
                FROM orders
                WHERE user_id = ${userId}
                  AND status = ${orderStatus}
                  AND total_amount >= ${minAmount}
                ORDER BY created_at DESC
                LIMIT ${pageSize};
            `;
        "#;

        let queries = scan_ts_queries(ts);
        assert_eq!(queries.len(), 1);
        let q = &queries[0];

        // Assert sequential parameter replacement
        assert!(
            q.sql.contains("WHERE user_id = $1"),
            "Expected user_id = $1"
        );
        assert!(q.sql.contains("AND status = $2"), "Expected status = $2");
        assert!(
            q.sql.contains("AND total_amount >= $3"),
            "Expected total_amount >= $3"
        );
        assert!(q.sql.contains("LIMIT $4;"), "Expected LIMIT $4;");

        assert_eq!(q.source_map.interpolations.len(), 4);
        let expected = [
            (1, "userId"),
            (2, "orderStatus"),
            (3, "minAmount"),
            (4, "pageSize"),
        ];

        for (i, (idx, expr)) in expected.iter().enumerate() {
            let p = &q.source_map.interpolations[i];
            assert_eq!(p.index, *idx);
            assert_eq!(p.expr, *expr);
            assert_eq!(&ts[p.host_range.clone()], format!("${{{}}}", expr));
            assert_eq!(&q.sql[p.sql_range.clone()], format!("${}", idx));
        }

        assert!(pg_query::parse(&q.sql).is_ok());
    }

    #[test]
    fn test_scan_utf8_multibyte_emoji_and_non_ascii_preservation() {
        let ts = r#"
            export const query = sql`
                SELECT '🦀' AS crab, 'café' AS drink, 'München' AS city, 'Привет мир' AS ru
                FROM users
                WHERE emoji = ${userEmoji} AND status = 'active';
            `;
        "#;

        let queries = scan_ts_queries(ts);
        assert_eq!(queries.len(), 1);
        let q = &queries[0];

        // Assert no Latin-1 byte mangling (e.g. 🦀 is 4 bytes [0xF0, 0x9F, 0xA6, 0x80])
        assert!(q.sql.contains("'🦀'"), "Emoji must not be corrupted");
        assert!(
            q.sql.contains("'café'"),
            "Accented character must be intact"
        );
        assert!(
            q.sql.contains("'München'"),
            "Umlaut character must be intact"
        );
        assert!(
            q.sql.contains("'Привет мир'"),
            "Cyrillic string must be intact"
        );
        assert!(
            q.sql.contains("WHERE emoji = $1"),
            "Parameter mapped correctly"
        );

        assert_eq!(q.source_map.interpolations.len(), 1);
        assert_eq!(q.source_map.interpolations[0].expr, "userEmoji");

        // PostgreSQL parser accepts multi-byte UTF-8 literals
        assert!(pg_query::parse(&q.sql).is_ok());
    }

    #[test]
    fn test_source_mapping_accuracy_before_during_after_interpolations() {
        let ts = "const q = sql`SELECT id FROM users WHERE id = ${superLongInterpolatedExpression} AND status = 'active';`;";
        let queries = scan_ts_queries(ts);
        assert_eq!(queries.len(), 1);
        let q = &queries[0];

        // 1. Before interpolation: "SELECT" and "users"
        let sql_users_start = q.sql.find("users").unwrap();
        let host_users_start = ts.find("users").unwrap();
        assert_eq!(
            q.source_map.sql_to_host_offset(sql_users_start),
            host_users_start,
            "Offset before interpolation must match host directly"
        );

        // 2. At interpolation: "$1" mapped to "${superLongInterpolatedExpression}"
        let sql_param_start = q.sql.find("$1").unwrap();
        let interp = &q.source_map.interpolations[0];
        assert_eq!(
            q.source_map.sql_to_host_offset(sql_param_start),
            interp.host_range.start
        );

        let host_param_range = q
            .source_map
            .sql_to_host_range(sql_param_start..sql_param_start + 2);
        assert_eq!(host_param_range, interp.host_range);

        // 3. After interpolation: "status"
        let sql_status_start = q.sql.find("status").unwrap();
        let host_status_start = ts.find("status").unwrap();
        assert_eq!(
            q.source_map.sql_to_host_offset(sql_status_start),
            host_status_start,
            "Offset after interpolation must compensate for length delta"
        );

        let sql_status_range = sql_status_start..sql_status_start + "status".len();
        let host_status_range = q.source_map.sql_to_host_range(sql_status_range);
        assert_eq!(&ts[host_status_range], "status");
    }

    #[test]
    fn test_reverse_mapping_from_host_offsets_back_to_sql() {
        let ts = "const q = sql`SELECT id FROM users WHERE id = ${userId} AND active = true;`;";
        let queries = scan_ts_queries(ts);
        assert_eq!(queries.len(), 1);
        let q = &queries[0];

        // Host offset inside "SELECT"
        let host_sel = ts.find("SELECT").unwrap();
        let sql_sel = q.source_map.host_to_sql_offset(host_sel);
        assert_eq!(sql_sel, Some(q.sql.find("SELECT").unwrap()));

        // Host offset inside "${userId}" (middle of the expression)
        let host_mid_interp = ts.find("userId").unwrap() + 2;
        let sql_interp = q.source_map.host_to_sql_offset(host_mid_interp);
        assert_eq!(sql_interp, Some(q.sql.find("$1").unwrap()));

        // Host offset inside "active" after interpolation
        let host_act = ts.find("active").unwrap();
        let sql_act = q.source_map.host_to_sql_offset(host_act);
        assert_eq!(sql_act, Some(q.sql.find("active").unwrap()));

        // Host offset outside template literal (e.g. before backtick)
        let host_outside = ts.find("const").unwrap();
        assert_eq!(q.source_map.host_to_sql_offset(host_outside), None);
    }

    #[test]
    fn test_complex_and_nested_interpolation_expressions() {
        let ts = r#"
            const q = sql`
                SELECT id FROM items
                WHERE a = ${user.profile.getId()}
                  AND b = ${{ key: "val" }.key}
                  AND c = ${"escaped}brace"};
            `;
        "#;
        let queries = scan_ts_queries(ts);
        assert_eq!(queries.len(), 1);
        let q = &queries[0];

        assert!(q.sql.contains("WHERE a = $1"));
        assert!(q.sql.contains("AND b = $2"));
        assert!(q.sql.contains("AND c = $3;"));
        assert_eq!(q.source_map.interpolations.len(), 3);
        assert_eq!(q.source_map.interpolations[0].expr, "user.profile.getId()");
        assert_eq!(q.source_map.interpolations[1].expr, "{ key: \"val\" }.key");
        assert_eq!(q.source_map.interpolations[2].expr, "\"escaped}brace\"");
    }

    #[test]
    fn test_source_map_round_trip() {
        let ts = "const q = sql`SELECT id FROM users WHERE id = ${userId} AND status = 'active';`;";
        let queries = scan_ts_queries(ts);
        assert_eq!(queries.len(), 1);
        let q = &queries[0];
        let sql = &q.sql;

        let sql_users_idx = sql.find("users").unwrap();
        let host_users_idx = ts.find("users").unwrap();
        assert_eq!(
            q.source_map.sql_to_host_offset(sql_users_idx),
            host_users_idx
        );
        assert_eq!(
            q.source_map.host_to_sql_offset(host_users_idx),
            Some(sql_users_idx)
        );

        let sql_status_idx = sql.find("status").unwrap();
        let host_status_idx = ts.find("status").unwrap();
        assert_eq!(
            q.source_map.sql_to_host_offset(sql_status_idx),
            host_status_idx
        );
        assert_eq!(
            q.source_map.host_to_sql_offset(host_status_idx),
            Some(sql_status_idx)
        );
    }
}
