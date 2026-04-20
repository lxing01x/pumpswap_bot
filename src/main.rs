mod config;
mod trading;
mod strategy;
mod grpc_subscriber;

use anyhow::Result;
use clap::Parser;
use env_logger::Env;
use log::info;

use crate::config::BotConfig;
use crate::strategy::TradingStrategy;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    #[arg(short, long, default_value = "config.json")]
    config: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    env_logger::Builder::from_env(Env::default().default_filter_or("info"))
        .format_timestamp_millis()
        .init();

    let args = Args::parse();
    
    info!("Loading config from: {}", args.config);
    let config = BotConfig::from_file(&args.config)?;
    
    info!("Initializing trading strategy...");
    let strategy = TradingStrategy::new(config).await?;
    
    info!("Starting bot...");
    strategy.run().await?;
    
    info!("Bot completed.");
    Ok(())
}
