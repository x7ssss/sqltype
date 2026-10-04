use std::ops::Range;

pub use crate::ts_scanner::{
    ExtractedQuery, InterpolatedParam, SourceMap, SourceMappingSegment,
};

/// High-level wrapper around an extracted query with source mapping metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MappedQuery {
    pub query: ExtractedQuery,
}

impl MappedQuery {
    pub fn new(query: ExtractedQuery) -> Self {
        Self { query }
    }

    /// Maps a SQL byte offset to a host document byte offset.
    pub fn sql_to_host_offset(&self, sql_offset: usize) -> usize {
        self.query.source_map.sql_to_host_offset(sql_offset)
    }

    /// Maps a SQL byte range to a host document byte range.
    pub fn sql_to_host_range(&self, sql_range: Range<usize>) -> Range<usize> {
        self.query.source_map.sql_to_host_range(sql_range)
    }

    /// Maps a host document byte offset back to a SQL byte offset.
    pub fn host_to_sql_offset(&self, host_offset: usize) -> Option<usize> {
        self.query.source_map.host_to_sql_offset(host_offset)
    }

    /// Returns the 1-based parameter index at the given host document byte offset, if any.
    pub fn get_param_at_host_offset(&self, host_offset: usize) -> Option<usize> {
        self.query
            .source_map
            .find_interpolated_param(host_offset)
            .map(|p| p.index)
    }
}

/// Translates a SQL byte range to a host document range.
pub fn sql_range_to_host_range(
    query: &ExtractedQuery,
    sql_range: Range<usize>,
) -> Range<usize> {
    query.source_map.sql_to_host_range(sql_range)
}

/// Translates a SQL byte offset to a host document offset.
pub fn sql_offset_to_host_offset(query: &ExtractedQuery, sql_offset: usize) -> usize {
    query.source_map.sql_to_host_offset(sql_offset)
}

/// Translates a host document byte offset into a SQL byte offset.
pub fn host_offset_to_sql_offset(query: &ExtractedQuery, host_offset: usize) -> Option<usize> {
    query.source_map.host_to_sql_offset(host_offset)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ts_scanner::scan_ts_queries;

    #[test]
    fn test_mapped_query_wrapper_and_param_lookup() {
        let ts = "const q = sql`SELECT id FROM users WHERE id = ${userId} AND age = ${minAge};`;";
        let queries = scan_ts_queries(ts);
        assert_eq!(queries.len(), 1);

        let mapped = MappedQuery::new(queries[0].clone());
        assert_eq!(mapped.query.name.as_deref(), Some("Q"));

        // Host offset in ${userId}
        let user_id_pos = ts.find("userId").unwrap();
        assert_eq!(mapped.get_param_at_host_offset(user_id_pos), Some(1));

        // Host offset in ${minAge}
        let min_age_pos = ts.find("minAge").unwrap();
        assert_eq!(mapped.get_param_at_host_offset(min_age_pos), Some(2));

        // Host offset outside interpolations
        let select_pos = ts.find("SELECT").unwrap();
        assert_eq!(mapped.get_param_at_host_offset(select_pos), None);
    }
}
