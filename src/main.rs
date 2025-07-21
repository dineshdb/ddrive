use clap::Parser;
use ddrive::cli::{Cli, run_command};
use tracing::error;
use tracing_subscriber::{self, EnvFilter};

#[tokio::main]
async fn main() {
    // Initialize tracing with minimal formatting (INFO messages only, no date/callsite)
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env().add_directive("ddrive=info".parse().unwrap()),
        )
        .without_time()
        .with_level(false)
        .with_ansi(true)
        .with_target(false)
        .init();

    let cli = Cli::parse();
    if let Err(e) = run_command(cli).await {
        let exit_code = e.exit_code();
        error!("error: {}", e);
        std::process::exit(exit_code);
    }
}
