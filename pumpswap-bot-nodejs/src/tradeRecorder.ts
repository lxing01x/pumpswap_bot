import * as fs from 'fs';
import * as path from 'path';

export interface CompleteTradeRecord {
    id: string;
    mint: string;
    buyPrice: number;
    buySolAmount: number;
    buyTimestamp: number;
    buySignature: string;
    sellPrice: number | null;
    sellSolAmount: number | null;
    sellTimestamp: number | null;
    sellSignature: string | null;
    highestPrice: number;
    lowestPrice: number;
    holdTimeSeconds: number | null;
    profitLossPercent: number | null;
    profitLossSol: number | null;
    status: 'open' | 'closed';
}

export interface ActivePosition {
    mint: string;
    buyPrice: number;
    buySolAmount: number;
    buyTimestamp: number;
    buySignature: string;
    highestPrice: number;
    lowestPrice: number;
}

export class TradeRecorder {
    private recordsDir: string;
    private activePositions: Map<string, ActivePosition> = new Map();

    constructor(recordsDir?: string) {
        this.recordsDir = recordsDir || path.join(process.cwd(), '.trade_records');
        this.ensureDirectory();
        this.loadActivePositions();
    }

    private ensureDirectory(): void {
        if (!fs.existsSync(this.recordsDir)) {
            fs.mkdirSync(this.recordsDir, { recursive: true });
        }
    }

    private getActivePositionsPath(): string {
        return path.join(this.recordsDir, 'active_positions.json');
    }

    private getClosedTradesPath(): string {
        return path.join(this.recordsDir, 'closed_trades.json');
    }

    private getDailyTradesPath(): string {
        const today = new Date().toISOString().split('T')[0];
        return path.join(this.recordsDir, `trades_${today}.json`);
    }

    private loadActivePositions(): void {
        const activePath = this.getActivePositionsPath();
        if (fs.existsSync(activePath)) {
            try {
                const content = fs.readFileSync(activePath, 'utf-8');
                const positions: ActivePosition[] = JSON.parse(content);
                this.activePositions.clear();
                positions.forEach(pos => {
                    this.activePositions.set(pos.mint, pos);
                });
            } catch (e) {
                console.error('Failed to load active positions:', e);
            }
        }
    }

    private saveActivePositions(): void {
        const positions = Array.from(this.activePositions.values());
        fs.writeFileSync(this.getActivePositionsPath(), JSON.stringify(positions, null, 2));
    }

    private appendClosedTrade(record: CompleteTradeRecord): void {
        const closedPath = this.getClosedTradesPath();
        let trades: CompleteTradeRecord[] = [];
        
        if (fs.existsSync(closedPath)) {
            try {
                const content = fs.readFileSync(closedPath, 'utf-8');
                trades = JSON.parse(content);
            } catch (e) {
                console.error('Failed to load closed trades:', e);
            }
        }
        
        trades.push(record);
        fs.writeFileSync(closedPath, JSON.stringify(trades, null, 2));

        const dailyPath = this.getDailyTradesPath();
        let dailyTrades: CompleteTradeRecord[] = [];
        if (fs.existsSync(dailyPath)) {
            try {
                const content = fs.readFileSync(dailyPath, 'utf-8');
                dailyTrades = JSON.parse(content);
            } catch (e) {
                console.error('Failed to load daily trades:', e);
            }
        }
        dailyTrades.push(record);
        fs.writeFileSync(dailyPath, JSON.stringify(dailyTrades, null, 2));
    }

    recordBuy(
        mint: string,
        buyPrice: number,
        buySolAmount: number,
        signature: string
    ): void {
        const now = Date.now();
        
        const position: ActivePosition = {
            mint,
            buyPrice,
            buySolAmount,
            buyTimestamp: now,
            buySignature: signature,
            highestPrice: buyPrice,
            lowestPrice: buyPrice,
        };
        
        this.activePositions.set(mint, position);
        this.saveActivePositions();
        
        console.log(`[TradeRecorder] Recorded BUY for ${mint}`);
        console.log(`  - Price: ${buyPrice} SOL/token`);
        console.log(`  - Amount: ${buySolAmount} lamports`);
        console.log(`  - Signature: ${signature}`);
    }

    updatePrice(mint: string, currentPrice: number): void {
        const position = this.activePositions.get(mint);
        if (!position) {
            return;
        }
        
        if (currentPrice > position.highestPrice) {
            position.highestPrice = currentPrice;
        }
        if (currentPrice < position.lowestPrice) {
            position.lowestPrice = currentPrice;
        }
        
        this.saveActivePositions();
    }

    recordSell(
        mint: string,
        sellPrice: number,
        sellSolAmount: number,
        signature: string
    ): CompleteTradeRecord | null {
        const position = this.activePositions.get(mint);
        if (!position) {
            console.warn(`[TradeRecorder] No active position found for ${mint}`);
            return null;
        }
        
        const now = Date.now();
        const holdTimeSeconds = Math.floor((now - position.buyTimestamp) / 1000);
        
        let profitLossPercent: number | null = null;
        let profitLossSol: number | null = null;
        
        if (position.buyPrice > 0) {
            profitLossPercent = ((sellPrice - position.buyPrice) / position.buyPrice) * 100;
        }
        
        if (position.buySolAmount > 0 && sellSolAmount > 0) {
            profitLossSol = sellSolAmount - position.buySolAmount;
        }
        
        const record: CompleteTradeRecord = {
            id: `${mint}_${position.buyTimestamp}`,
            mint,
            buyPrice: position.buyPrice,
            buySolAmount: position.buySolAmount,
            buyTimestamp: position.buyTimestamp,
            buySignature: position.buySignature,
            sellPrice,
            sellSolAmount,
            sellTimestamp: now,
            sellSignature: signature,
            highestPrice: position.highestPrice,
            lowestPrice: position.lowestPrice,
            holdTimeSeconds,
            profitLossPercent,
            profitLossSol,
            status: 'closed',
        };
        
        this.activePositions.delete(mint);
        this.saveActivePositions();
        
        this.appendClosedTrade(record);
        
        console.log(`[TradeRecorder] Recorded SELL for ${mint}`);
        console.log(`  - Buy Price: ${position.buyPrice} SOL/token`);
        console.log(`  - Sell Price: ${sellPrice} SOL/token`);
        console.log(`  - Highest: ${position.highestPrice} SOL/token`);
        console.log(`  - Lowest: ${position.lowestPrice} SOL/token`);
        console.log(`  - Hold Time: ${holdTimeSeconds} seconds`);
        console.log(`  - P/L: ${profitLossPercent?.toFixed(2)}%`);
        console.log(`  - P/L SOL: ${profitLossSol ? (profitLossSol / 1e9).toFixed(9) : 'N/A'} SOL`);
        console.log(`  - Signature: ${signature}`);
        
        return record;
    }

    getActivePosition(mint: string): ActivePosition | undefined {
        return this.activePositions.get(mint);
    }

    getAllActivePositions(): ActivePosition[] {
        return Array.from(this.activePositions.values());
    }

    isHolding(mint: string): boolean {
        return this.activePositions.has(mint);
    }

    getClosedTrades(): CompleteTradeRecord[] {
        const closedPath = this.getClosedTradesPath();
        if (!fs.existsSync(closedPath)) {
            return [];
        }
        try {
            const content = fs.readFileSync(closedPath, 'utf-8');
            return JSON.parse(content);
        } catch (e) {
            console.error('Failed to load closed trades:', e);
            return [];
        }
    }

    getStatistics(): {
        totalTrades: number;
        winningTrades: number;
        losingTrades: number;
        totalProfitLossSol: number;
        averageHoldTimeSeconds: number;
        winRate: number;
    } {
        const trades = this.getClosedTrades();
        
        let winningTrades = 0;
        let losingTrades = 0;
        let totalProfitLossSol = 0;
        let totalHoldTime = 0;
        
        trades.forEach(trade => {
            if (trade.profitLossSol !== null) {
                totalProfitLossSol += trade.profitLossSol;
                if (trade.profitLossSol > 0) {
                    winningTrades++;
                } else if (trade.profitLossSol < 0) {
                    losingTrades++;
                }
            }
            if (trade.holdTimeSeconds !== null) {
                totalHoldTime += trade.holdTimeSeconds;
            }
        });
        
        return {
            totalTrades: trades.length,
            winningTrades,
            losingTrades,
            totalProfitLossSol,
            averageHoldTimeSeconds: trades.length > 0 ? totalHoldTime / trades.length : 0,
            winRate: trades.length > 0 ? (winningTrades / trades.length) * 100 : 0,
        };
    }
}
