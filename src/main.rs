use anyhow::Result;
use clap::Parser;
use otaflux::Cli;
use otaflux::run;

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    run(cli).await
}
