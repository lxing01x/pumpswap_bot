use anyhow::Result;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::sync::Mutex;

use solana_streamer_sdk::streaming::event_parser::{
    common::filter::EventTypeFilter,
    common::EventType,
    protocols::pumpswap::PumpSwapBuyEvent,
    protocols::pumpswap::PumpSwapSellEvent,
    Protocol, UnifiedEvent,
};
use solana_streamer_sdk::streaming::yellowstone_grpc::{AccountFilter, TransactionFilter};
use solana_streamer_sdk::streaming::YellowstoneGrpc;
use solana_streamer_sdk::match_event;

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
}

impl GrpcSubscriber {
    pub fn new(grpc_url: String, grpc_token: Option<String>) -> Self {
        Self {
            grpc_url,
            grpc_token,
        }
    }

    pub async fn subscribe(&self) -> Result<mpsc::UnboundedReceiver<TransactionUpdate>> {
        let (tx, rx) = mpsc::unbounded_channel();
        
        let grpc_url = self.grpc_url.clone();
        let grpc_token = self.grpc_token.clone();
        
        let tx = Arc::new(Mutex::new(tx));
        
        tokio::spawn(async move {
            if let Err(e) = Self::run_subscription(grpc_url, grpc_token, tx).await {
                log::error!("gRPC subscription error: {:?}", e);
            }
        });
        
        Ok(rx)
    }

    async fn run_subscription(
        grpc_url: String,
        grpc_token: Option<String>,
        tx: Arc<Mutex<mpsc::UnboundedSender<TransactionUpdate>>>,
    ) -> Result<()> {
        log::info!("Connecting to gRPC: {}", grpc_url);
        
        match &grpc_token {
            Some(token) => {
                if token.is_empty() {
                    log::warn!("gRPC token is empty!");
                } else {
                    log::info!("gRPC token is configured (length: {})", token.len());
                    log::debug!("Token preview: {}...", &token[..token.len().min(8)]);
                }
            }
            None => {
                log::warn!("gRPC token is not configured!");
                log::warn!("The Solana Yellowstone gRPC service requires a personal token.");
            }
        }
        
        let grpc = match YellowstoneGrpc::new(grpc_url.clone(), grpc_token) {
            Ok(g) => {
                log::info!("Successfully created YellowstoneGrpc client");
                g
            }
            Err(e) => {
                log::error!("Failed to create YellowstoneGrpc client: {:?}", e);
                return Err(anyhow::anyhow!("Failed to create YellowstoneGrpc client: {:?}", e));
            }
        };
        
        let protocols = vec![Protocol::PumpSwap];
        
        let account_include = vec![
            solana_streamer_sdk::streaming::event_parser::protocols::pumpswap::parser::PUMPSWAP_PROGRAM_ID.to_string(),
        ];
        let account_exclude = vec![];
        let account_required = vec![];
        
        log::info!("Account filter - include: {:?}", account_include);
        
        let transaction_filter = TransactionFilter {
            account_include: account_include.clone(),
            account_exclude,
            account_required,
        };
        
        let account_filter = AccountFilter { account: vec![], owner: vec![], filters: vec![] };
        
        let event_type_filter = EventTypeFilter {
            include: vec![EventType::PumpSwapBuy, EventType::PumpSwapSell],
        };
        
        log::info!("Subscribing to PumpSwap events...");
        log::info!("  - Protocols: {:?}", protocols);
        log::info!("  - Event types: PumpSwapBuy, PumpSwapSell");
        log::info!("  - Listening to ALL tokens (WSOL pairs)");
        
        let tx_clone = tx.clone();
        let callback = move |event: Box<dyn UnifiedEvent>| {
            let tx = tx_clone.clone();
            tokio::spawn(async move {
                Self::handle_event(event, tx).await;
            });
        };
        
        if let Err(e) = grpc.subscribe_events_immediate(
            protocols,
            None,
            vec![transaction_filter],
            vec![account_filter],
            Some(event_type_filter),
            None,
            callback,
        )
        .await {
            log::error!("Failed to subscribe to gRPC events: {:?}", e);
            return Err(anyhow::anyhow!("Failed to subscribe to gRPC events: {:?}", e));
        }
        
        log::info!("gRPC subscription started successfully");
        log::info!("Waiting for PumpSwap events...");
        
        loop {
            tokio::time::sleep(tokio::time::Duration::from_secs(60)).await;
        }
    }

    async fn handle_event(
        event: Box<dyn UnifiedEvent>,
        tx: Arc<Mutex<mpsc::UnboundedSender<TransactionUpdate>>>,
    ) {
        match_event!(event, {
            PumpSwapBuyEvent => |e: PumpSwapBuyEvent| {
                log::debug!("Received PumpSwapBuyEvent");
                
                let is_wsol_base = e.base_mint == sol_trade_sdk::constants::WSOL_TOKEN_ACCOUNT;
                let is_wsol_quote = e.quote_mint == sol_trade_sdk::constants::WSOL_TOKEN_ACCOUNT;
                
                if !is_wsol_base && !is_wsol_quote {
                    log::debug!("Event not involving WSOL: base={}, quote={}", e.base_mint, e.quote_mint);
                    return;
                }
                
                let mint = if is_wsol_base {
                    e.quote_mint
                } else {
                    e.base_mint
                };
                
                let (token_amount, sol_amount) = if is_wsol_base {
                    (e.pool_quote_token_reserves, e.pool_base_token_reserves)
                } else {
                    (e.pool_base_token_reserves, e.pool_quote_token_reserves)
                };
                
                let blocktime_us = e.timestamp as i64 * 1_000_000;
                let signature = e.signature().to_string();
                
                log::info!(
                    "PumpSwap BUY event: mint={}, token_amount={}, sol_amount={}, signature={}, timestamp={}, blocktime_us={}",
                    mint, token_amount, sol_amount, signature, e.timestamp, blocktime_us
                );
                
                let update = TransactionUpdate {
                    mint: mint.to_string(),
                    token_amount,
                    sol_amount,
                    is_buy: true,
                    signature,
                    blocktime_us,
                };
                
                tokio::spawn(async move {
                    let tx = tx.lock().await;
                    if let Err(e) = tx.send(update) {
                        log::error!("Failed to send transaction update: {}", e);
                    }
                });
            },
            PumpSwapSellEvent => |e: PumpSwapSellEvent| {
                log::debug!("Received PumpSwapSellEvent");
                
                let is_wsol_base = e.base_mint == sol_trade_sdk::constants::WSOL_TOKEN_ACCOUNT;
                let is_wsol_quote = e.quote_mint == sol_trade_sdk::constants::WSOL_TOKEN_ACCOUNT;
                
                if !is_wsol_base && !is_wsol_quote {
                    log::debug!("Event not involving WSOL: base={}, quote={}", e.base_mint, e.quote_mint);
                    return;
                }
                
                let mint = if is_wsol_base {
                    e.quote_mint
                } else {
                    e.base_mint
                };
                
                let (token_amount, sol_amount) = if is_wsol_base {
                    (e.pool_quote_token_reserves, e.pool_base_token_reserves)
                } else {
                    (e.pool_base_token_reserves, e.pool_quote_token_reserves)
                };
                
                let blocktime_us = e.timestamp as i64 * 1_000_000;
                let signature = e.signature().to_string();
                
                log::info!(
                    "PumpSwap SELL event: mint={}, token_amount={}, sol_amount={}, signature={}, timestamp={}, blocktime_us={}",
                    mint, token_amount, sol_amount, signature, e.timestamp, blocktime_us
                );
                
                let update = TransactionUpdate {
                    mint: mint.to_string(),
                    token_amount,
                    sol_amount,
                    is_buy: false,
                    signature,
                    blocktime_us,
                };
                
                tokio::spawn(async move {
                    let tx = tx.lock().await;
                    if let Err(e) = tx.send(update) {
                        log::error!("Failed to send transaction update: {}", e);
                    }
                });
            }
        });
    }
}
