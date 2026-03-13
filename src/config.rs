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
    #[serde(default)]
    pub execution: Execution,
    #[serde(default)]
    pub weather: Weather,
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct General {
    pub mode: String,
    pub log_level: String,
    #[serde(default = "default_db_path")]
    pub db_path: String,
    pub db_retention_days: u32,
}

fn default_db_path() -> String {
    let home = dirs::home_dir().unwrap_or_else(|| std::path::PathBuf::from("."));
    home.join(".polymarket-bot")
        .join("data.db")
        .to_string_lossy()
        .to_string()
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
    /// Minimum spot displacement from open (%) to consider trading.
    /// Filters out noise moves. 0.15 = 0.15% (~$120 on $80K BTC).
    #[serde(default = "default_min_displacement_pct")]
    pub min_displacement_pct: f64,
    /// EMA smoothing time constant (seconds) for the market agreement midpoint filter.
    /// Prevents brief order-book flash crashes from bypassing the filter.
    /// Half-life ≈ tau * ln(2). Default 45s → half-life ≈ 31s.
    #[serde(default = "default_midpoint_ema_tau")]
    pub midpoint_ema_tau_secs: f64,
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

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Execution {
    /// GTD order expiry in seconds (Phase 1 duration).
    #[serde(default = "default_gtd_expiry")]
    pub gtd_expiry_secs: u64,
    /// Maximum signal age (seconds) before FOK fallback is skipped.
    #[serde(default = "default_max_signal_age")]
    pub max_signal_age_secs: u64,
    /// Price bump (in price units) for FOK fallback above/below the book.
    #[serde(default = "default_fok_price_bump")]
    pub fok_price_bump: f64,
    /// Minimum seconds before market resolution to place any order.
    #[serde(default = "default_min_time_before_resolution")]
    pub min_time_before_resolution_secs: u64,
    /// How often (seconds) to poll order status while GTD is resting.
    #[serde(default = "default_order_poll_interval")]
    pub order_poll_interval_secs: u64,
    /// Price bump (in price units) added to GTD limit price for faster fills.
    /// Bumps buy price above best_ask to increase instant fill probability.
    #[serde(default = "default_gtd_price_bump")]
    pub gtd_price_bump: f64,
}

fn default_gtd_expiry() -> u64 {
    15
}
fn default_max_signal_age() -> u64 {
    20
}
fn default_fok_price_bump() -> f64 {
    0.01
}
fn default_min_time_before_resolution() -> u64 {
    60
}
fn default_order_poll_interval() -> u64 {
    3
}
fn default_gtd_price_bump() -> f64 {
    0.01
}

impl Default for Execution {
    fn default() -> Self {
        Self {
            gtd_expiry_secs: default_gtd_expiry(),
            max_signal_age_secs: default_max_signal_age(),
            fok_price_bump: default_fok_price_bump(),
            min_time_before_resolution_secs: default_min_time_before_resolution(),
            order_poll_interval_secs: default_order_poll_interval(),
            gtd_price_bump: default_gtd_price_bump(),
        }
    }
}

#[derive(Debug, Deserialize, Serialize, Clone)]
pub struct Weather {
    #[serde(default = "default_weather_poll_interval")]
    pub poll_interval_secs: u64,
    #[serde(default = "default_max_forecast_horizon")]
    pub max_forecast_horizon_hours: u64,
    #[serde(default = "default_edge_threshold")]
    pub edge_threshold: f64,
    #[serde(default = "default_max_tail_price")]
    pub max_tail_price: f64,
    #[serde(default = "default_tail_buckets")]
    pub tail_buckets: u8,
}

impl Default for Weather {
    fn default() -> Self {
        Self {
            poll_interval_secs: default_weather_poll_interval(),
            max_forecast_horizon_hours: default_max_forecast_horizon(),
            edge_threshold: default_edge_threshold(),
            max_tail_price: default_max_tail_price(),
            tail_buckets: default_tail_buckets(),
        }
    }
}

const fn default_weather_poll_interval() -> u64 {
    1800
}
const fn default_max_forecast_horizon() -> u64 {
    36
}
const fn default_edge_threshold() -> f64 {
    0.03
}
const fn default_max_tail_price() -> f64 {
    0.10
}
const fn default_tail_buckets() -> u8 {
    3
}

fn default_min_displacement_pct() -> f64 {
    0.15
}

fn default_max_spread() -> f64 {
    0.03
}

fn default_midpoint_ema_tau() -> f64 {
    45.0
}

impl Config {
    pub fn load(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| format!("failed to read config at {path}: {e}"))?;
        let config: Self = toml::from_str(&content)
            .map_err(|e| format!("failed to parse config at {path}: {e}"))?;
        Ok(config)
    }
}
