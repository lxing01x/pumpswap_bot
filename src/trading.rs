use anyhow::{Context, Result};
use chrono::Utc;
use redis::{AsyncCommands, Client as RedisClient};
use solana_address::Address;
use solana_commitment_config::CommitmentConfig;
use solana_hash::Hash;
use solana_keypair::Keypair;
use solana_keypair::Signer;
use solana_sdk::pubkey::Pubkey;
use sol_trade_sdk::{
    TradingClient,
    common::types::TradeConfig,
    swqos::{SwqosConfig, SwqosRegion},
    common::gas_fee_strategy::GasFeeStrategy,
    trading::factory::DexType,
    TradeBuyParams,
    TradeSellParams,
    TradeTokenType,
    trading::core::params::{DexParamEnum, PumpSwapParams},
    instruction::utils::pumpswap,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::time::{sleep, Duration};

pub const PUMP_PROGRAM_ID: Pubkey = solana_sdk::pubkey!("6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P");
pub const PUMPSWAP_PROGRAM_ID: Pubkey = solana_sdk::pubkey!("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA");
pub const WSOL_MINT: Pubkey = solana_sdk::pubkey!("So11111111111111111111111111111111111111112");
pub const CANONICAL_POOL_INDEX: u16 = 0;

pub const MAYHEM_FEE_RECIPIENT_SWAP: Address = solana_address::address!("8N3GDaZ2iwN65oxVatKTLPNooAVUJTbfiVJ1ahyqwjSk");
pub const TOKEN_PROGRAM: Address = solana_address::address!("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA");
pub const TOKEN_PROGRAM_2022: Address = solana_address::address!("TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb");

fn pubkey_to_address(pubkey: &Pubkey) -> Address {
    Address::from(pubkey.to_bytes())
}

fn address_to_pubkey(address: &Address) -> Pubkey {
    Pubkey::from(address.to_bytes())
}

fn parse_jito_region(region: &str) -> SwqosRegion {
    match region.to_lowercase().as_str() {
        "frankfurt" => SwqosRegion::Frankfurt,
        "newyork" => SwqosRegion::NewYork,
        "tokyo" => SwqosRegion::Tokyo,
        "amsterdam" => SwqosRegion::Amsterdam,
        _ => SwqosRegion::Frankfurt,
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenTradeRecord {
    pub mint: String,
    pub token_amount: u64,
    pub sol_amount: u64,
    pub price: f64,
    
    #[serde(alias = "timestamp", deserialize_with = "deserialize_blocktime")]
    pub blocktime_us: i64,
    
    pub is_buy: bool,
    
    #[serde(default)]
    pub signature: String,
}

fn deserialize_blocktime<'de, D>(deserializer: D) -> Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::Deserialize;
    
    let value = serde_json::Value::deserialize(deserializer)?;
    
    if let Some(num) = value.as_i64() {
        if num < 1_000_000_000_000 {
            Ok(num * 1_000_000)
        } else {
            Ok(num)
        }
    } else if let Some(ts_str) = value.as_str() {
        if let Ok(num) = ts_str.parse::<i64>() {
            if num < 1_000_000_000_000 {
                Ok(num * 1_000_000)
            } else {
                Ok(num)
            }
        } else {
            Ok(Utc::now().timestamp_micros())
        }
    } else {
        Ok(Utc::now().timestamp_micros())
    }
}

impl TokenTradeRecord {
    pub fn from_transaction(
        mint: &str,
        token_amount: u64,
        sol_amount: u64,
        is_buy: bool,
        signature: &str,
        blocktime_us: i64,
    ) -> Self {
        // sol_amount is in lamports (1 SOL = 1_000_000_000 lamports, 9 decimals)
        // token_amount is in token's smallest unit (Pump.fun tokens have 6 decimals)
        // 
        // Correct price calculation:
        // SOL = sol_amount / 10^9
        // tokens = token_amount / 10^6
        // price = SOL / tokens = (sol_amount / 10^9) / (token_amount / 10^6)
        //       = (sol_amount / token_amount) * (10^6 / 10^9)
        //       = (sol_amount / token_amount) / 1000
        
        let price = if token_amount > 0 {
            (sol_amount as f64 / token_amount as f64) / 1000.0
        } else {
            0.0
        };
        log::info!("TokenTradeRecord: mint={}, token_amount={}, sol_amount={}, price={:.12} SOL/token", 
            mint, token_amount, sol_amount, price);
        Self {
            mint: mint.to_string(),
            token_amount,
            sol_amount,
            price,
            blocktime_us,
            is_buy,
            signature: signature.to_string(),
        }
    }

    pub fn calculate_price_from_amounts(sol_amount: u64, token_amount: u64) -> f64 {
        // sol_amount is in lamports (9 decimals), token_amount is in token units (6 decimals)
        // price = (sol_amount / token_amount) / 1000
        if token_amount > 0 {
            (sol_amount as f64 / token_amount as f64) / 1000.0
        } else {
            0.0
        }
    }

    pub fn effective_price(&self) -> f64 {
        // Use the pre-calculated price field which is already correctly computed
        self.price
    }
}

pub struct RedisStore {
    client: RedisClient,
    max_trades_per_token: usize,
}

impl RedisStore {
    pub fn new(redis_url: &str, max_trades_per_token: usize) -> Result<Self> {
        let client = RedisClient::open(redis_url)
            .context("Failed to create Redis client")?;
        Ok(Self {
            client,
            max_trades_per_token,
        })
    }

    fn signatures_key(mint: &str) -> String {
        format!("sigs:{}", mint)
    }

    pub async fn is_signature_exists(&self, mint: &str, signature: &str) -> Result<bool> {
        let sigs_key = Self::signatures_key(mint);
        let mut conn = self.client.get_async_connection().await
            .context("Failed to get Redis connection")?;
        
        let exists: bool = conn.sismember(&sigs_key, signature).await
            .context("Failed to check signature existence")?;
        
        Ok(exists)
    }

    pub async fn store_trade(&self, mint: &str, record: &TokenTradeRecord) -> Result<()> {
        if self.is_signature_exists(mint, &record.signature).await? {
            log::debug!("Signature {} already exists, skipping duplicate", record.signature);
            return Ok(());
        }

        let key = format!("trades:{}", mint);
        let sigs_key = Self::signatures_key(mint);
        let serialized = serde_json::to_string(record)
            .context("Failed to serialize trade record")?;
        
        let mut conn = self.client.get_async_connection().await
            .context("Failed to get Redis connection")?;
        
        let _: () = redis::pipe()
            .atomic()
            .lpush(&key, &serialized)
            .ltrim(&key, 0, self.max_trades_per_token as isize - 1)
            .sadd(&sigs_key, &record.signature)
            .query_async(&mut conn).await
            .context("Failed to store trade record to Redis")?;
        
        log::debug!("Stored trade record for {}: signature={}, blocktime_us={}", 
            mint, record.signature, record.blocktime_us);
        
        Ok(())
    }

    pub async fn get_recent_trades(&self, mint: &str, limit: usize) -> Result<Vec<TokenTradeRecord>> {
        let key = format!("trades:{}", mint);
        let mut conn = self.client.get_async_connection().await
            .context("Failed to get Redis connection")?;
        
        let records: Vec<String> = conn.lrange(&key, 0, limit as isize - 1).await
            .context("Failed to get trade records from Redis")?;
        
        let mut result = Vec::with_capacity(records.len());
        for record_str in records {
            let record: TokenTradeRecord = serde_json::from_str(&record_str)
                .context("Failed to deserialize trade record")?;
            result.push(record);
        }
        
        result.sort_by(|a, b| b.blocktime_us.cmp(&a.blocktime_us));
        
        Ok(result)
    }

    pub async fn get_trades_in_window(&self, mint: &str, seconds: u64) -> Result<Vec<TokenTradeRecord>> {
        let now_us = Utc::now().timestamp_micros();
        let cutoff_us = now_us - (seconds as i64 * 1_000_000);
        
        log::info!("get_trades_in_window: mint={}, seconds={}, now_us={}, cutoff_us={}", 
            mint, seconds, now_us, cutoff_us);
        
        let trades = self.get_recent_trades(mint, self.max_trades_per_token).await?;
        log::info!("  Total trades in Redis: {}", trades.len());
        
        let filtered: Vec<TokenTradeRecord> = trades
            .into_iter()
            .filter(|t| {
                let in_window = t.blocktime_us >= cutoff_us;
                // log::info!("    Trade (sig={}, blocktime_us={}): in_window={}", 
                //     t.signature, t.blocktime_us, in_window);
                in_window
            })
            .collect();
        
        log::info!("  Trades in window: {}", filtered.len());
        Ok(filtered)
    }

    pub async fn get_latest_price_from_trades(&self, mint: &str) -> Result<Option<f64>> {
        let trades = self.get_recent_trades(mint, 1).await?;
        
        if let Some(latest) = trades.first() {
            Ok(Some(latest.effective_price()))
        } else {
            Ok(None)
        }
    }

    pub async fn calculate_price_change(&self, mint: &str, seconds: u64) -> Result<Option<f64>> {
        let trades = self.get_trades_in_window(mint, seconds).await?;
        
        log::info!("calculate_price_change: mint={}, seconds={}, trades_in_window={}", mint, seconds, trades.len());
        for (i, trade) in trades.iter().enumerate() {
            log::info!("  Trade {}: signature={}, blocktime_us={}, price={:.12}", 
                i, trade.signature, trade.blocktime_us, trade.effective_price());
        }
        
        if trades.len() < 2 {
            return Ok(None);
        }
        
        let oldest_price = trades.last().unwrap().effective_price();
        let newest_price = trades.first().unwrap().effective_price();
        
        if oldest_price == 0.0 {
            return Ok(None);
        }
        
        let change_pct = ((newest_price - oldest_price) / oldest_price) * 100.0;
        Ok(Some(change_pct))
    }

    pub async fn calculate_price_change_from_records(&self, mint: &str, record_count: usize) -> Result<Option<f64>> {
        let trades = self.get_recent_trades(mint, record_count).await?;
        
        log::info!("calculate_price_change_from_records: mint={}, record_count={}, total_trades={}", mint, record_count, trades.len());
        for (i, trade) in trades.iter().enumerate() {
            log::info!("  Trade {}: signature={}, blocktime_us={}, price={:.12}", 
                i, trade.signature, trade.blocktime_us, trade.effective_price());
        }
        
        if trades.len() < 2 {
            return Ok(None);
        }
        
        let oldest_price = trades.last().unwrap().effective_price();
        let newest_price = trades.first().unwrap().effective_price();
        
        if oldest_price == 0.0 {
            return Ok(None);
        }
        
        let change_pct = ((newest_price - oldest_price) / oldest_price) * 100.0;
        Ok(Some(change_pct))
    }

    pub async fn get_active_mints(&self) -> Result<Vec<String>> {
        let mut conn = self.client.get_async_connection().await
            .context("Failed to get Redis connection")?;
        
        let keys: Vec<String> = redis::cmd("KEYS")
            .arg("trades:*")
            .query_async(&mut conn)
            .await
            .unwrap_or_default();
        
        let mints: Vec<String> = keys
            .into_iter()
            .filter_map(|k| k.strip_prefix("trades:").map(|s| s.to_string()))
            .collect();
        
        Ok(mints)
    }
}

#[derive(Debug, Clone)]
pub struct TradeInfo {
    pub pool: Address,
    pub base_mint: Address,
    pub quote_mint: Address,
    pub pool_base_token_account: Address,
    pub pool_quote_token_account: Address,
    pub pool_base_token_reserves: u64,
    pub pool_quote_token_reserves: u64,
    pub coin_creator_vault_ata: Address,
    pub coin_creator_vault_authority: Address,
    pub base_token_program: Address,
    pub quote_token_program: Address,
    pub fee_recipient: Address,
    pub is_cashback_coin: bool,
    pub is_mayhem_mode: bool,
}

impl TradeInfo {
    pub fn from_pumpswap_params(params: &PumpSwapParams) -> Self {
        let fee_recipient = if params.is_mayhem_mode {
            MAYHEM_FEE_RECIPIENT_SWAP
        } else {
            Address::default()
        };

        Self {
            pool: params.pool,
            base_mint: params.base_mint,
            quote_mint: params.quote_mint,
            pool_base_token_account: params.pool_base_token_account,
            pool_quote_token_account: params.pool_quote_token_account,
            pool_base_token_reserves: params.pool_base_token_reserves,
            pool_quote_token_reserves: params.pool_quote_token_reserves,
            coin_creator_vault_ata: params.coin_creator_vault_ata,
            coin_creator_vault_authority: params.coin_creator_vault_authority,
            base_token_program: params.base_token_program,
            quote_token_program: params.quote_token_program,
            fee_recipient,
            is_cashback_coin: params.is_cashback_coin,
            is_mayhem_mode: params.is_mayhem_mode,
        }
    }

    pub fn base_mint_pubkey(&self) -> Pubkey {
        address_to_pubkey(&self.base_mint)
    }

    pub fn derive_canonical_pool_address(mint: &Pubkey) -> Pubkey {
        let pool_authority = Self::derive_pool_authority_address(mint);
        Self::derive_pumpswap_pool_address(&pool_authority, mint, &WSOL_MINT)
    }

    fn derive_pool_authority_address(mint: &Pubkey) -> Pubkey {
        let seeds = &[b"pool-authority", mint.as_ref()];
        let (address, _bump) = Pubkey::find_program_address(seeds, &PUMP_PROGRAM_ID);
        address
    }

    fn derive_pumpswap_pool_address(
        creator: &Pubkey,
        base_mint: &Pubkey,
        quote_mint: &Pubkey,
    ) -> Pubkey {
        let index: u16 = 0;
        let index_bytes = index.to_le_bytes();
        let seeds: &[&[u8]] = &[
            b"pool",
            &index_bytes,
            creator.as_ref(),
            base_mint.as_ref(),
            quote_mint.as_ref(),
        ];
        let (address, _bump) = Pubkey::find_program_address(seeds, &PUMPSWAP_PROGRAM_ID);
        address
    }
}

pub struct Trader {
    client: TradingClient,
    slippage_bps: u64,
    max_retries: u32,
    retry_delay_ms: u64,
    buy_price: Option<f64>,
    buy_sol_amount: u64,
}

impl Trader {
    pub async fn new(
        rpc_url: String,
        keypair: Keypair,
        slippage_bps: u64,
    ) -> Result<Self> {
        Self::new_with_options(rpc_url, keypair, slippage_bps, 5, 1000, false, None, "Frankfurt").await
    }

    pub async fn new_with_retry(
        rpc_url: String,
        keypair: Keypair,
        slippage_bps: u64,
        max_retries: u32,
        retry_delay_ms: u64,
    ) -> Result<Self> {
        Self::new_with_options(rpc_url, keypair, slippage_bps, max_retries, retry_delay_ms, false, None, "Frankfurt").await
    }

    pub async fn new_with_options(
        rpc_url: String,
        keypair: Keypair,
        slippage_bps: u64,
        max_retries: u32,
        retry_delay_ms: u64,
        jito_enabled: bool,
        jito_uuid: Option<String>,
        jito_region: &str,
    ) -> Result<Self> {
        let commitment = CommitmentConfig::finalized();
        
        let swqos_configs: Vec<SwqosConfig> = if jito_enabled {
            let jito_uuid = jito_uuid.unwrap_or_default();
            let region = parse_jito_region(jito_region);
            vec![
                SwqosConfig::Jito(jito_uuid, region, None),
            ]
        } else {
            vec![
                SwqosConfig::Default(rpc_url.clone()),
            ]
        };

        let trade_config = TradeConfig::builder(rpc_url, swqos_configs, commitment)
            .create_wsol_ata_on_startup(true)
            .use_seed_optimize(false)
            .build();

        let client = TradingClient::new(Arc::new(keypair), trade_config).await;

        Ok(Self {
            client,
            slippage_bps,
            max_retries,
            retry_delay_ms,
            buy_price: None,
            buy_sol_amount: 0,
        })
    }

    fn derive_bonding_curve_address(mint: &Pubkey) -> Pubkey {
        let seeds = &[b"bonding-curve", mint.as_ref()];
        let (address, _bump) = Pubkey::find_program_address(seeds, &PUMP_PROGRAM_ID);
        address
    }

    fn derive_pool_authority_address(mint: &Pubkey) -> Pubkey {
        let seeds = &[b"pool-authority", mint.as_ref()];
        let (address, _bump) = Pubkey::find_program_address(seeds, &PUMP_PROGRAM_ID);
        address
    }

    fn derive_pumpswap_pool_address(
        creator: &Pubkey,
        base_mint: &Pubkey,
        quote_mint: &Pubkey,
    ) -> Pubkey {
        let index: u16 = 0;
        let index_bytes = index.to_le_bytes();
        let seeds: &[&[u8]] = &[
            b"pool",
            &index_bytes,
            creator.as_ref(),
            base_mint.as_ref(),
            quote_mint.as_ref(),
        ];
        let (address, _bump) = Pubkey::find_program_address(seeds, &PUMPSWAP_PROGRAM_ID);
        address
    }

    async fn diagnose_pool_address(&self, mint: &Pubkey) -> Result<()> {
        log::info!("=== Diagnosing PumpSwap pool address ===");
        log::info!("Mint: {}", mint);
        log::info!("WSOL Mint: {}", WSOL_MINT);
        log::info!("Pump Program ID: {}", PUMP_PROGRAM_ID);
        log::info!("PumpSwap Program ID: {}", PUMPSWAP_PROGRAM_ID);

        let bonding_curve = Self::derive_bonding_curve_address(mint);
        log::info!("Derived bonding curve address: {}", bonding_curve);

        let pool_authority = Self::derive_pool_authority_address(mint);
        log::info!("Derived pool-authority address (creator for canonical pool): {}", pool_authority);

        log::info!("Deriving pool addresses...");
        
        let pool_with_pool_authority_as_creator = Self::derive_pumpswap_pool_address(
            &pool_authority,
            mint,
            &WSOL_MINT,
        );
        log::info!(
            "Pool address (creator = pool-authority, index = 0): {}",
            pool_with_pool_authority_as_creator
        );

        log::info!("");
        log::info!("IMPORTANT: For canonical PumpSwap pools (created via migrate instruction):");
        log::info!("  - creator = pool-authority PDA (derived from [b\"pool-authority\", mint])");
        log::info!("  - index = 0");
        log::info!("  - base_mint = token mint");
        log::info!("  - quote_mint = WSOL");
        log::info!("");
        log::info!("Please verify this pool address manually:");
        log::info!("  1. Check if {} exists on Solana", pool_with_pool_authority_as_creator);
        log::info!("  2. Verify it is owned by PumpSwap program ({})", PUMPSWAP_PROGRAM_ID);
        log::info!("");

        log::info!("=== Diagnosis complete ===");
        Ok(())
    }

    pub async fn fetch_trade_info_with_retry(
        &self,
        mint: &Pubkey,
    ) -> Result<TradeInfo> {
        self.diagnose_pool_address(mint).await?;

        log::info!("Searching for pool using find_by_mint (more reliable method) for mint: {}", mint);

        let mut last_error: Option<anyhow::Error> = None;

        for attempt in 0..self.max_retries {
            log::info!(
                "Attempt {}/{} to find and fetch pool data for mint: {}",
                attempt + 1,
                self.max_retries,
                mint
            );

            match pumpswap::find_by_mint(self.client.get_rpc(), mint).await {
                Ok((pool_address, pool_data)) => {
                    log::info!("Successfully found pool on attempt {}: {}", attempt + 1, pool_address);
                    log::info!("Using fetch_pool + from_pool_data to build complete params...");

                    match PumpSwapParams::from_pool_data(
                        self.client.get_rpc(),
                        &pool_address,
                        &pool_data,
                    ).await {
                        Ok(params) => {
                            log::info!("Successfully built PumpSwapParams from pool data");
                            
                            let trade_info = TradeInfo::from_pumpswap_params(&params);
                            
                            log::info!("TradeInfo built from pool data:");
                            log::info!("  Pool: {}", trade_info.pool);
                            log::info!("  Base mint: {}", trade_info.base_mint);
                            log::info!("  Quote mint: {}", trade_info.quote_mint);
                            log::info!("  Pool base token account: {}", trade_info.pool_base_token_account);
                            log::info!("  Pool quote token account: {}", trade_info.pool_quote_token_account);
                            log::info!("  Pool base token reserves: {}", trade_info.pool_base_token_reserves);
                            log::info!("  Pool quote token reserves: {}", trade_info.pool_quote_token_reserves);
                            log::info!("  Coin creator vault ATA: {}", trade_info.coin_creator_vault_ata);
                            log::info!("  Coin creator vault authority: {}", trade_info.coin_creator_vault_authority);
                            log::info!("  Base token program: {}", trade_info.base_token_program);
                            log::info!("  Quote token program: {}", trade_info.quote_token_program);
                            log::info!("  Fee recipient: {}", trade_info.fee_recipient);
                            log::info!("  Is cashback coin: {}", trade_info.is_cashback_coin);
                            log::info!("  Is mayhem mode: {}", trade_info.is_mayhem_mode);
                            
                            return Ok(trade_info);
                        }
                        Err(e) => {
                            log::warn!(
                                "Attempt {}/{} failed to build params from pool data: {}",
                                attempt + 1,
                                self.max_retries,
                                e
                            );
                            last_error = Some(anyhow::anyhow!("{}", e));
                        }
                    }
                }
                Err(e) => {
                    log::warn!(
                        "Attempt {}/{} failed to find pool: {}",
                        attempt + 1,
                        self.max_retries,
                        e
                    );
                    last_error = Some(anyhow::anyhow!("{}", e));
                }
            }

            if attempt < self.max_retries - 1 {
                let delay = self.retry_delay_ms * (attempt as u64 + 1);
                log::info!("Waiting {} ms before next attempt...", delay);
                sleep(Duration::from_millis(delay)).await;
            }
        }

        Err(anyhow::anyhow!(
            "Failed to fetch pool data after {} attempts. Last error: {}. \n\
            Mint: {} \n\
            Possible causes: \n\
            1) The token hasn't fully migrated to PumpSwap yet (check if bonding curve account still exists) \n\
            2) RPC node is not fully synced \n\
            3) The pool doesn't exist on PumpSwap \n\
            If the token is still in bonding curve stage, you may need to use PumpFun instead of PumpSwap.",
            self.max_retries,
            last_error.unwrap_or_else(|| anyhow::anyhow!("Unknown error")),
            mint
        ))
    }

    fn build_pumpswap_params_from_trade_info(&self, trade_info: &TradeInfo) -> PumpSwapParams {
        log::info!("Building PumpSwapParams via from_trade (hot path, no RPC):");
        log::info!("  Pool: {}", trade_info.pool);
        log::info!("  Base mint: {}", trade_info.base_mint);
        log::info!("  Quote mint: {}", trade_info.quote_mint);
        log::info!("  Pool base token account: {}", trade_info.pool_base_token_account);
        log::info!("  Pool quote token account: {}", trade_info.pool_quote_token_account);
        log::info!("  Pool base token reserves: {}", trade_info.pool_base_token_reserves);
        log::info!("  Pool quote token reserves: {}", trade_info.pool_quote_token_reserves);
        log::info!("  Coin creator vault ATA: {}", trade_info.coin_creator_vault_ata);
        log::info!("  Coin creator vault authority: {}", trade_info.coin_creator_vault_authority);
        log::info!("  Base token program: {}", trade_info.base_token_program);
        log::info!("  Quote token program: {}", trade_info.quote_token_program);
        log::info!("  Fee recipient: {}", trade_info.fee_recipient);
        log::info!("  Is cashback coin: {}", trade_info.is_cashback_coin);

        PumpSwapParams::from_trade(
            trade_info.pool,
            trade_info.base_mint,
            trade_info.quote_mint,
            trade_info.pool_base_token_account,
            trade_info.pool_quote_token_account,
            trade_info.pool_base_token_reserves,
            trade_info.pool_quote_token_reserves,
            trade_info.coin_creator_vault_ata,
            trade_info.coin_creator_vault_authority,
            trade_info.base_token_program,
            trade_info.quote_token_program,
            trade_info.fee_recipient,
            trade_info.is_cashback_coin,
        )
    }

    pub fn set_buy_price(&mut self, price: f64, sol_amount: u64) {
        self.buy_price = Some(price);
        self.buy_sol_amount = sol_amount;
    }

    pub fn get_buy_price(&self) -> Option<f64> {
        self.buy_price
    }

    pub fn get_buy_sol_amount(&self) -> u64 {
        self.buy_sol_amount
    }

    pub fn calculate_price_from_pool(&self, trade_info: &TradeInfo) -> f64 {
        let base_reserves = trade_info.pool_base_token_reserves as f64;
        let quote_reserves = trade_info.pool_quote_token_reserves as f64;
        
        if base_reserves == 0.0 {
            0.0
        } else {
            quote_reserves / base_reserves
        }
    }

    pub fn calculate_profit_loss_pct(&self, current_price: f64) -> Option<f64> {
        match self.buy_price {
            Some(buy_price) if buy_price > 0.0 => {
                let pct = ((current_price - buy_price) / buy_price) * 100.0;
                Some(pct)
            }
            _ => None,
        }
    }

    pub fn should_sell(&self, current_price: f64, profit_threshold: f64, stop_loss_threshold: f64) -> bool {
        match self.calculate_profit_loss_pct(current_price) {
            Some(pct) => {
                if pct >= profit_threshold {
                    log::info!("Should sell: Profit {:.2}% exceeds threshold {:.2}%", pct, profit_threshold);
                    true
                } else if pct <= -stop_loss_threshold {
                    log::info!("Should sell: Loss {:.2}% exceeds stop loss threshold {:.2}%", pct.abs(), stop_loss_threshold);
                    true
                } else {
                    false
                }
            }
            None => false,
        }
    }

    pub async fn buy(
        &mut self,
        trade_info: &TradeInfo,
        sol_amount: u64,
    ) -> Result<()> {
        log::info!("Starting buy operation for mint: {}", trade_info.base_mint);
        log::info!("Buy amount: {} lamports ({:.9} SOL)", sol_amount, sol_amount as f64 / 1_000_000_000.0);
        log::info!("Using pool: {}", trade_info.pool);
        log::info!("Is cashback coin: {}", trade_info.is_cashback_coin);

        let payer = self.client.get_payer();
        let payer_pubkey = payer.pubkey();
        let rpc = self.client.get_rpc();
        
        let wallet_balance = rpc.get_balance(&payer_pubkey).await
            .context("Failed to get wallet balance")?;
        
        log::info!("Wallet balance: {} lamports ({:.9} SOL)", wallet_balance, wallet_balance as f64 / 1_000_000_000.0);

        const GAS_RESERVE_LAMPORTS: u64 = 10_000_000;
        let required_balance = sol_amount.checked_add(GAS_RESERVE_LAMPORTS)
            .unwrap_or(u64::MAX);

        if wallet_balance < required_balance {
            log::warn!(
                "Insufficient wallet balance for buy operation. \n\
                Wallet balance: {} lamports ({:.9} SOL)\n\
                Required: {} lamports ({:.9} SOL) [buy amount: {} + gas reserve: {}]\n\
                Skipping buy operation for mint: {} and continuing...",
                wallet_balance, wallet_balance as f64 / 1_000_000_000.0,
                required_balance, required_balance as f64 / 1_000_000_000.0,
                sol_amount, GAS_RESERVE_LAMPORTS,
                trade_info.base_mint
            );
            log::info!("Buy operation skipped due to insufficient balance, continuing normal execution");
            return Ok(());
        }

        log::info!("Wallet balance is sufficient for buy operation");

        let current_price = self.calculate_price_from_pool(trade_info);
        log::info!("Current price before buy: {} SOL/token", current_price);

        let gas_fee_strategy = GasFeeStrategy::new();
        gas_fee_strategy.set_global_fee_strategy(
            150000, 150000, 
            500000, 500000, 
            0.0001, 0.0001
        );

        let recent_blockhash = self.get_latest_blockhash().await
            .context("Failed to get recent blockhash")?;

        let pumpswap_params = self.build_pumpswap_params_from_trade_info(trade_info);

        let mint_bytes = trade_info.base_mint.to_bytes();
        let buy_params = TradeBuyParams {
            dex_type: DexType::PumpSwap,
            input_token_type: TradeTokenType::WSOL,
            mint: mint_bytes.into(),
            input_token_amount: sol_amount,
            slippage_basis_points: Some(self.slippage_bps),
            recent_blockhash: Some(recent_blockhash),
            extension_params: DexParamEnum::PumpSwap(pumpswap_params.clone()),
            address_lookup_table_account: None,
            wait_tx_confirmed: true,
            create_input_token_ata: true,
            close_input_token_ata: true,
            create_mint_ata: true,
            durable_nonce: None,
            fixed_output_token_amount: None,
            gas_fee_strategy: gas_fee_strategy.clone(),
            simulate: false,
            use_exact_sol_amount: Some(true),
            grpc_recv_us: None,
        };

        log::info!("Executing buy transaction...");
        let result = self.client.buy(buy_params).await;

        match result {
            Ok((success, sigs, error, _timings)) => {
                if success {
                    log::info!("Buy transaction successful! Signatures: {:?}", sigs);
                    self.set_buy_price(current_price, sol_amount);
                    log::info!("Recorded buy price: {} token/SOL, sol_amount: {} lamports", current_price, sol_amount);
                    Ok(())
                } else {
                    log::error!("Buy transaction failed: {:?}", error);
                    Err(anyhow::anyhow!("Buy failed: {:?}", error))
                }
            }
            Err(e) => {
                log::error!("Buy transaction error: {}", e);
                Err(anyhow::anyhow!("Buy error: {}", e))
            }
        }
    }

    pub async fn sell(
        &self,
        trade_info: &TradeInfo,
    ) -> Result<()> {
        log::info!("Starting sell operation for mint: {}", trade_info.base_mint);
        log::info!("Using pool: {}", trade_info.pool);
        log::info!("Is cashback coin: {}", trade_info.is_cashback_coin);

        let token_balance = self.get_token_balance(&trade_info.base_mint).await?;
        if token_balance == 0 {
            return Err(anyhow::anyhow!("No token balance to sell for mint: {}", trade_info.base_mint));
        }
        log::info!("Token balance to sell: {}", token_balance);

        let gas_fee_strategy = GasFeeStrategy::new();
        gas_fee_strategy.set_global_fee_strategy(
            150000, 150000,
            500000, 500000,
            0.0001, 0.0001
        );

        let recent_blockhash = self.get_latest_blockhash().await
            .context("Failed to get recent blockhash")?;

        let pumpswap_params = self.build_pumpswap_params_from_trade_info(trade_info);

        let mint_bytes = trade_info.base_mint.to_bytes();
        let sell_params = TradeSellParams {
            dex_type: DexType::PumpSwap,
            output_token_type: TradeTokenType::WSOL,
            mint: mint_bytes.into(),
            input_token_amount: token_balance,
            slippage_basis_points: Some(self.slippage_bps),
            recent_blockhash: Some(recent_blockhash),
            extension_params: DexParamEnum::PumpSwap(pumpswap_params.clone()),
            address_lookup_table_account: None,
            wait_tx_confirmed: true,
            create_output_token_ata: true,
            close_output_token_ata: true,
            durable_nonce: None,
            fixed_output_token_amount: None,
            gas_fee_strategy: gas_fee_strategy.clone(),
            simulate: false,
            with_tip: false,
            close_mint_token_ata: true,
            grpc_recv_us: None,
        };

        log::info!("Executing sell transaction...");
        let result = self.client.sell(sell_params).await;

        match result {
            Ok((success, sigs, error, _timings)) => {
                if success {
                    log::info!("Sell transaction successful! Signatures: {:?}", sigs);
                    Ok(())
                } else {
                    log::error!("Sell transaction failed: {:?}", error);
                    Err(anyhow::anyhow!("Sell failed: {:?}", error))
                }
            }
            Err(e) => {
                log::error!("Sell transaction error: {}", e);
                Err(anyhow::anyhow!("Sell error: {}", e))
            }
        }
    }

    async fn get_token_balance(&self, mint: &Address) -> Result<u64> {
        let owner = self.client.get_payer();
        let owner_bytes = owner.pubkey().to_bytes();
        let mint_bytes = mint.to_bytes();
        
        let token_account = spl_associated_token_account::get_associated_token_address(
            &owner_bytes.into(),
            &mint_bytes.into(),
        );

        let rpc_client = self.client.get_rpc();
        
        match rpc_client.get_token_account_balance(&token_account).await {
            Ok(balance) => {
                log::info!("Token account balance: {} (decimals: {})", balance.amount, balance.decimals);
                Ok(balance.amount.parse::<u64>().unwrap_or(0))
            }
            Err(e) => {
                log::warn!("Failed to get token balance for mint {}: {}. Assuming 0 balance.", mint, e);
                Ok(0)
            }
        }
    }

    async fn get_latest_blockhash(&self) -> Result<Hash> {
        let rpc = self.client.get_rpc();
        let blockhash = rpc
            .get_latest_blockhash()
            .await
            .context("Failed to get latest blockhash")?;
        Ok(blockhash)
    }

    pub fn client(&self) -> &TradingClient {
        &self.client
    }
}
