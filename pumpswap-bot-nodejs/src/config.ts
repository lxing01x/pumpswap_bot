import * as fs from 'fs';
import * as path from 'path';
import { Keypair, PublicKey } from '@solana/web3.js';
import bs58 from 'bs58';

export interface BotConfig {
    grpcUrl: string;
    rpcUrl: string;
    grpcToken?: string;
    privateKey: string;
    targetMint: string;
    buyAmountSol: number;
    holdSeconds: number;
    slippageBps: number;
    maxRetries: number;
    retryDelayMs: number;
    jitoEnabled: boolean;
    jitoUuid?: string;
    jitoRegion: string;
    redisUrl: string;
    maxTradesPerToken: number;
    buyThresholdPct: number;
    buyTimeWindowSec: number;
    buyRecordCount: number;
    sellProfitPct: number;
    sellStopLossPct: number;
}

interface BotConfigRaw {
    grpc_url: string;
    rpc_url: string;
    grpc_token?: string;
    private_key: string;
    target_mint: string;
    buy_amount_sol: number;
    hold_seconds: number;
    slippage_bps: number;
    max_retries?: number;
    retry_delay_ms?: number;
    jito_enabled?: boolean;
    jito_uuid?: string;
    jito_region?: string;
    redis_url?: string;
    max_trades_per_token?: number;
    buy_threshold_pct?: number;
    buy_time_window_sec?: number;
    buy_record_count?: number;
    sell_profit_pct?: number;
    sell_stop_loss_pct?: number;
}

const DEFAULT_CONFIG: Partial<BotConfigRaw> = {
    max_retries: 5,
    retry_delay_ms: 1000,
    jito_enabled: false,
    jito_region: 'Frankfurt',
    redis_url: 'redis://127.0.0.1/',
    max_trades_per_token: 1000,
    buy_threshold_pct: 10.0,
    buy_time_window_sec: 5,
    buy_record_count: 5,
    sell_profit_pct: 10.0,
    sell_stop_loss_pct: 5.0,
};

export class ConfigLoader {
    static fromFile(configPath: string): BotConfig {
        const fullPath = path.resolve(configPath);
        const content = fs.readFileSync(fullPath, 'utf-8');
        const raw: BotConfigRaw = JSON.parse(content);
        
        const merged: BotConfigRaw = { ...DEFAULT_CONFIG, ...raw };
        
        return {
            grpcUrl: merged.grpc_url,
            rpcUrl: merged.rpc_url,
            grpcToken: merged.grpc_token,
            privateKey: merged.private_key,
            targetMint: merged.target_mint,
            buyAmountSol: merged.buy_amount_sol,
            holdSeconds: merged.hold_seconds,
            slippageBps: merged.slippage_bps,
            maxRetries: merged.max_retries!,
            retryDelayMs: merged.retry_delay_ms!,
            jitoEnabled: merged.jito_enabled!,
            jitoUuid: merged.jito_uuid,
            jitoRegion: merged.jito_region!,
            redisUrl: merged.redis_url!,
            maxTradesPerToken: merged.max_trades_per_token!,
            buyThresholdPct: merged.buy_threshold_pct!,
            buyTimeWindowSec: merged.buy_time_window_sec!,
            buyRecordCount: merged.buy_record_count!,
            sellProfitPct: merged.sell_profit_pct!,
            sellStopLossPct: merged.sell_stop_loss_pct!,
        };
    }

    static getKeypair(privateKey: string): Keypair {
        try {
            const secretKey = bs58.decode(privateKey);
            return Keypair.fromSecretKey(secretKey);
        } catch {
            try {
                const secretKey = Uint8Array.from(JSON.parse(privateKey));
                return Keypair.fromSecretKey(secretKey);
            } catch {
                throw new Error('Invalid private key format. Expected base58 string or JSON array.');
            }
        }
    }

    static getPublicKey(privateKey: string): PublicKey {
        return this.getKeypair(privateKey).publicKey;
    }

    static parseMint(mint: string): PublicKey {
        return new PublicKey(mint);
    }

    static buyAmountLamports(config: BotConfig): number {
        return Math.floor(config.buyAmountSol * 1_000_000_000);
    }
}
