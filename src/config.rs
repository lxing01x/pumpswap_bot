use anyhow::{Context, Result};
use serde::Deserialize;
use solana_keypair::Keypair;
use solana_keypair::Signer;
use solana_sdk::{
    pubkey::Pubkey,
};
use std::{fs, str::FromStr};

#[derive(Debug, Deserialize, Clone)]
pub struct BotConfig {
    pub grpc_url: String,
    pub rpc_url: String,
    pub grpc_token: Option<String>,
    pub private_key: String,
    pub target_mint: String,
    pub buy_amount_sol: f64,
    pub hold_seconds: u64,
    pub slippage_bps: u64,
    #[serde(default = "default_max_retries")]
    pub max_retries: u32,
    #[serde(default = "default_retry_delay_ms")]
    pub retry_delay_ms: u64,
}

fn default_max_retries() -> u32 {
    5
}

fn default_retry_delay_ms() -> u64 {
    1000
}

impl BotConfig {
    pub fn from_file(path: &str) -> Result<Self> {
        let config_str = fs::read_to_string(path)
            .with_context(|| format!("Failed to read config file: {}", path))?;
        let config: BotConfig = serde_json::from_str(&config_str)
            .with_context(|| "Failed to parse config JSON")?;
        Ok(config)
    }

    pub fn get_keypair(&self) -> Result<Keypair> {
        Keypair::try_from_base58_string(&self.private_key)
            .map_err(|e| anyhow::anyhow!("Invalid private key: {}", e))
    }

    pub fn get_pubkey(&self) -> Result<Pubkey> {
        let kp = self.get_keypair()?;
        let pubkey_bytes = kp.pubkey().to_bytes();
        Ok(Pubkey::new_from_array(pubkey_bytes))
    }

    pub fn get_target_mint(&self) -> Result<Pubkey> {
        Pubkey::from_str(&self.target_mint)
            .map_err(|e| anyhow::anyhow!("Invalid target mint address: {}", e))
    }

    pub fn buy_amount_lamports(&self) -> u64 {
        (self.buy_amount_sol * 1_000_000_000.0) as u64
    }
}
