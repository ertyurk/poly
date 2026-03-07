# Polymarket Paper-Trading Bot Implementation Plan

> **For Claude:** REQUIRED SUB-SKILL: Use superpowers:executing-plans to implement this plan task-by-task.

**Goal:** Build a Rust paper-trading bot that connects to Binance and Polymarket WebSockets, runs Bayesian signal processing, makes fee-aware trading decisions with half-Kelly sizing, and logs everything to SQLite for dashboard reporting.

**Architecture:** Single-binary actor-style monolith. Five tokio tasks (actors) connected via mpsc channels: Data Ingestion -> Signal Engine -> Decision Engine -> Paper Executor, with a SQLite Writer receiving fire-and-forget events from all actors. See `docs/plans/2026-03-07-polymarket-bot-design.md`.

**Tech Stack:** Rust, tokio, tokio-tungstenite, polymarket-client-sdk, rusqlite (WAL mode), serde, toml, tracing.

---

### Task 1: Project Scaffolding

**Files:**
- Create: `Cargo.toml`
- Create: `config.toml`
- Create: `src/main.rs`
- Create: `src/config.rs`
- Create: `src/types.rs`
- Create: `.gitignore`
- Create: `data/.gitkeep`

**Step 1: Initialize cargo project**

Run: `cargo init --name polymarket-bot /Users/meer/Developer/prvt/trade`

**Step 2: Write Cargo.toml**

```toml
[package]
name = "polymarket-bot"
version = "0.1.0"
edition = "2021"

[dependencies]
tokio = { version = "1", features = ["full"] }
tokio-tungstenite = { version = "0.24", features = ["native-tls"] }
futures-util = "0.3"
rusqlite = { version = "0.31", features = ["bundled"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
toml = "0.8"
tracing = "0.1"
tracing-subscriber = { version = "0.3", features = ["env-filter"] }
chrono = { version = "0.4", features = ["serde"] }
reqwest = { version = "0.12", features = ["json"] }
url = "2"

[dev-dependencies]
tempfile = "3"
approx = "0.5"
```

Note: We are NOT using `polymarket-client-sdk` as a dependency initially. We'll make direct REST/WS calls via `reqwest` and `tokio-tungstenite` — this gives us full control and avoids version compatibility surprises. We can swap in the SDK later if needed.

**Step 3: Write .gitignore**

```
/target
data/bot.db
data/bot.db-wal
data/bot.db-shm
.env
```

**Step 4: Write config.toml**

Use the exact config from the design doc (see design doc "Configuration" section).

**Step 5: Write src/config.rs**

```rust
use serde::Deserialize;

#[derive(Debug, Deserialize, Clone)]
pub struct Config {
    pub general: General,
    pub bankroll: Bankroll,
    pub strategy: Strategy,
    pub markets: Markets,
    pub binance: Binance,
    pub polymarket: Polymarket,
    pub writer: Writer,
}

#[derive(Debug, Deserialize, Clone)]
pub struct General {
    pub mode: String,
    pub log_level: String,
    pub db_path: String,
    pub db_retention_days: u32,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Bankroll {
    pub initial: f64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Strategy {
    pub tau_min: f64,
    pub kelly_fraction: f64,
    pub max_volume_pct: f64,
    pub min_confidence: f64,
    pub liquidity_b: f64,
    pub decay: Decay,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Decay {
    pub spot_lambda: f64,
    pub news_lambda: f64,
    pub social_lambda: f64,
    pub onchain_lambda: f64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Markets {
    pub enabled: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Binance {
    pub ws_url: String,
    pub streams: Vec<String>,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Polymarket {
    pub clob_url: String,
    pub ws_url: String,
    pub gamma_url: String,
    pub poll_interval_secs: u64,
    pub fee_refresh_secs: u64,
}

#[derive(Debug, Deserialize, Clone)]
pub struct Writer {
    pub batch_size: usize,
    pub flush_interval_ms: u64,
}

impl Config {
    pub fn load(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = toml::from_str(&content)?;
        Ok(config)
    }
}
```

**Step 6: Write src/types.rs**

```rust
use serde::Serialize;
use std::time::Instant;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Asset {
    BTC,
    ETH,
}

impl Asset {
    pub fn as_str(&self) -> &'static str {
        match self {
            Asset::BTC => "BTC",
            Asset::ETH => "ETH",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
pub enum Window {
    FiveMin,
    FifteenMin,
}

impl Window {
    pub fn as_str(&self) -> &'static str {
        match self {
            Window::FiveMin => "5m",
            Window::FifteenMin => "15m",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Side {
    Yes,
    No,
}

impl Side {
    pub fn as_str(&self) -> &'static str {
        match self {
            Side::Yes => "YES",
            Side::No => "NO",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum Outcome {
    Win,
    Loss,
}

impl Outcome {
    pub fn as_str(&self) -> &'static str {
        match self {
            Outcome::Win => "WIN",
            Outcome::Loss => "LOSS",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum SkipReason {
    InsufficientEdge,
    FeeTooHigh,
    VolumeCapHit,
    LowConfidence,
    BankrollDepleted,
}

impl SkipReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            SkipReason::InsufficientEdge => "INSUFFICIENT_EDGE",
            SkipReason::FeeTooHigh => "FEE_TOO_HIGH",
            SkipReason::VolumeCapHit => "VOLUME_CAP_HIT",
            SkipReason::LowConfidence => "LOW_CONFIDENCE",
            SkipReason::BankrollDepleted => "BANKROLL_DEPLETED",
        }
    }
}

// Timestamps are unix microseconds (i64)
pub type TsMicros = i64;

pub fn now_micros() -> TsMicros {
    chrono::Utc::now().timestamp_micros()
}

// --- Messages passed between actors ---

#[derive(Debug, Clone)]
pub struct SpotPrice {
    pub asset: Asset,
    pub price: f64,
    pub ts: TsMicros,
}

#[derive(Debug, Clone)]
pub struct MarketState {
    pub market_id: String,
    pub asset: Asset,
    pub window: Window,
    pub token_yes: String,
    pub token_no: String,
    pub best_bid: f64,
    pub best_ask: f64,
    pub midpoint: f64,
    pub resolution_ts: TsMicros,
    pub open_ts: TsMicros,
    pub open_price: Option<f64>,
    pub volume_24h: f64,
}

#[derive(Debug, Clone)]
pub struct FeeScheduleEntry {
    pub prob_low: f64,
    pub prob_high: f64,
    pub fee_bps: f64,
}

#[derive(Debug, Clone)]
pub struct FeeUpdate {
    pub window: Window,
    pub schedule: Vec<FeeScheduleEntry>,
}

#[derive(Debug, Clone)]
pub struct Signal {
    pub market_id: String,
    pub p_hat: f64,
    pub confidence: f64,
    pub prior: f64,
    pub n_observations: u32,
    pub ts: TsMicros,
}

#[derive(Debug, Clone)]
pub struct TradeDecision {
    pub market_id: String,
    pub side: Side,
    pub size: f64,
    pub price: f64,
    pub edge: f64,
    pub effective_edge: f64,
    pub fee_rate: f64,
    pub kelly_fraction: f64,
    pub ts: TsMicros,
}

#[derive(Debug, Clone)]
pub struct NoTrade {
    pub market_id: String,
    pub edge: f64,
    pub effective_edge: f64,
    pub fee_rate: f64,
    pub reason: SkipReason,
    pub ts: TsMicros,
}

#[derive(Debug, Clone)]
pub struct TradeResult {
    pub decision_id: i64,
    pub market_id: String,
    pub side: Side,
    pub entry_price: f64,
    pub size: f64,
    pub fee_paid: f64,
    pub outcome: Outcome,
    pub pnl: f64,
    pub bankroll_after: f64,
    pub entry_ts: TsMicros,
    pub resolved_ts: TsMicros,
}

// --- Unified event for SQLite Writer ---

#[derive(Debug, Clone)]
pub enum DbEvent {
    SpotPrice(SpotPrice),
    Market(MarketState),
    BookSnapshot {
        market_id: String,
        best_bid: f64,
        best_ask: f64,
        midpoint: f64,
        spread: f64,
        ts: TsMicros,
    },
    Signal(Signal),
    Decision(TradeDecision),
    Skip(NoTrade),
    Trade(TradeResult),
    ConfigSnapshot {
        config_json: String,
        ts: TsMicros,
    },
}
```

**Step 7: Write initial src/main.rs**

```rust
mod config;
mod types;

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive("polymarket_bot=info".parse()?))
        .init();

    let config = config::Config::load("config.toml")?;
    tracing::info!(mode = %config.general.mode, "loaded config");

    // Placeholder — actors will be wired here
    tracing::info!("polymarket-bot starting in {} mode", config.general.mode);

    tokio::signal::ctrl_c().await?;
    tracing::info!("shutting down");
    Ok(())
}
```

**Step 8: Verify it compiles and runs**

Run: `cd /Users/meer/Developer/prvt/trade && cargo build`
Expected: compiles with no errors

Run: `cargo run`
Expected: prints "loaded config" and "polymarket-bot starting in paper mode", waits for ctrl+c

**Step 9: Commit**

```bash
git init
git add Cargo.toml Cargo.lock config.toml .gitignore src/ data/.gitkeep docs/
git commit -m "feat: project scaffolding with config, types, and main entry point"
```

---

### Task 2: Math Module — Pure Functions

All formulas from the research docs. Pure functions, no IO, easy to TDD.

**Files:**
- Create: `src/math/mod.rs`
- Create: `src/math/lmsr.rs`
- Create: `src/math/bayesian.rs`
- Create: `src/math/kelly.rs`
- Create: `src/math/decay.rs`
- Create: `tests/math_tests.rs`

**Step 1: Write tests/math_tests.rs**

```rust
use approx::assert_relative_eq;

// --- LMSR tests ---

#[test]
fn test_lmsr_cost_binary_equal_quantities() {
    // C(q) = b * ln(sum(e^(q_i/b)))
    // With q = [0, 0], b = 100000: C = 100000 * ln(2) ≈ 69314.72
    let cost = polymarket_bot::math::lmsr::cost(&[0.0, 0.0], 100_000.0);
    assert_relative_eq!(cost, 69_314.718, epsilon = 1.0);
}

#[test]
fn test_lmsr_price_equal_quantities() {
    // At q = [0, 0], both prices should be 0.5
    let prices = polymarket_bot::math::lmsr::prices(&[0.0, 0.0], 100_000.0);
    assert_relative_eq!(prices[0], 0.5, epsilon = 1e-10);
    assert_relative_eq!(prices[1], 0.5, epsilon = 1e-10);
}

#[test]
fn test_lmsr_prices_sum_to_one() {
    let prices = polymarket_bot::math::lmsr::prices(&[1000.0, 500.0], 100_000.0);
    assert_relative_eq!(prices[0] + prices[1], 1.0, epsilon = 1e-10);
}

#[test]
fn test_lmsr_trade_cost() {
    // Cost to move q_0 from 0 to 1000 with q = [0, 0], b = 100000
    let cost = polymarket_bot::math::lmsr::trade_cost(&[0.0, 0.0], 0, 1000.0, 100_000.0);
    assert!(cost > 0.0);
}

#[test]
fn test_lmsr_optimal_trade_size() {
    // doc page 3 eq 8: delta* = b * ln(p_hat/p * (1-p)/(1-p_hat))
    // p_hat=0.70, p=0.50, b=100000 -> delta* ≈ 84730
    let size = polymarket_bot::math::lmsr::optimal_trade_size(0.70, 0.50, 100_000.0);
    assert_relative_eq!(size, 84_730.0, epsilon = 100.0);
}

// --- Bayesian tests ---

#[test]
fn test_bayesian_log_update_no_data() {
    // With no observations, posterior = prior
    let log_prior = (0.5_f64).ln();
    let posterior = polymarket_bot::math::bayesian::log_posterior(log_prior, &[]);
    assert_relative_eq!(posterior.exp(), 0.5, epsilon = 1e-10);
}

#[test]
fn test_bayesian_log_update_positive_evidence() {
    // Positive log-likelihoods should increase posterior
    let log_prior = (0.5_f64).ln();
    let log_likelihoods = vec![0.1, 0.2, 0.1]; // positive evidence for UP
    let posterior = polymarket_bot::math::bayesian::log_posterior(log_prior, &log_likelihoods);
    assert!(posterior.exp() > 0.5);
}

#[test]
fn test_bayesian_probability_from_return() {
    // A positive return should give p > 0.5
    let p = polymarket_bot::math::bayesian::probability_from_return(0.001, 0.005);
    assert!(p > 0.5);
    // A negative return should give p < 0.5
    let p = polymarket_bot::math::bayesian::probability_from_return(-0.001, 0.005);
    assert!(p < 0.5);
}

// --- Kelly tests ---

#[test]
fn test_full_kelly_even_odds() {
    // f* = (p_hat - p) / (1 - p), doc page 4 eq 5
    // p_hat=0.60, p=0.50 -> f* = 0.10 / 0.50 = 0.20
    let f = polymarket_bot::math::kelly::full_kelly(0.60, 0.50);
    assert_relative_eq!(f, 0.20, epsilon = 1e-10);
}

#[test]
fn test_half_kelly() {
    // f_prod = (p_hat - p) / 2(1 - p), doc page 4 eq 6
    // p_hat=0.60, p=0.50 -> f_prod = 0.10 / 1.0 = 0.10
    let f = polymarket_bot::math::kelly::half_kelly(0.60, 0.50);
    assert_relative_eq!(f, 0.10, epsilon = 1e-10);
}

#[test]
fn test_kelly_no_edge_returns_zero() {
    let f = polymarket_bot::math::kelly::half_kelly(0.50, 0.50);
    assert_relative_eq!(f, 0.0, epsilon = 1e-10);
}

#[test]
fn test_kelly_negative_edge_returns_zero() {
    // If p_hat < p, no bet
    let f = polymarket_bot::math::kelly::half_kelly(0.40, 0.50);
    assert_relative_eq!(f, 0.0, epsilon = 1e-10);
}

#[test]
fn test_position_size() {
    // half_kelly=0.10, bankroll=100000 -> position=$10000
    let size = polymarket_bot::math::kelly::position_size(0.60, 0.50, 0.5, 100_000.0);
    assert_relative_eq!(size, 10_000.0, epsilon = 1.0);
}

// --- Decay tests ---

#[test]
fn test_decay_weight_at_zero() {
    // w = exp(-lambda * 0) = 1.0
    let w = polymarket_bot::math::decay::weight(0.00230, 0.0);
    assert_relative_eq!(w, 1.0, epsilon = 1e-10);
}

#[test]
fn test_decay_weight_at_half_life() {
    // half-life for lambda=0.00230 is ln(2)/0.00230 ≈ 301.3 seconds
    // at half-life, weight should be 0.5
    let half_life = (2.0_f64).ln() / 0.00230;
    let w = polymarket_bot::math::decay::weight(0.00230, half_life);
    assert_relative_eq!(w, 0.5, epsilon = 1e-3);
}

#[test]
fn test_decay_weight_at_one_hour() {
    // lambda=0.00230, t=3600s -> w = exp(-0.00230 * 3600) ≈ 0.000255
    let w = polymarket_bot::math::decay::weight(0.00230, 3600.0);
    assert!(w < 0.001);
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test --test math_tests`
Expected: compilation error, modules don't exist yet

**Step 3: Write src/math/mod.rs**

```rust
pub mod lmsr;
pub mod bayesian;
pub mod kelly;
pub mod decay;
```

**Step 4: Write src/math/lmsr.rs**

```rust
/// LMSR cost function: C(q) = b * ln(sum(e^(q_i/b)))
/// Doc page 1, equation (1)
pub fn cost(quantities: &[f64], b: f64) -> f64 {
    let max_q = quantities.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let sum_exp: f64 = quantities.iter().map(|q| ((q - max_q) / b).exp()).sum();
    b * (max_q / b + sum_exp.ln())
}

/// Price function (softmax): p_i = e^(q_i/b) / sum(e^(q_j/b))
/// Doc page 1, equation (3)
pub fn prices(quantities: &[f64], b: f64) -> Vec<f64> {
    let max_q = quantities.iter().cloned().fold(f64::NEG_INFINITY, f64::max);
    let exps: Vec<f64> = quantities.iter().map(|q| ((q - max_q) / b).exp()).collect();
    let sum_exp: f64 = exps.iter().sum();
    exps.iter().map(|e| e / sum_exp).collect()
}

/// Cost of a trade: moving outcome i from q_i to q_i + delta
/// Doc page 1, equation (4)
pub fn trade_cost(quantities: &[f64], outcome: usize, delta: f64, b: f64) -> f64 {
    let mut new_q = quantities.to_vec();
    new_q[outcome] += delta;
    cost(&new_q, b) - cost(quantities, b)
}

/// Optimal trade size for binary markets
/// Doc page 3, equation (8): delta* = b * ln(p_hat/p * (1-p)/(1-p_hat))
pub fn optimal_trade_size(p_hat: f64, p_market: f64, b: f64) -> f64 {
    let ratio = (p_hat / p_market) * ((1.0 - p_market) / (1.0 - p_hat));
    b * ratio.ln()
}

/// Effective spread cost for minimum trade
/// Doc page 3, equation (6): c_i = p_i(1-p_i)/b * delta_min
pub fn effective_spread(p: f64, b: f64, delta_min: f64) -> f64 {
    p * (1.0 - p) / b * delta_min
}
```

**Step 5: Write src/math/bayesian.rs**

```rust
/// Compute log-posterior from log-prior and log-likelihoods
/// Doc page 2, equation (3): log P(H|D) = log P(H) + sum(log P(D_k|H)) - log Z
/// Returns unnormalized log-posterior (caller normalizes for binary case)
pub fn log_posterior(log_prior: f64, log_likelihoods: &[f64]) -> f64 {
    log_prior + log_likelihoods.iter().sum::<f64>()
}

/// Convert a price return to a directional probability estimate
/// Models returns as Gaussian: P(UP) = Phi(return / volatility)
/// Uses logistic approximation to normal CDF for speed
pub fn probability_from_return(ret: f64, volatility: f64) -> f64 {
    if volatility <= 0.0 {
        return 0.5;
    }
    let z = ret / volatility;
    // Logistic approximation: Phi(z) ≈ 1 / (1 + exp(-1.7 * z))
    1.0 / (1.0 + (-1.7 * z).exp())
}

/// Normalize two log-posteriors (binary case) to probabilities
/// Returns (p_up, p_down) where p_up + p_down = 1
pub fn normalize_binary(log_up: f64, log_down: f64) -> (f64, f64) {
    let max_log = log_up.max(log_down);
    let exp_up = (log_up - max_log).exp();
    let exp_down = (log_down - max_log).exp();
    let total = exp_up + exp_down;
    (exp_up / total, exp_down / total)
}
```

**Step 6: Write src/math/kelly.rs**

```rust
/// Full Kelly fraction for binary prediction markets
/// Doc page 4, equation (5): f* = (p_hat - p) / (1 - p)
pub fn full_kelly(p_hat: f64, p_market: f64) -> f64 {
    let edge = p_hat - p_market;
    if edge <= 0.0 {
        return 0.0;
    }
    edge / (1.0 - p_market)
}

/// Half-Kelly (production rule)
/// Doc page 4, equation (6): f_prod = (p_hat - p) / 2(1 - p)
pub fn half_kelly(p_hat: f64, p_market: f64) -> f64 {
    full_kelly(p_hat, p_market) / 2.0
}

/// Fractional Kelly with configurable fraction
pub fn fractional_kelly(p_hat: f64, p_market: f64, fraction: f64) -> f64 {
    full_kelly(p_hat, p_market) * fraction
}

/// Position size in dollars
/// kelly_fraction_config is the fraction to use (e.g. 0.5 for half-Kelly)
pub fn position_size(p_hat: f64, p_market: f64, kelly_fraction_config: f64, bankroll: f64) -> f64 {
    let f = fractional_kelly(p_hat, p_market, kelly_fraction_config);
    f * bankroll
}
```

**Step 7: Write src/math/decay.rs**

```rust
/// Exponential decay weight
/// Doc page 4, equation (7): w_k = exp(-lambda * (t - t_k))
/// `elapsed_secs` is (t - t_k) in seconds
pub fn weight(lambda: f64, elapsed_secs: f64) -> f64 {
    (-lambda * elapsed_secs).exp()
}

/// Weighted average of multiple source estimates (precision weighting)
/// Doc page 4, equation (8): p_hat_fused = sum(w_s * p_hat_s / sigma_s^2) / sum(w_s / sigma_s^2)
pub fn fuse_estimates(estimates: &[(f64, f64, f64)]) -> f64 {
    // Each tuple: (p_hat, variance, weight)
    let (num, den) = estimates.iter().fold((0.0, 0.0), |(n, d), &(p_hat, var, w)| {
        if var <= 0.0 {
            return (n, d);
        }
        (n + w * p_hat / var, d + w / var)
    });
    if den <= 0.0 {
        return 0.5; // fallback to uninformed
    }
    num / den
}
```

**Step 8: Add math module to main.rs and make it a library**

Add to `src/main.rs` (top):
```rust
mod math;
```

Also create `src/lib.rs`:
```rust
pub mod math;
pub mod types;
pub mod config;
```

This allows tests to import via `polymarket_bot::math::*`.

**Step 9: Run tests**

Run: `cargo test --test math_tests`
Expected: all 13 tests pass

**Step 10: Commit**

```bash
git add src/math/ src/lib.rs tests/
git commit -m "feat: math module — LMSR, Bayesian, Kelly, decay with tests"
```

---

### Task 3: Database Module

**Files:**
- Create: `src/db/mod.rs`
- Create: `src/db/schema.rs`
- Create: `src/db/queries.rs`
- Create: `tests/db_tests.rs`

**Step 1: Write tests/db_tests.rs**

```rust
use tempfile::NamedTempFile;
use polymarket_bot::db;
use polymarket_bot::types::*;

#[test]
fn test_init_creates_tables() {
    let tmp = NamedTempFile::new().unwrap();
    let conn = db::init(tmp.path().to_str().unwrap()).unwrap();
    // Verify tables exist by querying sqlite_master
    let count: i32 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('spot_prices','markets','book_snapshots','signals','decisions','trades','config_snapshots')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 7);
}

#[test]
fn test_wal_mode_enabled() {
    let tmp = NamedTempFile::new().unwrap();
    let conn = db::init(tmp.path().to_str().unwrap()).unwrap();
    let mode: String = conn
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
}

#[test]
fn test_insert_spot_price() {
    let tmp = NamedTempFile::new().unwrap();
    let conn = db::init(tmp.path().to_str().unwrap()).unwrap();
    let sp = SpotPrice { asset: Asset::BTC, price: 85000.0, ts: 1000000 };
    db::queries::insert_spot_price(&conn, &sp).unwrap();
    let count: i32 = conn.query_row("SELECT COUNT(*) FROM spot_prices", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 1);
}

#[test]
fn test_insert_decision_and_trade() {
    let tmp = NamedTempFile::new().unwrap();
    let conn = db::init(tmp.path().to_str().unwrap()).unwrap();

    // Insert market first (FK)
    let ms = MarketState {
        market_id: "test-mkt-1".into(),
        asset: Asset::BTC,
        window: Window::FiveMin,
        token_yes: "tok_yes".into(),
        token_no: "tok_no".into(),
        best_bid: 0.48,
        best_ask: 0.52,
        midpoint: 0.50,
        resolution_ts: 2000000,
        open_ts: 1000000,
        open_price: Some(85000.0),
        volume_24h: 50000.0,
    };
    db::queries::insert_market(&conn, &ms).unwrap();

    let dec = TradeDecision {
        market_id: "test-mkt-1".into(),
        side: Side::Yes,
        size: 1000.0,
        price: 0.50,
        edge: 0.10,
        effective_edge: 0.07,
        fee_rate: 0.03,
        kelly_fraction: 0.10,
        ts: 1500000,
    };
    let decision_id = db::queries::insert_decision(&conn, &dec).unwrap();
    assert!(decision_id > 0);

    let tr = TradeResult {
        decision_id,
        market_id: "test-mkt-1".into(),
        side: Side::Yes,
        entry_price: 0.50,
        size: 1000.0,
        fee_paid: 30.0,
        outcome: Outcome::Win,
        pnl: 470.0,
        bankroll_after: 100_470.0,
        entry_ts: 1500000,
        resolved_ts: 2000000,
    };
    db::queries::insert_trade(&conn, &tr).unwrap();

    let count: i32 = conn.query_row("SELECT COUNT(*) FROM trades", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 1);
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test --test db_tests`
Expected: compilation error

**Step 3: Write src/db/mod.rs**

```rust
pub mod schema;
pub mod queries;

use rusqlite::Connection;

pub fn init(path: &str) -> Result<Connection, rusqlite::Error> {
    let conn = Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "busy_timeout", 5000)?;
    schema::create_tables(&conn)?;
    Ok(conn)
}
```

**Step 4: Write src/db/schema.rs**

```rust
use rusqlite::Connection;

pub fn create_tables(conn: &Connection) -> Result<(), rusqlite::Error> {
    conn.execute_batch(include_str!("../../schema.sql"))
}
```

Also create `schema.sql` at project root with the exact SQL from the design doc (all CREATE TABLE and CREATE INDEX statements).

**Step 5: Write src/db/queries.rs**

```rust
use rusqlite::{params, Connection};
use crate::types::*;

pub fn insert_spot_price(conn: &Connection, sp: &SpotPrice) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO spot_prices (asset, price, ts) VALUES (?1, ?2, ?3)",
        params![sp.asset.as_str(), sp.price, sp.ts],
    )?;
    Ok(())
}

pub fn insert_market(conn: &Connection, ms: &MarketState) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT OR IGNORE INTO markets (market_id, asset, window, token_yes, token_no, open_ts, resolution_ts, open_price)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
        params![
            ms.market_id,
            ms.asset.as_str(),
            ms.window.as_str(),
            ms.token_yes,
            ms.token_no,
            ms.open_ts,
            ms.resolution_ts,
            ms.open_price,
        ],
    )?;
    Ok(())
}

pub fn update_market_resolution(conn: &Connection, market_id: &str, resolved_side: &str) -> Result<(), rusqlite::Error> {
    conn.execute(
        "UPDATE markets SET resolved_side = ?1 WHERE market_id = ?2",
        params![resolved_side, market_id],
    )?;
    Ok(())
}

pub fn insert_book_snapshot(
    conn: &Connection,
    market_id: &str,
    best_bid: f64,
    best_ask: f64,
    midpoint: f64,
    spread: f64,
    ts: TsMicros,
) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO book_snapshots (market_id, best_bid, best_ask, midpoint, spread, ts) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![market_id, best_bid, best_ask, midpoint, spread, ts],
    )?;
    Ok(())
}

pub fn insert_signal(conn: &Connection, sig: &Signal) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO signals (market_id, p_hat, confidence, prior, n_observations, ts) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![sig.market_id, sig.p_hat, sig.confidence, sig.prior, sig.n_observations, sig.ts],
    )?;
    Ok(())
}

/// Returns the inserted row id (used as decision_id for trades)
pub fn insert_decision(conn: &Connection, dec: &TradeDecision) -> Result<i64, rusqlite::Error> {
    conn.execute(
        "INSERT INTO decisions (market_id, action, side, size, price, edge, effective_edge, fee_rate, kelly_fraction, ts)
         VALUES (?1, 'TRADE', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            dec.market_id,
            dec.side.as_str(),
            dec.size,
            dec.price,
            dec.edge,
            dec.effective_edge,
            dec.fee_rate,
            dec.kelly_fraction,
            dec.ts,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn insert_skip(conn: &Connection, skip: &NoTrade) -> Result<i64, rusqlite::Error> {
    conn.execute(
        "INSERT INTO decisions (market_id, action, edge, effective_edge, fee_rate, skip_reason, ts)
         VALUES (?1, 'SKIP', ?2, ?3, ?4, ?5, ?6)",
        params![
            skip.market_id,
            skip.edge,
            skip.effective_edge,
            skip.fee_rate,
            skip.reason.as_str(),
            skip.ts,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

pub fn insert_trade(conn: &Connection, tr: &TradeResult) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO trades (decision_id, market_id, side, entry_price, size, fee_paid, outcome, pnl, bankroll_after, entry_ts, resolved_ts)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            tr.decision_id,
            tr.market_id,
            tr.side.as_str(),
            tr.entry_price,
            tr.size,
            tr.fee_paid,
            tr.outcome.as_str(),
            tr.pnl,
            tr.bankroll_after,
            tr.entry_ts,
            tr.resolved_ts,
        ],
    )?;
    Ok(())
}

pub fn insert_config_snapshot(conn: &Connection, json: &str, ts: TsMicros) -> Result<(), rusqlite::Error> {
    conn.execute(
        "INSERT INTO config_snapshots (config_json, ts) VALUES (?1, ?2)",
        params![json, ts],
    )?;
    Ok(())
}

pub fn prune_old_data(conn: &Connection, cutoff_ts: TsMicros) -> Result<usize, rusqlite::Error> {
    let mut total = 0;
    total += conn.execute("DELETE FROM spot_prices WHERE ts < ?1", params![cutoff_ts])?;
    total += conn.execute("DELETE FROM book_snapshots WHERE ts < ?1", params![cutoff_ts])?;
    Ok(total)
}
```

**Step 6: Create schema.sql at project root**

Use the exact SQL from the design doc — all 7 CREATE TABLE + all CREATE INDEX statements.

**Step 7: Add db module to src/lib.rs**

```rust
pub mod math;
pub mod types;
pub mod config;
pub mod db;
```

**Step 8: Run tests**

Run: `cargo test --test db_tests`
Expected: all 4 tests pass

**Step 9: Commit**

```bash
git add src/db/ src/lib.rs schema.sql tests/db_tests.rs
git commit -m "feat: SQLite database module with WAL, schema, and query helpers"
```

---

### Task 4: SQLite Writer Actor

**Files:**
- Create: `src/actors/mod.rs`
- Create: `src/actors/writer.rs`
- Create: `tests/writer_tests.rs`

**Step 1: Write tests/writer_tests.rs**

```rust
use tempfile::NamedTempFile;
use tokio::sync::mpsc;
use polymarket_bot::types::*;
use polymarket_bot::actors::writer::WriterActor;

#[tokio::test]
async fn test_writer_processes_spot_price() {
    let tmp = NamedTempFile::new().unwrap();
    let path = tmp.path().to_str().unwrap().to_string();
    let (tx, rx) = mpsc::channel::<DbEvent>(100);

    let handle = tokio::spawn(async move {
        let mut actor = WriterActor::new(&path, 10, 100).unwrap();
        actor.run(rx).await;
    });

    tx.send(DbEvent::SpotPrice(SpotPrice {
        asset: Asset::BTC,
        price: 85000.0,
        ts: now_micros(),
    })).await.unwrap();

    drop(tx); // close channel, actor will flush and exit
    handle.await.unwrap();

    // Verify data was written
    let conn = polymarket_bot::db::init(tmp.path().to_str().unwrap()).unwrap();
    let count: i32 = conn.query_row("SELECT COUNT(*) FROM spot_prices", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 1);
}

#[tokio::test]
async fn test_writer_batches_writes() {
    let tmp = NamedTempFile::new().unwrap();
    let path = tmp.path().to_str().unwrap().to_string();
    let (tx, rx) = mpsc::channel::<DbEvent>(200);

    let handle = tokio::spawn(async move {
        let mut actor = WriterActor::new(&path, 50, 5000).unwrap();
        actor.run(rx).await;
    });

    // Send 100 events rapidly
    for i in 0..100 {
        tx.send(DbEvent::SpotPrice(SpotPrice {
            asset: Asset::ETH,
            price: 3000.0 + i as f64,
            ts: now_micros(),
        })).await.unwrap();
    }

    drop(tx);
    handle.await.unwrap();

    let conn = polymarket_bot::db::init(tmp.path().to_str().unwrap()).unwrap();
    let count: i32 = conn.query_row("SELECT COUNT(*) FROM spot_prices", [], |r| r.get(0)).unwrap();
    assert_eq!(count, 100);
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test --test writer_tests`
Expected: compilation error

**Step 3: Write src/actors/mod.rs**

```rust
pub mod writer;
```

**Step 4: Write src/actors/writer.rs**

```rust
use rusqlite::Connection;
use tokio::sync::mpsc;
use tokio::time::{self, Duration};
use crate::db;
use crate::types::*;

pub struct WriterActor {
    conn: Connection,
    batch_size: usize,
    flush_interval: Duration,
    buffer: Vec<DbEvent>,
}

impl WriterActor {
    pub fn new(db_path: &str, batch_size: usize, flush_interval_ms: u64) -> Result<Self, rusqlite::Error> {
        let conn = db::init(db_path)?;
        Ok(Self {
            conn,
            batch_size,
            flush_interval: Duration::from_millis(flush_interval_ms),
            buffer: Vec::with_capacity(batch_size),
        })
    }

    pub async fn run(&mut self, mut rx: mpsc::Receiver<DbEvent>) {
        let mut interval = time::interval(self.flush_interval);

        loop {
            tokio::select! {
                Some(event) = rx.recv() => {
                    self.buffer.push(event);
                    if self.buffer.len() >= self.batch_size {
                        self.flush();
                    }
                }
                _ = interval.tick() => {
                    if !self.buffer.is_empty() {
                        self.flush();
                    }
                }
                else => {
                    // Channel closed, flush remaining
                    if !self.buffer.is_empty() {
                        self.flush();
                    }
                    break;
                }
            }
        }
    }

    fn flush(&mut self) {
        let events: Vec<DbEvent> = self.buffer.drain(..).collect();
        if let Err(e) = self.write_batch(&events) {
            tracing::error!(error = %e, count = events.len(), "failed to write batch");
        } else {
            tracing::debug!(count = events.len(), "flushed batch");
        }
    }

    fn write_batch(&mut self, events: &[DbEvent]) -> Result<(), rusqlite::Error> {
        let tx = self.conn.transaction()?;
        for event in events {
            match event {
                DbEvent::SpotPrice(sp) => {
                    db::queries::insert_spot_price(&tx, sp)?;
                }
                DbEvent::Market(ms) => {
                    db::queries::insert_market(&tx, ms)?;
                }
                DbEvent::BookSnapshot { market_id, best_bid, best_ask, midpoint, spread, ts } => {
                    db::queries::insert_book_snapshot(&tx, market_id, *best_bid, *best_ask, *midpoint, *spread, *ts)?;
                }
                DbEvent::Signal(sig) => {
                    db::queries::insert_signal(&tx, sig)?;
                }
                DbEvent::Decision(dec) => {
                    db::queries::insert_decision(&tx, dec)?;
                }
                DbEvent::Skip(skip) => {
                    db::queries::insert_skip(&tx, skip)?;
                }
                DbEvent::Trade(tr) => {
                    db::queries::insert_trade(&tx, tr)?;
                }
                DbEvent::ConfigSnapshot { config_json, ts } => {
                    db::queries::insert_config_snapshot(&tx, config_json, *ts)?;
                }
            }
        }
        tx.commit()
    }
}
```

**Step 5: Add actors module to src/lib.rs**

```rust
pub mod math;
pub mod types;
pub mod config;
pub mod db;
pub mod actors;
```

**Step 6: Run tests**

Run: `cargo test --test writer_tests`
Expected: both tests pass

**Step 7: Commit**

```bash
git add src/actors/ tests/writer_tests.rs src/lib.rs
git commit -m "feat: SQLite writer actor with batching and flush interval"
```

---

### Task 5: Data Ingestion Actor — Binance WebSocket

**Files:**
- Create: `src/actors/ingest.rs`
- Modify: `src/actors/mod.rs`

This actor has real network dependencies, so we test it via integration test against live Binance (optional, can skip in CI) and unit test the message parsing.

**Step 1: Write Binance message parser test in tests/ingest_tests.rs**

```rust
use polymarket_bot::actors::ingest::parse_binance_trade;
use polymarket_bot::types::Asset;

#[test]
fn test_parse_binance_trade_btc() {
    let msg = r#"{"e":"trade","E":1709800000000,"s":"BTCUSDT","t":123,"p":"85000.50","q":"0.1","T":1709800000000,"m":false}"#;
    let result = parse_binance_trade(msg).unwrap();
    assert_eq!(result.asset, Asset::BTC);
    assert!((result.price - 85000.50).abs() < 0.01);
}

#[test]
fn test_parse_binance_trade_eth() {
    let msg = r#"{"e":"trade","E":1709800000000,"s":"ETHUSDT","t":456,"p":"3200.25","q":"1.0","T":1709800000000,"m":false}"#;
    let result = parse_binance_trade(msg).unwrap();
    assert_eq!(result.asset, Asset::ETH);
    assert!((result.price - 3200.25).abs() < 0.01);
}

#[test]
fn test_parse_binance_trade_unknown_symbol() {
    let msg = r#"{"e":"trade","E":1709800000000,"s":"SOLUSDT","t":789,"p":"100.0","q":"1.0","T":1709800000000,"m":false}"#;
    assert!(parse_binance_trade(msg).is_none());
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test --test ingest_tests`
Expected: compilation error

**Step 3: Write src/actors/ingest.rs**

```rust
use futures_util::{SinkExt, StreamExt};
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;
use crate::config::Config;
use crate::types::*;

#[derive(Debug, serde::Deserialize)]
struct BinanceTrade {
    #[serde(rename = "s")]
    symbol: String,
    #[serde(rename = "p")]
    price: String,
    #[serde(rename = "T")]
    trade_time: i64,
}

pub fn parse_binance_trade(msg: &str) -> Option<SpotPrice> {
    let trade: BinanceTrade = serde_json::from_str(msg).ok()?;
    let asset = match trade.symbol.as_str() {
        "BTCUSDT" => Asset::BTC,
        "ETHUSDT" => Asset::ETH,
        _ => return None,
    };
    let price: f64 = trade.price.parse().ok()?;
    Some(SpotPrice {
        asset,
        price,
        ts: trade.trade_time * 1000, // ms to micros
    })
}

pub struct IngestActor {
    config: Config,
}

impl IngestActor {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub async fn run(
        &self,
        spot_tx: mpsc::Sender<SpotPrice>,
        db_tx: mpsc::Sender<DbEvent>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        // Build combined stream URL
        let streams = self.config.binance.streams.join("/");
        let url = format!("{}/{}", self.config.binance.ws_url, streams);

        let mut retry_count = 0u32;

        loop {
            if *shutdown.borrow() {
                break;
            }

            tracing::info!(url = %url, "connecting to Binance WebSocket");
            match connect_async(&url).await {
                Ok((ws_stream, _)) => {
                    retry_count = 0;
                    tracing::info!("connected to Binance");
                    let (_, mut read) = ws_stream.split();

                    loop {
                        tokio::select! {
                            msg = read.next() => {
                                match msg {
                                    Some(Ok(Message::Text(text))) => {
                                        if let Some(sp) = parse_binance_trade(&text) {
                                            let _ = db_tx.try_send(DbEvent::SpotPrice(sp.clone()));
                                            let _ = spot_tx.try_send(sp);
                                        }
                                    }
                                    Some(Ok(Message::Close(_))) | None => {
                                        tracing::warn!("Binance WS closed");
                                        break;
                                    }
                                    Some(Err(e)) => {
                                        tracing::error!(error = %e, "Binance WS error");
                                        break;
                                    }
                                    _ => {}
                                }
                            }
                            _ = shutdown.changed() => {
                                tracing::info!("ingest actor shutting down");
                                return;
                            }
                        }
                    }
                }
                Err(e) => {
                    retry_count += 1;
                    if retry_count > 5 {
                        tracing::error!("Binance WS: max retries exceeded, giving up");
                        return;
                    }
                    let backoff = std::time::Duration::from_secs(2u64.pow(retry_count));
                    tracing::warn!(error = %e, retry = retry_count, backoff_secs = ?backoff, "Binance WS connection failed");
                    tokio::time::sleep(backoff).await;
                }
            }
        }
    }
}
```

**Step 4: Add to src/actors/mod.rs**

```rust
pub mod writer;
pub mod ingest;
```

**Step 5: Run tests**

Run: `cargo test --test ingest_tests`
Expected: all 3 tests pass

**Step 6: Commit**

```bash
git add src/actors/ingest.rs src/actors/mod.rs tests/ingest_tests.rs
git commit -m "feat: data ingestion actor with Binance WS and message parsing"
```

---

### Task 6: Signal Engine Actor

**Files:**
- Create: `src/actors/signal.rs`
- Modify: `src/actors/mod.rs`
- Create: `tests/signal_tests.rs`

**Step 1: Write tests/signal_tests.rs**

```rust
use polymarket_bot::actors::signal::MarketWindow;
use polymarket_bot::types::*;

#[test]
fn test_market_window_initial_prior() {
    let w = MarketWindow::new("mkt-1".into(), 0.00230);
    assert!((w.p_hat() - 0.5).abs() < 1e-10);
}

#[test]
fn test_market_window_update_positive() {
    let mut w = MarketWindow::new("mkt-1".into(), 0.00230);
    // Simulate price going up: positive return
    w.update(0.002, 0.005, 0.0); // return, volatility, elapsed_secs
    assert!(w.p_hat() > 0.5);
}

#[test]
fn test_market_window_update_negative() {
    let mut w = MarketWindow::new("mkt-1".into(), 0.00230);
    w.update(-0.002, 0.005, 0.0);
    assert!(w.p_hat() < 0.5);
}

#[test]
fn test_market_window_multiple_updates_converge() {
    let mut w = MarketWindow::new("mkt-1".into(), 0.00230);
    // 10 positive returns should push p_hat well above 0.5
    for _ in 0..10 {
        w.update(0.001, 0.005, 0.0);
    }
    assert!(w.p_hat() > 0.7);
}

#[test]
fn test_market_window_decay_reduces_old_signal() {
    let mut w = MarketWindow::new("mkt-1".into(), 0.00230);
    w.update(0.003, 0.005, 0.0); // strong signal at t=0
    let p_early = w.p_hat();

    // Now add a neutral observation 10 minutes later
    w.update(0.0, 0.005, 600.0); // 600s elapsed
    let p_late = w.p_hat();

    // The old strong signal should have decayed, pulling p_hat back toward 0.5
    assert!(p_late < p_early);
}

#[test]
fn test_market_window_observation_count() {
    let mut w = MarketWindow::new("mkt-1".into(), 0.00230);
    assert_eq!(w.n_observations(), 0);
    w.update(0.001, 0.005, 0.0);
    w.update(0.001, 0.005, 1.0);
    assert_eq!(w.n_observations(), 2);
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test --test signal_tests`
Expected: compilation error

**Step 3: Write src/actors/signal.rs**

```rust
use tokio::sync::mpsc;
use std::collections::HashMap;
use crate::config::Config;
use crate::math::{bayesian, decay};
use crate::types::*;

/// Tracks Bayesian state for one market window
pub struct MarketWindow {
    pub market_id: String,
    lambda: f64,
    log_prior_up: f64,
    log_prior_down: f64,
    observations: Vec<(f64, f64)>, // (log_likelihood_ratio, elapsed_secs)
    count: u32,
}

impl MarketWindow {
    pub fn new(market_id: String, lambda: f64) -> Self {
        Self {
            market_id,
            lambda,
            log_prior_up: 0.0_f64.ln_1p() - (2.0_f64).ln(), // ln(0.5)
            log_prior_down: 0.0_f64.ln_1p() - (2.0_f64).ln(),
            observations: Vec::with_capacity(300),
            count: 0,
        }
    }

    /// Update with a new price observation
    /// `ret` = price return since window open
    /// `vol` = estimated volatility at this timescale
    /// `elapsed` = seconds since this observation was generated (for decay)
    pub fn update(&mut self, ret: f64, vol: f64, elapsed: f64) {
        let p_up = bayesian::probability_from_return(ret, vol);
        let p_down = 1.0 - p_up;

        // Log-likelihood ratio (how much this observation favors UP vs DOWN)
        let ll_up = p_up.ln();
        let ll_down = p_down.ln();

        self.observations.push((ll_up - ll_down, elapsed));
        self.count += 1;

        // Keep ring buffer bounded
        if self.observations.len() > 300 {
            self.observations.remove(0);
        }
    }

    /// Current probability estimate for UP
    pub fn p_hat(&self) -> f64 {
        if self.observations.is_empty() {
            return 0.5;
        }

        // Compute decay-weighted log-likelihood sum
        let latest_time = self.observations.last().map(|o| o.1).unwrap_or(0.0);
        let mut weighted_ll_sum = 0.0;

        for &(ll_ratio, obs_elapsed) in &self.observations {
            let age = latest_time - obs_elapsed;
            let w = decay::weight(self.lambda, age.max(0.0));
            weighted_ll_sum += w * ll_ratio;
        }

        // log_up = log(0.5) + weighted_sum, log_down = log(0.5) - weighted_sum
        let log_up = self.log_prior_up + weighted_ll_sum;
        let log_down = self.log_prior_down;

        let (p_up, _) = bayesian::normalize_binary(log_up, log_down);
        p_up
    }

    /// Confidence: how far from 0.5 (0.0 = no info, 1.0 = certain)
    pub fn confidence(&self) -> f64 {
        (self.p_hat() - 0.5).abs() * 2.0
    }

    pub fn n_observations(&self) -> u32 {
        self.count
    }
}

pub struct SignalActor {
    config: Config,
    windows: HashMap<String, MarketWindow>,
}

impl SignalActor {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            windows: HashMap::new(),
        }
    }

    pub async fn run(
        &mut self,
        mut spot_rx: mpsc::Receiver<SpotPrice>,
        mut market_rx: mpsc::Receiver<MarketState>,
        signal_tx: mpsc::Sender<Signal>,
        db_tx: mpsc::Sender<DbEvent>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        // Track open prices per market for return calculation
        let mut open_prices: HashMap<String, f64> = HashMap::new();
        // Track which asset each market is for
        let mut market_assets: HashMap<String, Asset> = HashMap::new();

        loop {
            tokio::select! {
                Some(ms) = market_rx.recv() => {
                    // Register new market window
                    if !self.windows.contains_key(&ms.market_id) {
                        let lambda = self.config.strategy.decay.spot_lambda;
                        self.windows.insert(
                            ms.market_id.clone(),
                            MarketWindow::new(ms.market_id.clone(), lambda),
                        );
                        if let Some(open_price) = ms.open_price {
                            open_prices.insert(ms.market_id.clone(), open_price);
                        }
                        market_assets.insert(ms.market_id.clone(), ms.asset);
                        tracing::debug!(market_id = %ms.market_id, "registered new market window");
                    }
                }
                Some(sp) = spot_rx.recv() => {
                    // Update all windows matching this asset
                    let matching_ids: Vec<String> = market_assets
                        .iter()
                        .filter(|(_, a)| **a == sp.asset)
                        .map(|(id, _)| id.clone())
                        .collect();

                    for market_id in matching_ids {
                        if let (Some(window), Some(&open_price)) =
                            (self.windows.get_mut(&market_id), open_prices.get(&market_id))
                        {
                            let ret = (sp.price - open_price) / open_price;
                            // Estimate vol from window type (rough: 5min BTC vol ~ 0.003)
                            let vol = 0.003;
                            window.update(ret, vol, 0.0);

                            let signal = Signal {
                                market_id: market_id.clone(),
                                p_hat: window.p_hat(),
                                confidence: window.confidence(),
                                prior: 0.5,
                                n_observations: window.n_observations(),
                                ts: sp.ts,
                            };

                            let _ = db_tx.try_send(DbEvent::Signal(signal.clone()));
                            let _ = signal_tx.try_send(signal);
                        }
                    }
                }
                _ = shutdown.changed() => {
                    tracing::info!("signal actor shutting down");
                    break;
                }
                else => break,
            }
        }
    }

    /// Remove windows past their resolution time
    pub fn cleanup(&mut self, now: TsMicros) {
        // Called periodically from run loop — omitted for brevity but
        // removes entries from self.windows older than 10s past resolution
    }
}
```

**Step 4: Add to src/actors/mod.rs**

```rust
pub mod writer;
pub mod ingest;
pub mod signal;
```

**Step 5: Run tests**

Run: `cargo test --test signal_tests`
Expected: all 6 tests pass

**Step 6: Commit**

```bash
git add src/actors/signal.rs src/actors/mod.rs tests/signal_tests.rs
git commit -m "feat: signal engine actor with Bayesian updates and decay"
```

---

### Task 7: Decision Engine Actor

**Files:**
- Create: `src/actors/decision.rs`
- Modify: `src/actors/mod.rs`
- Create: `tests/decision_tests.rs`

**Step 1: Write tests/decision_tests.rs**

```rust
use polymarket_bot::actors::decision::*;
use polymarket_bot::types::*;

#[test]
fn test_compute_edge() {
    let edge = compute_edge(0.65, 0.50);
    assert!((edge - 0.15).abs() < 1e-10);
}

#[test]
fn test_compute_edge_negative() {
    let edge = compute_edge(0.40, 0.50);
    assert!((edge - (-0.10)).abs() < 1e-10);
}

#[test]
fn test_fee_at_50_percent_15m() {
    // Dynamic fee at 50% on 15m markets ≈ 3.15%
    let fee = lookup_fee(0.50, Window::FifteenMin, &default_fee_schedule_15m());
    assert!(fee > 0.03 && fee < 0.04);
}

#[test]
fn test_fee_at_extremes_lower() {
    let fee = lookup_fee(0.10, Window::FifteenMin, &default_fee_schedule_15m());
    assert!(fee < 0.02); // lower fee at extremes
}

#[test]
fn test_effective_edge_positive() {
    let eff = effective_edge(0.15, 0.02);
    assert!((eff - 0.13).abs() < 1e-10);
}

#[test]
fn test_effective_edge_fee_exceeds() {
    let eff = effective_edge(0.02, 0.0315);
    assert!(eff < 0.0); // fee kills edge
}

#[test]
fn test_entry_gate_passes() {
    // edge=0.15, tau_min=0.05, c_i ≈ 0.0025 (at p=0.5, b=100000, delta_min=1)
    let passes = check_entry_gate(0.15, 0.05, 0.5, 100_000.0, 1.0);
    assert!(passes);
}

#[test]
fn test_entry_gate_fails_small_edge() {
    let passes = check_entry_gate(0.03, 0.05, 0.5, 100_000.0, 1.0);
    assert!(!passes);
}

#[test]
fn test_stealth_cap() {
    // 0.02 * 50000 = 1000
    let capped = apply_stealth_cap(5000.0, 50_000.0, 0.02);
    assert!((capped - 1000.0).abs() < 1e-10);
}

#[test]
fn test_stealth_cap_no_change() {
    let capped = apply_stealth_cap(500.0, 50_000.0, 0.02);
    assert!((capped - 500.0).abs() < 1e-10);
}

#[test]
fn test_decide_trade_full_pipeline() {
    let result = decide(
        0.65,       // p_hat
        0.50,       // p_market
        0.01,       // fee_rate
        0.05,       // tau_min
        100_000.0,  // b
        0.5,        // kelly_fraction
        100_000.0,  // bankroll
        50_000.0,   // volume_24h
        0.02,       // max_volume_pct
        0.60,       // min_confidence
        0.30,       // confidence
        "mkt-1",
    );
    // edge=0.15, fee=0.01, eff_edge=0.14, confidence=0.30 < 0.60 -> SKIP
    assert!(result.is_err()); // NoTrade due to low confidence

    // With sufficient confidence
    let result = decide(
        0.65, 0.50, 0.01, 0.05, 100_000.0, 0.5, 100_000.0, 50_000.0, 0.02, 0.20, 0.30, "mkt-1",
    );
    assert!(result.is_ok());
    let dec = result.unwrap();
    assert_eq!(dec.side, Side::Yes);
    assert!(dec.size > 0.0);
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test --test decision_tests`
Expected: compilation error

**Step 3: Write src/actors/decision.rs**

```rust
use tokio::sync::mpsc;
use std::collections::HashMap;
use crate::config::Config;
use crate::math::{lmsr, kelly};
use crate::types::*;

pub fn compute_edge(p_hat: f64, p_market: f64) -> f64 {
    p_hat - p_market
}

pub fn effective_edge(edge_abs: f64, fee_rate: f64) -> f64 {
    edge_abs - fee_rate
}

pub fn default_fee_schedule_15m() -> Vec<FeeScheduleEntry> {
    // Approximation of Polymarket's dynamic fee schedule for 15m markets
    vec![
        FeeScheduleEntry { prob_low: 0.0, prob_high: 0.10, fee_bps: 100.0 },
        FeeScheduleEntry { prob_low: 0.10, prob_high: 0.20, fee_bps: 150.0 },
        FeeScheduleEntry { prob_low: 0.20, prob_high: 0.35, fee_bps: 200.0 },
        FeeScheduleEntry { prob_low: 0.35, prob_high: 0.65, fee_bps: 315.0 },
        FeeScheduleEntry { prob_low: 0.65, prob_high: 0.80, fee_bps: 200.0 },
        FeeScheduleEntry { prob_low: 0.80, prob_high: 0.90, fee_bps: 150.0 },
        FeeScheduleEntry { prob_low: 0.90, prob_high: 1.0, fee_bps: 100.0 },
    ]
}

pub fn lookup_fee(p_market: f64, window: Window, schedule: &[FeeScheduleEntry]) -> f64 {
    match window {
        Window::FiveMin => {
            // 5m markets: use schedule if available, fallback to flat 1%
            for entry in schedule {
                if p_market >= entry.prob_low && p_market < entry.prob_high {
                    return entry.fee_bps / 10_000.0;
                }
            }
            0.01
        }
        Window::FifteenMin => {
            for entry in schedule {
                if p_market >= entry.prob_low && p_market < entry.prob_high {
                    return entry.fee_bps / 10_000.0;
                }
            }
            0.0315 // worst case default
        }
    }
}

pub fn check_entry_gate(edge_abs: f64, tau_min: f64, p: f64, b: f64, delta_min: f64) -> bool {
    let c_i = lmsr::effective_spread(p, b, delta_min);
    edge_abs > tau_min + c_i
}

pub fn apply_stealth_cap(size: f64, volume_24h: f64, max_pct: f64) -> f64 {
    size.min(volume_24h * max_pct)
}

#[allow(clippy::too_many_arguments)]
pub fn decide(
    p_hat: f64,
    p_market: f64,
    fee_rate: f64,
    tau_min: f64,
    b: f64,
    kelly_fraction: f64,
    bankroll: f64,
    volume_24h: f64,
    max_volume_pct: f64,
    min_confidence: f64,
    confidence: f64,
    market_id: &str,
) -> Result<TradeDecision, NoTrade> {
    let ts = now_micros();
    let edge = compute_edge(p_hat, p_market);
    let edge_abs = edge.abs();
    let eff_edge = effective_edge(edge_abs, fee_rate);

    // Check confidence
    if confidence < min_confidence {
        return Err(NoTrade {
            market_id: market_id.into(),
            edge,
            effective_edge: eff_edge,
            fee_rate,
            reason: SkipReason::LowConfidence,
            ts,
        });
    }

    // Check fee kills edge
    if eff_edge <= 0.0 {
        return Err(NoTrade {
            market_id: market_id.into(),
            edge,
            effective_edge: eff_edge,
            fee_rate,
            reason: SkipReason::FeeTooHigh,
            ts,
        });
    }

    // Entry gate
    if !check_entry_gate(eff_edge, tau_min, p_market, b, 1.0) {
        return Err(NoTrade {
            market_id: market_id.into(),
            edge,
            effective_edge: eff_edge,
            fee_rate,
            reason: SkipReason::InsufficientEdge,
            ts,
        });
    }

    // Position sizing
    let optimal_size = lmsr::optimal_trade_size(p_hat, p_market, b).abs();
    let kelly_size = kelly::position_size(p_hat, p_market, kelly_fraction, bankroll);
    let mut size = optimal_size.min(kelly_size);

    // Stealth cap
    let capped = apply_stealth_cap(size, volume_24h, max_volume_pct);
    if capped < size {
        size = capped;
    }

    if size <= 0.0 {
        return Err(NoTrade {
            market_id: market_id.into(),
            edge,
            effective_edge: eff_edge,
            fee_rate,
            reason: SkipReason::InsufficientEdge,
            ts,
        });
    }

    let side = if edge > 0.0 { Side::Yes } else { Side::No };
    let kf = kelly::fractional_kelly(p_hat, p_market, kelly_fraction);

    Ok(TradeDecision {
        market_id: market_id.into(),
        side,
        size,
        price: p_market,
        edge,
        effective_edge: eff_edge,
        fee_rate,
        kelly_fraction: kf,
        ts,
    })
}

pub struct DecisionActor {
    config: Config,
    fee_schedules: HashMap<Window, Vec<FeeScheduleEntry>>,
    market_states: HashMap<String, MarketState>,
}

impl DecisionActor {
    pub fn new(config: Config) -> Self {
        let mut fee_schedules = HashMap::new();
        fee_schedules.insert(Window::FifteenMin, default_fee_schedule_15m());
        // 5m uses same schedule as placeholder; updated via FeeUpdate
        fee_schedules.insert(Window::FiveMin, default_fee_schedule_15m());
        Self {
            config,
            fee_schedules,
            market_states: HashMap::new(),
        }
    }

    pub async fn run(
        &mut self,
        mut signal_rx: mpsc::Receiver<Signal>,
        mut market_rx: mpsc::Receiver<MarketState>,
        mut fee_rx: mpsc::Receiver<FeeUpdate>,
        decision_tx: mpsc::Sender<TradeDecision>,
        db_tx: mpsc::Sender<DbEvent>,
        bankroll: &mut f64,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        loop {
            tokio::select! {
                Some(ms) = market_rx.recv() => {
                    self.market_states.insert(ms.market_id.clone(), ms);
                }
                Some(fu) = fee_rx.recv() => {
                    self.fee_schedules.insert(fu.window, fu.schedule);
                }
                Some(signal) = signal_rx.recv() => {
                    if let Some(ms) = self.market_states.get(&signal.market_id) {
                        let window = ms.window;
                        let schedule = self.fee_schedules.get(&window)
                            .cloned()
                            .unwrap_or_else(|| default_fee_schedule_15m());
                        let fee = lookup_fee(ms.midpoint, window, &schedule);

                        match decide(
                            signal.p_hat,
                            ms.midpoint,
                            fee,
                            self.config.strategy.tau_min,
                            self.config.strategy.liquidity_b,
                            self.config.strategy.kelly_fraction,
                            *bankroll,
                            ms.volume_24h,
                            self.config.strategy.max_volume_pct,
                            self.config.strategy.min_confidence,
                            signal.confidence,
                            &signal.market_id,
                        ) {
                            Ok(dec) => {
                                let _ = db_tx.try_send(DbEvent::Decision(dec.clone()));
                                let _ = decision_tx.try_send(dec);
                            }
                            Err(skip) => {
                                let _ = db_tx.try_send(DbEvent::Skip(skip));
                            }
                        }
                    }
                }
                _ = shutdown.changed() => {
                    tracing::info!("decision actor shutting down");
                    break;
                }
                else => break,
            }
        }
    }
}
```

**Step 4: Add to src/actors/mod.rs**

```rust
pub mod writer;
pub mod ingest;
pub mod signal;
pub mod decision;
```

**Step 5: Run tests**

Run: `cargo test --test decision_tests`
Expected: all 11 tests pass

**Step 6: Commit**

```bash
git add src/actors/decision.rs src/actors/mod.rs tests/decision_tests.rs
git commit -m "feat: decision engine with fee-aware edge gating and half-Kelly sizing"
```

---

### Task 8: Paper Executor Actor

**Files:**
- Create: `src/actors/executor.rs`
- Modify: `src/actors/mod.rs`
- Create: `tests/executor_tests.rs`

**Step 1: Write tests/executor_tests.rs**

```rust
use polymarket_bot::actors::executor::PaperExecutor;
use polymarket_bot::types::*;

#[test]
fn test_executor_fill_win() {
    let mut exec = PaperExecutor::new(100_000.0);

    let dec = TradeDecision {
        market_id: "mkt-1".into(),
        side: Side::Yes,
        size: 1000.0,
        price: 0.50,
        edge: 0.15,
        effective_edge: 0.12,
        fee_rate: 0.03,
        kelly_fraction: 0.10,
        ts: 1000000,
    };

    let best_ask = 0.52; // fillable
    let fill = exec.try_fill(&dec, best_ask, 0.48);
    assert!(fill.is_some());
    assert_eq!(exec.open_positions().len(), 1);
}

#[test]
fn test_executor_fill_rejected_price_slipped() {
    let mut exec = PaperExecutor::new(100_000.0);

    let dec = TradeDecision {
        market_id: "mkt-1".into(),
        side: Side::Yes,
        size: 1000.0,
        price: 0.50,
        edge: 0.15,
        effective_edge: 0.12,
        fee_rate: 0.03,
        kelly_fraction: 0.10,
        ts: 1000000,
    };

    let best_ask = 0.90; // way too high, slipped
    let fill = exec.try_fill(&dec, best_ask, 0.10);
    assert!(fill.is_none());
}

#[test]
fn test_executor_settle_win() {
    let mut exec = PaperExecutor::new(100_000.0);

    let dec = TradeDecision {
        market_id: "mkt-1".into(),
        side: Side::Yes,
        size: 1000.0,
        price: 0.50,
        edge: 0.15,
        effective_edge: 0.12,
        fee_rate: 0.03,
        kelly_fraction: 0.10,
        ts: 1000000,
    };
    exec.try_fill(&dec, 0.52, 0.48);

    let results = exec.settle("mkt-1", Side::Yes, 2000000);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, Outcome::Win);
    assert!(results[0].pnl > 0.0);
    assert!(exec.bankroll() > 100_000.0);
}

#[test]
fn test_executor_settle_loss() {
    let mut exec = PaperExecutor::new(100_000.0);

    let dec = TradeDecision {
        market_id: "mkt-1".into(),
        side: Side::Yes,
        size: 1000.0,
        price: 0.50,
        edge: 0.15,
        effective_edge: 0.12,
        fee_rate: 0.03,
        kelly_fraction: 0.10,
        ts: 1000000,
    };
    exec.try_fill(&dec, 0.52, 0.48);

    let results = exec.settle("mkt-1", Side::No, 2000000); // resolved NO, we bet YES
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, Outcome::Loss);
    assert!(results[0].pnl < 0.0);
    assert!(exec.bankroll() < 100_000.0);
}
```

**Step 2: Run tests to verify they fail**

Run: `cargo test --test executor_tests`
Expected: compilation error

**Step 3: Write src/actors/executor.rs**

```rust
use tokio::sync::mpsc;
use std::collections::HashMap;
use crate::types::*;

#[derive(Debug, Clone)]
struct OpenPosition {
    decision_id: i64,
    market_id: String,
    side: Side,
    entry_price: f64,
    size: f64,
    fee_rate: f64,
    entry_ts: TsMicros,
}

pub struct PaperExecutor {
    bankroll: f64,
    positions: Vec<OpenPosition>,
    next_decision_id: i64,
}

impl PaperExecutor {
    pub fn new(initial_bankroll: f64) -> Self {
        Self {
            bankroll: initial_bankroll,
            positions: Vec::new(),
            next_decision_id: 1,
        }
    }

    pub fn bankroll(&self) -> f64 {
        self.bankroll
    }

    pub fn open_positions(&self) -> &[OpenPosition] {
        // Workaround: return empty slice reference won't work with private type
        // Tests use len() check via this method
        &self.positions
    }

    /// Try to fill a trade decision against the current order book
    /// Returns the simulated decision_id if filled
    pub fn try_fill(&mut self, dec: &TradeDecision, best_ask: f64, best_bid: f64) -> Option<i64> {
        // For YES buys, check against best_ask
        // For NO buys, check against best_ask for NO side
        let fill_price = match dec.side {
            Side::Yes => best_ask,
            Side::No => 1.0 - best_bid, // NO price = 1 - YES bid
        };

        // Reject if price slipped more than 10% from expected
        if (fill_price - dec.price).abs() / dec.price > 0.10 {
            tracing::debug!(
                market_id = %dec.market_id,
                expected = dec.price,
                actual = fill_price,
                "fill rejected: price slipped"
            );
            return None;
        }

        let id = self.next_decision_id;
        self.next_decision_id += 1;

        self.positions.push(OpenPosition {
            decision_id: id,
            market_id: dec.market_id.clone(),
            side: dec.side,
            entry_price: fill_price,
            size: dec.size,
            fee_rate: dec.fee_rate,
            entry_ts: dec.ts,
        });

        tracing::info!(
            market_id = %dec.market_id,
            side = ?dec.side,
            size = dec.size,
            price = fill_price,
            "paper fill"
        );

        Some(id)
    }

    /// Settle all positions for a resolved market
    pub fn settle(&mut self, market_id: &str, resolved_side: Side, resolved_ts: TsMicros) -> Vec<TradeResult> {
        let (to_settle, remaining): (Vec<_>, Vec<_>) = self.positions
            .drain(..)
            .partition(|p| p.market_id == market_id);

        self.positions = remaining;

        let mut results = Vec::new();
        for pos in to_settle {
            let won = pos.side == resolved_side;
            let fee_paid = pos.size * pos.entry_price * pos.fee_rate;
            let gross_pnl = if won {
                pos.size * (1.0 - pos.entry_price) // payout $1, paid entry_price
            } else {
                -(pos.size * pos.entry_price) // lost the entry cost
            };
            let pnl = gross_pnl - fee_paid;
            self.bankroll += pnl;

            let outcome = if won { Outcome::Win } else { Outcome::Loss };

            results.push(TradeResult {
                decision_id: pos.decision_id,
                market_id: pos.market_id,
                side: pos.side,
                entry_price: pos.entry_price,
                size: pos.size,
                fee_paid,
                outcome,
                pnl,
                bankroll_after: self.bankroll,
                entry_ts: pos.entry_ts,
                resolved_ts,
            });

            tracing::info!(
                market = %results.last().unwrap().market_id,
                outcome = ?outcome,
                pnl = pnl,
                bankroll = self.bankroll,
                "position settled"
            );
        }

        results
    }
}
```

**Step 4: Add to src/actors/mod.rs**

```rust
pub mod writer;
pub mod ingest;
pub mod signal;
pub mod decision;
pub mod executor;
```

**Step 5: Run tests**

Run: `cargo test --test executor_tests`
Expected: all 4 tests pass

**Step 6: Commit**

```bash
git add src/actors/executor.rs src/actors/mod.rs tests/executor_tests.rs
git commit -m "feat: paper executor with simulated fills, settlement, and P&L"
```

---

### Task 9: Wire Everything in main.rs

**Files:**
- Modify: `src/main.rs`

**Step 1: Write the full main.rs**

```rust
mod config;
mod types;
mod math;
mod actors;
mod db;

use tokio::sync::{mpsc, watch};
use tracing_subscriber::EnvFilter;
use crate::types::*;
use crate::actors::{writer::WriterActor, ingest::IngestActor, signal::SignalActor, decision::DecisionActor, executor::PaperExecutor};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("polymarket_bot=info".parse()?)
        )
        .init();

    let config = config::Config::load("config.toml")?;
    tracing::info!(mode = %config.general.mode, "loaded config");

    // Ensure data directory exists
    std::fs::create_dir_all("data").ok();

    // Shutdown signal
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // --- Channels ---
    let (db_tx, db_rx) = mpsc::channel::<DbEvent>(10_000);
    let (spot_tx, spot_rx) = mpsc::channel::<SpotPrice>(5_000);
    let (market_tx_signal, market_rx_signal) = mpsc::channel::<MarketState>(100);
    let (market_tx_decision, market_rx_decision) = mpsc::channel::<MarketState>(100);
    let (signal_tx, signal_rx) = mpsc::channel::<Signal>(1_000);
    let (fee_tx, fee_rx) = mpsc::channel::<FeeUpdate>(10);
    let (decision_tx, decision_rx) = mpsc::channel::<TradeDecision>(100);

    // Snapshot config
    let config_json = serde_json::to_string(&serde_json::json!({
        "mode": config.general.mode,
        "bankroll": config.bankroll.initial,
        "tau_min": config.strategy.tau_min,
        "kelly_fraction": config.strategy.kelly_fraction,
        "markets": config.markets.enabled,
    }))?;
    let _ = db_tx.send(DbEvent::ConfigSnapshot {
        config_json,
        ts: now_micros(),
    }).await;

    // --- Spawn actors ---

    // 1. SQLite Writer
    let writer_db_path = config.general.db_path.clone();
    let writer_batch = config.writer.batch_size;
    let writer_flush = config.writer.flush_interval_ms;
    tokio::spawn(async move {
        let mut actor = WriterActor::new(&writer_db_path, writer_batch, writer_flush)
            .expect("failed to init SQLite");
        actor.run(db_rx).await;
    });

    // 2. Data Ingestion (Binance)
    let ingest_config = config.clone();
    let ingest_db_tx = db_tx.clone();
    let ingest_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        let actor = IngestActor::new(ingest_config);
        actor.run(spot_tx, ingest_db_tx, ingest_shutdown).await;
    });

    // 3. Signal Engine
    let signal_config = config.clone();
    let signal_db_tx = db_tx.clone();
    let signal_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        let mut actor = SignalActor::new(signal_config);
        actor.run(spot_rx, market_rx_signal, signal_tx, signal_db_tx, signal_shutdown).await;
    });

    // 4. Decision Engine
    let decision_config = config.clone();
    let decision_db_tx = db_tx.clone();
    let decision_shutdown = shutdown_rx.clone();
    let mut bankroll = config.bankroll.initial;
    tokio::spawn(async move {
        let mut actor = DecisionActor::new(decision_config);
        actor.run(signal_rx, market_rx_decision, fee_rx, decision_tx, decision_db_tx, &mut bankroll, decision_shutdown).await;
    });

    // 5. Paper Executor (runs in main task for now)
    let mut executor = PaperExecutor::new(config.bankroll.initial);
    let exec_db_tx = db_tx.clone();
    let mut exec_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        let mut decision_rx = decision_rx;
        loop {
            tokio::select! {
                Some(dec) = decision_rx.recv() => {
                    // Simulate fill at current price (simplified: use decision price as best_ask)
                    if let Some(_id) = executor.try_fill(&dec, dec.price + 0.01, dec.price - 0.01) {
                        // Position opened, will settle when market resolves
                    }
                }
                _ = exec_shutdown.changed() => {
                    tracing::info!("executor shutting down");
                    break;
                }
                else => break,
            }
        }
    });

    tracing::info!(
        bankroll = config.bankroll.initial,
        markets = ?config.markets.enabled,
        "polymarket-bot running — press Ctrl+C to stop"
    );

    // Wait for shutdown
    tokio::signal::ctrl_c().await?;
    tracing::info!("shutdown signal received");
    let _ = shutdown_tx.send(true);

    // Give actors time to flush
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    tracing::info!("shutdown complete");

    Ok(())
}
```

**Step 2: Verify it compiles**

Run: `cargo build`
Expected: compiles with no errors

**Step 3: Verify it runs (will connect to Binance, log prices)**

Run: `cargo run`
Expected: connects to Binance WS, logs spot prices, Ctrl+C shuts down cleanly.

**Step 4: Commit**

```bash
git add src/main.rs
git commit -m "feat: wire all actors together in main with graceful shutdown"
```

---

### Task 10: Integration Smoke Test

**Files:**
- Create: `tests/integration_test.rs`

**Step 1: Write a smoke test that runs the full pipeline with mock data**

```rust
use tokio::sync::mpsc;
use polymarket_bot::types::*;
use polymarket_bot::actors::signal::MarketWindow;
use polymarket_bot::actors::decision::decide;
use polymarket_bot::actors::executor::PaperExecutor;

#[test]
fn test_full_pipeline_paper_trade() {
    // Simulate: BTC goes up during a 5-min window
    // 1. Signal engine detects upward movement
    let mut window = MarketWindow::new("test-mkt".into(), 0.00230);
    for _ in 0..20 {
        window.update(0.001, 0.003, 0.0); // positive returns
    }
    let p_hat = window.p_hat();
    assert!(p_hat > 0.6, "p_hat should be > 0.6, got {}", p_hat);

    // 2. Decision engine evaluates
    let result = decide(
        p_hat,
        0.50,       // market price
        0.01,       // fee
        0.05,       // tau_min
        100_000.0,  // b
        0.5,        // kelly_fraction
        100_000.0,  // bankroll
        50_000.0,   // volume
        0.02,       // max_vol_pct
        0.10,       // min_confidence (low threshold for test)
        window.confidence(),
        "test-mkt",
    );
    assert!(result.is_ok(), "should decide to trade");
    let dec = result.unwrap();
    assert_eq!(dec.side, Side::Yes);

    // 3. Executor fills and settles
    let mut exec = PaperExecutor::new(100_000.0);
    let fill = exec.try_fill(&dec, 0.52, 0.48);
    assert!(fill.is_some());

    // Market resolves YES (BTC went up)
    let results = exec.settle("test-mkt", Side::Yes, now_micros());
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, Outcome::Win);
    assert!(results[0].pnl > 0.0);
    assert!(exec.bankroll() > 100_000.0);
}

#[test]
fn test_full_pipeline_skip_low_edge() {
    // BTC barely moves — signal engine stays near 0.5
    let mut window = MarketWindow::new("test-mkt-2".into(), 0.00230);
    for _ in 0..5 {
        window.update(0.0001, 0.003, 0.0); // tiny returns
    }
    let p_hat = window.p_hat();

    let result = decide(
        p_hat,
        0.50,
        0.0315,     // high fee (15m market at 50%)
        0.05,
        100_000.0,
        0.5,
        100_000.0,
        50_000.0,
        0.02,
        0.10,
        window.confidence(),
        "test-mkt-2",
    );
    // Should skip — edge < fee
    assert!(result.is_err());
}
```

**Step 2: Run integration test**

Run: `cargo test --test integration_test`
Expected: both tests pass

**Step 3: Run all tests**

Run: `cargo test`
Expected: all tests pass (math, db, writer, ingest, signal, decision, executor, integration)

**Step 4: Commit**

```bash
git add tests/integration_test.rs
git commit -m "feat: integration smoke test for full paper-trading pipeline"
```

---

## Summary

| Task | Component | Key Files | Tests |
|---|---|---|---|
| 1 | Scaffolding | Cargo.toml, config.rs, types.rs, main.rs | compiles + runs |
| 2 | Math module | math/{lmsr,bayesian,kelly,decay}.rs | 13 tests |
| 3 | Database | db/{schema,queries}.rs, schema.sql | 4 tests |
| 4 | SQLite Writer | actors/writer.rs | 2 tests |
| 5 | Data Ingestion | actors/ingest.rs | 3 tests |
| 6 | Signal Engine | actors/signal.rs | 6 tests |
| 7 | Decision Engine | actors/decision.rs | 11 tests |
| 8 | Paper Executor | actors/executor.rs | 4 tests |
| 9 | Wire main.rs | main.rs | compiles + runs |
| 10 | Integration | integration_test.rs | 2 tests |
