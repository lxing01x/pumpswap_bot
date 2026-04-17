use anyhow::{Context, Result};
use solana_sdk::pubkey::Pubkey;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use tokio::time::{sleep, Duration};

use crate::config::BotConfig;
use crate::trading::{Trader, TradeInfo};

pub const PUMPSWAP_PROGRAM_ID: &str = "SwaPpA9LAaLfeMiqDymXsF4U2oZd5fJtQZ48X7GjVfA";

pub struct TradingStrategy {
    config: BotConfig,
    trader: Trader,
    target_mint: Pubkey,
    executed: Arc<AtomicBool>,
    latest_trade_info: Arc<Mutex<Option<TradeInfo>>>,
}

impl TradingStrategy {
    pub async fn new(config: BotConfig) -> Result<Self> {
        let keypair = config.get_keypair()?;
        let target_mint = config.get_target_mint()?;
        
        let trader = Trader::new_with_retry(
            config.rpc_url.clone(),
            keypair,
            config.slippage_bps,
            config.max_retries,
            config.retry_delay_ms,
        )
        .await
        .context("Failed to create Trader instance")?;

        Ok(Self {
            config,
            trader,
            target_mint,
            executed: Arc::new(AtomicBool::new(false)),
            latest_trade_info: Arc::new(Mutex::new(None)),
        })
    }

    pub async fn run(&self) -> Result<()> {
        log::info!("Starting PumpSwap trading bot...");
        log::info!("Target mint: {}", self.target_mint);
        log::info!("Buy amount: {} SOL", self.config.buy_amount_sol);
        log::info!("Hold time: {} seconds", self.config.hold_seconds);
        log::info!("Slippage: {} bps", self.config.slippage_bps);
        log::info!("Max retries: {}", self.config.max_retries);
        log::info!("Retry delay: {} ms", self.config.retry_delay_ms);

        log::info!("Listening for transactions on mint: {}...", self.target_mint);
        
        self.start_listening().await?;

        Ok(())
    }

    async fn start_listening(&self) -> Result<()> {
        log::info!("Starting simple monitoring mode...");
        log::info!("Bot will monitor for any activity and trigger the trade sequence.");
        
        log::info!("Prefetching TradeInfo for target mint...");
        let trade_info = self.trader.fetch_trade_info_with_retry(&self.target_mint).await
            .context("Failed to prefetch TradeInfo")?;
        
        log::info!("Successfully prefetched TradeInfo:");
        log::info!("  Pool: {}", trade_info.pool);
        log::info!("  Base mint: {}", trade_info.base_mint);
        log::info!("  Quote mint: {}", trade_info.quote_mint);
        log::info!("  Pool base token account: {}", trade_info.pool_base_token_account);
        log::info!("  Pool quote token account: {}", trade_info.pool_quote_token_account);
        log::info!("  Is cashback coin: {}", trade_info.is_cashback_coin);
        
        self.store_latest_trade_info(trade_info.clone());
        
        self.execute_trade_sequence().await?;

        Ok(())
    }

    fn store_latest_trade_info(&self, trade_info: TradeInfo) {
        let mut info = self.latest_trade_info.lock().unwrap();
        *info = Some(trade_info);
        log::info!("Stored latest TradeInfo for future use");
    }

    fn get_latest_trade_info(&self) -> Option<TradeInfo> {
        let info = self.latest_trade_info.lock().unwrap();
        info.clone()
    }

    async fn execute_trade_sequence(&self) -> Result<()> {
        if self.executed.load(Ordering::Relaxed) {
            log::info!("Trade sequence already executed, skipping.");
            return Ok(());
        }

        self.executed.store(true, Ordering::Relaxed);
        log::info!("=== Starting trade sequence ===");

        let trade_info = self.get_latest_trade_info()
            .context("No TradeInfo available. Make sure to prefetch TradeInfo before executing trades.")?;

        if let Err(e) = self.trader.buy(
            &trade_info,
            self.config.buy_amount_lamports(),
        ).await {
            log::error!("Buy failed: {}", e);
            return Err(e);
        }

        log::info!("Buy successful. Waiting {} seconds before selling...", self.config.hold_seconds);
        sleep(Duration::from_secs(self.config.hold_seconds)).await;

        if let Err(e) = self.trader.sell(&trade_info).await {
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
