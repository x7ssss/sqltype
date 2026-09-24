pub mod analyzer;
pub mod catalog;
pub mod cli;
pub mod codegen;
pub mod commands;
pub mod config;
pub mod lsp;
pub mod ts_scanner;
pub mod watcher;

pub use analyzer::{
    ColumnMeta, InferredType, JoinKind, PgType, QueryScope, ScopeContext, SetOpKind, TableBinding,
    unify_cte_types, unify_types,
};
pub use catalog::{Catalog, DriverTarget, SchemaCatalog};
pub use cli::{CheckArgs, Cli, Commands, GenerateArgs, InitArgs, LspArgs};
pub use codegen::{
    CodegenOptions, format_property_key, get_output_file_name, is_valid_js_identifier,
};
pub use config::SqltypeConfig;
pub use ts_scanner::{ExtractedQuery, is_query_file, is_ts_js_file, scan_ts_queries};
