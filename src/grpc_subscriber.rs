use anyhow::{Context, Result};
use solana_sdk::pubkey::Pubkey;
use std::collections::HashMap;
use tokio::sync::mpsc;
use tonic::{metadata::MetadataValue, Request, service::Interceptor};
use yellowstone_grpc_proto::geyser::*;
use yellowstone_grpc_proto::prelude::CommitmentLevel;
use yellowstone_grpc_proto::prelude::geyser_client::GeyserClient;

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
                log::error!("gRPC subscription error: {}", e);
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
        
        let endpoint = tonic::transport::Endpoint::from_shared(grpc_url.clone())
            .context("Failed to create gRPC endpoint")?
            .tcp_keepalive(Some(std::time::Duration::from_secs(30)))
            .keep_alive_while_idle(true)
            .connect_timeout(std::time::Duration::from_secs(10));
        
        let channel = endpoint.connect().await.context("Failed to connect to gRPC")?;

        let token_clone = grpc_token.clone();
        let interceptor = move |mut req: Request<()>| {
            if let Some(ref token) = token_clone {
                let token: MetadataValue<_> = format!("Bearer {}", token).parse().unwrap();
                req.metadata_mut().insert("x-token", token);
            }
            Ok(req)
        };

        let mut client = GeyserClient::with_interceptor(channel, interceptor);

        log::info!("gRPC connected. Subscribing to transactions for mint: {}", target_mint);

        let mut transactions_filter = HashMap::new();
        transactions_filter.insert(
            "target_mint".to_string(),
            SubscribeRequestFilterTransactions {
                vote: Some(false),
                failed: Some(false),
                signature: None,
                account_include: vec![target_mint.to_string()],
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

        let (subscribe_tx, subscribe_rx) = mpsc::unbounded_channel();
        subscribe_tx.send(subscribe_request)
            .map_err(|_| anyhow::anyhow!("Failed to send subscribe request"))?;

        let response = client
            .subscribe(Request::new(tokio_stream::wrappers::UnboundedReceiverStream::new(subscribe_rx)))
            .await
            .context("Failed to subscribe to gRPC stream")?;

        let mut stream = response.into_inner();

        log::info!("gRPC subscription started. Waiting for transactions...");

        while let Some(message) = stream.message().await? {
            if let Some(subscribe_update::UpdateOneof::Transaction(transaction_update)) = message.update_oneof {
                Self::process_transaction_update(transaction_update, &target_mint, &tx);
            }
        }

        log::warn!("gRPC stream ended");
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
                    if let Err(e) = tx.send(update) {
                        log::error!("Failed to send transaction update: {}", e);
                    }
                }
            }
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

        let mut target_pre_balance: Option<u64> = None;
        let mut target_post_balance: Option<u64> = None;

        for balance in pre_token_balances {
            if balance.mint == target_mint_str {
                if let Some(ui_amt) = &balance.ui_token_amount {
                    if let Ok(amt) = ui_amt.amount.parse::<u64>() {
                        target_pre_balance = Some(amt);
                    }
                }
            }
        }

        for balance in post_token_balances {
            if balance.mint == target_mint_str {
                if let Some(ui_amt) = &balance.ui_token_amount {
                    if let Ok(amt) = ui_amt.amount.parse::<u64>() {
                        target_post_balance = Some(amt);
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
