use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::{generate, Shell};

use crate::types::{Asset, Window};

/// Polymarket prediction market trading bot.
///
/// Connects to Binance for real-time spot prices and Polymarket for market data.
/// Runs Bayesian signal processing + Kelly-criterion sizing to trade crypto
/// prediction markets.
#[derive(Parser, Debug)]
#[command(name = "trade", version, about)]
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

    #[command(subcommand)]
    pub command: Option<Commands>,
}

#[derive(Subcommand, Debug)]
pub enum Commands {
    /// Generate shell completions (zsh, bash, fish)
    Completions {
        /// Shell type
        #[arg(value_enum)]
        shell: Shell,
    },
}

impl Cli {
    /// Handle built-in subcommands (completions). Returns true if handled.
    pub fn handle_subcommand(&self) -> bool {
        match &self.command {
            Some(Commands::Completions { shell }) => {
                let mut cmd = Self::command();
                generate(*shell, &mut cmd, "trade", &mut std::io::stdout());
                true
            }
            None => false,
        }
    }
}

#[derive(Debug, Clone)]
pub enum AssetFilter {
    Single(Asset),
    All,
}

#[derive(Debug, Clone)]
pub enum WindowFilter {
    Single(Window),
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
        "all" => Ok(WindowFilter::All),
        _ => Err(format!("unknown window: {s}. Use 5m, 15m, or all")),
    }
}
