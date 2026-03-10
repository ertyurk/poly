# Auto-Claim Implementation Plan

Research and implementation plan for automatically redeeming winning positions on resolved Polymarket markets.

## Key Finding: SDK Already Supports This

The `polymarket-client-sdk` crate (v0.4.3) has full on-chain redemption behind the `ctf` feature flag. No custom contract interaction needed.

### SDK Surface

The `ctf::Client` exposes:

| Method | Description |
|---|---|
| `redeem_positions(&req)` | Burns winning CTF tokens → recovers USDC (standard markets) |
| `redeem_neg_risk(&req)` | Same but for neg-risk markets (uses NegRisk adapter contract) |
| `split_position(&req)` | Deposit USDC → mint YES+NO tokens |
| `merge_positions(&req)` | Burn YES+NO tokens → recover USDC |

Request builder: `RedeemPositionsRequest::for_binary_market(collateral_addr, condition_id)` — uses index sets `[1, 2]` for binary (Yes/No) markets.

### Contracts (Polygon Mainnet, chain 137)

| Contract | Address |
|---|---|
| CTF Exchange (standard) | `0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E` |
| CTF Exchange (neg-risk) | `0xC5d563A36AE78145C45a50134d48A1215220f80a` |
| NegRisk Adapter | `0xd91E80cF2E7be2e162c6513ceD06f1dD0dA35296` |
| Conditional Tokens (CTF) | `0x4D97DCd97eC945f40cF65F87097ACe5EA0476045` |
| USDC (collateral) | `0x2791Bca1f2de4661ED88A30C99A7a9449Aa84174` |

### Prerequisites for Redemption

1. Wallet must have called `CTF.setApprovalForAll(exchange, true)` — one-time setup
2. Wallet must hold the winning CTF tokens (ERC-1155 position tokens)
3. Market must be resolved (condition resolved by oracle)
4. Wallet needs MATIC/POL for gas (~0.001-0.01 POL per redemption tx)

## Implementation Plan

### Step 1: Enable `ctf` feature

```toml
# Cargo.toml
polymarket-client-sdk = { version = "0.4", features = ["clob", "ctf"] }
alloy = { version = "1.6", default-features = false, features = [
    "signer-local", "signers", "contract", "providers", "reqwest"
] }
```

### Step 2: Add `OnChainRedeemer` to `src/polymarket/redeemer.rs`

```rust
use alloy::providers::ProviderBuilder;
use alloy::signers::local::PrivateKeySigner;
use polymarket_client_sdk::ctf;

pub struct OnChainRedeemer {
    ctf_client: ctf::Client</* provider type */>,
    ctf_neg_risk: ctf::Client</* provider type */>,
    collateral: Address, // USDC on Polygon
}

impl OnChainRedeemer {
    pub async fn new(private_key: &str, rpc_url: &str) -> Result<Self, ...> {
        let signer: PrivateKeySigner = pk.parse()?;
        let provider = ProviderBuilder::new()
            .wallet(signer)
            .connect(rpc_url)
            .await?;

        let ctf_client = ctf::Client::new(provider.clone(), 137);
        let ctf_neg_risk = ctf::Client::with_neg_risk(provider, 137);

        Ok(Self { ctf_client, ctf_neg_risk, collateral: USDC_ADDRESS })
    }

    pub async fn redeem(&self, condition_id: B256, neg_risk: bool) -> Result<TxHash, ...> {
        let req = RedeemPositionsRequest::for_binary_market(self.collateral, condition_id);
        let resp = if neg_risk {
            self.ctf_neg_risk.redeem_neg_risk(&req).await?
        } else {
            self.ctf_client.redeem_positions(&req).await?
        };
        Ok(resp.transaction_hash)
    }
}
```

### Step 3: Integrate with Executor settle flow

In `executor.rs`, after `settle()` detects a win in live mode:

```rust
// After settle confirms a winning position:
if mode == Mode::Live && outcome == Outcome::Win {
    match redeemer.redeem(condition_id, is_neg_risk).await {
        Ok(tx_hash) => tracing::info!(%tx_hash, "auto-claimed winning position"),
        Err(e) => tracing::warn!(%e, "auto-claim failed — claim manually on polymarket.com"),
    }
}
```

Failure should **never** block the bot — just log a warning. The user can always claim manually.

### Step 4: One-time approval setup

Before first redemption, the wallet needs ERC-1155 approval for the exchange contracts. Add a CLI subcommand or startup check:

```rust
// Check if approval exists, if not, send approval tx
// CTF.setApprovalForAll(CTF_EXCHANGE, true)
// CTF.setApprovalForAll(NEG_RISK_EXCHANGE, true)
```

This only needs to happen once per wallet. Could be:
- A `cargo run -- setup-approvals` CLI subcommand
- An automatic check on first live-mode startup

### Step 5: Track `condition_id` in OpenPosition

Currently `OpenPosition` stores `market_id` (our internal ID like `BTC_5m_0x23c2ca`). For redemption we need the raw `condition_id` (bytes32 hash from Gamma API).

Changes needed:
- Add `condition_id: String` field to `OpenPosition` struct
- Store it in `open_positions` DB table
- Pass it from market discovery → decision → executor → redeemer
- Add `neg_risk: bool` flag (most crypto up/down markets are neg-risk)

### Step 6: Config additions

```toml
[general]
auto_claim = true  # default false — opt-in

[polygon]
rpc_url = "https://polygon-rpc.com"  # or Alchemy/Infura endpoint
```

A public Polygon RPC is fine for occasional redemption txs. For heavy usage, use Alchemy/Infura.

### Step 7: Telegram notification

Extend `TelegramAlert` enum:

```rust
TelegramAlert::PositionClaimed { market_id: String, pnl: f64, tx_hash: String }
```

## Data Flow

```
Market resolves (detected by market_fetcher)
  → executor.settle() calculates PnL
  → if Win && auto_claim enabled && live mode:
      → redeemer.redeem(condition_id) sends on-chain tx
      → wait for receipt (tx_hash, block_number)
      → log success / send telegram alert
  → if redemption fails:
      → log warning, continue (user claims manually)
```

## New Files

| File | Purpose |
|---|---|
| `src/polymarket/redeemer.rs` | `OnChainRedeemer` — wraps SDK CTF client |

## Modified Files

| File | Change |
|---|---|
| `Cargo.toml` | Add `ctf` feature, expand alloy features |
| `src/polymarket/mod.rs` | Add `pub mod redeemer` |
| `src/actors/executor.rs` | Call redeemer after winning settle |
| `src/actors/telegram.rs` | Add `PositionClaimed` alert variant |
| `src/config.rs` | Add `auto_claim`, `rpc_url` fields |
| `src/main.rs` | Construct redeemer, pass to executor |
| `src/db/schema.rs` | Add `condition_id` to open_positions |
| `src/actors/market_fetcher.rs` | Pass `condition_id` through to downstream |

## Open Questions

1. **Standard vs neg-risk**: How do we know if a market is neg-risk? The Gamma API may have a field for this — needs investigation at implementation time.
2. **Gas costs**: Redemption txs cost ~0.001-0.01 POL. Should we check POL balance before attempting?
3. **Batch redemption**: If multiple positions resolve at once, should we batch or redeem one-by-one? SDK calls are per-condition, so one tx per market.
4. **Retry logic**: If a redemption tx fails (gas spike, RPC timeout), should we retry? A simple single retry with backoff seems reasonable.
5. **Approval persistence**: How to track whether approvals have been granted? Check on-chain via `isApprovedForAll` view call on startup.
