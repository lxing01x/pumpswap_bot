use anyhow::{Context, Result};
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
        let commitment = CommitmentConfig::processed();
        
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

    async fn get_pumpswap_params_with_retry(
        &self,
        mint: &Pubkey,
    ) -> Result<PumpSwapParams> {
        let mint_bytes = mint.to_bytes();
        let mut last_error: Option<anyhow::Error> = None;

        for attempt in 0..self.max_retries {
            log::info!(
                "Attempt {}/{} to get PumpSwap params for mint: {}",
                attempt + 1,
                self.max_retries,
                mint
            );

            match PumpSwapParams::from_mint_by_rpc(
                self.client.get_rpc(),
                &mint_bytes.into(),
            )
            .await
            {
                Ok(params) => {
                    log::info!("Successfully got PumpSwap params on attempt {}", attempt + 1);
                    return Ok(params);
                }
                Err(e) => {
                    log::warn!(
                        "Attempt {}/{} failed to get PumpSwap params: {}",
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
            "Failed to get PumpSwap params after {} attempts. Last error: {}. \
            This may happen if: 1) The token hasn't migrated to PumpSwap yet, \
            2) RPC node is not synced, or 3) The mint address is incorrect. \
            If the token is still in bonding curve stage, you may need to use PumpFun instead of PumpSwap.",
            self.max_retries,
            last_error.unwrap_or_else(|| anyhow::anyhow!("Unknown error"))
        ))
    }

    pub async fn buy(
        &self,
        mint: Pubkey,
        sol_amount: u64,
    ) -> Result<()> {
        log::info!("Starting buy operation for mint: {}", mint);
        log::info!("Buy amount: {} lamports ({:.9} SOL)", sol_amount, sol_amount as f64 / 1_000_000_000.0);

        let gas_fee_strategy = GasFeeStrategy::new();
        gas_fee_strategy.set_global_fee_strategy(
            150000, 150000, 
            500000, 500000, 
            0.001, 0.001
        );

        let recent_blockhash = self.get_latest_blockhash().await
            .context("Failed to get recent blockhash")?;

        let pumpswap_params = self.get_pumpswap_params_with_retry(&mint).await?;

        let mint_bytes = mint.to_bytes();
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
        mint: Pubkey,
    ) -> Result<()> {
        log::info!("Starting sell operation for mint: {}", mint);

        let token_balance = self.get_token_balance(mint).await?;
        if token_balance == 0 {
            return Err(anyhow::anyhow!("No token balance to sell for mint: {}", mint));
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

        let pumpswap_params = self.get_pumpswap_params_with_retry(&mint).await?;

        let mint_bytes = mint.to_bytes();
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

    async fn get_token_balance(&self, mint: Pubkey) -> Result<u64> {
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
