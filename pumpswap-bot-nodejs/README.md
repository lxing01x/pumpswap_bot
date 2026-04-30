# PumpSwap Trading Bot (Node.js)

一个基于 Node.js 的 PumpSwap DEX 交易机器人，支持自动买入、卖出和策略交易。

## 功能特性

### 核心交易功能
- **买入交易**：在 PumpSwap 上自动执行买入操作
- **卖出交易**：自动执行卖出操作
- **MEV 保护**：支持 Jito、BloXroute 等 MEV 保护服务

### 策略交易
- **价格追踪买入**：监控代币价格涨幅，达到阈值时自动买入
- **止盈止损**：持仓时自动监控价格，达到止盈或止损条件时自动卖出
- **多代币监控**：同时监控多个活跃代币的交易机会

### 数据与记录
- **Redis 存储**：实时存储交易事件，用于价格分析
- **交易记录**：每笔完整交易（买入+卖出）记录到 JSON 文件，包含详细信息
- **统计功能**：查看胜率、平均持仓时间、总盈亏等统计数据

## 项目结构

```
pumpswap-bot-nodejs/
├── src/
│   ├── config.ts           # 配置加载模块
│   ├── trader.ts           # 交易核心模块
│   ├── strategy.ts         # 交易策略模块
│   ├── redisStore.ts       # Redis 存储模块
│   ├── tradeRecorder.ts    # 交易记录模块
│   ├── grpcSubscriber.ts   # gRPC 订阅模块
│   └── index.ts            # 主入口文件
├── .trade_records/         # 交易记录存储目录
│   ├── active_positions.json    # 活跃持仓
│   ├── closed_trades.json       # 已完成交易
│   └── trades_YYYY-MM-DD.json   # 每日交易记录
├── config.json.example     # 配置文件模板
├── package.json
├── tsconfig.json
└── README.md
```

## 安装与配置

### 环境要求
- Node.js >= 18.0.0
- Redis (可选，用于价格追踪)
- TypeScript (开发环境)

### 安装步骤

1. 安装依赖：
```bash
npm install
```

2. 复制配置文件模板：
```bash
cp config.json.example config.json
```

3. 编辑 `config.json`，填写你的配置信息：

```json
{
    "grpc_url": "https://api.mainnet-beta.solana.com",
    "rpc_url": "https://api.mainnet-beta.solana.com",
    "grpc_token": "your_grpc_token_here",
    "private_key": "your_base58_private_key_here",
    "target_mint": "target_token_mint_address",
    "buy_amount_sol": 0.01,
    "hold_seconds": 10,
    "slippage_bps": 500,
    "max_retries": 5,
    "retry_delay_ms": 1000,
    "jito_enabled": false,
    "jito_uuid": "your_jito_uuid_here",
    "jito_region": "Frankfurt",
    "redis_url": "redis://127.0.0.1/",
    "max_trades_per_token": 1000,
    "buy_threshold_pct": 10.0,
    "buy_time_window_sec": 5,
    "buy_record_count": 5,
    "sell_profit_pct": 10.0,
    "sell_stop_loss_pct": 5.0
}
```

### 配置参数说明

| 参数 | 类型 | 默认值 | 说明 |
|------|------|--------|------|
| grpc_url | string | - | Yellowstone gRPC 端点 URL |
| rpc_url | string | - | Solana RPC 端点 URL |
| grpc_token | string | - | gRPC 认证令牌（可选） |
| private_key | string | - | 钱包私钥（base58 格式） |
| target_mint | string | - | 目标代币 mint 地址 |
| buy_amount_sol | number | - | 每次买入的 SOL 数量 |
| hold_seconds | number | - | 持仓时间（秒） |
| slippage_bps | number | 500 | 滑点容忍度（基点，1% = 100 bps） |
| max_retries | number | 5 | 最大重试次数 |
| retry_delay_ms | number | 1000 | 重试间隔（毫秒） |
| jito_enabled | boolean | false | 是否启用 Jito MEV 保护 |
| jito_uuid | string | - | Jito UUID（如果启用 Jito） |
| jito_region | string | Frankfurt | Jito 区域 |
| redis_url | string | redis://127.0.0.1/ | Redis 连接 URL |
| max_trades_per_token | number | 1000 | 每个代币最多存储的交易记录数 |
| buy_threshold_pct | number | 10.0 | 买入阈值涨幅百分比 |
| buy_record_count | number | 5 | 计算价格涨幅使用的最近交易记录数 |
| sell_profit_pct | number | 10.0 | 止盈阈值百分比 |
| sell_stop_loss_pct | number | 5.0 | 止损阈值百分比 |

## 使用方法

### 编译

```bash
npm run build
```

### 运行

```bash
# 使用默认配置文件 config.json
npm start

# 或指定配置文件路径
node dist/index.js /path/to/your/config.json
```

### 开发模式

```bash
npm run dev
```

## 交易策略说明

### 买入策略

机器人通过以下逻辑判断是否买入：

1. **价格监控**：通过 gRPC 订阅实时接收 PumpSwap 交易事件
2. **价格计算**：基于最近 N 笔交易（`buy_record_count`）计算价格涨幅
3. **触发条件**：当价格涨幅超过 `buy_threshold_pct` 时触发买入

**示例**：
- `buy_threshold_pct = 10.0`
- `buy_record_count = 5`
- 如果最近 5 笔交易的价格涨幅超过 10%，则执行买入

### 卖出策略

持仓期间，机器人持续监控价格：

1. **止盈**：当前价格相比买入价格上涨 `sell_profit_pct` 时卖出
2. **止损**：当前价格相比买入价格下跌 `sell_stop_loss_pct` 时卖出

**示例**：
- `sell_profit_pct = 10.0`（上涨 10% 止盈）
- `sell_stop_loss_pct = 5.0`（下跌 5% 止损）

## 交易记录功能

每完成一笔完整交易（买入 + 卖出），系统会自动记录到 JSON 文件。

### 记录字段说明

| 字段 | 类型 | 说明 |
|------|------|------|
| id | string | 交易唯一标识 |
| mint | string | 代币 mint 地址 |
| buyPrice | number | 买入价格（SOL/token） |
| buySolAmount | number | 买入 SOL 数量（lamports） |
| buyTimestamp | number | 买入时间戳（毫秒） |
| buySignature | string | 买入交易签名 |
| sellPrice | number | 卖出价格（SOL/token） |
| sellSolAmount | number | 卖出获得 SOL 数量（lamports） |
| sellTimestamp | number | 卖出时间戳（毫秒） |
| sellSignature | string | 卖出交易签名 |
| highestPrice | number | 持仓期间最高价格 |
| lowestPrice | number | 持仓期间最低价格 |
| holdTimeSeconds | number | 持仓时间（秒） |
| profitLossPercent | number | 收益百分比 |
| profitLossSol | number | 收益 SOL 数量（lamports） |
| status | string | 交易状态（open/closed） |

### 记录文件位置

- **活跃持仓**：`.trade_records/active_positions.json`
- **已完成交易**：`.trade_records/closed_trades.json`
- **每日记录**：`.trade_records/trades_YYYY-MM-DD.json`

### 统计功能

可以通过 `TradeRecorder` 类获取交易统计：

```typescript
import { TradeRecorder } from './tradeRecorder';

const recorder = new TradeRecorder();
const stats = recorder.getStatistics();

console.log(`总交易数: ${stats.totalTrades}`);
console.log(`盈利交易: ${stats.winningTrades}`);
console.log(`亏损交易: ${stats.losingTrades}`);
console.log(`总盈亏: ${(stats.totalProfitLossSol / 1e9).toFixed(9)} SOL`);
console.log(`平均持仓时间: ${stats.averageHoldTimeSeconds} 秒`);
console.log(`胜率: ${stats.winRate.toFixed(2)}%`);
```

## 集成 sol-trade-sdk-nodejs

当前代码中的 `trader.ts` 包含了 `sol-trade-sdk-nodejs` 的集成示例。要使用实际的 SDK，请按照以下步骤操作：

### 1. 安装 SDK

```bash
# 方式1: 通过 npm
npm install sol-trade-sdk

# 方式2: 克隆仓库
git clone https://github.com/0xfnzero/sol-trade-sdk-nodejs
cd sol-trade-sdk-nodejs
npm install
npm run build
```

### 2. 修改 trader.ts

在 `trader.ts` 中，替换买入和卖出的占位实现。关键代码结构如下：

```typescript
import { 
    TradingClient, 
    TradeConfig, 
    SwqosConfig, 
    SwqosRegion,
    DexType, 
    TradeBuyParams, 
    TradeSellParams, 
    TradeTokenType, 
    GasFeeStrategy,
    PumpSwapParams
} from 'sol-trade-sdk';

// 创建交易客户端
const swqosConfigs: SwqosConfig[] = [
    { type: 'Default', rpcUrl: this.rpcUrl },
    // 或使用 Jito: { type: 'Jito', uuid: 'your_uuid', region: SwqosRegion.Frankfurt }
];

const tradeConfig = new TradeConfig(rpcUrl, swqosConfigs);
const client = new TradingClient(keypair, tradeConfig);

// 配置 Gas 策略
const gasFeeStrategy = new GasFeeStrategy();
gasFeeStrategy.setGlobalFeeStrategy(
    150000, 150000,  // compute unit limit / price
    500000, 500000,  // priority fee
    0.0001, 0.0001    // tip
);

// 构建买入参数
const pumpswapParams = PumpSwapParams.from_trade(/* 池参数 */);
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

// 执行买入
const result = await client.buy(buyParams);
```

## 依赖说明

### 主要依赖

| 包名 | 版本 | 用途 |
|------|------|------|
| @solana/web3.js | ^1.95.0 | Solana 区块链交互 |
| ioredis | ^5.4.0 | Redis 客户端 |
| bs58 | ^5.0.0 | Base58 编解码 |
| winston | ^3.13.0 | 日志记录 |

### 开发依赖

| 包名 | 版本 | 用途 |
|------|------|------|
| typescript | ^5.5.0 | TypeScript 编译器 |
| ts-node | ^10.9.2 | TypeScript 执行器 |
| @types/node | ^20.14.0 | Node.js 类型定义 |

## 注意事项

### 安全警告
1. **私钥安全**：永远不要将包含私钥的配置文件提交到版本控制
2. **审计代码**：在使用真实资金之前，请仔细审计所有代码
3. **小额测试**：建议先用小额资金进行测试

### 风险提示
1. **加密货币交易风险极高**，可能导致资金损失
2. **滑点风险**：实际执行价格可能与预期价格有偏差
3. **MEV 风险**：交易可能被抢跑或三明治攻击
4. **网络风险**：RPC 或 gRPC 服务可能出现故障

### 性能建议
1. 使用低延迟的 RPC 节点（如 Helius、Alchemy）
2. 考虑使用 MEV 保护服务（Jito、BloXroute）
3. 确保 Redis 实例与机器人在同一网络中

## 与 Rust 版本的对比

| 特性 | Rust 版本 | Node.js 版本 |
|------|-----------|--------------|
| 交易功能 | ✅ 完整 | ✅ 完整（SDK 集成示例） |
| 策略交易 | ✅ 完整 | ✅ 完整 |
| gRPC 订阅 | ✅ 完整 | ✅ 接口化设计 |
| Redis 存储 | ✅ 完整 | ✅ 完整 |
| 交易记录 | ⚠️ 部分 | ✅ 完善（JSON 文件） |
| 开发体验 | 编译期检查 | 更灵活，易于调试 |
| 性能 | 极高 | 良好（适合大多数场景） |

## 许可证

MIT License

## 联系方式

如需支持或问题反馈，请参考原始 Rust SDK 仓库：
- https://github.com/0xfnzero/sol-trade-sdk-nodejs
- https://github.com/0xfnzero/sol-trade-sdk
