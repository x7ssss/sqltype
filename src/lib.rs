pub mod analyzer;
pub mod catalog;
pub mod cli;
pub mod codegen;
pub mod commands;
pub mod config;
pub mod lsp;
pub mod nullability;
pub mod params;
pub mod ts_scanner;

pub use analyzer::{
    ColumnMeta, InferredType, PgType, QueryScope, ScopeContext, SetOpKind,
    unify_cte_types, unify_types,
};
pub use nullability::{
    ColumnBinding, JoinKind, JoinTreeNode, TableBinding, format_nullable,
    project_ts_type, propagate_join_nullability, strip_root_null,
};
pub use params::{QueryParam, deduce_query_params};
pub use catalog::{Catalog, DriverTarget, SchemaCatalog};
pub use cli::{CheckArgs, Cli, Commands, GenerateArgs, InitArgs, LspArgs};
pub use codegen::{
    CodegenOptions, format_property_key, get_output_file_name, is_valid_js_identifier,
};
pub use config::SqltypeConfig;
pub use ts_scanner::{
    ExtractedQuery, InterpolatedParam, SourceMap, SourceMappingSegment, is_query_file,
    is_ts_js_file, scan_ts_queries,
};
