mod actors;
mod config;
mod db;
mod math;
mod types;

use actors::decision::{DecisionActor, DecisionInput, DecisionOutput};
use actors::executor::PaperExecutor;
use actors::ingest::IngestActor;
use actors::signal::SignalActor;
use actors::writer::WriterActor;
use types::*;

use tokio::sync::{mpsc, watch};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("polymarket_bot=info".parse()?),
        )
        .init();

    // 2. Load config
    let config = config::Config::load("config.toml")?;
    tracing::info!(mode = %config.general.mode, "loaded config");

    // 3. Create data/ directory if needed
    std::fs::create_dir_all("data").ok();

    // 4. Shutdown watch channel
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // 5. Create all mpsc channels

    // db_tx/db_rx — all actors send to writer
    let (db_tx, db_rx) = mpsc::channel::<DbEvent>(10_000);

    // spot_tx/spot_rx — ingest -> signal
    let (spot_tx, spot_rx) = mpsc::channel::<SpotPrice>(5_000);

    // market channels — we need to fan out MarketState to both signal and decision
    let (market_tx_signal, market_rx_signal) = mpsc::channel::<MarketState>(100);
    let (market_tx_decision, market_rx_decision) = mpsc::channel::<MarketState>(100);

    // signal_tx/signal_rx — signal -> decision
    let (signal_tx, signal_rx) = mpsc::channel::<Signal>(1_000);

    // fee_tx/fee_rx — for decision engine
    let (fee_tx, fee_rx) = mpsc::channel::<FeeUpdate>(10);

    // DecisionActor uses a single DecisionInput channel
    let (decision_in_tx, decision_in_rx) = mpsc::channel::<DecisionInput>(200);

    // DecisionActor emits DecisionOutput
    let (decision_out_tx, mut decision_out_rx) = mpsc::channel::<DecisionOutput>(100);

    // 6. Send config snapshot to db_tx
    {
        let config_json = serde_json::to_string(&config)?;
        let _ = db_tx
            .send(DbEvent::ConfigSnapshot {
                config_json,
                ts: now_micros(),
            })
            .await;
    }

    // 7. Spawn all actors

    // Writer actor
    let mut writer = WriterActor::new(
        &config.general.db_path,
        config.writer.batch_size,
        config.writer.flush_interval_ms,
    )?;
    tokio::spawn(async move {
        writer.run(db_rx).await;
    });

    // Ingest actor
    let ingest = IngestActor::new(config.clone());
    let ingest_db_tx = db_tx.clone();
    let ingest_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        ingest.run(spot_tx, ingest_db_tx, ingest_shutdown).await;
    });

    // Signal actor
    let signal_actor = SignalActor::new(config.clone());
    let signal_db_tx = db_tx.clone();
    let signal_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        signal_actor
            .run(spot_rx, market_rx_signal, signal_tx, signal_db_tx, signal_shutdown)
            .await;
    });

    // Forwarding tasks: merge signal_rx, market_rx_decision, fee_rx into decision_in_tx
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
                if tx.send(DecisionInput::Market(ms)).await.is_err() {
                    break;
                }
            }
        });
    }
    {
        let tx = decision_in_tx.clone();
        tokio::spawn(async move {
            let mut fee_rx = fee_rx;
            while let Some(fu) = fee_rx.recv().await {
                if tx.send(DecisionInput::Fee(fu)).await.is_err() {
                    break;
                }
            }
        });
    }
    // Drop the original decision_in_tx so the channel closes when forwarders finish
    drop(decision_in_tx);

    // Decision actor
    let mut decision_actor = DecisionActor::new(
        decision_in_rx,
        decision_out_tx,
        config.strategy.tau_min,
        config.strategy.liquidity_b,
        config.strategy.kelly_fraction,
        config.bankroll.initial,
        config.strategy.max_volume_pct,
        config.strategy.min_confidence,
    );
    tokio::spawn(async move {
        decision_actor.run().await;
    });

    // Executor task
    let exec_db_tx = db_tx.clone();
    let mut exec_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        let mut executor = PaperExecutor::new(config.bankroll.initial);
        loop {
            tokio::select! {
                msg = decision_out_rx.recv() => {
                    match msg {
                        Some(DecisionOutput::Trade(dec)) => {
                            // Use the decision price as simulated best bid/ask
                            let best_ask = dec.price;
                            let best_bid = dec.price;
                            if let Some(_id) = executor.try_fill(&dec, best_ask, best_bid) {
                                let _ = exec_db_tx.try_send(DbEvent::Decision(dec));
                            }
                        }
                        Some(DecisionOutput::Skip(nt)) => {
                            let _ = exec_db_tx.try_send(DbEvent::Skip(nt));
                        }
                        None => break,
                    }
                }
                _ = exec_shutdown.changed() => {
                    tracing::info!("executor shutting down");
                    break;
                }
            }
        }
    });

    // Keep market_tx_signal and market_tx_decision alive for future use
    // (they would be used by a market-polling task; for now we hold references)
    let _market_tx_signal = market_tx_signal;
    let _market_tx_decision = market_tx_decision;
    let _fee_tx = fee_tx;

    // 8. Log startup info
    tracing::info!("polymarket-bot starting in {} mode", config.general.mode);

    // 9. Wait for ctrl+c
    tokio::signal::ctrl_c().await?;

    // 10. Send shutdown signal
    tracing::info!("shutting down");
    let _ = shutdown_tx.send(true);

    // 11. Sleep 2 seconds for actors to flush
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    // 12. Log shutdown complete
    tracing::info!("shutdown complete");
    Ok(())
}
