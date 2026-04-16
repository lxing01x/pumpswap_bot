use anyhow::{Context, Result};
use solana_sdk::{
    pubkey::Pubkey,
    signature::Keypair,
    commitment_config::CommitmentConfig,
};
use sol_trade_sdk::{
    TradingClient,
    TradeConfig,
    SwqosConfig,
    GasFeeStrategy,
    TradeBuyParams,
    TradeSellParams,
    DexType,
    TradeTokenType,
    trading::core::params::{DexParamEnum, PumpSwapParams},
};
use std::sync::Arc;

pub struct Trader {
    client: TradingClient,
    slippage_bps: u64,
}

impl Trader {
    pub async fn new(
        rpc_url: String,
        keypair: Keypair,
        slippage_bps: u64,
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
        })
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

        let recent_blockhash = self.client.get_latest_blockhash().await
            .context("Failed to get recent blockhash")?;

        let pumpswap_params = PumpSwapParams::from_mint_by_rpc(
            &self.client.get_rpc_client(),
            mint,
        )
        .await
        .context("Failed to get PumpSwap params from mint")?;

        let buy_params = TradeBuyParams {
            dex_type: DexType::PumpSwap,
            input_token_type: TradeTokenType::WSOL,
            mint,
            input_token_amount: sol_amount,
            slippage_basis_points: Some(self.slippage_bps),
            recent_blockhash: Some(recent_blockhash),
            extension_params: DexParamEnum::PumpSwap(pumpswap_params.clone()),
            address_lookup_table_account: None,
            wait_transaction_confirmed: true,
            create_input_token_ata: true,
            close_input_token_ata: true,
            create_mint_ata: true,
            durable_nonce: None,
            fixed_output_token_amount: None,
            gas_fee_strategy: gas_fee_strategy.clone(),
            simulate: false,
            use_exact_sol_amount: Some(true),
        };

        log::info!("Executing buy transaction...");
        let result = self.client.buy(buy_params).await;

        match result {
            Ok(sig) => {
                log::info!("Buy transaction successful! Signature: {}", sig);
                Ok(())
            }
            Err(e) => {
                log::error!("Buy transaction failed: {}", e);
                Err(anyhow::anyhow!("Buy failed: {}", e))
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

        let recent_blockhash = self.client.get_latest_blockhash().await
            .context("Failed to get recent blockhash")?;

        let pumpswap_params = PumpSwapParams::from_mint_by_rpc(
            &self.client.get_rpc_client(),
            mint,
        )
        .await
        .context("Failed to get PumpSwap params from mint")?;

        let sell_params = TradeSellParams {
            dex_type: DexType::PumpSwap,
            output_token_type: TradeTokenType::WSOL,
            mint,
            input_token_amount: token_balance,
            slippage_basis_points: Some(self.slippage_bps),
            recent_blockhash: Some(recent_blockhash),
            extension_params: DexParamEnum::PumpSwap(pumpswap_params.clone()),
            address_lookup_table_account: None,
            wait_transaction_confirmed: true,
            create_output_token_ata: true,
            close_output_token_ata: true,
            durable_nonce: None,
            fixed_output_token_amount: None,
            gas_fee_strategy: gas_fee_strategy.clone(),
            simulate: false,
            with_tip: false,
        };

        log::info!("Executing sell transaction...");
        let result = self.client.sell(sell_params).await;

        match result {
            Ok(sig) => {
                log::info!("Sell transaction successful! Signature: {}", sig);
                Ok(())
            }
            Err(e) => {
                log::error!("Sell transaction failed: {}", e);
                Err(anyhow::anyhow!("Sell failed: {}", e))
            }
        }
    }

    async fn get_token_balance(&self, mint: Pubkey) -> Result<u64> {
        let owner = self.client.payer();
        let token_account = spl_associated_token_account::get_associated_token_address(
            owner,
            &mint,
        );

        let rpc_client = self.client.get_rpc_client();
        
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

    pub fn client(&self) -> &TradingClient {
        &self.client
    }
}
