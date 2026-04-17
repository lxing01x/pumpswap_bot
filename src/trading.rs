use anyhow::{Context, Result};
use solana_address::Address;
use solana_commitment_config::CommitmentConfig;
use solana_hash::Hash;
use solana_keypair::Keypair;
use solana_keypair::Signer;
use solana_sdk::pubkey::Pubkey;
use sol_trade_sdk::{
    TradingClient,
    common::types::TradeConfig,
    swqos::SwqosConfig,
    common::gas_fee_strategy::GasFeeStrategy,
    trading::factory::DexType,
    TradeBuyParams,
    TradeSellParams,
    TradeTokenType,
    trading::core::params::{DexParamEnum, PumpSwapParams},
};
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
}

impl Trader {
    pub async fn new(
        rpc_url: String,
        keypair: Keypair,
        slippage_bps: u64,
    ) -> Result<Self> {
        Self::new_with_retry(rpc_url, keypair, slippage_bps, 5, 1000).await
    }

    pub async fn new_with_retry(
        rpc_url: String,
        keypair: Keypair,
        slippage_bps: u64,
        max_retries: u32,
        retry_delay_ms: u64,
    ) -> Result<Self> {
        let commitment = CommitmentConfig::finalized();
        
        let swqos_configs: Vec<SwqosConfig> = vec![
            SwqosConfig::Default(rpc_url.clone()),
        ];

        let trade_config = TradeConfig::builder(rpc_url, swqos_configs, commitment)
            .build();

        let client = TradingClient::new(Arc::new(keypair), trade_config).await;

        Ok(Self {
            client,
            slippage_bps,
            max_retries,
            retry_delay_ms,
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

        let pool_address = TradeInfo::derive_canonical_pool_address(mint);
        let pool_address_bytes = pool_address.to_bytes();
        
        log::info!("Derived canonical pool address: {}", pool_address);

        let mut last_error: Option<anyhow::Error> = None;

        for attempt in 0..self.max_retries {
            log::info!(
                "Attempt {}/{} to fetch pool data (via from_pool_address_by_rpc) for pool: {}",
                attempt + 1,
                self.max_retries,
                pool_address
            );

            match PumpSwapParams::from_pool_address_by_rpc(
                self.client.get_rpc(),
                &pool_address_bytes.into(),
            )
            .await
            {
                Ok(params) => {
                    log::info!("Successfully fetched pool data on attempt {}", attempt + 1);
                    
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
                        "Attempt {}/{} failed to fetch pool data: {}",
                        attempt + 1,
                        self.max_retries,
                        e
                    );
                    last_error = Some(anyhow::anyhow!("{}", e));

                    if attempt < self.max_retries - 1 {
                        let delay = self.retry_delay_ms * (attempt as u64 + 1);
                        log::info!("Waiting {} ms before next attempt...", delay);
                        sleep(Duration::from_millis(delay)).await;
                    }
                }
            }
        }

        Err(anyhow::anyhow!(
            "Failed to fetch pool data after {} attempts. Last error: {}. \n\
            Pool address used: {} \n\
            Possible causes: \n\
            1) The token hasn't fully migrated to PumpSwap yet (check if bonding curve account still exists) \n\
            2) RPC node is not fully synced \n\
            3) The pool address is incorrect \n\
            If the token is still in bonding curve stage, you may need to use PumpFun instead of PumpSwap.",
            self.max_retries,
            last_error.unwrap_or_else(|| anyhow::anyhow!("Unknown error")),
            pool_address
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

    pub async fn buy(
        &self,
        trade_info: &TradeInfo,
        sol_amount: u64,
    ) -> Result<()> {
        log::info!("Starting buy operation for mint: {}", trade_info.base_mint);
        log::info!("Buy amount: {} lamports ({:.9} SOL)", sol_amount, sol_amount as f64 / 1_000_000_000.0);
        log::info!("Using pool: {}", trade_info.pool);
        log::info!("Is cashback coin: {}", trade_info.is_cashback_coin);

        let gas_fee_strategy = GasFeeStrategy::new();
        gas_fee_strategy.set_global_fee_strategy(
            150000, 150000, 
            500000, 500000, 
            0.001, 0.001
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
            0.001, 0.001
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
