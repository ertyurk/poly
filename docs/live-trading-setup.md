# Live Trading Setup

Step-by-step guide to configure secrets and credentials for real order execution on Polymarket.

Paper trading (`--paper-trade`) requires **none** of this — it uses public endpoints only.

## Prerequisites

- An Ethereum wallet (MetaMask, Rabby, etc.) with a private key you control
- A Polymarket account linked to that wallet
- MATIC (POL) on Polygon for gas fees
- USDC on Polygon for trading capital

## Step 1: Get Your Polymarket API Credentials

1. Go to [polymarket.com](https://polymarket.com) and sign in
2. Open **Settings** > **API Keys** (or navigate to the API management page)
3. Click **Create API Key**
4. Save the three values shown:
   - **API Key** — your public key identifier
   - **API Secret** — base64-encoded HMAC secret (shown once, save it immediately)
   - **Passphrase** — chosen during key creation

These credentials authenticate requests to the CLOB API via HMAC-SHA256 signatures. The bot sends four headers on every authenticated request:

| Header | Value |
|---|---|
| `POLY-API-KEY` | Your API key |
| `POLY-SIGNATURE` | HMAC-SHA256 of `timestamp\nmethod\npath\nbody`, base64-encoded |
| `POLY-TIMESTAMP` | Unix timestamp (seconds) |
| `POLY-PASSPHRASE` | Your passphrase |

## Step 2: Export Your Wallet Private Key

The bot needs your Ethereum private key to sign EIP-712 order messages for the Polymarket CTF Exchange contract on Polygon (chain ID 137). The key must belong to the same wallet your Polymarket account is linked to.

**From Polymarket (Google/email sign-up via Magic.Link) — most common:**
1. Go to Polymarket **Settings** > **Private Key**
2. Click **Start Export**
3. Authenticate with Magic.Link (same Google/email you signed up with)
4. Copy the private key displayed (hex string, no `0x` prefix)
5. Log out of Magic.Link when done

This is the wallet Polymarket created for you automatically. You must use this key — a different wallet's key won't work because the CLOB API verifies the signer matches the account.

**From MetaMask (if you connected an external wallet):**
1. Click the three dots next to your account name
2. Select **Account Details** > **Show Private Key**
3. Enter your password to reveal it
4. Copy the hex string (without the `0x` prefix)

**From Rabby:**
1. Open Rabby > click your address > **More** > **Export Private Key**
2. Copy the hex string (without `0x`)

**Security notes:**
- This key has full control of your wallet. Never share it.
- The bot derives your wallet address from this key automatically (`signing::address_from_key`).
- Orders are signed locally — the private key is never sent over the network.

## Step 3: Create the `.env` File

Copy the example file and fill in your values:

```bash
cp .env.example .env
```

Edit `.env`:

```bash
# Logging (optional, defaults to info)
RUST_LOG=polymarket_bot=info

# Polymarket CLOB API credentials (from Step 1)
POLYMARKET_API_KEY=your_api_key_here
POLYMARKET_API_SECRET=your_base64_secret_here
POLYMARKET_PASSPHRASE=your_passphrase_here

# Ethereum private key for EIP-712 signing (from Step 2)
# Hex string, NO 0x prefix
PRIVATE_KEY=abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890
```

The bot loads `.env` automatically via `dotenvy` on startup.

## Step 4: Switch Config to Live Mode

In `config.toml`, change the mode:

```toml
[general]
mode = "live"
```

Or simply run without `--paper-trade`:

```bash
cargo run -- --asset btc --bankroll 1000
```

When `--paper-trade` is omitted, the bot reads all four env vars on startup and will fail immediately if any are missing.

## Step 5: (Optional) Telegram Notifications

Add to `config.toml`:

```toml
[telegram]
bot_token = "123456:ABC-DEF..."
chat_id = "your_chat_id"
enabled = true
summary_interval_mins = 15
```

**Get your bot token:**
1. Message [@BotFather](https://t.me/BotFather) on Telegram
2. Send `/newbot`, follow the prompts
3. Copy the token it gives you

**Get your chat ID:**
1. Message [@userinfobot](https://t.me/userinfobot) on Telegram
2. It replies with your numeric chat ID

**Start the bot:**
1. Open your bot in Telegram and press **Start** (or send `/start`)
2. The bot won't send messages until you've initiated a conversation with it

## Security Checklist

- [ ] `.env` is in `.gitignore` (it is by default)
- [ ] Never commit `.env` or paste credentials in code
- [ ] Private key wallet has only trading capital, not your main holdings
- [ ] Revoke and regenerate API keys if you suspect they're compromised
- [ ] If you accidentally expose a Telegram bot token, revoke it via `@BotFather` → `/revoketoken`
- [ ] Consider using a hardware wallet for large bankrolls (not supported yet — would need signing proxy)

## How Authentication Works

```
                        ┌──────────────────────────┐
                        │     .env file             │
                        │                           │
                        │  POLYMARKET_API_KEY       │
                        │  POLYMARKET_API_SECRET ───┼──► HMAC-SHA256 request signing
                        │  POLYMARKET_PASSPHRASE    │    (CLOB API auth headers)
                        │                           │
                        │  PRIVATE_KEY ─────────────┼──► EIP-712 order signing
                        │                           │    (Polygon on-chain settlement)
                        └──────────────────────────┘

CLOB API auth (every request):
  signature = HMAC-SHA256(api_secret, "timestamp\nmethod\npath\nbody")

Order signing (trade placement only):
  EIP-712 typed data → keccak256 → ECDSA sign with private_key
  Signed on Polygon (chain 137) for CTF Exchange contract
```

## Troubleshooting

| Error | Cause | Fix |
|---|---|---|
| `POLYMARKET_API_KEY env var required` | Missing `.env` or missing key | Check `.env` exists and has all 4 values |
| `failed to fetch fee rate` | API key doesn't have read permissions | Regenerate API key with full permissions |
| `order rejected` | Insufficient USDC balance or allowance | Fund wallet with USDC on Polygon, approve CTF Exchange |
| `order placement failed` | Network error or CLOB downtime | Check Polymarket status, retry later |
| `bad json` from Telegram | Invalid bot token | Verify token with `@BotFather`, regenerate if needed |
