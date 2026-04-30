import { ConfigLoader } from './config';
import { TradingStrategy } from './strategy';
import { GrpcSubscriber } from './grpcSubscriber';

const DEFAULT_CONFIG_PATH = 'config.json';

async function main() {
    const configPath = process.argv[2] || DEFAULT_CONFIG_PATH;
    
    console.log(`Loading config from: ${configPath}`);
    
    try {
        const config = ConfigLoader.fromFile(configPath);
        
        console.log('Initializing trading strategy...');
        const strategy = await TradingStrategy.create(config);
        
        if (config.grpcUrl) {
            console.log('Setting up gRPC subscription...');
            const grpcSubscriber = new GrpcSubscriber({
                grpcUrl: config.grpcUrl,
                grpcToken: config.grpcToken,
            });
            
            grpcSubscriber.addHandler(async (update) => {
                console.log(
                    `Received ${update.isBuy ? 'BUY' : 'SELL'} event: mint=${update.mint}, ` +
                    `tokenAmount=${update.tokenAmount}, solAmount=${update.solAmount}, ` +
                    `signature=${update.signature}`
                );
                
                await strategy.processTransactionUpdate(update);
            });
            
            await grpcSubscriber.subscribe();
        }
        
        console.log('Starting bot...');
        await strategy.run();
        
        console.log('Bot completed.');
    } catch (e) {
        console.error('Bot error:', e);
        process.exit(1);
    }
}

main().catch((e) => {
    console.error('Fatal error:', e);
    process.exit(1);
});
