use clap::{Parser, Subcommand};
use std::collections::HashSet;

use crate::types::{Asset, Window};

/// Polymarket prediction market trading bot.
///
/// Trade crypto and weather prediction markets on Polymarket with
/// Bayesian signals, Kelly-criterion sizing, and automated execution.
#[derive(Parser, Debug)]
#[command(name = "poly", version, about)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Path to config file (default: ~/.polymarket-bot/config.toml)
    #[arg(long, global = true)]
    pub config: Option<String>,

    /// Path to database file (default: ~/.polymarket-bot/data.db)
    #[arg(long, global = true)]
    pub db_path: Option<String>,
}

#[derive(Subcommand, Debug)]
pub enum Command {
    /// Run crypto prediction market trader (BTC/ETH up/down markets)
    Crypto {
        /// Paper-trade mode: real market data, simulated execution
        #[arg(long, alias = "paper")]
        paper_trade: bool,

        /// Starting bankroll in USD (overrides config.toml)
        #[arg(long)]
        bankroll: Option<f64>,

        /// Asset filter: btc, eth, or all
        #[arg(long, default_value = "all", value_parser = parse_asset_filter)]
        asset: AssetFilter,

        /// Window filter: 5m, 15m, or all
        #[arg(long, default_value = "all", value_parser = parse_window_filter)]
        window: WindowFilter,
    },

    /// Run weather prediction market trader (temperature markets)
    Weather {
        /// Paper-trade mode: real market data, simulated execution
        #[arg(long, alias = "paper")]
        paper_trade: bool,

        /// Starting bankroll in USD (overrides config.toml)
        #[arg(long)]
        bankroll: Option<f64>,
    },

    /// Launch web dashboard (reads from DB, runs in separate terminal)
    Dashboard {
        /// Host interface to bind
        #[arg(long, default_value = "127.0.0.1")]
        host: String,

        /// Port to serve on
        #[arg(long, default_value_t = 3030)]
        port: u16,
    },

    /// Show quick database status report
    Status,

    /// Remove database for clean state
    ResetDb,
}

#[derive(Debug, Clone)]
pub enum AssetFilter {
    Single(Asset),
    All,
}

#[derive(Debug, Clone)]
pub enum WindowFilter {
    Single(Window),
    Set(HashSet<Window>),
    All,
}

impl AssetFilter {
    pub fn matches(&self, asset: Asset) -> bool {
        match self {
            Self::All => true,
            Self::Single(a) => *a == asset,
        }
    }
}

impl WindowFilter {
    pub fn matches(&self, window: Window) -> bool {
        match self {
            Self::All => true,
            Self::Single(w) => *w == window,
            Self::Set(s) => s.contains(&window),
        }
    }

    /// Build a WindowFilter from the `markets.enabled` config entries.
    /// Entries like "BTC_5m", "ETH_15m" → extract unique windows.
    pub fn from_enabled(entries: &[String]) -> Self {
        let windows: HashSet<Window> = entries
            .iter()
            .filter_map(|e| {
                let suffix = e.split('_').nth(1)?;
                match suffix {
                    "5m" => Some(Window::FiveMin),
                    "15m" => Some(Window::FifteenMin),
                    "1h" => Some(Window::Hourly),
                    "1d" => Some(Window::Daily),
                    _ => None,
                }
            })
            .collect();
        if windows.is_empty() {
            Self::All
        } else {
            Self::Set(windows)
        }
    }
}

fn parse_asset_filter(s: &str) -> Result<AssetFilter, String> {
    match s.to_lowercase().as_str() {
        "btc" | "bitcoin" => Ok(AssetFilter::Single(Asset::BTC)),
        "eth" | "ethereum" => Ok(AssetFilter::Single(Asset::ETH)),
        "all" => Ok(AssetFilter::All),
        _ => Err(format!("unknown asset: {s}. Use btc, eth, or all")),
    }
}

fn parse_window_filter(s: &str) -> Result<WindowFilter, String> {
    match s.to_lowercase().as_str() {
        "5m" | "5min" => Ok(WindowFilter::Single(Window::FiveMin)),
        "15m" | "15min" => Ok(WindowFilter::Single(Window::FifteenMin)),
        "1h" | "hourly" => Ok(WindowFilter::Single(Window::Hourly)),
        "1d" | "daily" => Ok(WindowFilter::Single(Window::Daily)),
        "all" => Ok(WindowFilter::All),
        _ => Err(format!("unknown window: {s}. Use 5m, 15m, 1h, 1d, or all")),
    }
}
