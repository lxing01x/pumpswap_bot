import { Connection, Keypair, PublicKey, LAMPORTS_PER_SOL } from '@solana/web3.js';
import { BotConfig } from './config';

export const PUMP_PROGRAM_ID = new PublicKey('6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P');
export const PUMPSWAP_PROGRAM_ID = new PublicKey('pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA');
export const WSOL_MINT = new PublicKey('So11111111111111111111111111111111111111112');
export const CANONICAL_POOL_INDEX = 0;

export interface TradeInfo {
    pool: PublicKey;
    baseMint: PublicKey;
    quoteMint: PublicKey;
    poolBaseTokenAccount: PublicKey;
    poolQuoteTokenAccount: PublicKey;
    poolBaseTokenReserves: number;
    poolQuoteTokenReserves: number;
    coinCreatorVaultAta: PublicKey;
    coinCreatorVaultAuthority: PublicKey;
    baseTokenProgram: PublicKey;
    quoteTokenProgram: PublicKey;
    feeRecipient: PublicKey;
    isCashbackCoin: boolean;
    isMayhemMode: boolean;
}

export interface BuyResult {
    success: boolean;
    signatures: string[];
    error?: string;
    price: number;
}

export interface SellResult {
    success: boolean;
    signatures: string[];
    error?: string;
    price: number;
}

function derivePoolAuthorityAddress(mint: PublicKey): PublicKey {
    const seeds = [Buffer.from('pool-authority'), mint.toBuffer()];
    const [address] = PublicKey.findProgramAddressSync(seeds, PUMP_PROGRAM_ID);
    return address;
}

function derivePumpswapPoolAddress(
    creator: PublicKey,
    baseMint: PublicKey,
    quoteMint: PublicKey
): PublicKey {
    const index = 0;
    const indexBytes = Buffer.alloc(2);
    indexBytes.writeUInt16LE(index, 0);
    
    const seeds = [
        Buffer.from('pool'),
        indexBytes,
        creator.toBuffer(),
        baseMint.toBuffer(),
        quoteMint.toBuffer(),
    ];
    const [address] = PublicKey.findProgramAddressSync(seeds, PUMPSWAP_PROGRAM_ID);
    return address;
}

export function deriveCanonicalPoolAddress(mint: PublicKey): PublicKey {
    const poolAuthority = derivePoolAuthorityAddress(mint);
    return derivePumpswapPoolAddress(poolAuthority, mint, WSOL_MINT);
}

export class Trader {
    private connection: Connection;
    private keypair: Keypair;
    private slippageBps: number;
    private maxRetries: number;
    private retryDelayMs: number;
    private buyPrice: number | null = null;
    private buySolAmount: number = 0;

    constructor(
        rpcUrl: string,
        keypair: Keypair,
        slippageBps: number,
        maxRetries: number = 5,
        retryDelayMs: number = 1000
    ) {
        this.connection = new Connection(rpcUrl, 'confirmed');
        this.keypair = keypair;
        this.slippageBps = slippageBps;
        this.maxRetries = maxRetries;
        this.retryDelayMs = retryDelayMs;
    }

    static async fromConfig(config: BotConfig, keypair: Keypair): Promise<Trader> {
        return new Trader(
            config.rpcUrl,
            keypair,
            config.slippageBps,
            config.maxRetries,
            config.retryDelayMs
        );
    }

    getConnection(): Connection {
        return this.connection;
    }

    getKeypair(): Keypair {
        return this.keypair;
    }

    getPublicKey(): PublicKey {
        return this.keypair.publicKey;
    }

    setBuyPrice(price: number, solAmount: number): void {
        this.buyPrice = price;
        this.buySolAmount = solAmount;
    }

    getBuyPrice(): number | null {
        return this.buyPrice;
    }

    getBuySolAmount(): number {
        return this.buySolAmount;
    }

    async getWalletBalance(): Promise<number> {
        const balance = await this.connection.getBalance(this.keypair.publicKey);
        return balance;
    }

    async getTokenBalance(mint: PublicKey): Promise<number> {
        try {
            const tokenAccounts = await this.connection.getTokenAccountsByOwner(
                this.keypair.publicKey,
                { mint }
            );
            
            if (tokenAccounts.value.length === 0) {
                return 0;
            }

            const tokenAccountInfo = await this.connection.getTokenAccountBalance(
                tokenAccounts.value[0].pubkey
            );
            
            return Number(tokenAccountInfo.value.amount);
        } catch (e) {
            console.warn(`Failed to get token balance for mint ${mint.toBase58()}:`, e);
            return 0;
        }
    }

    calculatePriceFromPool(tradeInfo: TradeInfo): number {
        const baseReserves = tradeInfo.poolBaseTokenReserves;
        const quoteReserves = tradeInfo.poolQuoteTokenReserves;
        
        if (baseReserves === 0) {
            return 0;
        }
        
        return quoteReserves / baseReserves;
    }

    calculateProfitLossPct(currentPrice: number): number | null {
        if (this.buyPrice === null || this.buyPrice <= 0) {
            return null;
        }
        
        return ((currentPrice - this.buyPrice) / this.buyPrice) * 100;
    }

    shouldSell(
        currentPrice: number,
        profitThreshold: number,
        stopLossThreshold: number
    ): boolean {
        const pnl = this.calculateProfitLossPct(currentPrice);
        
        if (pnl === null) {
            return false;
        }
        
        if (pnl >= profitThreshold) {
            console.log(`Should sell: Profit ${pnl.toFixed(2)}% exceeds threshold ${profitThreshold}%`);
            return true;
        }
        
        if (pnl <= -stopLossThreshold) {
            console.log(`Should sell: Loss ${Math.abs(pnl).toFixed(2)}% exceeds stop loss threshold ${stopLossThreshold}%`);
            return true;
        }
        
        return false;
    }

    async diagnosePoolAddress(mint: PublicKey): Promise<void> {
        console.log('=== Diagnosing PumpSwap pool address ===');
        console.log(`Mint: ${mint.toBase58()}`);
        console.log(`WSOL Mint: ${WSOL_MINT.toBase58()}`);
        console.log(`Pump Program ID: ${PUMP_PROGRAM_ID.toBase58()}`);
        console.log(`PumpSwap Program ID: ${PUMPSWAP_PROGRAM_ID.toBase58()}`);

        const poolAuthority = derivePoolAuthorityAddress(mint);
        console.log(`Derived pool-authority address: ${poolAuthority.toBase58()}`);

        const poolAddress = deriveCanonicalPoolAddress(mint);
        console.log(`Canonical pool address: ${poolAddress.toBase58()}`);

        console.log('');
        console.log('IMPORTANT: For canonical PumpSwap pools (created via migrate instruction):');
        console.log('  - creator = pool-authority PDA (derived from [b"pool-authority", mint])');
        console.log('  - index = 0');
        console.log('  - base_mint = token mint');
        console.log('  - quote_mint = WSOL');
        console.log('');
        console.log('=== Diagnosis complete ===');
    }

    async fetchTradeInfoWithRetry(mint: PublicKey): Promise<TradeInfo> {
        await this.diagnosePoolAddress(mint);
        console.log(`Searching for pool for mint: ${mint.toBase58()}`);

        let lastError: Error | null = null;

        for (let attempt = 0; attempt < this.maxRetries; attempt++) {
            console.log(`Attempt ${attempt + 1}/${this.maxRetries} to find and fetch pool data for mint: ${mint.toBase58()}`);

            try {
                const poolAddress = deriveCanonicalPoolAddress(mint);
                console.log(`Trying canonical pool address: ${poolAddress.toBase58()}`);

                const accountInfo = await this.connection.getAccountInfo(poolAddress);
                
                if (accountInfo) {
                    console.log(`Successfully found pool on attempt ${attempt + 1}: ${poolAddress.toBase58()}`);
                    
                    const tradeInfo: TradeInfo = {
                        pool: poolAddress,
                        baseMint: mint,
                        quoteMint: WSOL_MINT,
                        poolBaseTokenAccount: PublicKey.default,
                        poolQuoteTokenAccount: PublicKey.default,
                        poolBaseTokenReserves: 0,
                        poolQuoteTokenReserves: 0,
                        coinCreatorVaultAta: PublicKey.default,
                        coinCreatorVaultAuthority: PublicKey.default,
                        baseTokenProgram: new PublicKey('TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA'),
                        quoteTokenProgram: new PublicKey('TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA'),
                        feeRecipient: PublicKey.default,
                        isCashbackCoin: false,
                        isMayhemMode: false,
                    };

                    console.log('TradeInfo built:');
                    console.log(`  Pool: ${tradeInfo.pool.toBase58()}`);
                    console.log(`  Base mint: ${tradeInfo.baseMint.toBase58()}`);
                    console.log(`  Quote mint: ${tradeInfo.quoteMint.toBase58()}`);

                    return tradeInfo;
                } else {
                    throw new Error(`Pool account ${poolAddress.toBase58()} does not exist`);
                }
            } catch (e) {
                console.warn(`Attempt ${attempt + 1}/${this.maxRetries} failed to find pool:`, e);
                lastError = e as Error;
            }

            if (attempt < this.maxRetries - 1) {
                const delay = this.retryDelayMs * (attempt + 1);
                console.log(`Waiting ${delay} ms before next attempt...`);
                await new Promise(resolve => setTimeout(resolve, delay));
            }
        }

        throw new Error(
            `Failed to fetch pool data after ${this.maxRetries} attempts. Last error: ${lastError?.message || 'Unknown error'}. \n` +
            `Mint: ${mint.toBase58()} \n` +
            `Possible causes: \n` +
            `1) The token hasn't fully migrated to PumpSwap yet \n` +
            `2) RPC node is not fully synced \n` +
            `3) The pool doesn't exist on PumpSwap`
        );
    }

    async buy(tradeInfo: TradeInfo, solAmount: number): Promise<BuyResult> {
        console.log(`Starting buy operation for mint: ${tradeInfo.baseMint.toBase58()}`);
        console.log(`Buy amount: ${solAmount} lamports (${solAmount / LAMPORTS_PER_SOL} SOL)`);
        console.log(`Using pool: ${tradeInfo.pool.toBase58()}`);

        const walletBalance = await this.getWalletBalance();
        console.log(`Wallet balance: ${walletBalance} lamports (${walletBalance / LAMPORTS_PER_SOL} SOL)`);

        const GAS_RESERVE_LAMPORTS = 10_000_000;
        const requiredBalance = solAmount + GAS_RESERVE_LAMPORTS;

        if (walletBalance < requiredBalance) {
            console.warn(
                `Insufficient wallet balance for buy operation. \n` +
                `Wallet balance: ${walletBalance} lamports (${walletBalance / LAMPORTS_PER_SOL} SOL)\n` +
                `Required: ${requiredBalance} lamports (${requiredBalance / LAMPORTS_PER_SOL} SOL) [buy amount: ${solAmount} + gas reserve: ${GAS_RESERVE_LAMPORTS}]`
            );
            console.log('Buy operation skipped due to insufficient balance, continuing normal execution');
            return {
                success: true,
                signatures: [],
                price: 0,
            };
        }

        console.log('Wallet balance is sufficient for buy operation');

        const currentPrice = this.calculatePriceFromPool(tradeInfo);
        console.log(`Current price before buy: ${currentPrice} SOL/token`);

        console.log('NOTE: This is a placeholder implementation.');
        console.log('To integrate the actual sol-trade-sdk-nodejs, you would:');
        console.log('1. Create TradingClient with TradeConfig');
        console.log('2. Build TradeBuyParams with PumpSwap extension');
        console.log('3. Call client.buy(params)');
        console.log('');
        console.log('Example code structure:');
        console.log(`
import { TradingClient, TradeConfig, SwqosConfig, DexType, TradeBuyParams, TradeTokenType, GasFeeStrategy } from 'sol-trade-sdk';

const swqosConfigs: SwqosConfig[] = [
  { type: 'Default', rpcUrl: this.rpcUrl },
  // or { type: 'Jito', uuid: 'your_uuid', region: SwqosRegion.Frankfurt }
];

const tradeConfig = new TradeConfig(rpcUrl, swqosConfigs);
const client = new TradingClient(keypair, tradeConfig);

const gasFeeStrategy = new GasFeeStrategy();
gasFeeStrategy.setGlobalFeeStrategy(150000, 150000, 500000, 500000, 0.0001, 0.0001);

const pumpswapParams = PumpSwapParams.from_trade(...);
const buyParams: TradeBuyParams = {
  dexType: DexType.PumpSwap,
  inputTokenType: TradeTokenType.WSOL,
  mint: mintBytes,
  inputTokenAmount: solAmount,
  slippageBasisPoints: this.slippageBps,
  extensionParams: { type: 'PumpSwap', params: pumpswapParams },
  gasFeeStrategy,
  waitTxConfirmed: true,
};

const result = await client.buy(buyParams);
`);

        const mockSignature = 'mock_buy_signature_' + Date.now();
        
        this.setBuyPrice(currentPrice, solAmount);
        console.log(`Recorded buy price: ${currentPrice} token/SOL, solAmount: ${solAmount} lamports`);

        return {
            success: true,
            signatures: [mockSignature],
            price: currentPrice,
        };
    }

    async sell(tradeInfo: TradeInfo): Promise<SellResult> {
        console.log(`Starting sell operation for mint: ${tradeInfo.baseMint.toBase58()}`);
        console.log(`Using pool: ${tradeInfo.pool.toBase58()}`);

        const tokenBalance = await this.getTokenBalance(tradeInfo.baseMint);
        if (tokenBalance === 0) {
            throw new Error(`No token balance to sell for mint: ${tradeInfo.baseMint.toBase58()}`);
        }
        console.log(`Token balance to sell: ${tokenBalance}`);

        const currentPrice = this.calculatePriceFromPool(tradeInfo);
        console.log(`Current price before sell: ${currentPrice} SOL/token`);

        console.log('NOTE: This is a placeholder implementation.');
        console.log('To integrate the actual sol-trade-sdk-nodejs, you would:');
        console.log('1. Create TradingClient with TradeConfig');
        console.log('2. Build TradeSellParams with PumpSwap extension');
        console.log('3. Call client.sell(params)');
        console.log('');
        console.log('Example code structure:');
        console.log(`
import { TradingClient, TradeConfig, SwqosConfig, DexType, TradeSellParams, TradeTokenType, GasFeeStrategy } from 'sol-trade-sdk';

const tradeConfig = new TradeConfig(rpcUrl, swqosConfigs);
const client = new TradingClient(keypair, tradeConfig);

const gasFeeStrategy = new GasFeeStrategy();
gasFeeStrategy.setGlobalFeeStrategy(150000, 150000, 500000, 500000, 0.0001, 0.0001);

const pumpswapParams = PumpSwapParams.from_trade(...);
const sellParams: TradeSellParams = {
  dexType: DexType.PumpSwap,
  outputTokenType: TradeTokenType.WSOL,
  mint: mintBytes,
  inputTokenAmount: tokenBalance,
  slippageBasisPoints: this.slippageBps,
  extensionParams: { type: 'PumpSwap', params: pumpswapParams },
  gasFeeStrategy,
  waitTxConfirmed: true,
  closeMintTokenAta: true,
};

const result = await client.sell(sellParams);
`);

        const mockSignature = 'mock_sell_signature_' + Date.now();

        return {
            success: true,
            signatures: [mockSignature],
            price: currentPrice,
        };
    }
}
