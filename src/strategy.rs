use anyhow::{Context, Result};
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::Mutex;
use std::str::FromStr;
use tokio::sync::Mutex as TokioMutex;
use tokio::time::{sleep, Duration};

use crate::config::BotConfig;
use crate::grpc_subscriber::GrpcSubscriber;
use crate::trading::{Trader, TradeInfo, RedisStore, TokenTradeRecord};

pub const PUMPSWAP_PROGRAM_ID: &str = "SwaPpA9LAaLfeMiqDymXsF4U2oZd5fJtQZ48X7GjVfA";

#[derive(Debug, Clone)]
pub struct TokenPosition {
    pub mint: String,
    pub buy_price: f64,
    pub buy_sol_amount: u64,
    pub trade_info: TradeInfo,
}

pub struct TradingStrategy {
    config: BotConfig,
    trader: Arc<Mutex<Trader>>,
    positions: Arc<TokioMutex<HashMap<String, TokenPosition>>>,
    trade_info_cache: Arc<TokioMutex<HashMap<String, TradeInfo>>>,
    redis_store: Arc<TokioMutex<Option<RedisStore>>>,
}

impl TradingStrategy {
    pub async fn new(config: BotConfig) -> Result<Self> {
        let keypair = config.get_keypair()?;
        
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
            positions: Arc::new(TokioMutex::new(HashMap::new())),
            trade_info_cache: Arc::new(TokioMutex::new(HashMap::new())),
            redis_store: Arc::new(TokioMutex::new(redis_store)),
        })
    }

    pub async fn run(&self) -> Result<()> {
        log::info!("Starting PumpSwap trading bot...");
        log::info!("Buy amount: {} SOL", self.config.buy_amount_sol);
        log::info!("Slippage: {} bps", self.config.slippage_bps);
        log::info!("Max retries: {}", self.config.max_retries);
        log::info!("Retry delay: {} ms", self.config.retry_delay_ms);
        log::info!("Jito enabled: {}", self.config.jito_enabled);
        if self.config.jito_enabled {
            log::info!("Jito region: {}", self.config.jito_region);
        }
        log::info!("Buy threshold: {}% in last {} records", self.config.buy_threshold_pct, self.config.buy_record_count);
        log::info!("Sell profit threshold: {}%", self.config.sell_profit_pct);
        log::info!("Sell stop loss threshold: {}%", self.config.sell_stop_loss_pct);

        log::info!("Listening for ALL PumpSwap transactions...");
        
        self.start_monitoring().await?;

        Ok(())
    }

    async fn start_monitoring(&self) -> Result<()> {
        log::info!("Starting monitoring mode...");
        log::info!("Bot will monitor ALL PumpSwap tokens and execute trades based on configured thresholds.");
        
        self.start_grpc_subscription().await?;
        
        self.monitoring_loop().await?;

        Ok(())
    }

    async fn start_grpc_subscription(&self) -> Result<()> {
        log::info!("Starting gRPC subscription...");
        log::info!("gRPC URL: {}", self.config.grpc_url);
        
        let subscriber = GrpcSubscriber::new(
            self.config.grpc_url.clone(),
            self.config.grpc_token.clone(),
        );
        
        let mut rx = subscriber.subscribe().await
            .context("Failed to start gRPC subscription")?;
        
        let redis_store = self.redis_store.clone();
        
        tokio::spawn(async move {
            log::info!("gRPC subscription task started. Waiting for transactions...");
            
            while let Some(update) = rx.recv().await {
                log::debug!("Received transaction update: {:?}", update);
                
                let redis_store = redis_store.lock().await;
                
                if let Some(store) = redis_store.as_ref() {
                    let record = TokenTradeRecord::from_transaction(
                        &update.mint,
                        update.token_amount,
                        update.sol_amount,
                        update.is_buy,
                        &update.signature,
                        update.blocktime_us,
                    );
                    
                    match store.store_trade(&update.mint, &record).await {
                        Ok(_) => {
                            log::info!(
                                "Stored trade from gRPC: mint={}, signature={}, price={:.12} SOL/token",
                                update.mint,
                                update.signature,
                                record.effective_price()
                            );
                        }
                        Err(e) => {
                            log::warn!("Failed to store trade from gRPC: {}", e);
                        }
                    }
                } else {
                    log::warn!("Redis not available, cannot store trade from gRPC");
                }
            }
            
            log::warn!("gRPC subscription channel closed");
        });
        
        log::info!("gRPC subscription started successfully");
        Ok(())
    }

    async fn monitoring_loop(&self) -> Result<()> {
        log::info!("Entering monitoring loop...");
        
        let check_interval = Duration::from_secs(1);
        
        loop {
            let active_mints = self.get_active_mints().await?;
            log::debug!("Active mints with trade data: {:?}", active_mints);
            
            for mint in &active_mints {
                let is_holding = self.is_holding(mint).await;
                
                if is_holding {
                    self.check_sell_condition(mint).await?;
                } else {
                    self.check_buy_condition(mint).await?;
                }
            }
            
            sleep(check_interval).await;
        }
    }

    async fn get_active_mints(&self) -> Result<Vec<String>> {
        let redis_store = self.redis_store.lock().await;
        
        let store = match redis_store.as_ref() {
            Some(s) => s,
            None => {
                return Ok(vec![]);
            }
        };

        store.get_active_mints().await
    }

    async fn is_holding(&self, mint: &str) -> bool {
        let positions = self.positions.lock().await;
        positions.contains_key(mint)
    }

    async fn check_buy_condition(&self, mint: &str) -> Result<()> {
        if self.is_holding(mint).await {
            log::debug!("Already holding {}, skipping buy check", mint);
            return Ok(());
        }
        
        let current_price = self.get_latest_trade_price(mint).await?;
        
        if let Some(price) = current_price {
            log::debug!("Checking buy condition for {}: price={:.12} SOL/token", mint, price);

            let should_buy = self.check_price_increase(mint).await?;
            
            if should_buy {
                log::info!("Buy condition met for {}! Starting buy operation...", mint);
                self.execute_buy(mint).await?;
            }
        }
        
        Ok(())
    }

    async fn get_latest_trade_price(&self, mint: &str) -> Result<Option<f64>> {
        let redis_store = self.redis_store.lock().await;
        
        if let Some(store) = redis_store.as_ref() {
            store.get_latest_price_from_trades(mint).await
        } else {
            log::warn!("Redis not available, cannot get trade price");
            Ok(None)
        }
    }

    async fn check_price_increase(&self, mint: &str) -> Result<bool> {
        let redis_store = self.redis_store.lock().await;
        
        let store = match redis_store.as_ref() {
            Some(s) => s,
            None => {
                log::warn!("Redis not available, cannot check price increase.");
                return Ok(false);
            }
        };

        let price_change = store.calculate_price_change_from_records(mint, self.config.buy_record_count).await?;
        
        match price_change {
            Some(change_pct) => {
                log::info!("Price change for {} in last {} records: {:.2}%", mint, self.config.buy_record_count, change_pct);
                if change_pct >= self.config.buy_threshold_pct {
                    log::info!("Price increase {:.2}% exceeds threshold {:.2}% - BUY SIGNAL for {}", change_pct, self.config.buy_threshold_pct, mint);
                    Ok(true)
                } else {
                    Ok(false)
                }
            }
            None => {
                log::debug!("Not enough trade data for {} to calculate price change. Waiting for more data...", mint);
                Ok(false)
            }
        }
    }

    async fn check_sell_condition(&self, mint: &str) -> Result<()> {
        let positions = self.positions.lock().await;
        let position = match positions.get(mint) {
            Some(p) => p.clone(),
            None => {
                return Ok(());
            }
        };
        drop(positions);
        
        let current_price = self.get_latest_trade_price(mint).await?;
        
        if let Some(price) = current_price {
            let profit_pct = if position.buy_price > 0.0 {
                ((price - position.buy_price) / position.buy_price) * 100.0
            } else {
                0.0
            };
            
            log::info!("Position {}: current price={:.12}, buy_price={:.12}, P/L: {:.2}%", 
                mint, price, position.buy_price, profit_pct);

            let should_sell = if profit_pct >= self.config.sell_profit_pct {
                log::info!("Should sell {}: Profit {:.2}% exceeds threshold {:.2}%", mint, profit_pct, self.config.sell_profit_pct);
                true
            } else if profit_pct <= -self.config.sell_stop_loss_pct {
                log::info!("Should sell {}: Loss {:.2}% exceeds stop loss threshold {:.2}%", mint, profit_pct.abs(), self.config.sell_stop_loss_pct);
                true
            } else {
                false
            };
            
            if should_sell {
                log::info!("Sell condition met for {}! Starting sell operation...", mint);
                self.execute_sell(mint, &position.trade_info).await?;
            }
        }
        
        Ok(())
    }

    async fn get_or_fetch_trade_info(&self, mint: &str) -> Result<TradeInfo> {
        let mut cache = self.trade_info_cache.lock().await;
        
        if let Some(info) = cache.get(mint) {
            return Ok(info.clone());
        }
        
        let mint_pubkey = Pubkey::from_str(mint)
            .context(format!("Invalid mint address: {}", mint))?;
        
        log::info!("Fetching TradeInfo for mint: {}", mint);
        
        let trade_info = {
            let trader = self.trader.lock().unwrap();
            trader.fetch_trade_info_with_retry(&mint_pubkey).await
                .context(format!("Failed to fetch TradeInfo for {}", mint))?
        };
        
        log::info!("Successfully fetched TradeInfo for {}:", mint);
        log::info!("  Pool: {}", trade_info.pool);
        log::info!("  Base mint: {}", trade_info.base_mint);
        log::info!("  Quote mint: {}", trade_info.quote_mint);
        
        cache.insert(mint.to_string(), trade_info.clone());
        
        Ok(trade_info)
    }

    async fn execute_buy(&self, mint: &str) -> Result<()> {
        if self.is_holding(mint).await {
            log::info!("Already holding {}, skipping buy.", mint);
            return Ok(());
        }

        let trade_info = self.get_or_fetch_trade_info(mint).await?;

        let mut trader = self.trader.lock().unwrap();
        
        let current_price = trader.calculate_price_from_pool(&trade_info);
        log::info!("Buying {} at price: {} SOL/token", mint, current_price);
        
        if let Err(e) = trader.buy(
            &trade_info,
            self.config.buy_amount_lamports(),
        ).await {
            log::error!("Buy failed for {}: {}", mint, e);
            return Err(e);
        }

        let position = TokenPosition {
            mint: mint.to_string(),
            buy_price: current_price,
            buy_sol_amount: self.config.buy_amount_lamports(),
            trade_info: trade_info.clone(),
        };
        
        let mut positions = self.positions.lock().await;
        positions.insert(mint.to_string(), position);
        
        log::info!("Buy successful for {}. Now monitoring for sell conditions.", mint);

        Ok(())
    }

    async fn execute_sell(&self, mint: &str, trade_info: &TradeInfo) -> Result<()> {
        log::info!("=== Starting sell operation for {} ===", mint);

        let trader = self.trader.lock().unwrap();
        
        if let Err(e) = trader.sell(trade_info).await {
            log::error!("Sell failed for {}: {}", mint, e);
            return Err(e);
        }

        let mut positions = self.positions.lock().await;
        positions.remove(mint);

        log::info!("=== Sell completed successfully for {} ===", mint);

        Ok(())
    }

    pub fn config(&self) -> &BotConfig {
        &self.config
    }

    pub fn trader(&self) -> Arc<Mutex<Trader>> {
        self.trader.clone()
    }
}
