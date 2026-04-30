import { Connection, PublicKey, TransactionResponse, VersionedTransactionResponse } from '@solana/web3.js';
import { PUMPSWAP_PROGRAM, WSOL_TOKEN_ACCOUNT } from 'sol-trade-sdk';

export interface TransactionUpdate {
    mint: string;
    tokenAmount: number;
    solAmount: number;
    isBuy: boolean;
    signature: string;
    blocktimeUs: number;
}

export type TransactionHandler = (update: TransactionUpdate) => void | Promise<void>;

export const WSOL_MINT = WSOL_TOKEN_ACCOUNT.toBase58();

export interface GrpcSubscriberConfig {
    grpcUrl?: string;
    grpcToken?: string;
    rpcUrl: string;
}

const PUMPSWAP_BUY_DISCRIMINATOR = Buffer.from([102, 6, 61, 18, 1, 218, 235, 234]);
const PUMPSWAP_BUY_EXACT_QUOTE_IN_DISCRIMINATOR = Buffer.from([198, 46, 21, 82, 180, 217, 232, 112]);
const PUMPSWAP_SELL_DISCRIMINATOR = Buffer.from([51, 230, 133, 164, 1, 127, 131, 173]);

function parseInstructionData(data: Buffer): { isBuy: boolean; tokenAmount: bigint; solAmount: bigint } | null {
    if (data.length < 24) return null;

    const discriminator = data.subarray(0, 8);
    
    if (discriminator.equals(PUMPSWAP_BUY_DISCRIMINATOR)) {
        const tokenAmount = data.readBigUInt64LE(8);
        const solAmount = data.readBigUInt64LE(16);
        return { isBuy: true, tokenAmount, solAmount };
    }
    
    if (discriminator.equals(PUMPSWAP_BUY_EXACT_QUOTE_IN_DISCRIMINATOR)) {
        const solAmount = data.readBigUInt64LE(8);
        const minTokenAmount = data.readBigUInt64LE(16);
        return { isBuy: true, tokenAmount: minTokenAmount, solAmount };
    }
    
    if (discriminator.equals(PUMPSWAP_SELL_DISCRIMINATOR)) {
        const tokenAmount = data.readBigUInt64LE(8);
        const minSolAmount = data.readBigUInt64LE(16);
        return { isBuy: false, tokenAmount, solAmount: minSolAmount };
    }
    
    return null;
}

function extractMintFromAccountKeys(keys: PublicKey[]): string | null {
    for (const key of keys) {
        const base58 = key.toBase58();
        if (base58 !== WSOL_MINT && 
            base58 !== '11111111111111111111111111111111' &&
            base58 !== 'TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA' &&
            base58 !== 'ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL' &&
            base58 !== PUMPSWAP_PROGRAM.toBase58() &&
            base58 !== 'SysvarRent111111111111111111111111111111111' &&
            !base58.startsWith('p') &&
            key.toBuffer()[0] !== 0) {
            return base58;
        }
    }
    return null;
}

interface ParsedTransaction {
    mint: string;
    tokenAmount: bigint;
    solAmount: bigint;
    isBuy: boolean;
}

function parseVersionedTransaction(tx: VersionedTransactionResponse): ParsedTransaction | null {
    const message = tx.transaction.message;
    const staticAccountKeys = message.staticAccountKeys;
    
    for (let i = 0; i < message.compiledInstructions.length; i++) {
        const ix = message.compiledInstructions[i];
        const programId = staticAccountKeys[ix.programIdIndex];
        
        if (programId && programId.equals(PUMPSWAP_PROGRAM)) {
            const data = Buffer.from(ix.data);
            
            const parsed = parseInstructionData(data);
            if (parsed) {
                const accountKeyList = ix.accountKeyIndexes.map((idx: number) => staticAccountKeys[idx]);
                
                let mint = extractMintFromAccountKeys(accountKeyList);
                
                if (mint) {
                    return {
                        mint,
                        tokenAmount: parsed.tokenAmount,
                        solAmount: parsed.solAmount,
                        isBuy: parsed.isBuy,
                    };
                }
            }
        }
    }
    return null;
}

function parseLegacyTransaction(tx: TransactionResponse): ParsedTransaction | null {
    const message = tx.transaction.message;
    const accountKeys = message.accountKeys;
    
    for (let i = 0; i < message.instructions.length; i++) {
        const ix = message.instructions[i];
        const programId = accountKeys[ix.programIdIndex];
        
        if (programId && programId.equals(PUMPSWAP_PROGRAM)) {
            const data = Buffer.from(ix.data);
            
            const parsed = parseInstructionData(data);
            if (parsed) {
                const accountKeyList = ix.accounts.map((idx: number) => accountKeys[idx]);
                
                let mint = extractMintFromAccountKeys(accountKeyList);
                
                if (mint) {
                    return {
                        mint,
                        tokenAmount: parsed.tokenAmount,
                        solAmount: parsed.solAmount,
                        isBuy: parsed.isBuy,
                    };
                }
            }
        }
    }
    return null;
}

function parseTransaction(tx: TransactionResponse | VersionedTransactionResponse): ParsedTransaction | null {
    if ('versioned' in tx.transaction) {
        return parseVersionedTransaction(tx as VersionedTransactionResponse);
    } else {
        return parseLegacyTransaction(tx as TransactionResponse);
    }
}

export class GrpcSubscriber {
    private config: GrpcSubscriberConfig;
    private handlers: TransactionHandler[] = [];
    private running: boolean = false;
    private connection: Connection | null = null;
    private subscriptionId: number | null = null;
    private processedSignatures: Set<string> = new Set();
    private readonly maxProcessedSignatures: number = 10000;

    constructor(config: GrpcSubscriberConfig) {
        this.config = config;
    }

    addHandler(handler: TransactionHandler): void {
        this.handlers.push(handler);
    }

    private async emitUpdate(update: TransactionUpdate): Promise<void> {
        for (const handler of this.handlers) {
            try {
                await Promise.resolve(handler(update));
            } catch (e) {
                console.error('Error in transaction handler:', e);
            }
        }
    }

    private isSignatureProcessed(signature: string): boolean {
        return this.processedSignatures.has(signature);
    }

    private markSignatureProcessed(signature: string): void {
        this.processedSignatures.add(signature);
        if (this.processedSignatures.size > this.maxProcessedSignatures) {
            const signatures = Array.from(this.processedSignatures);
            const toRemove = signatures.slice(0, signatures.length - this.maxProcessedSignatures / 2);
            for (const sig of toRemove) {
                this.processedSignatures.delete(sig);
            }
        }
    }

    async subscribe(): Promise<void> {
        console.log('Starting subscription to PumpSwap transactions...');
        
        if (this.config.grpcUrl && this.config.grpcToken) {
            console.log(`gRPC URL: ${this.config.grpcUrl}`);
            console.log('NOTE: gRPC subscription requires Yellowstone gRPC service.');
            console.log('Falling back to websocket subscription for now...');
        }
        
        console.log(`RPC URL: ${this.config.rpcUrl}`);
        console.log(`PumpSwap Program ID: ${PUMPSWAP_PROGRAM.toBase58()}`);
        console.log('');
        console.log('Using websocket logs subscription to monitor PumpSwap transactions.');
        console.log('This will subscribe to all logs from the PumpSwap program.');
        console.log('');

        this.connection = new Connection(this.config.rpcUrl, {
            commitment: 'confirmed',
            wsEndpoint: this.config.rpcUrl.replace('https://', 'wss://').replace('http://', 'ws://'),
        });

        this.running = true;

        try {
            this.subscriptionId = this.connection.onLogs(
                PUMPSWAP_PROGRAM,
                async (logs) => {
                    if (!this.running) return;
                    
                    const signature = logs.signature;
                    
                    if (this.isSignatureProcessed(signature)) {
                        return;
                    }
                    
                    if (logs.err) {
                        console.debug(`Skipping failed transaction: ${signature}`);
                        return;
                    }

                    try {
                        const tx = await this.connection!.getTransaction(signature, {
                            commitment: 'confirmed',
                            maxSupportedTransactionVersion: 0,
                        });

                        if (tx) {
                            const parsed = parseTransaction(tx);
                            
                            if (parsed) {
                                const blocktime = tx.blockTime ? tx.blockTime * 1_000_000 : Date.now() * 1000;
                                
                                const update: TransactionUpdate = {
                                    mint: parsed.mint,
                                    tokenAmount: Number(parsed.tokenAmount),
                                    solAmount: Number(parsed.solAmount),
                                    isBuy: parsed.isBuy,
                                    signature,
                                    blocktimeUs: blocktime,
                                };

                                this.markSignatureProcessed(signature);
                                
                                console.log(
                                    `[${parsed.isBuy ? 'BUY' : 'SELL'}] mint=${parsed.mint}, ` +
                                    `tokenAmount=${parsed.tokenAmount}, solAmount=${parsed.solAmount}, ` +
                                    `signature=${signature.slice(0, 20)}...`
                                );
                                
                                await this.emitUpdate(update);
                            }
                        }
                    } catch (e) {
                        console.error(`Error processing transaction ${signature}:`, e);
                    }
                },
                'confirmed'
            );

            console.log('Websocket subscription started successfully!');
            console.log('');
            console.log('Monitoring for PumpSwap transactions...');
            console.log('');
            console.log('Press Ctrl+C to stop.');
            console.log('');

        } catch (e) {
            console.error('Failed to start subscription:', e);
            this.running = false;
            throw e;
        }
    }

    unsubscribe(): void {
        this.running = false;
        
        if (this.subscriptionId !== null && this.connection) {
            this.connection.removeOnLogsListener(this.subscriptionId);
            this.subscriptionId = null;
        }
        
        console.log('Subscription stopped');
    }

    isRunning(): boolean {
        return this.running;
    }

    static processPumpSwapEvent(event: {
        baseMint: string;
        quoteMint: string;
        poolBaseTokenReserves: number;
        poolQuoteTokenReserves: number;
        timestamp: number;
        signature: string;
        isBuy: boolean;
    }): TransactionUpdate | null {
        const isWsolBase = event.baseMint === WSOL_MINT;
        const isWsolQuote = event.quoteMint === WSOL_MINT;

        if (!isWsolBase && !isWsolQuote) {
            console.debug(`Event not involving WSOL: base=${event.baseMint}, quote=${event.quoteMint}`);
            return null;
        }

        const mint = isWsolBase ? event.quoteMint : event.baseMint;
        const tokenAmount = isWsolBase 
            ? event.poolQuoteTokenReserves 
            : event.poolBaseTokenReserves;
        const solAmount = isWsolBase 
            ? event.poolBaseTokenReserves 
            : event.poolQuoteTokenReserves;

        const blocktimeUs = event.timestamp * 1_000_000;

        return {
            mint,
            tokenAmount,
            solAmount,
            isBuy: event.isBuy,
            signature: event.signature,
            blocktimeUs,
        };
    }
}
