use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Config {
    pub general: General,
    pub bankroll: Bankroll,
    pub strategy: Strategy,
    pub markets: Markets,
    pub binance: Binance,
    pub polymarket: Polymarket,
    pub writer: Writer,
    pub telegram: Option<Telegram>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct General {
    pub mode: String,
    pub log_level: String,
    pub db_path: String,
    pub db_retention_days: u32,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Bankroll {
    pub initial: f64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Strategy {
    pub tau_min: f64,
    pub kelly_fraction: f64,
    pub max_volume_pct: f64,
    pub min_confidence: f64,
    pub liquidity_b: f64,
    /// Maximum fraction of bankroll risked per trade (hard cap).
    /// 0.10 = max 10% of bankroll per position.
    pub max_bet_fraction: f64,
    /// Maximum fraction of bankroll committed across ALL open positions.
    /// 0.50 = max 50% of bankroll at risk at any time.
    pub max_total_exposure: f64,
    /// Maximum bid-ask spread to accept when discovering markets.
    /// Markets wider than this are skipped as illiquid.
    /// 0.03 = 3¢ spread (tight), 0.10 = 10¢ (loose).
    #[serde(default = "default_max_spread")]
    pub max_spread: f64,
    #[serde(default)]
    pub adapt: Adapt,
    pub decay: Decay,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Adapt {
    #[serde(default = "default_w_zscore")]
    pub w_zscore: f64,
    #[serde(default = "default_w_ofi")]
    pub w_ofi: f64,
    #[serde(default = "default_w_cross")]
    pub w_cross: f64,
    #[serde(default = "default_w_volume")]
    pub w_volume: f64,
    #[serde(default = "default_min_confidence_quiet")]
    pub min_confidence_quiet: f64,
    #[serde(default = "default_min_confidence_normal")]
    pub min_confidence_normal: f64,
    #[serde(default = "default_min_confidence_hot")]
    pub min_confidence_hot: f64,
    #[serde(default = "default_late_window_pct")]
    pub late_window_pct: f64,
    #[serde(default = "default_late_window_kelly_mult")]
    pub late_window_kelly_mult: f64,
}

impl Default for Adapt {
    fn default() -> Self {
        Self {
            w_zscore: 0.50,
            w_ofi: 0.25,
            w_cross: 0.15,
            w_volume: 0.10,
            min_confidence_quiet: 0.10,
            min_confidence_normal: 0.15,
            min_confidence_hot: 0.25,
            late_window_pct: 0.70,
            late_window_kelly_mult: 1.5,
        }
    }
}

fn default_w_zscore() -> f64 {
    0.50
}
fn default_w_ofi() -> f64 {
    0.25
}
fn default_w_cross() -> f64 {
    0.15
}
fn default_w_volume() -> f64 {
    0.10
}
fn default_min_confidence_quiet() -> f64 {
    0.10
}
fn default_min_confidence_normal() -> f64 {
    0.15
}
fn default_min_confidence_hot() -> f64 {
    0.25
}
fn default_late_window_pct() -> f64 {
    0.70
}
fn default_late_window_kelly_mult() -> f64 {
    1.5
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Decay {
    pub spot_lambda: f64,
    pub news_lambda: f64,
    pub social_lambda: f64,
    pub onchain_lambda: f64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Markets {
    pub enabled: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Binance {
    pub ws_url: String,
    pub streams: Vec<String>,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Polymarket {
    pub clob_url: String,
    pub ws_url: String,
    pub gamma_url: String,
    pub poll_interval_secs: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Writer {
    pub batch_size: usize,
    pub flush_interval_ms: u64,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Telegram {
    pub bot_token: String,
    pub chat_id: String,
    pub enabled: bool,
    /// Send a P&L summary every N minutes (default: 30).
    #[serde(default = "default_summary_interval")]
    pub summary_interval_mins: u64,
}

const fn default_summary_interval() -> u64 {
    30
}

fn default_max_spread() -> f64 {
    0.03
}

impl Config {
    pub fn load(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)?;
        let config: Self = toml::from_str(&content)?;
        Ok(config)
    }
}
