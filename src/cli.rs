use crate::catalog::DriverTarget;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser, Debug)]
#[command(
    name = "sqltype",
    about = "Ultra-fast, local-first SQL-to-TypeScript compiler CLI in Rust",
    version
)]
pub struct Cli {
    /// Explicit path to configuration file (sqltype.toml or sqltype.json)
    #[arg(long, short = 'c', global = true)]
    pub config: Option<PathBuf>,

    #[command(subcommand)]
    pub command: Commands,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Initializes a new sqltype project with configuration and demo files
    Init(InitArgs),

    /// Validates all queries against the migration schema without writing files
    Check(CheckArgs),

    /// Compiles queries and emits TypeScript types and driver wrappers
    Generate(GenerateArgs),

    /// Watch for file changes and re-generate TypeScript types incrementally
    Watch(GenerateArgs),

    /// Starts the Language Server Protocol (LSP) server for IDE integration
    Lsp(LspArgs),
}

#[derive(Parser, Debug, Clone)]
pub struct InitArgs {
    /// Target root directory to initialize
    #[arg(default_value = ".")]
    pub path: PathBuf,

    /// Configuration file format to emit (toml or json)
    #[arg(long, default_value = "toml")]
    pub format: String,

    /// Explicit database driver override (postgres.js, pg, bun:sql)
    #[arg(long, short = 'd')]
    pub driver: Option<String>,

    /// Overwrite existing configuration and demo files
    #[arg(long, short = 'f', default_value_t = false)]
    pub force: bool,
}

#[derive(Parser, Debug, Clone)]
pub struct CheckArgs {
    /// Directory containing PostgreSQL migration SQL files
    #[arg(long, short = 'm')]
    pub migrations: Option<PathBuf>,

    /// Directory or pattern containing SQL query files
    #[arg(long, short = 'q')]
    pub queries: Option<PathBuf>,

    /// Driver target profile (postgres, pg, bun)
    #[arg(long, short = 'd', value_enum)]
    pub driver: Option<DriverTarget>,

    /// Validate type-safe query execution wrappers
    #[arg(long, short = 'w', default_value_t = false)]
    pub wrappers: bool,

    /// Output diagnostics as machine-readable JSON
    #[arg(long, default_value_t = false)]
    pub json: bool,

    /// Stop verification immediately on the first error
    #[arg(long, default_value_t = false)]
    pub fail_fast: bool,
}

#[derive(Parser, Debug, Clone)]
pub struct GenerateArgs {
    /// Directory containing PostgreSQL migration SQL files
    #[arg(long, short = 'm')]
    pub migrations: Option<PathBuf>,

    /// Directory or pattern containing SQL query files
    #[arg(long, short = 'q')]
    pub queries: Option<PathBuf>,

    /// Directory or file where TypeScript files will be emitted
    #[arg(long, short = 'o')]
    pub out: Option<PathBuf>,

    /// Watch for file changes and re-generate TypeScript types incrementally
    #[arg(long, short = 'w', default_value_t = false)]
    pub watch: bool,

    /// Driver target profile (postgres, pg, bun)
    #[arg(long, short = 'd', value_enum)]
    pub driver: Option<DriverTarget>,

    /// Generate type-safe query execution wrappers
    #[arg(long, default_value_t = false)]
    pub wrappers: bool,

    /// Number of worker threads for parallel query analysis
    #[arg(long, short = 't')]
    pub threads: Option<usize>,

    /// Emit TypeScript type declaration files (.d.ts) only
    #[arg(long, default_value_t = false)]
    pub declaration_only: bool,
}

#[derive(Parser, Debug, Clone)]
pub struct LspArgs {
    /// Directory containing PostgreSQL migration SQL files
    #[arg(long, short = 'm', default_value = "./migrations")]
    pub migrations: PathBuf,
}
