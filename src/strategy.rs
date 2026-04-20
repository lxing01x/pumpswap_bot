use anyhow::{Context, Result};
use solana_sdk::pubkey::Pubkey;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::sync::Mutex;
use tokio::time::{sleep, Duration};
use uuid::Uuid;

use crate::config::BotConfig;
use crate::trading::{Trader, TradeInfo, RedisStore, TokenTradeRecord};

pub const PUMPSWAP_PROGRAM_ID: &str = "SwaPpA9LAaLfeMiqDymXsF4U2oZd5fJtQZ48X7GjVfA";

pub struct TradingStrategy {
    config: BotConfig,
    trader: Arc<Mutex<Trader>>,
    target_mint: Pubkey,
    executed: Arc<AtomicBool>,
    bought: Arc<AtomicBool>,
    latest_trade_info: Arc<Mutex<Option<TradeInfo>>>,
    redis_store: Arc<Mutex<Option<RedisStore>>>,
}

impl TradingStrategy {
    pub async fn new(config: BotConfig) -> Result<Self> {
        let keypair = config.get_keypair()?;
        let target_mint = config.get_target_mint()?;
        
        let trader = Trader::new_with_options(
            config.rpc_url.clone(),
            keypair,
            config.slippage_bps,
            config.max_retries,
            config.retry_delay_ms,
            config.jito_enabled,
            config.jito_uuid.clone(),
            &config.jito_region,
        )
        .await
        .context("Failed to create Trader instance")?;

        let redis_store = match RedisStore::new(&config.redis_url, config.max_trades_per_token) {
            Ok(store) => {
                log::info!("Redis connection established successfully");
                Some(store)
            }
            Err(e) => {
                log::warn!("Failed to connect to Redis: {}. Trading will continue without Redis storage.", e);
                None
            }
        };

        Ok(Self {
            config,
            trader: Arc::new(Mutex::new(trader)),
            target_mint,
            executed: Arc::new(AtomicBool::new(false)),
            bought: Arc::new(AtomicBool::new(false)),
            latest_trade_info: Arc::new(Mutex::new(None)),
            redis_store: Arc::new(Mutex::new(redis_store)),
        })
    }

    pub async fn run(&self) -> Result<()> {
        log::info!("Starting PumpSwap trading bot...");
        log::info!("Target mint: {}", self.target_mint);
        log::info!("Buy amount: {} SOL", self.config.buy_amount_sol);
        log::info!("Slippage: {} bps", self.config.slippage_bps);
        log::info!("Max retries: {}", self.config.max_retries);
        log::info!("Retry delay: {} ms", self.config.retry_delay_ms);
        log::info!("Jito enabled: {}", self.config.jito_enabled);
        if self.config.jito_enabled {
            log::info!("Jito region: {}", self.config.jito_region);
        }
        log::info!("Buy threshold: {}% in {} seconds", self.config.buy_threshold_pct, self.config.buy_time_window_sec);
        log::info!("Sell profit threshold: {}%", self.config.sell_profit_pct);
        log::info!("Sell stop loss threshold: {}%", self.config.sell_stop_loss_pct);

        log::info!("Listening for transactions on mint: {}...", self.target_mint);
        
        self.start_monitoring().await?;

        Ok(())
    }

    async fn start_monitoring(&self) -> Result<()> {
        log::info!("Starting monitoring mode...");
        log::info!("Bot will monitor price changes and execute trades based on configured thresholds.");
        
        log::info!("Prefetching TradeInfo for target mint...");
        let trade_info = {
            let trader = self.trader.lock().unwrap();
            trader.fetch_trade_info_with_retry(&self.target_mint).await
                .context("Failed to prefetch TradeInfo")?
        };
        
        log::info!("Successfully prefetched TradeInfo:");
        log::info!("  Pool: {}", trade_info.pool);
        log::info!("  Base mint: {}", trade_info.base_mint);
        log::info!("  Quote mint: {}", trade_info.quote_mint);
        log::info!("  Pool base token account: {}", trade_info.pool_base_token_account);
        log::info!("  Pool quote token account: {}", trade_info.pool_quote_token_account);
        log::info!("  Is cashback coin: {}", trade_info.is_cashback_coin);
        
        self.store_latest_trade_info(trade_info.clone());
        
        self.monitoring_loop().await?;

        Ok(())
    }

    async fn monitoring_loop(&self) -> Result<()> {
        log::info!("Entering monitoring loop...");
        
        let check_interval = Duration::from_secs(1);
        
        loop {
            if self.bought.load(Ordering::Relaxed) {
                self.check_sell_condition().await?;
            } else {
                self.check_buy_condition().await?;
            }
            
            if self.executed.load(Ordering::Relaxed) {
                log::info!("Trade sequence completed, exiting monitoring loop.");
                break;
            }
            
            sleep(check_interval).await;
        }
        
        Ok(())
    }

    async fn check_buy_condition(&self) -> Result<()> {
        let trade_info = match self.get_latest_trade_info() {
            Some(info) => info,
            None => {
                log::warn!("No TradeInfo available for buy condition check");
                return Ok(());
            }
        };

        let trader = self.trader.lock().unwrap();
        let current_price = trader.calculate_price_from_pool(&trade_info);
        log::info!("Current price: {} token/SOL", current_price);

        let signature = Self::generate_temp_signature();
        self.store_trade_record_with_price(&trade_info.base_mint.to_string(), current_price, true, &signature).await?;

        let should_buy = self.check_price_increase(&trade_info.base_mint.to_string()).await?;
        
        drop(trader);
        
        if should_buy {
            log::info!("Buy condition met! Starting buy operation...");
            self.execute_buy().await?;
        }
        
        Ok(())
    }

    async fn check_price_increase(&self, mint: &str) -> Result<bool> {
        let redis_store = self.redis_store.lock().unwrap();
        
        let store = match redis_store.as_ref() {
            Some(s) => s,
            None => {
                log::warn!("Redis not available, cannot check price increase. Buying immediately.");
                return Ok(true);
            }
        };

        let price_change = store.calculate_price_change(mint, self.config.buy_time_window_sec).await?;
        
        match price_change {
            Some(change_pct) => {
                log::info!("Price change in last {} seconds: {:.2}%", self.config.buy_time_window_sec, change_pct);
                if change_pct >= self.config.buy_threshold_pct {
                    log::info!("Price increase {:.2}% exceeds threshold {:.2}% - BUY SIGNAL", change_pct, self.config.buy_threshold_pct);
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            None => {
                log::info!("Not enough trade data to calculate price change. Waiting for more data...");
                Ok(false)
            }
        }
    }

    async fn check_sell_condition(&self) -> Result<()> {
        let trade_info = match self.get_latest_trade_info() {
            Some(info) => info,
            None => {
                log::warn!("No TradeInfo available for sell condition check");
                return Ok(());
            }
        };

        let trader = self.trader.lock().unwrap();
        let current_price = trader.calculate_price_from_pool(&trade_info);
        
        if let Some(profit_pct) = trader.calculate_profit_loss_pct(current_price) {
            log::info!("Current price: {} token/SOL, P/L: {:.2}%", current_price, profit_pct);
        }

        let signature = Self::generate_temp_signature();
        self.store_trade_record_with_price(&trade_info.base_mint.to_string(), current_price, false, &signature).await?;

        let should_sell = trader.should_sell(
            current_price,
            self.config.sell_profit_pct,
            self.config.sell_stop_loss_pct,
        );
        
        drop(trader);
        
        if should_sell {
            log::info!("Sell condition met! Starting sell operation...");
            self.execute_sell().await?;
        }
        
        Ok(())
    }

    async fn store_trade_record_with_amounts(
        &self,
        mint: &str,
        token_amount: u64,
        sol_amount: u64,
        is_buy: bool,
        signature: &str,
        blocktime_us: i64,
    ) -> Result<()> {
        let redis_store = self.redis_store.lock().unwrap();
        
        if let Some(store) = redis_store.as_ref() {
            let record = TokenTradeRecord::new(
                mint,
                token_amount,
                sol_amount,
                is_buy,
                signature,
                blocktime_us,
            );
            
            if let Err(e) = store.store_trade(mint, &record).await {
                log::warn!("Failed to store trade record: {}", e);
            } else {
                log::debug!("Stored trade record for {}: signature={}, token_amount={}, sol_amount={}", 
                    mint, signature, token_amount, sol_amount);
            }
        }
        
        Ok(())
    }

    async fn store_trade_record_with_price(
        &self,
        mint: &str,
        price: f64,
        is_buy: bool,
        signature: &str,
    ) -> Result<()> {
        let redis_store = self.redis_store.lock().unwrap();
        
        if let Some(store) = redis_store.as_ref() {
            let record = TokenTradeRecord::with_price_now(mint, price, is_buy, signature);
            
            if let Err(e) = store.store_trade(mint, &record).await {
                log::warn!("Failed to store trade record: {}", e);
            } else {
                log::debug!("Stored trade record for {}: price={} SOL/token, is_buy={}", mint, price, is_buy);
            }
        }
        
        Ok(())
    }

    fn generate_temp_signature() -> String {
        Uuid::new_v4().to_string()
    }

    async fn execute_buy(&self) -> Result<()> {
        if self.bought.load(Ordering::Relaxed) {
            log::info!("Already bought, skipping buy.");
            return Ok(());
        }

        let trade_info = self.get_latest_trade_info()
            .context("No TradeInfo available for buy")?;

        let mut trader = self.trader.lock().unwrap();
        
        if let Err(e) = trader.buy(
            &trade_info,
            self.config.buy_amount_lamports(),
        ).await {
            log::error!("Buy failed: {}", e);
            return Err(e);
        }

        self.bought.store(true, Ordering::Relaxed);
        log::info!("Buy successful. Now monitoring for sell conditions.");

        Ok(())
    }

    async fn execute_sell(&self) -> Result<()> {
        if self.executed.load(Ordering::Relaxed) {
            log::info!("Trade sequence already executed, skipping.");
            return Ok(());
        }

        self.executed.store(true, Ordering::Relaxed);
        log::info!("=== Starting sell operation ===");

        let trade_info = self.get_latest_trade_info()
            .context("No TradeInfo available for sell")?;

        let trader = self.trader.lock().unwrap();
        
        if let Err(e) = trader.sell(&trade_info).await {
            log::error!("Sell failed: {}", e);
            return Err(e);
        }

        log::info!("=== Trade sequence completed successfully ===");
        log::info!("Bot will now exit.");

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

    pub fn is_executed(&self) -> bool {
        self.executed.load(Ordering::Relaxed)
    }

    pub fn is_bought(&self) -> bool {
        self.bought.load(Ordering::Relaxed)
    }

    pub fn config(&self) -> &BotConfig {
        &self.config
    }

    pub fn trader(&self) -> Arc<Mutex<Trader>> {
        self.trader.clone()
    }
}
