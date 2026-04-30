import { PublicKey } from '@solana/web3.js';
import { BotConfig, ConfigLoader } from './config';
import { Trader, TradeInfo } from './trader';
import { RedisStore, TokenTradeRecord } from './redisStore';
import { TradeRecorder } from './tradeRecorder';

export interface TokenPosition {
    mint: string;
    buyPrice: number;
    buySolAmount: number;
    tradeInfo: TradeInfo;
}

export class TradingStrategy {
    private config: BotConfig;
    private trader: Trader;
    private positions: Map<string, TokenPosition> = new Map();
    private tradeInfoCache: Map<string, TradeInfo> = new Map();
    private redisStore: RedisStore | null = null;
    private tradeRecorder: TradeRecorder;
    private running: boolean = false;

    private constructor(
        config: BotConfig,
        trader: Trader,
        redisStore: RedisStore | null,
        tradeRecorder: TradeRecorder
    ) {
        this.config = config;
        this.trader = trader;
        this.redisStore = redisStore;
        this.tradeRecorder = tradeRecorder;
    }

    static async create(config: BotConfig): Promise<TradingStrategy> {
        const keypair = ConfigLoader.getKeypair(config.privateKey);
        const trader = await Trader.fromConfig(config, keypair);

        let redisStore: RedisStore | null = null;
        try {
            redisStore = new RedisStore(config.redisUrl, config.maxTradesPerToken);
            console.log('Redis connection established successfully');
        } catch (e) {
            console.warn(`Failed to connect to Redis: ${e}. Trading will continue without Redis storage.`);
        }

        const tradeRecorder = new TradeRecorder();

        return new TradingStrategy(config, trader, redisStore, tradeRecorder);
    }

    async run(): Promise<void> {
        console.log('Starting PumpSwap trading bot...');
        console.log(`Buy amount: ${this.config.buyAmountSol} SOL`);
        console.log(`Slippage: ${this.config.slippageBps} bps`);
        console.log(`Max retries: ${this.config.maxRetries}`);
        console.log(`Retry delay: ${this.config.retryDelayMs} ms`);
        console.log(`Jito enabled: ${this.config.jitoEnabled}`);
        if (this.config.jitoEnabled) {
            console.log(`Jito region: ${this.config.jitoRegion}`);
        }
        console.log(`Buy threshold: ${this.config.buyThresholdPct}% in last ${this.config.buyRecordCount} records`);
        console.log(`Sell profit threshold: ${this.config.sellProfitPct}%`);
        console.log(`Sell stop loss threshold: ${this.config.sellStopLossPct}%`);

        console.log('Listening for PumpSwap transactions...');

        this.running = true;
        await this.startMonitoring();
    }

    private async startMonitoring(): Promise<void> {
        console.log('Starting monitoring mode...');
        console.log('Bot will monitor PumpSwap tokens and execute trades based on configured thresholds.');
        
        await this.monitoringLoop();
    }

    private async monitoringLoop(): Promise<void> {
        console.log('Entering monitoring loop...');
        
        const checkInterval = 1000;
        
        while (this.running) {
            try {
                const activeMints = await this.getActiveMints();
                console.debug(`Active mints with trade data: ${activeMints}`);
                
                for (const mint of activeMints) {
                    const isHolding = this.isHolding(mint) || this.tradeRecorder.isHolding(mint);
                    
                    if (isHolding) {
                        await this.checkSellCondition(mint);
                    } else {
                        await this.checkBuyCondition(mint);
                    }
                }
            } catch (e) {
                console.error('Error in monitoring loop:', e);
            }
            
            await new Promise(resolve => setTimeout(resolve, checkInterval));
        }
    }

    stop(): void {
        this.running = false;
    }

    private async getActiveMints(): Promise<string[]> {
        if (!this.redisStore) {
            return [];
        }
        return this.redisStore.getActiveMints();
    }

    private isHolding(mint: string): boolean {
        return this.positions.has(mint);
    }

    private async checkBuyCondition(mint: string): Promise<void> {
        if (this.isHolding(mint) || this.tradeRecorder.isHolding(mint)) {
            console.debug(`Already holding ${mint}, skipping buy check`);
            return;
        }
        
        const currentPrice = await this.getLatestTradePrice(mint);
        
        if (currentPrice !== null) {
            console.debug(`Checking buy condition for ${mint}: price=${currentPrice.toFixed(12)} SOL/token`);

            const shouldBuy = await this.checkPriceIncrease(mint);
            
            if (shouldBuy) {
                console.log(`Buy condition met for ${mint}! Starting buy operation...`);
                await this.executeBuy(mint);
            }
        }
    }

    private async getLatestTradePrice(mint: string): Promise<number | null> {
        if (!this.redisStore) {
            console.warn('Redis not available, cannot get trade price');
            return null;
        }
        return this.redisStore.getLatestPriceFromTrades(mint);
    }

    private async checkPriceIncrease(mint: string): Promise<boolean> {
        if (!this.redisStore) {
            console.warn('Redis not available, cannot check price increase.');
            return false;
        }

        const priceChange = await this.redisStore.calculatePriceChangeFromRecords(
            mint,
            this.config.buyRecordCount
        );
        
        if (priceChange === null) {
            console.debug(`Not enough trade data for ${mint} to calculate price change. Waiting for more data...`);
            return false;
        }

        console.log(
            `Price change for ${mint} in last ${this.config.buyRecordCount} records: ${priceChange.toFixed(2)}%`
        );
        
        if (priceChange >= this.config.buyThresholdPct) {
            console.log(
                `Price increase ${priceChange.toFixed(2)}% exceeds threshold ${this.config.buyThresholdPct}% - BUY SIGNAL for ${mint}`
            );
            return true;
        }
        
        return false;
    }

    private async checkSellCondition(mint: string): Promise<void> {
        const position = this.positions.get(mint);
        const activePosition = this.tradeRecorder.getActivePosition(mint);
        
        if (!position && !activePosition) {
            return;
        }
        
        const currentPrice = await this.getLatestTradePrice(mint);
        
        if (currentPrice === null) {
            return;
        }

        if (activePosition) {
            this.tradeRecorder.updatePrice(mint, currentPrice);
        }

        const buyPrice = position?.buyPrice ?? activePosition?.buyPrice ?? 0;
        
        const profitPct = buyPrice > 0 
            ? ((currentPrice - buyPrice) / buyPrice) * 100 
            : 0;
        
        console.log(
            `Position ${mint}: current price=${currentPrice.toFixed(12)}, buy_price=${buyPrice.toFixed(12)}, P/L: ${profitPct.toFixed(2)}%`
        );

        const shouldSell = profitPct >= this.config.sellProfitPct 
            ? (console.log(`Should sell ${mint}: Profit ${profitPct.toFixed(2)}% exceeds threshold ${this.config.sellProfitPct}%`), true)
            : profitPct <= -this.config.sellStopLossPct
            ? (console.log(`Should sell ${mint}: Loss ${Math.abs(profitPct).toFixed(2)}% exceeds stop loss threshold ${this.config.sellStopLossPct}%`), true)
            : false;
        
        if (shouldSell) {
            console.log(`Sell condition met for ${mint}! Starting sell operation...`);
            const tradeInfo = position?.tradeInfo ?? this.tradeInfoCache.get(mint);
            if (tradeInfo) {
                await this.executeSell(mint, tradeInfo);
            }
        }
    }

    private async getOrFetchTradeInfo(mint: string): Promise<TradeInfo> {
        const cached = this.tradeInfoCache.get(mint);
        if (cached) {
            return cached;
        }
        
        const mintPubkey = new PublicKey(mint);
        
        console.log(`Fetching TradeInfo for mint: ${mint}`);
        
        const tradeInfo = await this.trader.fetchTradeInfoWithRetry(mintPubkey);
        
        console.log(`Successfully fetched TradeInfo for ${mint}:`);
        console.log(`  Pool: ${tradeInfo.pool.toBase58()}`);
        console.log(`  Base mint: ${tradeInfo.baseMint.toBase58()}`);
        console.log(`  Quote mint: ${tradeInfo.quoteMint.toBase58()}`);
        
        this.tradeInfoCache.set(mint, tradeInfo);
        
        return tradeInfo;
    }

    private async executeBuy(mint: string): Promise<void> {
        if (this.isHolding(mint) || this.tradeRecorder.isHolding(mint)) {
            console.log(`Already holding ${mint}, skipping buy.`);
            return;
        }

        const tradeInfo = await this.getOrFetchTradeInfo(mint);

        const currentPrice = this.trader.calculatePriceFromPool(tradeInfo);
        console.log(`Buying ${mint} at price: ${currentPrice} SOL/token`);
        
        const buyAmountLamports = ConfigLoader.buyAmountLamports(this.config);
        
        try {
            const result = await this.trader.buy(tradeInfo, buyAmountLamports);
            
            if (!result.success) {
                throw new Error(result.error || 'Buy failed');
            }

            const position: TokenPosition = {
                mint,
                buyPrice: result.price,
                buySolAmount: buyAmountLamports,
                tradeInfo,
            };
            
            this.positions.set(mint, position);
            
            const signature = result.signatures[0] || 'unknown';
            this.tradeRecorder.recordBuy(mint, result.price, buyAmountLamports, signature);
            
            console.log(`Buy successful for ${mint}. Now monitoring for sell conditions.`);
        } catch (e) {
            console.error(`Buy failed for ${mint}:`, e);
            throw e;
        }
    }

    private async executeSell(mint: string, tradeInfo: TradeInfo): Promise<void> {
        console.log(`=== Starting sell operation for ${mint} ===`);
        
        try {
            const result = await this.trader.sell(tradeInfo);
            
            if (!result.success) {
                throw new Error(result.error || 'Sell failed');
            }

            this.positions.delete(mint);
            
            const activePosition = this.tradeRecorder.getActivePosition(mint);
            const signature = result.signatures[0] || 'unknown';
            
            let sellSolAmount = 0;
            if (activePosition) {
                const buySolAmount = activePosition.buySolAmount;
                const buyPrice = activePosition.buyPrice;
                const sellPrice = result.price;
                
                if (buyPrice > 0) {
                    const pnlPct = (sellPrice - buyPrice) / buyPrice;
                    sellSolAmount = Math.floor(buySolAmount * (1 + pnlPct));
                }
            }
            
            this.tradeRecorder.recordSell(mint, result.price, sellSolAmount, signature);

            console.log(`=== Sell completed successfully for ${mint} ===`);
        } catch (e) {
            console.error(`Sell failed for ${mint}:`, e);
            throw e;
        }
    }

    async processTransactionUpdate(update: {
        mint: string;
        tokenAmount: number;
        solAmount: number;
        isBuy: boolean;
        signature: string;
        blocktimeUs: number;
    }): Promise<void> {
        if (!this.redisStore) {
            console.warn('Redis not available, cannot store trade from gRPC');
            return;
        }

        await this.redisStore.storeTransactionUpdate(update);
    }

    getConfig(): BotConfig {
        return this.config;
    }

    getTrader(): Trader {
        return this.trader;
    }

    getTradeRecorder(): TradeRecorder {
        return this.tradeRecorder;
    }

    getPositions(): Map<string, TokenPosition> {
        return this.positions;
    }
}
