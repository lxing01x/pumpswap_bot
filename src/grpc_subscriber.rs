use anyhow::{Context, Result};
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use tokio::sync::mpsc;
use tonic::{metadata::MetadataValue, Request};
use yellowstone_grpc_proto::geyser::*;
use yellowstone_grpc_proto::prelude::CommitmentLevel;
use yellowstone_grpc_proto::prelude::geyser_client::GeyserClient;

pub const PUMPSWAP_PROGRAM_ID: Pubkey = solana_sdk::pubkey!("pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA");

#[derive(Debug, Clone)]
pub struct TransactionUpdate {
    pub mint: String,
    pub token_amount: u64,
    pub sol_amount: u64,
    pub is_buy: bool,
    pub signature: String,
    pub blocktime_us: i64,
}

pub struct GrpcSubscriber {
    grpc_url: String,
    grpc_token: Option<String>,
    target_mint: Pubkey,
}

impl GrpcSubscriber {
    pub fn new(grpc_url: String, grpc_token: Option<String>, target_mint: Pubkey) -> Self {
        Self {
            grpc_url,
            grpc_token,
            target_mint,
        }
    }

    pub async fn subscribe(&self) -> Result<mpsc::UnboundedReceiver<TransactionUpdate>> {
        let (tx, rx) = mpsc::unbounded_channel();
        
        let grpc_url = self.grpc_url.clone();
        let grpc_token = self.grpc_token.clone();
        let target_mint = self.target_mint;
        
        tokio::spawn(async move {
            if let Err(e) = Self::run_subscription(grpc_url, grpc_token, target_mint, tx).await {
                log::error!("gRPC subscription error: {:?}", e);
            }
        });
        
        Ok(rx)
    }

    async fn run_subscription(
        grpc_url: String,
        grpc_token: Option<String>,
        target_mint: Pubkey,
        tx: mpsc::UnboundedSender<TransactionUpdate>,
    ) -> Result<()> {
        log::info!("Connecting to gRPC: {}", grpc_url);
        
        let endpoint_url = if grpc_url.starts_with("http") {
            grpc_url.clone()
        } else {
            format!("https://{}", grpc_url)
        };
        
        log::info!("Using endpoint URL: {}", endpoint_url);
        
        let endpoint = match tonic::transport::Endpoint::from_shared(endpoint_url.clone()) {
            Ok(ep) => ep,
            Err(e) => {
                log::error!("Failed to create gRPC endpoint from URL {}: {:?}", endpoint_url, e);
                return Err(anyhow::anyhow!("Failed to create gRPC endpoint: {:?}", e));
            }
        };
        
        let endpoint = endpoint
            .tcp_keepalive(Some(std::time::Duration::from_secs(30)))
            .keep_alive_while_idle(true)
            .connect_timeout(std::time::Duration::from_secs(30))
            .timeout(std::time::Duration::from_secs(60));
        
        log::info!("Connecting to gRPC endpoint...");
        
        let channel = match endpoint.connect().await {
            Ok(ch) => {
                log::info!("Successfully connected to gRPC");
                ch
            }
            Err(e) => {
                log::error!("Failed to connect to gRPC: {:?}", e);
                return Err(anyhow::anyhow!("Failed to connect to gRPC: {:?}", e));
            }
        };

        let token_clone = grpc_token.clone();
        let interceptor = move |mut req: Request<()>| {
            if let Some(ref token) = token_clone {
                let token: MetadataValue<_> = format!("Bearer {}", token).parse().unwrap();
                req.metadata_mut().insert("x-token", token);
            }
            Ok(req)
        };

        let mut client = GeyserClient::with_interceptor(channel, interceptor);

        log::info!("gRPC client created. Subscribing to transactions...");
        log::info!("Target mint: {}", target_mint);
        log::info!("PumpSwap program ID: {}", PUMPSWAP_PROGRAM_ID);

        let mut transactions_filter = HashMap::new();
        transactions_filter.insert(
            "target".to_string(),
            SubscribeRequestFilterTransactions {
                vote: Some(false),
                failed: Some(false),
                signature: None,
                account_include: vec![
                    target_mint.to_string(),
                    PUMPSWAP_PROGRAM_ID.to_string(),
                ],
                account_exclude: vec![],
                account_required: vec![],
            },
        );

        let subscribe_request = SubscribeRequest {
            accounts: HashMap::new(),
            slots: HashMap::new(),
            transactions: transactions_filter,
            transactions_status: HashMap::new(),
            blocks: HashMap::new(),
            blocks_meta: HashMap::new(),
            entry: HashMap::new(),
            commitment: Some(CommitmentLevel::Processed as i32),
            accounts_data_slice: vec![],
            ping: None,
            from_slot: None,
        };

        log::info!("Subscribe request created. Sending subscription request...");
        
        let (subscribe_tx, subscribe_rx) = mpsc::unbounded_channel();
        
        if let Err(e) = subscribe_tx.send(subscribe_request) {
            log::error!("Failed to send subscribe request to channel: {:?}", e);
            return Err(anyhow::anyhow!("Failed to send subscribe request: {:?}", e));
        }

        log::info!("Calling subscribe() on gRPC client...");
        
        let response = match client
            .subscribe(Request::new(tokio_stream::wrappers::UnboundedReceiverStream::new(subscribe_rx)))
            .await
        {
            Ok(resp) => {
                log::info!("Subscribe response received successfully");
                resp
            }
            Err(e) => {
                log::error!("Failed to subscribe to gRPC stream: {:?}", e);
                log::error!("Error details: code={:?}, message={}", e.code(), e.message());
                return Err(anyhow::anyhow!("Failed to subscribe to gRPC stream: {:?}", e));
            }
        };

        let mut stream = response.into_inner();

        log::info!("gRPC subscription started. Waiting for transactions...");

        loop {
            match stream.message().await {
                Ok(Some(message)) => {
                    log::debug!("Received gRPC message: {:?}", message.filters);
                    
                    if let Some(subscribe_update::UpdateOneof::Transaction(transaction_update)) = message.update_oneof {
                        log::info!("Received transaction update from slot: {}", transaction_update.slot);
                        Self::process_transaction_update(transaction_update, &target_mint, &tx);
                    }
                }
                Ok(None) => {
                    log::warn!("gRPC stream ended (received None)");
                    break;
                }
                Err(e) => {
                    log::error!("Error receiving from gRPC stream: {:?}", e);
                    return Err(anyhow::anyhow!("gRPC stream error: {:?}", e));
                }
            }
        }

        log::warn!("gRPC subscription loop ended");
        Ok(())
    }

    fn process_transaction_update(
        transaction_update: SubscribeUpdateTransaction,
        target_mint: &Pubkey,
        tx: &mpsc::UnboundedSender<TransactionUpdate>,
    ) {
        let _slot = transaction_update.slot;
        let blocktime_us = chrono::Utc::now().timestamp_micros();

        if let Some(transaction_info) = &transaction_update.transaction {
            let signature = if !transaction_info.signature.is_empty() {
                bs58::encode(&transaction_info.signature).into_string()
            } else {
                log::debug!("Transaction update without signature");
                return;
            };

            log::info!("Processing transaction: {}", signature);

            if let Some(meta) = &transaction_info.meta {
                if meta.err.is_some() {
                    log::debug!("Skipping failed transaction: {}", signature);
                    return;
                }

                if let Some(update) = Self::extract_trade_from_meta(
                    meta,
                    &transaction_info.transaction,
                    target_mint,
                    &signature,
                    blocktime_us,
                ) {
                    log::info!("Extracted trade update: {:?}", update);
                    
                    if let Err(e) = tx.send(update) {
                        log::error!("Failed to send transaction update: {}", e);
                    }
                } else {
                    log::debug!("No trade extracted from transaction: {}", signature);
                }
            } else {
                log::debug!("No meta in transaction info: {}", signature);
            }
        } else {
            log::debug!("No transaction info in update");
        }
    }

    fn extract_trade_from_meta(
        meta: &yellowstone_grpc_proto::solana::storage::confirmed_block::TransactionStatusMeta,
        _transaction: &Option<yellowstone_grpc_proto::solana::storage::confirmed_block::Transaction>,
        target_mint: &Pubkey,
        signature: &str,
        blocktime_us: i64,
    ) -> Option<TransactionUpdate> {
        let pre_token_balances = &meta.pre_token_balances;
        let post_token_balances = &meta.post_token_balances;
        let pre_balances = &meta.pre_balances;
        let post_balances = &meta.post_balances;

        let target_mint_str = target_mint.to_string();

        log::debug!("Pre token balances count: {}", pre_token_balances.len());
        log::debug!("Post token balances count: {}", post_token_balances.len());

        let mut target_pre_balance: Option<u64> = None;
        let mut target_post_balance: Option<u64> = None;

        for balance in pre_token_balances {
            log::debug!("Pre balance - mint: {}", balance.mint);
            if balance.mint == target_mint_str {
                if let Some(ui_amt) = &balance.ui_token_amount {
                    if let Ok(amt) = ui_amt.amount.parse::<u64>() {
                        target_pre_balance = Some(amt);
                        log::debug!("Found target pre balance: {}", amt);
                    }
                }
            }
        }

        for balance in post_token_balances {
            log::debug!("Post balance - mint: {}", balance.mint);
            if balance.mint == target_mint_str {
                if let Some(ui_amt) = &balance.ui_token_amount {
                    if let Ok(amt) = ui_amt.amount.parse::<u64>() {
                        target_post_balance = Some(amt);
                        log::debug!("Found target post balance: {}", amt);
                    }
                }
            }
        }

        let mut sol_change: i128 = 0;
        
        for (pre, post) in pre_balances.iter().zip(post_balances.iter()) {
            if pre != post {
                sol_change += *post as i128 - *pre as i128;
            }
        }

        log::debug!("SOL change: {}", sol_change);
        log::debug!("Target pre balance: {:?}", target_pre_balance);
        log::debug!("Target post balance: {:?}", target_post_balance);

        match (target_pre_balance, target_post_balance) {
            (Some(pre), Some(post)) if pre != post => {
                let token_change = post as i128 - pre as i128;
                let is_buy = token_change > 0;
                let token_amount = token_change.unsigned_abs() as u64;
                
                let sol_amount = if sol_change < 0 {
                    (-sol_change) as u64
                } else {
                    sol_change as u64
                };

                if token_amount > 0 && sol_amount > 0 {
                    log::info!(
                        "Detected trade: signature={}, is_buy={}, token_amount={}, sol_amount={}",
                        signature, is_buy, token_amount, sol_amount
                    );
                    
                    Some(TransactionUpdate {
                        mint: target_mint_str,
                        token_amount,
                        sol_amount,
                        is_buy,
                        signature: signature.to_string(),
                        blocktime_us,
                    })
                } else {
                    None
                }
            }
            _ => None,
        }
    }
}
