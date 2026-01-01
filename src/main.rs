use anyhow::Result;
use clap::Parser;
use otaflux::run;
use otaflux::Cli;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    run(cli).await
}
