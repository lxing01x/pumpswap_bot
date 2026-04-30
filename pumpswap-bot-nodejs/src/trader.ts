import { Connection, Keypair, PublicKey, LAMPORTS_PER_SOL } from '@solana/web3.js';
import { BotConfig } from './config';
import {
    TradeConfigBuilder,
    SwqosConfig,
    SwqosType,
    SwqosRegion,
    DexType,
    TradeTokenType,
    TradeBuyParams,
    TradeSellParams,
    findByMint,
    getPoolV2PDA,
    getCanonicalPoolPDA,
    getTokenBalances,
    PUMPSWAP_PROGRAM,
    WSOL_TOKEN_ACCOUNT,
    TOKEN_PROGRAM,
    TradingClient,
    TradeResult,
    GasFeeStrategyConfig,
    PumpSwapParams as PumpSwapParamsInterface,
    DexParamEnum,
} from 'sol-trade-sdk';

export const PUMP_PROGRAM_ID = new PublicKey('6EF8rrecthR5Dkzon8Nwu78hRvfCKubJ14M5uBEwF6P');
export const PUMPSWAP_PROGRAM_ID = PUMPSWAP_PROGRAM;
export const WSOL_MINT = WSOL_TOKEN_ACCOUNT;
export const CANONICAL_POOL_INDEX = 0;

export interface TradeInfo {
    pool: PublicKey;
    baseMint: PublicKey;
    quoteMint: PublicKey;
    poolBaseTokenAccount: PublicKey;
    poolQuoteTokenAccount: PublicKey;
    poolBaseTokenReserves: bigint;
    poolQuoteTokenReserves: bigint;
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

function parseJitoRegion(region: string): SwqosRegion {
    switch (region.toLowerCase()) {
        case 'frankfurt': return SwqosRegion.Frankfurt;
        case 'newyork': return SwqosRegion.NewYork;
        case 'tokyo': return SwqosRegion.Tokyo;
        case 'amsterdam': return SwqosRegion.Amsterdam;
        case 'singapore': return SwqosRegion.Singapore;
        case 'slc': return SwqosRegion.SLC;
        case 'london': return SwqosRegion.London;
        case 'losangeles': return SwqosRegion.LosAngeles;
        default: return SwqosRegion.Frankfurt;
    }
}

function convertPoolToTradeInfo(
    poolAddress: PublicKey,
    pool: {
        poolBump: number;
        index: number;
        creator: PublicKey;
        baseMint: PublicKey;
        quoteMint: PublicKey;
        lpMint: PublicKey;
        poolBaseTokenAccount: PublicKey;
        poolQuoteTokenAccount: PublicKey;
        lpSupply: bigint;
        coinCreator: PublicKey;
        isMayhemMode: boolean;
        isCashbackCoin: boolean;
    },
    baseReserves: bigint,
    quoteReserves: bigint
): TradeInfo {
    return {
        pool: poolAddress,
        baseMint: pool.baseMint,
        quoteMint: pool.quoteMint,
        poolBaseTokenAccount: pool.poolBaseTokenAccount,
        poolQuoteTokenAccount: pool.poolQuoteTokenAccount,
        poolBaseTokenReserves: baseReserves,
        poolQuoteTokenReserves: quoteReserves,
        coinCreatorVaultAta: PublicKey.default,
        coinCreatorVaultAuthority: pool.coinCreator,
        baseTokenProgram: TOKEN_PROGRAM,
        quoteTokenProgram: TOKEN_PROGRAM,
        feeRecipient: PublicKey.default,
        isCashbackCoin: pool.isCashbackCoin,
        isMayhemMode: pool.isMayhemMode,
    };
}

function createGasFeeStrategyConfig(): GasFeeStrategyConfig {
    return {
        buyPriorityFee: 150000,
        sellPriorityFee: 150000,
        buyComputeUnits: 500000,
        sellComputeUnits: 500000,
        buyTipLamports: 100000,
        sellTipLamports: 100000,
    };
}

function tradeInfoToPumpSwapParams(tradeInfo: TradeInfo): PumpSwapParamsInterface {
    return {
        pool: tradeInfo.pool,
        baseMint: tradeInfo.baseMint,
        quoteMint: tradeInfo.quoteMint,
        poolBaseTokenAccount: tradeInfo.poolBaseTokenAccount,
        poolQuoteTokenAccount: tradeInfo.poolQuoteTokenAccount,
        poolBaseTokenReserves: Number(tradeInfo.poolBaseTokenReserves),
        poolQuoteTokenReserves: Number(tradeInfo.poolQuoteTokenReserves),
        coinCreatorVaultAta: tradeInfo.coinCreatorVaultAta,
        coinCreatorVaultAuthority: tradeInfo.coinCreatorVaultAuthority,
        baseTokenProgram: tradeInfo.baseTokenProgram,
        quoteTokenProgram: tradeInfo.quoteTokenProgram,
        isMayhemMode: tradeInfo.isMayhemMode,
        isCashbackCoin: tradeInfo.isCashbackCoin,
    };
}

function createDexParamEnum(tradeInfo: TradeInfo): DexParamEnum {
    return {
        type: 'PumpSwap',
        params: tradeInfoToPumpSwapParams(tradeInfo),
    };
}

interface RpcAdapter {
    getAccountInfo: (pubkey: PublicKey) => Promise<{ value?: { data: Buffer } | undefined }>;
    getProgramAccounts?: (programId: PublicKey, config?: unknown) => Promise<Array<{
        pubkey: PublicKey;
        account: { data: Buffer };
    }>>;
    getTokenAccountBalance: (pubkey: PublicKey) => Promise<{ value?: { amount: string } | undefined }>;
}

function createRpcAdapter(connection: Connection): RpcAdapter {
    return {
        getAccountInfo: async (pubkey: PublicKey) => {
            const info = await connection.getAccountInfo(pubkey);
            return { value: info || undefined };
        },
        getProgramAccounts: async (programId: PublicKey, config?: unknown) => {
            const accounts = await connection.getProgramAccounts(programId, config as any);
            if (Array.isArray(accounts)) {
                return accounts.map((a: { pubkey: PublicKey; account: { data: Buffer } }) => ({
                    pubkey: a.pubkey,
                    account: { data: a.account.data },
                }));
            }
            return [];
        },
        getTokenAccountBalance: async (pubkey: PublicKey) => {
            const balance = await connection.getTokenAccountBalance(pubkey);
            return { value: balance.value };
        },
    };
}

function tradeErrorToString(error: unknown): string {
    if (error instanceof Error) {
        return error.message;
    }
    return String(error);
}

export class Trader {
    private connection: Connection;
    private keypair: Keypair;
    private slippageBps: number;
    private maxRetries: number;
    private retryDelayMs: number;
    private buyPrice: number | null = null;
    private buySolAmount: number = 0;
    private client: TradingClient;
    private gasFeeConfig: GasFeeStrategyConfig;
    private rpcAdapter: RpcAdapter;

    constructor(
        rpcUrl: string,
        keypair: Keypair,
        slippageBps: number,
        maxRetries: number = 5,
        retryDelayMs: number = 1000,
        jitoEnabled: boolean = false,
        jitoUuid?: string,
        jitoRegion: string = 'Frankfurt'
    ) {
        this.connection = new Connection(rpcUrl, 'confirmed');
        this.keypair = keypair;
        this.slippageBps = slippageBps;
        this.maxRetries = maxRetries;
        this.retryDelayMs = retryDelayMs;
        this.rpcAdapter = createRpcAdapter(this.connection);

        const swqosConfigs: SwqosConfig[] = jitoEnabled 
            ? [
                {
                    type: SwqosType.Jito,
                    region: parseJitoRegion(jitoRegion),
                    apiKey: jitoUuid || '',
                }
            ]
            : [
                {
                    type: SwqosType.Default,
                    region: SwqosRegion.Default,
                    apiKey: '',
                    customUrl: rpcUrl,
                }
            ];

        const tradeConfig = TradeConfigBuilder.create(rpcUrl)
            .swqosConfigs(swqosConfigs)
            .build();

        this.client = new TradingClient(keypair, tradeConfig);
        this.gasFeeConfig = createGasFeeStrategyConfig();
    }

    static async fromConfig(config: BotConfig, keypair: Keypair): Promise<Trader> {
        return new Trader(
            config.rpcUrl,
            keypair,
            config.slippageBps,
            config.maxRetries,
            config.retryDelayMs,
            config.jitoEnabled,
            config.jitoUuid,
            config.jitoRegion
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

    getClient(): TradingClient {
        return this.client;
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

            const accountInfo = await this.connection.getTokenAccountBalance(
                tokenAccounts.value[0].pubkey
            );
            
            return Number(accountInfo.value.amount);
        } catch (e) {
            console.warn(`Failed to get token balance for mint ${mint.toBase58()}:`, e);
            return 0;
        }
    }

    calculatePriceFromPool(tradeInfo: TradeInfo): number {
        const baseReserves = Number(tradeInfo.poolBaseTokenReserves);
        const quoteReserves = Number(tradeInfo.poolQuoteTokenReserves);
        
        if (baseReserves === 0) {
            return 0;
        }
        
        const price = (quoteReserves / baseReserves) / 1000;
        return price;
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
        console.log(`WSOL Mint: ${WSOL_TOKEN_ACCOUNT.toBase58()}`);
        console.log(`Pump Program ID: ${PUMP_PROGRAM_ID.toBase58()}`);
        console.log(`PumpSwap Program ID: ${PUMPSWAP_PROGRAM.toBase58()}`);

        const poolV2 = getPoolV2PDA(mint);
        console.log(`Pool v2 PDA: ${poolV2.toBase58()}`);

        const canonicalPool = getCanonicalPoolPDA(mint);
        console.log(`Canonical pool PDA: ${canonicalPool.toBase58()}`);

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
                const result = await findByMint(this.rpcAdapter, mint);
                
                if (result) {
                    console.log(`Successfully found pool on attempt ${attempt + 1}: ${result.poolAddress.toBase58()}`);
                    
                    const balances = await getTokenBalances(this.rpcAdapter, result.pool);
                    const baseReserves = balances?.baseBalance || 0n;
                    const quoteReserves = balances?.quoteBalance || 0n;

                    const tradeInfo = convertPoolToTradeInfo(
                        result.poolAddress,
                        result.pool,
                        baseReserves,
                        quoteReserves
                    );
                    
                    console.log('TradeInfo built from pool data:');
                    console.log(`  Pool: ${tradeInfo.pool.toBase58()}`);
                    console.log(`  Base mint: ${tradeInfo.baseMint.toBase58()}`);
                    console.log(`  Quote mint: ${tradeInfo.quoteMint.toBase58()}`);
                    console.log(`  Pool base token reserves: ${tradeInfo.poolBaseTokenReserves}`);
                    console.log(`  Pool quote token reserves: ${tradeInfo.poolQuoteTokenReserves}`);
                    console.log(`  Is cashback coin: ${tradeInfo.isCashbackCoin}`);
                    console.log(`  Is mayhem mode: ${tradeInfo.isMayhemMode}`);

                    return tradeInfo;
                } else {
                    throw new Error(`No pool found for mint ${mint.toBase58()}`);
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
        console.log(`Is cashback coin: ${tradeInfo.isCashbackCoin}`);

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
        console.log(`Current price before buy: ${currentPrice.toFixed(12)} SOL/token`);

        const blockhashResult = await this.client.getLatestBlockhash();
        const recentBlockhash = blockhashResult.blockhash;

        const extensionParams = createDexParamEnum(tradeInfo);

        const buyParams: TradeBuyParams = {
            dexType: DexType.PumpSwap,
            inputTokenType: TradeTokenType.WSOL,
            mint: tradeInfo.baseMint,
            inputTokenAmount: solAmount,
            slippageBasisPoints: this.slippageBps,
            recentBlockhash,
            extensionParams,
            waitTxConfirmed: true,
            createInputTokenAta: true,
            closeInputTokenAta: true,
            createMintAta: true,
            gasFeeStrategy: this.gasFeeConfig,
        };

        console.log('Executing buy transaction...');
        
        try {
            const result: TradeResult = await this.client.buy(buyParams);
            
            if (result.success && result.signatures.length > 0) {
                const signature = result.signatures[0];
                console.log(`Buy transaction successful! Signature: ${signature}`);
                this.setBuyPrice(currentPrice, solAmount);
                console.log(`Recorded buy price: ${currentPrice.toFixed(12)} SOL/token, solAmount: ${solAmount} lamports`);
                
                return {
                    success: true,
                    signatures: result.signatures,
                    price: currentPrice,
                };
            } else {
                const errorMsg = result.error ? tradeErrorToString(result.error) : 'Buy failed without error message';
                console.error(`Buy transaction failed: ${errorMsg}`);
                return {
                    success: false,
                    signatures: [],
                    error: errorMsg,
                    price: currentPrice,
                };
            }
        } catch (e) {
            const errorMsg = e instanceof Error ? e.message : 'Unknown error';
            console.error(`Buy transaction error: ${errorMsg}`);
            return {
                success: false,
                signatures: [],
                error: errorMsg,
                price: currentPrice,
            };
        }
    }

    async sell(tradeInfo: TradeInfo): Promise<SellResult> {
        console.log(`Starting sell operation for mint: ${tradeInfo.baseMint.toBase58()}`);
        console.log(`Using pool: ${tradeInfo.pool.toBase58()}`);
        console.log(`Is cashback coin: ${tradeInfo.isCashbackCoin}`);

        const tokenBalance = await this.getTokenBalance(tradeInfo.baseMint);
        if (tokenBalance === 0) {
            throw new Error(`No token balance to sell for mint: ${tradeInfo.baseMint.toBase58()}`);
        }
        console.log(`Token balance to sell: ${tokenBalance}`);

        const blockhashResult = await this.client.getLatestBlockhash();
        const recentBlockhash = blockhashResult.blockhash;

        const extensionParams = createDexParamEnum(tradeInfo);

        const currentPrice = this.calculatePriceFromPool(tradeInfo);
        console.log(`Current price before sell: ${currentPrice.toFixed(12)} SOL/token`);

        const sellParams: TradeSellParams = {
            dexType: DexType.PumpSwap,
            outputTokenType: TradeTokenType.WSOL,
            mint: tradeInfo.baseMint,
            inputTokenAmount: tokenBalance,
            slippageBasisPoints: this.slippageBps,
            recentBlockhash,
            extensionParams,
            waitTxConfirmed: true,
            createOutputTokenAta: true,
            closeOutputTokenAta: true,
            closeMintTokenAta: true,
            gasFeeStrategy: this.gasFeeConfig,
        };

        console.log('Executing sell transaction...');
        
        try {
            const result: TradeResult = await this.client.sell(sellParams);
            
            if (result.success && result.signatures.length > 0) {
                const signature = result.signatures[0];
                console.log(`Sell transaction successful! Signature: ${signature}`);
                
                return {
                    success: true,
                    signatures: result.signatures,
                    price: currentPrice,
                };
            } else {
                const errorMsg = result.error ? tradeErrorToString(result.error) : 'Sell failed without error message';
                console.error(`Sell transaction failed: ${errorMsg}`);
                return {
                    success: false,
                    signatures: [],
                    error: errorMsg,
                    price: currentPrice,
                };
            }
        } catch (e) {
            const errorMsg = e instanceof Error ? e.message : 'Unknown error';
            console.error(`Sell transaction error: ${errorMsg}`);
            return {
                success: false,
                signatures: [],
                error: errorMsg,
                price: currentPrice,
            };
        }
    }
}
