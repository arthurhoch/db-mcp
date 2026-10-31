use std::{path::PathBuf, process::ExitCode};

use clap::Parser;
use db_mcp::{config::Catalog, execution::ExecutionService, mcp::DbMcp};
use rmcp::{ServiceExt, transport::async_rw::AsyncRwTransport};

#[derive(Parser)]
#[command(
    version,
    about = "Local MCP server for Oracle, PostgreSQL, MySQL and SQLite"
)]
struct Cli {
    /// Global TOML configuration. Relative database paths resolve against this file.
    #[arg(long)]
    config: PathBuf,
    /// Validate the complete configuration without connecting or starting MCP.
    #[arg(long)]
    check_config: bool,
}

#[tokio::main]
async fn main() -> ExitCode {
    match run().await {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

async fn run() -> std::result::Result<(), String> {
    let cli = Cli::parse();
    let catalog = Catalog::load(&cli.config).map_err(|error| error.to_string())?;
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(tracing_subscriber::EnvFilter::new(format!(
            "off,db_mcp={}",
            catalog.global.logging.level
        )))
        .with_target(false)
        .with_ansi(false)
        .init();
    if cli.check_config {
        eprintln!(
            "Configuração válida: {} conexão(ões). Nenhum banco foi conectado.",
            catalog.databases.len()
        );
        return Ok(());
    }
    let max_line = catalog
        .global
        .execution
        .max_sql_bytes
        .saturating_add(catalog.global.execution.max_parameter_bytes)
        .saturating_mul(6)
        .saturating_add(65_536);
    let transport = AsyncRwTransport::new(tokio::io::stdin(), tokio::io::stdout())
        .with_max_line_length(max_line);
    let server = DbMcp::new(ExecutionService::new(catalog))
        .serve(transport)
        .await
        .map_err(|_| "Não foi possível iniciar o transporte MCP STDIO.".to_owned())?;
    tokio::select! {
        result = server.waiting() => { result.map_err(|_| "O transporte MCP encerrou com falha.".to_owned())?; }
        _ = tokio::signal::ctrl_c() => {}
    }
    Ok(())
}
