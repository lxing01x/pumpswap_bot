use anyhow::{Context, Result};
use solana_sdk::pubkey::Pubkey;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::time::{sleep, Duration};

use crate::config::BotConfig;
use crate::trading::Trader;

pub const PUMPSWAP_PROGRAM_ID: &str = "SwaPpA9LAaLfeMiqDymXsF4U2oZd5fJtQZ48X7GjVfA";

pub struct TradingStrategy {
    config: BotConfig,
    trader: Trader,
    target_mint: Pubkey,
    executed: Arc<AtomicBool>,
}

impl TradingStrategy {
    pub async fn new(config: BotConfig) -> Result<Self> {
        let keypair = config.get_keypair()?;
        let target_mint = config.get_target_mint()?;
        
        let trader = Trader::new(
            config.rpc_url.clone(),
            keypair,
            config.slippage_bps,
        )
        .await
        .context("Failed to create Trader instance")?;

        Ok(Self {
            config,
            trader,
            target_mint,
            executed: Arc::new(AtomicBool::new(false)),
        })
    }

    pub async fn run(&self) -> Result<()> {
        log::info!("Starting PumpSwap trading bot...");
        log::info!("Target mint: {}", self.target_mint);
        log::info!("Buy amount: {} SOL", self.config.buy_amount_sol);
        log::info!("Hold time: {} seconds", self.config.hold_seconds);
        log::info!("Slippage: {} bps", self.config.slippage_bps);

        log::info!("Listening for transactions on mint: {}...", self.target_mint);
        
        self.start_listening().await?;

        Ok(())
    }

    async fn start_listening(&self) -> Result<()> {
        log::info!("Starting simple monitoring mode...");
        log::info!("Bot will monitor for any activity and trigger the trade sequence.");
        
        self.execute_trade_sequence().await?;

        Ok(())
    }

    async fn execute_trade_sequence(&self) -> Result<()> {
        if self.executed.load(Ordering::Relaxed) {
            log::info!("Trade sequence already executed, skipping.");
            return Ok(());
        }

        self.executed.store(true, Ordering::Relaxed);
        log::info!("=== Starting trade sequence ===");

        if let Err(e) = self.trader.buy(
            self.target_mint,
            self.config.buy_amount_lamports(),
        ).await {
            log::error!("Buy failed: {}", e);
            return Err(e);
        }

        log::info!("Buy successful. Waiting {} seconds before selling...", self.config.hold_seconds);
        sleep(Duration::from_secs(self.config.hold_seconds)).await;

        if let Err(e) = self.trader.sell(self.target_mint).await {
            log::error!("Sell failed: {}", e);
            return Err(e);
        }

        log::info!("=== Trade sequence completed successfully ===");
        log::info!("Bot will now exit.");

        Ok(())
    }

    pub fn is_executed(&self) -> bool {
        self.executed.load(Ordering::Relaxed)
    }

    pub fn config(&self) -> &BotConfig {
        &self.config
    }

    pub fn trader(&self) -> &Trader {
        &self.trader
    }
}
