import Redis from 'ioredis';

export interface TokenTradeRecord {
    mint: string;
    tokenAmount: number;
    solAmount: number;
    price: number;
    blocktimeUs: number;
    isBuy: boolean;
    signature: string;
}

function calculatePriceFromAmounts(solAmount: number, tokenAmount: number): number {
    if (tokenAmount <= 0) {
        return 0;
    }
    return (solAmount / tokenAmount) / 1000;
}

function createTradeRecord(
    mint: string,
    tokenAmount: number,
    solAmount: number,
    isBuy: boolean,
    signature: string,
    blocktimeUs: number
): TokenTradeRecord {
    const price = calculatePriceFromAmounts(solAmount, tokenAmount);
    
    console.log(
        `TokenTradeRecord: mint=${mint}, tokenAmount=${tokenAmount}, solAmount=${solAmount}, price=${price.toFixed(12)} SOL/token`
    );
    
    return {
        mint,
        tokenAmount,
        solAmount,
        price,
        blocktimeUs,
        isBuy,
        signature,
    };
}

export class RedisStore {
    private redis: Redis;
    private maxTradesPerToken: number;

    constructor(redisUrl: string, maxTradesPerToken: number = 1000) {
        this.redis = new Redis(redisUrl);
        this.maxTradesPerToken = maxTradesPerToken;
    }

    private static signaturesKey(mint: string): string {
        return `sigs:${mint}`;
    }

    private static tradesKey(mint: string): string {
        return `trades:${mint}`;
    }

    async isSignatureExists(mint: string, signature: string): Promise<boolean> {
        const sigsKey = RedisStore.signaturesKey(mint);
        const exists = await this.redis.sismember(sigsKey, signature);
        return exists === 1;
    }

    async storeTrade(mint: string, record: TokenTradeRecord): Promise<void> {
        if (await this.isSignatureExists(mint, record.signature)) {
            console.debug(`Signature ${record.signature} already exists, skipping duplicate`);
            return;
        }

        const key = RedisStore.tradesKey(mint);
        const sigsKey = RedisStore.signaturesKey(mint);
        const serialized = JSON.stringify(record);

        const pipeline = this.redis.pipeline();
        pipeline.lpush(key, serialized);
        pipeline.ltrim(key, 0, this.maxTradesPerToken - 1);
        pipeline.sadd(sigsKey, record.signature);
        
        await pipeline.exec();

        console.debug(
            `Stored trade record for ${mint}: signature=${record.signature}, blocktimeUs=${record.blocktimeUs}`
        );
    }

    async getRecentTrades(mint: string, limit: number): Promise<TokenTradeRecord[]> {
        const key = RedisStore.tradesKey(mint);
        const records: string[] = await this.redis.lrange(key, 0, limit - 1);

        const result: TokenTradeRecord[] = [];
        for (const recordStr of records) {
            try {
                const record: TokenTradeRecord = JSON.parse(recordStr);
                result.push(record);
            } catch (e) {
                console.error('Failed to deserialize trade record:', e);
            }
        }

        result.sort((a, b) => b.blocktimeUs - a.blocktimeUs);
        return result;
    }

    async getTradesInWindow(mint: string, seconds: number): Promise<TokenTradeRecord[]> {
        const nowUs = Date.now() * 1000;
        const cutoffUs = nowUs - (seconds * 1_000_000);

        console.log(
            `getTradesInWindow: mint=${mint}, seconds=${seconds}, nowUs=${nowUs}, cutoffUs=${cutoffUs}`
        );

        const trades = await this.getRecentTrades(mint, this.maxTradesPerToken);
        console.log(`  Total trades in Redis: ${trades.length}`);

        const filtered = trades.filter(t => t.blocktimeUs >= cutoffUs);

        console.log(`  Trades in window: ${filtered.length}`);
        return filtered;
    }

    async getLatestPriceFromTrades(mint: string): Promise<number | null> {
        const trades = await this.getRecentTrades(mint, 1);

        if (trades.length > 0) {
            return trades[0].price;
        }
        return null;
    }

    async calculatePriceChange(mint: string, seconds: number): Promise<number | null> {
        const trades = await this.getTradesInWindow(mint, seconds);

        console.log(
            `calculatePriceChange: mint=${mint}, seconds=${seconds}, tradesInWindow=${trades.length}`
        );
        trades.forEach((trade, i) => {
            console.log(
                `  Trade ${i}: signature=${trade.signature}, blocktimeUs=${trade.blocktimeUs}, price=${trade.price.toFixed(12)}`
            );
        });

        if (trades.length < 2) {
            return null;
        }

        const oldestPrice = trades[trades.length - 1].price;
        const newestPrice = trades[0].price;

        if (oldestPrice === 0) {
            return null;
        }

        const changePct = ((newestPrice - oldestPrice) / oldestPrice) * 100;
        return changePct;
    }

    async calculatePriceChangeFromRecords(mint: string, recordCount: number): Promise<number | null> {
        const trades = await this.getRecentTrades(mint, recordCount);

        console.log(
            `calculatePriceChangeFromRecords: mint=${mint}, recordCount=${recordCount}, totalTrades=${trades.length}`
        );
        trades.forEach((trade, i) => {
            console.log(
                `  Trade ${i}: signature=${trade.signature}, blocktimeUs=${trade.blocktimeUs}, price=${trade.price.toFixed(12)}`
            );
        });

        if (trades.length < 2) {
            return null;
        }

        const oldestPrice = trades[trades.length - 1].price;
        const newestPrice = trades[0].price;

        if (oldestPrice === 0) {
            return null;
        }

        const changePct = ((newestPrice - oldestPrice) / oldestPrice) * 100;
        return changePct;
    }

    async getActiveMints(): Promise<string[]> {
        const keys: string[] = await this.redis.keys('trades:*');

        const mints: string[] = keys
            .map(k => k.replace('trades:', ''))
            .filter(k => k.length > 0);

        return mints;
    }

    async storeTransactionUpdate(update: {
        mint: string;
        tokenAmount: number;
        solAmount: number;
        isBuy: boolean;
        signature: string;
        blocktimeUs: number;
    }): Promise<void> {
        const record = createTradeRecord(
            update.mint,
            update.tokenAmount,
            update.solAmount,
            update.isBuy,
            update.signature,
            update.blocktimeUs
        );

        await this.storeTrade(update.mint, record);

        console.log(
            `Stored trade from gRPC: mint=${update.mint}, signature=${update.signature}, price=${record.price.toFixed(12)} SOL/token`
        );
    }

    async disconnect(): Promise<void> {
        await this.redis.disconnect();
    }
}

export { createTradeRecord, calculatePriceFromAmounts };
