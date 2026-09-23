use clap::Parser;
use sqltype::cli::{Cli, Commands};
use sqltype::commands::check::run_check;
use sqltype::commands::generate::run_generate;
use sqltype::commands::init::run_init;
use sqltype::commands::watch::execute_watch;
use std::env;
use std::process::ExitCode;

fn main() -> ExitCode {
    let cli = Cli::parse();
    let root = env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));

    let res = match &cli.command {
        Commands::Init(args) => run_init(args),
        Commands::Check(args) => run_check(args, &root, cli.config.as_deref()),
        Commands::Generate(args) => {
            if args.watch {
                execute_watch(args, &root, cli.config.as_deref())
            } else {
                run_generate(args, &root, cli.config.as_deref())
            }
        }
        Commands::Watch(args) => execute_watch(args, &root, cli.config.as_deref()),
        Commands::Lsp(args) => {
            let mig = if args.migrations.is_relative() {
                root.join(&args.migrations)
            } else {
                args.migrations.clone()
            };
            sqltype::lsp::run_lsp_server(mig).map_err(|e| format!("[LSP Error] {}", e))
        }
    };

    match res {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}", e);
            ExitCode::FAILURE
        }
    }
}
