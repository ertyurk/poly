use clap::Parser;
use std::collections::HashSet;

use crate::types::{Asset, Window};

/// Polymarket prediction market trading bot.
///
/// Connects to Binance for real-time spot prices and Polymarket for market data.
/// Runs Bayesian signal processing + Kelly-criterion sizing to trade crypto
/// prediction markets.
#[derive(Parser, Debug)]
#[command(name = "polymarket-bot", version, about)]
pub struct Cli {
    /// Asset filter: btc, eth, or all
    #[arg(long, default_value = "all", value_parser = parse_asset_filter)]
    pub asset: AssetFilter,

    /// Window filter: 5m, 15m, or all
    #[arg(long, default_value = "all", value_parser = parse_window_filter)]
    pub window: WindowFilter,

    /// Starting bankroll in USD (overrides config.toml)
    #[arg(long)]
    pub bankroll: Option<f64>,

    /// Paper-trade mode: real market data, simulated execution
    #[arg(long, alias = "dry-run")]
    pub paper_trade: bool,

    /// Path to config file
    #[arg(long, default_value = "config.toml")]
    pub config: String,
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
