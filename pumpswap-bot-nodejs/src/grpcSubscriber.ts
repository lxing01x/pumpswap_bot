export interface TransactionUpdate {
    mint: string;
    tokenAmount: number;
    solAmount: number;
    isBuy: boolean;
    signature: string;
    blocktimeUs: number;
}

export type TransactionHandler = (update: TransactionUpdate) => void | Promise<void>;

export const WSOL_MINT = 'So11111111111111111111111111111111111111112';

export interface GrpcSubscriberConfig {
    grpcUrl: string;
    grpcToken?: string;
}

export class GrpcSubscriber {
    private config: GrpcSubscriberConfig;
    private handlers: TransactionHandler[] = [];
    private running: boolean = false;

    constructor(config: GrpcSubscriberConfig) {
        this.config = config;
    }

    addHandler(handler: TransactionHandler): void {
        this.handlers.push(handler);
    }

    async subscribe(): Promise<void> {
        console.log('Starting gRPC subscription...');
        console.log(`gRPC URL: ${this.config.grpcUrl}`);
        
        if (this.config.grpcToken) {
            if (this.config.grpcToken.length === 0) {
                console.warn('gRPC token is empty!');
            } else {
                console.log(`gRPC token is configured (length: ${this.config.grpcToken.length})`);
            }
        } else {
            console.warn('gRPC token is not configured!');
            console.warn('The Solana Yellowstone gRPC service requires a personal token.');
        }

        console.log('');
        console.log('NOTE: This is a placeholder gRPC subscriber implementation.');
        console.log('To integrate the actual Yellowstone gRPC client, you will need to:');
        console.log('');
        console.log('1. Install a gRPC client library (e.g., @grpc/grpc-js)');
        console.log('2. Use the Yellowstone gRPC proto files to generate TypeScript types');
        console.log('3. Subscribe to transaction streams filtering for PumpSwap program');
        console.log('');
        console.log('PumpSwap program ID: pAMMBay6oceH9fJKBRHGP5D4bD4sWpmSwMn52FMfXEA');
        console.log('');
        console.log('Example integration approach:');
        console.log(`
import { ChannelCredentials, Metadata } from '@grpc/grpc-js';
import { GeyserClient } from './generated/geyser';

const client = new GeyserClient(
  this.config.grpcUrl,
  ChannelCredentials.createSsl()
);

const metadata = new Metadata();
if (this.config.grpcToken) {
  metadata.add('x-token', this.config.grpcToken);
}

const request = {
  slots: { slot: [] },
  transactions: {
    vote: false,
    failed: false,
    accountInclude: [PUMPSWAP_PROGRAM_ID],
    accountExclude: [],
    accountRequired: [],
  },
  accounts: { account: [], owner: [], filters: [] },
  entry: {},
  blocks: {},
  blocksMeta: {},
  accountsDataSlice: [],
  transactionsStatus: {},
};

const stream = client.subscribe(request, metadata);

stream.on('data', (message) => {
  // Parse transaction and extract PumpSwap buy/sell events
  // Update mint, tokenAmount, solAmount, isBuy, signature, blocktimeUs
  const update: TransactionUpdate = {
    mint: extractedMint,
    tokenAmount: extractedTokenAmount,
    solAmount: extractedSolAmount,
    isBuy: isBuyEvent,
    signature: transactionSignature,
    blocktimeUs: blockTime * 1_000_000,
  };
  this._emitUpdate(update);
});

stream.on('error', (error) => {
  console.error('gRPC stream error:', error);
});

stream.on('end', () => {
  console.log('gRPC stream ended');
});
`);
        console.log('');

        this.running = true;
        
        console.log('gRPC subscription started in placeholder mode.');
        console.log('To receive real transaction updates, implement the actual gRPC client integration.');
    }

    unsubscribe(): void {
        this.running = false;
        console.log('gRPC subscription stopped');
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
