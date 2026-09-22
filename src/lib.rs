pub mod analyzer;
pub mod catalog;
pub mod codegen;
pub mod lsp;
pub mod ts_scanner;
pub mod watcher;

pub use analyzer::{
    ColumnMeta, JoinKind, PgType, QueryScope, ScopeContext, SetOpKind, TableBinding,
    unify_cte_types, unify_types,
};
pub use catalog::DriverTarget;
pub use codegen::{CodegenOptions, get_output_file_name};
pub use ts_scanner::{ExtractedQuery, is_query_file, is_ts_js_file, scan_ts_queries};
