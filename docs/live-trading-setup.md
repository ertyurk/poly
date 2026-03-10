# Live Trading Setup

Step-by-step guide to configure secrets and credentials for real order execution on Polymarket.

Paper trading (`--paper-trade`) requires **none** of this — it uses public endpoints only.

## Prerequisites

- A Polymarket account (Google sign-up or external wallet)
- USDC on Polygon for trading capital
- Your wallet's private key (see below)

## Step 1: Export Your Wallet Private Key

The bot uses the official Polymarket Rust SDK (`polymarket-client-sdk`), which derives all API credentials (API key, secret, passphrase) from your private key automatically via L1 authentication. You only need the private key.

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
- The SDK derives your wallet address and API credentials from this key locally.
- Orders are signed locally — the private key is never sent over the network.

## Step 2: Create the `.env` File

Copy the example file and fill in your values:

```bash
cp .env.example .env
```

Edit `.env`:

```bash
# Logging (optional, defaults to info)
RUST_LOG=polymarket_bot=info

# Wallet private key (from Step 1) — hex, with or without 0x prefix
PRIVATE_KEY=abcdef1234567890abcdef1234567890abcdef1234567890abcdef1234567890
```

The bot loads `.env` automatically via `dotenvy` on startup.

## Step 3: Switch Config to Live Mode

In `config.toml`, change the mode:

```toml
[general]
mode = "live"
```

Or simply run without `--paper-trade`:

```bash
cargo run -- --asset btc --bankroll 1000
```

When `--paper-trade` is omitted, the bot reads `PRIVATE_KEY` on startup, authenticates with the Polymarket SDK, and will fail immediately if the key is missing or invalid.

## Step 4: (Optional) Telegram Notifications

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
                        │  PRIVATE_KEY ─────────────┼──► SDK L1 auth (EIP-191 sign)
                        │                           │    derives API key/secret/passphrase
                        │                           │    automatically on connect()
                        │                           │
                        │                           │──► EIP-712 order signing
                        │                           │    (Polygon on-chain settlement)
                        └──────────────────────────┘

On startup (LiveTrader::connect):
  1. Sign L1 auth message with private key → derive API credentials
  2. SDK stores credentials internally for all subsequent requests

Order placement:
  SDK builds order → EIP-712 sign → POST /order with derived auth headers
```

## Troubleshooting

| Error | Cause | Fix |
|---|---|---|
| `PRIVATE_KEY env var required` | Missing `.env` or missing key | Check `.env` exists with `PRIVATE_KEY` |
| `polymarket auth failed` | Invalid private key or network error | Verify key is correct hex, check internet |
| `order rejected` | Insufficient USDC balance or allowance | Fund wallet with USDC on Polygon |
| `order placement failed` | Network error or CLOB downtime | Check Polymarket status, retry later |
| `bad json` from Telegram | Invalid bot token | Verify token with `@BotFather`, regenerate if needed |
