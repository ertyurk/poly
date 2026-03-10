mod actors;
mod cli;
mod config;
mod db;
mod math;
mod polymarket;
mod types;

use actors::decision::{DecisionActor, DecisionInput, DecisionOutput};
use actors::executor::{Executor, Mode};
use actors::ingest::IngestActor;
use actors::market_fetcher::MarketFetcher;
use actors::signal::{AssetTracker, SignalActor};
use actors::telegram::{TelegramActor, TelegramAlert, TelegramStats};
use actors::writer::WriterActor;
use cli::Cli;
use polymarket::PolymarketClient;
use types::*;

use clap::Parser;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use tracing_subscriber::EnvFilter;

#[allow(clippy::too_many_lines)]
#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Parse CLI
    let cli = Cli::parse();

    // 2. Load .env file from current directory (if present)
    dotenvy::dotenv().ok();

    // 3. Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env().add_directive("polymarket_bot=info".parse()?),
        )
        .init();

    // 4. Load config
    let config = config::Config::load(&cli.config)?;

    // 5. Apply CLI overrides
    let bankroll = cli.bankroll.unwrap_or(config.bankroll.initial);
    let paper_trade = cli.paper_trade;
    let mode_str = if paper_trade { "paper" } else { "real" };

    tracing::info!(mode = mode_str, bankroll = bankroll, "loaded config");

    // 6. Create data/ directory if needed
    std::fs::create_dir_all("data").ok();

    // 7. Build Polymarket client for order execution (live mode only)
    let exec_client: Option<PolymarketClient> = if paper_trade {
        None
    } else {
        let api_key = std::env::var("POLYMARKET_API_KEY")
            .map_err(|_| "POLYMARKET_API_KEY env var required for real trading")?;
        let api_secret = std::env::var("POLYMARKET_API_SECRET")
            .map_err(|_| "POLYMARKET_API_SECRET env var required for real trading")?;
        let passphrase = std::env::var("POLYMARKET_PASSPHRASE")
            .map_err(|_| "POLYMARKET_PASSPHRASE env var required for real trading")?;
        let private_key = std::env::var("PRIVATE_KEY")
            .map_err(|_| "PRIVATE_KEY env var required for real trading")?;

        Some(
            PolymarketClient::new_authenticated(
                &config.polymarket.gamma_url,
                &config.polymarket.clob_url,
                api_key,
                api_secret,
                passphrase,
                private_key,
            )
            .map_err(|e| -> Box<dyn std::error::Error> { e })?,
        )
    };

    // 8. Shutdown watch channel
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // 9. Create all mpsc channels

    // db_tx/db_rx — all actors send to writer
    let (db_tx, db_rx) = mpsc::channel::<DbEvent>(10_000);

    // spot_tx — ingest sends here, then we fan out
    let (spot_tx, mut spot_rx_fanout) = mpsc::channel::<SpotPrice>(5_000);
    let (spot_tx_signal, spot_rx_signal) = mpsc::channel::<SpotPrice>(5_000);

    // market channels — market_fetcher → signal and decision
    let (market_tx_signal, market_rx_signal) = mpsc::channel::<MarketState>(100);
    let (market_tx_decision, market_rx_decision) = mpsc::channel::<MarketState>(100);

    // signal_tx/signal_rx — signal → decision
    let (signal_tx, signal_rx) = mpsc::channel::<Signal>(1_000);

    // DecisionActor input/output
    let (decision_in_tx, decision_in_rx) = mpsc::channel::<DecisionInput>(200);
    let (decision_out_tx, mut decision_out_rx) = mpsc::channel::<DecisionOutput>(100);

    // Settle channel — market_fetcher → executor
    let (settle_tx, mut settle_rx) = mpsc::channel::<SettleCommand>(100);

    // Market registration channel — market_fetcher → executor
    let (market_reg_tx, mut market_reg_rx) = mpsc::channel::<MarketState>(100);

    // 10. Send config snapshot to DB
    {
        let config_json = serde_json::to_string(&config)?;
        let _ = db_tx
            .send(DbEvent::ConfigSnapshot {
                config_json,
                ts: now_micros(),
            })
            .await;
    }

    // 11. Spawn all actors

    // Writer actor
    let mut writer = WriterActor::new(
        &config.general.db_path,
        config.writer.batch_size,
        config.writer.flush_interval_ms,
    )?;
    tokio::spawn(async move {
        writer.run(db_rx).await;
    });

    // Ingest actor (Binance spot prices)
    let ingest = IngestActor::new(config.clone());
    let ingest_db_tx = db_tx.clone();
    let ingest_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        ingest.run(spot_tx, ingest_db_tx, ingest_shutdown).await;
    });

    // Spot price fan-out: ingest → signal
    tokio::spawn(async move {
        while let Some(sp) = spot_rx_fanout.recv().await {
            let _ = spot_tx_signal.try_send(sp);
        }
    });

    // Signal actor — load warm-up state from previous session if available
    let warm_trackers = {
        let warm_conn = db::init(&config.general.db_path)?;
        // Accept state saved within the last 30 minutes
        let states = db::queries::load_signal_states(&warm_conn, 1800).unwrap_or_default();
        let mut trackers: HashMap<Asset, AssetTracker> = HashMap::new();
        for s in states {
            let asset = match s.asset.as_str() {
                "BTC" => Asset::BTC,
                "ETH" => Asset::ETH,
                _ => continue,
            };
            trackers.insert(
                asset,
                AssetTracker::restore(
                    s.last_price,
                    s.last_ts,
                    s.valid_ticks,
                    s.variance,
                    s.drift,
                    s.slow_drift,
                    s.lambda,
                ),
            );
        }
        trackers
    };
    let signal_actor = SignalActor::new(config.clone()).with_warm_state(warm_trackers);
    let signal_db_tx = db_tx.clone();
    let signal_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        signal_actor
            .run(
                spot_rx_signal,
                market_rx_signal,
                signal_tx,
                signal_db_tx,
                signal_shutdown,
            )
            .await;
    });

    // Forwarding: merge signal_rx and market_rx_decision into decision_in_tx
    {
        let tx = decision_in_tx.clone();
        tokio::spawn(async move {
            let mut signal_rx = signal_rx;
            while let Some(sig) = signal_rx.recv().await {
                if tx.send(DecisionInput::Signal(sig)).await.is_err() {
                    break;
                }
            }
        });
    }
    {
        let tx = decision_in_tx.clone();
        tokio::spawn(async move {
            let mut market_rx = market_rx_decision;
            while let Some(ms) = market_rx.recv().await {
                // Also send to registration channel for executor
                let _ = market_reg_tx.send(ms.clone()).await;
                if tx.send(DecisionInput::Market(ms)).await.is_err() {
                    break;
                }
            }
        });
    }
    drop(decision_in_tx);

    // Decision actor
    let mut decision_actor = DecisionActor::new(
        decision_in_rx,
        decision_out_tx,
        config.strategy.tau_min,
        config.strategy.liquidity_b,
        config.strategy.kelly_fraction,
        bankroll,
        config.strategy.max_volume_pct,
        config.strategy.max_bet_fraction,
        config.strategy.min_confidence,
    );
    tokio::spawn(async move {
        decision_actor.run().await;
    });

    // Market fetcher (replaces simulator — uses real Polymarket data)
    // Fetcher only needs read-only access — only executor needs auth
    let fetcher_client =
        PolymarketClient::new_readonly(&config.polymarket.gamma_url, &config.polymarket.clob_url);
    let fetcher = MarketFetcher::new(
        fetcher_client,
        cli.asset.clone(),
        cli.window.clone(),
        config.polymarket.poll_interval_secs,
    );
    let fetcher_shutdown = shutdown_rx.clone();
    let fetcher_db_tx = db_tx.clone();
    tokio::spawn(async move {
        fetcher
            .run(
                market_tx_signal,
                market_tx_decision,
                settle_tx,
                fetcher_db_tx,
                fetcher_shutdown,
            )
            .await;
    });

    // Telegram alert actor (optional — only spawned if configured and enabled)
    let (telegram_tx, tg_stats): (Option<mpsc::Sender<TelegramAlert>>, Option<Arc<TelegramStats>>) =
        if let Some(ref tg) = config.telegram {
            if tg.enabled {
                let stats = Arc::new(TelegramStats::new(bankroll));
                let (tg_tx, tg_rx) = mpsc::channel::<TelegramAlert>(100);
                let actor = TelegramActor::new(
                    tg.bot_token.clone(),
                    tg.chat_id.clone(),
                    tg.summary_interval_mins,
                    Arc::clone(&stats),
                );
                let tg_shutdown = shutdown_rx.clone();
                tokio::spawn(async move {
                    actor.run(tg_rx, tg_shutdown).await;
                });
                tracing::info!(
                    summary_interval_mins = tg.summary_interval_mins,
                    "telegram alerts enabled with periodic summary"
                );
                (Some(tg_tx), Some(stats))
            } else {
                (None, None)
            }
        } else {
            (None, None)
        };
    let exec_tg_stats = tg_stats.clone();

    // Executor task — handles fills + settlements, returns stats
    let exec_db_tx = db_tx.clone();
    let mut exec_shutdown = shutdown_rx.clone();
    let exec_handle = tokio::spawn(async move {
        let exec_mode = if paper_trade { Mode::Paper } else { Mode::Live };
        let mut executor =
            Executor::new(exec_mode, bankroll, exec_client, config.strategy.max_total_exposure);

        let mut trades_placed: u32 = 0;
        let mut trades_skipped: u32 = 0;
        let mut markets_resolved: u32 = 0;
        let mut wins: u32 = 0;
        let mut losses: u32 = 0;
        let mut total_fees: f64 = 0.0;
        let mut total_pnl: f64 = 0.0;

        loop {
            tokio::select! {
                // Register market tokens with executor
                msg = market_reg_rx.recv() => {
                    if let Some(ms) = msg {
                        executor.register_market(&ms.market_id, &ms.token_yes, &ms.token_no);
                    }
                }

                msg = decision_out_rx.recv() => {
                    match msg {
                        Some(DecisionOutput::Trade(dec)) => {
                            let best_ask = dec.price;
                            let best_bid = dec.price;
                            if executor.try_fill(&dec, best_ask, best_bid).await.is_some() {
                                trades_placed += 1;
                                if let Some(ref stats) = exec_tg_stats {
                                    stats.record_fill();
                                }
                                if let Some(ref tg) = telegram_tx {
                                    let _ = tg.try_send(TelegramAlert::TradeFilled(dec.clone()));
                                }
                                let _ = exec_db_tx.try_send(DbEvent::Decision(dec));
                            }
                        }
                        Some(DecisionOutput::Skip(nt)) => {
                            trades_skipped += 1;
                            let _ = exec_db_tx.try_send(DbEvent::Skip(nt));
                        }
                        None => break,
                    }
                }

                msg = settle_rx.recv() => {
                    if let Some(cmd) = msg {
                        markets_resolved += 1;
                        let results = executor.settle(&cmd.market_id, cmd.resolved_side, cmd.resolved_ts);

                        for tr in &results {
                            match tr.outcome {
                                Outcome::Win => wins += 1,
                                Outcome::Loss => losses += 1,
                            }
                            total_fees += tr.fee_paid;
                            total_pnl += tr.pnl;

                            tracing::info!(
                                market = %tr.market_id,
                                outcome = %tr.outcome,
                                size = format_args!("${:.2}", tr.size),
                                pnl = format_args!("{:+.2}", tr.pnl),
                                bankroll = format_args!("${:.2}", tr.bankroll_after),
                                "settled"
                            );

                            if let Some(ref stats) = exec_tg_stats {
                                stats.record_settlement(tr).await;
                            }
                            if let Some(ref tg) = telegram_tx {
                                let _ = tg.try_send(TelegramAlert::TradeSettled(tr.clone()));
                            }
                            let _ = exec_db_tx.try_send(DbEvent::Trade(tr.clone()));
                        }
                    }
                }

                _ = exec_shutdown.changed() => {
                    // Drain remaining settle commands
                    while let Ok(cmd) = settle_rx.try_recv() {
                        markets_resolved += 1;
                        let results = executor.settle(&cmd.market_id, cmd.resolved_side, cmd.resolved_ts);
                        for tr in &results {
                            match tr.outcome {
                                Outcome::Win => wins += 1,
                                Outcome::Loss => losses += 1,
                            }
                            total_fees += tr.fee_paid;
                            total_pnl += tr.pnl;
                            let _ = exec_db_tx.try_send(DbEvent::Trade(tr.clone()));
                        }
                    }
                    break;
                }
            }
        }

        // Return stats for main to print after all actors finish
        (
            executor.bankroll(),
            trades_placed,
            trades_skipped,
            markets_resolved,
            wins,
            losses,
            total_fees,
            total_pnl,
        )
    });

    // 12. Log startup info
    tracing::info!(
        "polymarket-bot v{} — {mode_str} mode — ${bankroll:.2} bankroll",
        env!("CARGO_PKG_VERSION"),
    );
    tracing::info!("asset={:?} window={:?}", cli.asset, cli.window,);
    if paper_trade {
        tracing::info!("paper-trade mode: real market data, simulated execution");
        tracing::info!("no API keys required — using public Polymarket endpoints");
    } else {
        tracing::info!("REAL trading mode: orders will be placed on Polymarket");
    }
    tracing::info!("press Ctrl+C to stop and see the summary");

    // 13. Wait for ctrl+c
    tokio::signal::ctrl_c().await?;

    // 14. Send shutdown signal
    tracing::info!("shutting down...");
    let _ = shutdown_tx.send(true);

    // 15. Wait for actors to finish, then print summary
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    if let Ok((
        final_bankroll,
        trades_placed,
        trades_skipped,
        markets_resolved,
        wins,
        losses,
        total_fees,
        total_pnl,
    )) = exec_handle.await
    {
        let total_trades = wins + losses;
        let win_rate = if total_trades > 0 {
            100.0 * f64::from(wins) / f64::from(total_trades)
        } else {
            0.0
        };
        let return_pct = (final_bankroll - bankroll) / bankroll * 100.0;

        tracing::info!("══════════════════════════════════════════");
        tracing::info!("         TRADING SUMMARY ({mode_str})      ");
        tracing::info!("══════════════════════════════════════════");
        tracing::info!("  Markets resolved:   {markets_resolved}");
        tracing::info!("  Trades placed:      {trades_placed}");
        tracing::info!("  Signals skipped:    {trades_skipped}");
        tracing::info!("  Wins / Losses:      {wins} / {losses}");
        tracing::info!("  Win rate:           {win_rate:.1}%");
        tracing::info!("  Total fees:         ${total_fees:.2}");
        tracing::info!("  Net P&L:            {total_pnl:+.2}");
        tracing::info!("  Starting bankroll:  ${bankroll:.2}");
        tracing::info!("  Final bankroll:     ${final_bankroll:.2}");
        tracing::info!("  Return:             {return_pct:+.2}%");
        tracing::info!("══════════════════════════════════════════");
    }

    tracing::info!("shutdown complete — query data/bot.db for detailed dashboard data");
    Ok(())
}
