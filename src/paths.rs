use std::path::PathBuf;

const APP_DIR: &str = ".polymarket";
const DEFAULT_CONFIG: &str = "config.toml";
const DEFAULT_DB: &str = "data.db";

pub struct AppPaths {
    pub root: PathBuf,
    pub config: PathBuf,
    pub db: PathBuf,
}

impl AppPaths {
    pub fn resolve(config_override: Option<&str>, db_override: Option<&str>) -> Self {
        let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let root = home.join(APP_DIR);

        let config = config_override
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join(DEFAULT_CONFIG));

        let db = db_override
            .map(PathBuf::from)
            .unwrap_or_else(|| root.join(DEFAULT_DB));

        Self { root, config, db }
    }

    pub fn ensure_dirs(&self) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.root)
    }

    pub fn ensure_config(&self) -> Result<(), Box<dyn std::error::Error>> {
        self.ensure_dirs()?;
        if !self.config.exists() {
            std::fs::write(&self.config, DEFAULT_CONFIG_CONTENT)?;
            tracing::info!(
                path = %self.config.display(),
                "created default config"
            );
        }
        Ok(())
    }

    pub fn db_str(&self) -> String {
        self.db.to_string_lossy().to_string()
    }

    pub fn config_str(&self) -> String {
        self.config.to_string_lossy().to_string()
    }
}

const DEFAULT_CONFIG_CONTENT: &str = r#"# ============================================================================
# Polymarket Trading Bot Configuration
# ============================================================================
# Location: ~/.polymarket/config.toml
# Database:  ~/.polymarket/data.db

[general]
mode = "paper"
log_level = "info"
db_retention_days = 7

[bankroll]
initial = 100.0

[strategy]
tau_min = 0.03
kelly_fraction = 0.25
max_volume_pct = 0.02
min_confidence = 0.40
liquidity_b = 100000.0
max_bet_fraction = 0.10
max_total_exposure = 0.50
min_displacement_pct = 0.15
max_spread = 0.03
midpoint_ema_tau_secs = 45.0

[strategy.adapt]
w_zscore = 0.50
w_ofi = 0.25
w_cross = 0.15
w_volume = 0.10
min_confidence_quiet = 0.10
min_confidence_normal = 0.15
min_confidence_hot = 0.25
late_window_pct = 0.70
late_window_kelly_mult = 1.5

[strategy.decay]
spot_lambda = 0.00230
news_lambda = 0.00019
social_lambda = 0.00039
onchain_lambda = 0.000096

[markets]
enabled = ["BTC_5m", "BTC_15m", "ETH_5m", "ETH_15m"]

[binance]
ws_url = "wss://stream.binance.com:9443/ws"
streams = ["btcusdt@trade", "ethusdt@trade"]

[polymarket]
clob_url = "https://clob.polymarket.com"
ws_url = "wss://ws-subscriptions-clob.polymarket.com"
gamma_url = "https://gamma-api.polymarket.com"
poll_interval_secs = 30

[writer]
batch_size = 100
flush_interval_ms = 500

[execution]
gtd_expiry_secs = 7
max_signal_age_secs = 20
fok_price_bump = 0.03
gtd_price_bump = 0.01
min_time_before_resolution_secs = 60
order_poll_interval_secs = 1

[weather]
poll_interval_secs = 1800
max_forecast_horizon_hours = 36
edge_threshold = 0.03
max_tail_price = 0.10
tail_buckets = 3
"#;
