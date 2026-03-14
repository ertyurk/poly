mod actors;
mod cli;
mod config;
mod dashboard;
mod db;
mod flow;
mod math;
mod paths;
mod polymarket;
mod types;
mod weather;

use actors::decision::{DecisionActor, DecisionInput, DecisionOutput};
use actors::executor::{Executor, GtdResult, Mode};
use actors::ingest::IngestActor;
use actors::market_fetcher::MarketFetcher;
use actors::signal::{AssetTracker, SignalActor};
use actors::telegram::{TelegramActor, TelegramAlert, TelegramStats};
use actors::writer::WriterActor;
use cli::{Cli, Command};
use paths::AppPaths;
use polymarket::{LiveTrader, PolymarketClient};
use types::*;

use clap::Parser;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
use tracing_subscriber::EnvFilter;

/// Private key baked in at compile time via:
///   PRIVATE_KEY=abc123 cargo install --path .
/// Falls back to runtime env var if not compiled in.
const COMPILED_PRIVATE_KEY: Option<&str> = option_env!("PRIVATE_KEY");

fn get_private_key() -> Result<String, String> {
    if let Some(key) = COMPILED_PRIVATE_KEY {
        return Ok(key.to_string());
    }
    std::env::var("PRIVATE_KEY").map_err(|_| {
        "PRIVATE_KEY not set. Build with: PRIVATE_KEY=xxx cargo install --path . \
                       Or set PRIVATE_KEY env var."
            .to_string()
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    // Load .env from ~/.polymarket-bot/ and current dir
    let app_paths = AppPaths::resolve(cli.config.as_deref(), cli.db_path.as_deref());
    let env_path = app_paths.root.join(".env");
    if env_path.exists() {
        dotenvy::from_path(&env_path).ok();
    }
    dotenvy::dotenv().ok();

    match cli.command {
        Command::Crypto {
            paper_trade,
            bankroll,
            asset,
            window,
        } => run_crypto(&app_paths, paper_trade, bankroll, asset, window).await,
        Command::Weather {
            paper_trade,
            bankroll,
        } => run_weather(&app_paths, paper_trade, bankroll).await,
        Command::Dashboard { host, port } => run_dashboard(&app_paths, &host, port).await,
        Command::Status => run_status(&app_paths),
        Command::ResetDb => run_reset_db(&app_paths),
    }
}

// ─── Status ──────────────────────────────────────────────────────────────────

fn run_status(paths: &AppPaths) -> Result<(), Box<dyn std::error::Error>> {
    let db_path = paths.db_str();
    if !paths.db.exists() {
        eprintln!("No database found at {db_path}");
        eprintln!("Run `poly crypto` or `poly weather` first.");
        return Ok(());
    }

    let conn = rusqlite::Connection::open(&db_path)?;

    let (decisions, trades, rejections, spot_ticks, active_mkts): (i64, i64, i64, i64, i64) =
        conn.query_row(
            "SELECT
                (SELECT count(*) FROM decisions),
                (SELECT count(*) FROM trades),
                (SELECT count(*) FROM fill_rejections),
                (SELECT count(*) FROM spot_prices),
                (SELECT count(*) FROM markets WHERE resolution_ts > strftime('%s', 'now') * 1000000)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?, row.get(4)?)),
        )?;

    let trade_decisions: i64 = conn.query_row(
        "SELECT count(*) FROM decisions WHERE action='TRADE'",
        [],
        |row| row.get(0),
    )?;

    let (total_wins, total_losses, total_pnl): (i64, i64, f64) = conn
        .query_row(
            "SELECT
                COALESCE(sum(case when outcome='WIN' then 1 else 0 end), 0),
                COALESCE(sum(case when outcome='LOSS' then 1 else 0 end), 0),
                COALESCE(sum(pnl), 0.0)
            FROM trades",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap_or((0, 0, 0.0));

    let bankroll: f64 = conn
        .query_row(
            "SELECT bankroll_after FROM trades ORDER BY resolved_ts DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .or_else(|_| {
            // No trades yet — try to get initial bankroll from config snapshot
            conn.query_row(
                "SELECT json_extract(config_json, '$.bankroll.initial')
                 FROM config_snapshots ORDER BY ts DESC LIMIT 1",
                [],
                |row| row.get(0),
            )
        })
        .unwrap_or(0.0);

    let open_positions: i64 = conn
        .query_row("SELECT count(*) FROM open_positions", [], |row| row.get(0))
        .unwrap_or(0);

    let top_skip: String = conn
        .query_row(
            "SELECT COALESCE(skip_reason, 'NONE') || ' (' || count(*) || ')'
             FROM decisions WHERE action='SKIP'
             GROUP BY skip_reason ORDER BY count(*) DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap_or_else(|_| "none".to_string());

    // Truncate strings for display
    let db_short: String = if db_path.len() > 38 {
        format!("...{}", &db_path[db_path.len() - 35..])
    } else {
        db_path.clone()
    };
    let skip_short: String = top_skip.chars().take(28).collect();

    eprintln!("╔════════════════════════════════════════════╗");
    eprintln!("║         POLYMARKET BOT STATUS              ║");
    eprintln!("╠════════════════════════════════════════════╣");
    eprintln!("║  DB: {:<38}║", db_short);
    eprintln!("║  Spot ticks:     {:<26}║", format_num(spot_ticks));
    eprintln!("║  Active markets: {:<26}║", active_mkts);
    eprintln!("║  Decisions:      {:<26}║", format_num(decisions));
    eprintln!("║  Trade signals:  {:<26}║", trade_decisions);
    eprintln!("║  Top skip:       {:<26}║", skip_short);
    eprintln!("╠════════════════════════════════════════════╣");
    eprintln!("║  Trades:         {:<26}║", trades);
    eprintln!(
        "║  Wins/Losses:    {:<26}║",
        format!("{total_wins}/{total_losses}")
    );
    eprintln!("║  P&L:            {:<26}║", format!("${total_pnl:+.2}"));
    eprintln!("║  Bankroll:       {:<26}║", format!("${bankroll:.2}"));
    eprintln!("║  Open positions: {:<26}║", open_positions);
    eprintln!("║  Fill rejects:   {:<26}║", rejections);
    eprintln!("╚════════════════════════════════════════════╝");

    // Weather section (only if weather tables exist)
    let has_weather: bool = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name='weather_markets'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap_or(0)
        > 0;

    if has_weather {
        let wx_markets: i64 = conn
            .query_row(
                "SELECT count(DISTINCT city || target_date) FROM weather_markets",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let wx_signals: i64 = conn
            .query_row(
                "SELECT count(*) FROM weather_markets WHERE edge > 0",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let wx_trades: i64 = conn
            .query_row(
                "SELECT count(*) FROM trades WHERE market_id LIKE 'WX_%'",
                [],
                |row| row.get(0),
            )
            .unwrap_or(0);
        let (wx_wins, wx_losses, wx_pnl): (i64, i64, f64) = conn
            .query_row(
                "SELECT COALESCE(sum(case when outcome='WIN' then 1 else 0 end), 0),
                        COALESCE(sum(case when outcome='LOSS' then 1 else 0 end), 0),
                        COALESCE(sum(pnl), 0.0)
                 FROM trades WHERE market_id LIKE 'WX_%'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap_or((0, 0, 0.0));

        eprintln!("╔════════════════════════════════════════════╗");
        eprintln!("║         WEATHER STATUS                     ║");
        eprintln!("╠════════════════════════════════════════════╣");
        eprintln!("║  City-dates:     {:<26}║", wx_markets);
        eprintln!("║  Tail signals:   {:<26}║", wx_signals);
        eprintln!("╠════════════════════════════════════════════╣");
        eprintln!("║  Trades:         {:<26}║", wx_trades);
        eprintln!(
            "║  Wins/Losses:    {:<26}║",
            format!("{wx_wins}/{wx_losses}")
        );
        eprintln!(
            "║  P&L:            {:<26}║",
            format!("${wx_pnl:+.2}")
        );
        eprintln!("╚════════════════════════════════════════════╝");
    }

    Ok(())
}

fn format_num(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

// ─── Reset DB ────────────────────────────────────────────────────────────────

fn run_reset_db(paths: &AppPaths) -> Result<(), Box<dyn std::error::Error>> {
    let db_path = &paths.db;
    let wal = db_path.with_extension("db-wal");
    let shm = db_path.with_extension("db-shm");

    let mut removed = false;
    for path in [db_path.as_path(), wal.as_path(), shm.as_path()] {
        if path.exists() {
            std::fs::remove_file(path)?;
            eprintln!("removed {}", path.display());
            removed = true;
        }
    }

    if removed {
        eprintln!("database reset complete");
    } else {
        eprintln!("no database found at {}", db_path.display());
    }

    Ok(())
}

// ─── Dashboard ───────────────────────────────────────────────────────────────

async fn run_dashboard(
    paths: &AppPaths,
    host: &str,
    port: u16,
) -> Result<(), Box<dyn std::error::Error>> {
    use bytes::Bytes;
    use http_body_util::Full;
    use hyper::body::Incoming;
    use hyper::header::{self, HeaderValue};
    use hyper::server::conn::http1;
    use hyper::service::service_fn;
    use hyper::{Method, Request, Response, StatusCode};
    use hyper_util::rt::TokioIo;
    use std::convert::Infallible;
    use std::net::SocketAddr;
    use tokio::net::TcpListener;

    const DASHBOARD_HTML: &str = include_str!("../assets/dashboard.html");

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("poly=info".parse()?)
                .add_directive("polymarket_bot=info".parse()?),
        )
        .init();

    let db_path = paths.db_str();
    if !paths.db.exists() {
        return Err(format!("database not found: {db_path}. Run a trader first.").into());
    }

    let preview = dashboard::load_dashboard_payload(&db_path)
        .map_err(|e| -> Box<dyn std::error::Error> { e.to_string().into() })?;
    let addr: SocketAddr = format!("{host}:{port}").parse()?;
    let listener = TcpListener::bind(addr).await?;
    let db_arc = Arc::new(db_path.clone());

    tracing::info!(
        url = %format!("http://{host}:{port}"),
        db = %db_path,
        trades = preview.trades.len(),
        skips = preview.skips.len(),
        "dashboard ready"
    );

    fn response_with(
        status: StatusCode,
        content_type: &'static str,
        body: Bytes,
    ) -> Response<Full<Bytes>> {
        let mut response = Response::new(Full::new(body));
        *response.status_mut() = status;
        response
            .headers_mut()
            .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
        response
    }

    loop {
        tokio::select! {
            accept = listener.accept() => {
                let (stream, _peer) = accept?;
                let io = TokioIo::new(stream);
                let db_ref = Arc::clone(&db_arc);

                tokio::spawn(async move {
                    let service = service_fn(move |req: Request<Incoming>| {
                        let db = Arc::clone(&db_ref);
                        async move {
                            let resp = match (req.method(), req.uri().path()) {
                                (&Method::GET, "/") | (&Method::GET, "/index.html") => {
                                    response_with(
                                        StatusCode::OK,
                                        "text/html; charset=utf-8",
                                        Bytes::from_static(DASHBOARD_HTML.as_bytes()),
                                    )
                                }
                                (&Method::GET, "/api/bootstrap") => {
                                    match dashboard::load_dashboard_payload(&*db) {
                                        Ok(payload) => match serde_json::to_vec(&payload) {
                                            Ok(body) => response_with(
                                                StatusCode::OK,
                                                "application/json; charset=utf-8",
                                                Bytes::from(body),
                                            ),
                                            Err(e) => response_with(
                                                StatusCode::INTERNAL_SERVER_ERROR,
                                                "text/plain",
                                                Bytes::from(e.to_string()),
                                            ),
                                        },
                                        Err(e) => response_with(
                                            StatusCode::INTERNAL_SERVER_ERROR,
                                            "text/plain",
                                            Bytes::from(e.to_string()),
                                        ),
                                    }
                                }
                                (&Method::GET, "/api/health") => response_with(
                                    StatusCode::OK,
                                    "text/plain",
                                    Bytes::from_static(b"ok"),
                                ),
                                _ => response_with(
                                    StatusCode::NOT_FOUND,
                                    "text/plain",
                                    Bytes::from_static(b"not found"),
                                ),
                            };
                            Ok::<_, Infallible>(resp)
                        }
                    });
                    let _ = http1::Builder::new().serve_connection(io, service).await;
                });
            }
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("dashboard shutting down");
                break;
            }
        }
    }

    Ok(())
}

// ─── Weather ─────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_lines)]
async fn run_weather(
    paths: &AppPaths,
    paper_trade: bool,
    bankroll_override: Option<f64>,
) -> Result<(), Box<dyn std::error::Error>> {
    paths.ensure_config()?;
    paths.ensure_dirs()?;

    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("poly=info".parse()?)
                .add_directive("polymarket_bot=info".parse()?),
        )
        .init();

    let mut config = config::Config::load(&paths.config_str())?;
    config.general.db_path = paths.db_str();

    let mode_str = if paper_trade { "paper" } else { "real" };

    // Restore bankroll from DB if available
    let startup_conn = db::init(&config.general.db_path)?;
    let bankroll = if let Some(b) = bankroll_override {
        b
    } else if let Ok(Some(last)) = db::queries::last_bankroll(&startup_conn) {
        tracing::info!(
            bankroll = format_args!("${last:.2}"),
            "restored bankroll from DB"
        );
        last
    } else {
        config.bankroll.initial
    };
    let next_decision_id =
        db::queries::max_decision_id(&startup_conn).unwrap_or(0) + 1;
    drop(startup_conn);

    tracing::info!(
        mode = mode_str,
        bankroll = format_args!("${bankroll:.2}"),
        poll_secs = config.weather.poll_interval_secs,
        horizon_h = config.weather.max_forecast_horizon_hours,
        "weather config loaded"
    );

    // Build LiveTrader for live mode
    let live_trader: Option<LiveTrader> = if paper_trade {
        None
    } else {
        let private_key =
            get_private_key().map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
        Some(
            LiveTrader::connect(&private_key)
                .await
                .map_err(|e| -> Box<dyn std::error::Error> { e })?,
        )
    };

    // Shutdown watch channel
    let (_shutdown_tx, shutdown_rx) = watch::channel(false);

    // DB writer channel + actor
    let (db_tx, db_rx) = mpsc::channel::<DbEvent>(10_000);
    let mut writer = WriterActor::new(
        &config.general.db_path,
        config.writer.batch_size,
        config.writer.flush_interval_ms,
    )?;
    let writer_handle = tokio::spawn(async move {
        writer.run(db_rx).await;
    });

    // Send config snapshot
    {
        let config_json = serde_json::to_string(&config)?;
        let _ = db_tx
            .send(DbEvent::ConfigSnapshot {
                config_json,
                ts: now_micros(),
            })
            .await;
    }

    // Executor
    let exec_mode = if paper_trade { Mode::Paper } else { Mode::Live };
    let mut executor = Executor::new(
        exec_mode,
        bankroll,
        live_trader,
        config.strategy.max_total_exposure,
    );
    executor.set_next_decision_id(next_decision_id);

    // Telegram alert actor
    let (telegram_tx, tg_stats): (
        Option<mpsc::Sender<TelegramAlert>>,
        Option<Arc<TelegramStats>>,
    ) = if let Some(ref tg) = config.telegram {
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
            tracing::info!("telegram alerts enabled for weather");
            (Some(tg_tx), Some(stats))
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };

    tracing::info!(
        "poly v{} weather — {mode_str} — ${bankroll:.2} bankroll",
        env!("CARGO_PKG_VERSION"),
    );
    tracing::info!("press Ctrl+C to stop");

    // Separate DB connection for weather-specific inserts
    // (avoids modifying the shared DbEvent enum in types.rs)
    let wx_conn = db::init(&config.general.db_path)?;

    // HTTP client for API calls
    let http = reqwest::Client::new();
    let gamma_url = config.polymarket.gamma_url.clone();
    let poll_duration = tokio::time::Duration::from_secs(
        config.weather.poll_interval_secs,
    );
    let wx_config = config.weather.clone();

    let mut poll_interval = tokio::time::interval(poll_duration);
    // interval's first tick() returns immediately, so the loop body fires on startup

    let mut trades_placed: u32 = 0;
    let mut trades_skipped: u32 = 0;
    let mut total_pnl: f64 = 0.0;
    let mut wins: u32 = 0;
    let mut losses: u32 = 0;

    // Map wx_market_id → condition_id for settlement CLOB lookups
    let mut wx_condition_ids: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let clob_url = config.polymarket.clob_url.clone();

    loop {
        tokio::select! {
            _ = poll_interval.tick() => {
                let events = match weather::fetcher::fetch_weather_events(
                    &http, &gamma_url,
                ).await {
                    Ok(evts) => evts,
                    Err(e) => {
                        tracing::warn!(error = %e, "failed to fetch weather events");
                        continue;
                    }
                };

                let now_ts = now_micros();
                let horizon_micros =
                    wx_config.max_forecast_horizon_hours as i64 * 3_600 * 1_000_000;

                for event in &events {
                    // Parse end_date to micros for horizon check
                    let end_ts = chrono::DateTime::parse_from_rfc3339(&event.end_date)
                        .map(|dt| dt.timestamp_micros())
                        .unwrap_or(i64::MAX);

                    if end_ts - now_ts > horizon_micros || end_ts < now_ts {
                        continue;
                    }

                    let city_cfg = match weather::types::CityConfig::find(&event.city) {
                        Some(c) => c,
                        None => continue,
                    };

                    let temps = match weather::forecast::fetch_ensemble(
                        &http, city_cfg, &event.target_date,
                    ).await {
                        Ok(t) => t,
                        Err(e) => {
                            tracing::warn!(
                                city = %event.city,
                                date = %event.target_date,
                                error = %e,
                                "failed to fetch ensemble"
                            );
                            continue;
                        }
                    };

                    let bucket_structs: Vec<_> = event.buckets.iter()
                        .map(|bm| bm.bucket.clone())
                        .collect();
                    let probs = weather::forecast::bucket_probabilities(
                        &bucket_structs, &temps,
                    );
                    let market_prices: Vec<f64> = event.buckets.iter()
                        .map(|bm| bm.midpoint)
                        .collect();

                    // Persist forecast members (direct DB write)
                    for (i, &t) in temps.iter().enumerate() {
                        let _ = db::queries::insert_weather_forecast(
                            &wx_conn,
                            &event.city,
                            &event.target_date,
                            "ensemble",
                            i as i32,
                            t,
                            now_ts,
                        );
                    }

                    // Persist market snapshots and register markets
                    for (i, bm) in event.buckets.iter().enumerate() {
                        let wx_market_id = weather::types::weather_market_id(
                            &event.city, &event.target_date, bm.bucket.index,
                        );

                        executor.register_market(
                            &wx_market_id,
                            &bm.token_yes,
                            &bm.token_no,
                            end_ts,
                        );

                        // Track condition_id for settlement lookups
                        if !bm.condition_id.is_empty() {
                            wx_condition_ids.insert(
                                wx_market_id.clone(),
                                bm.condition_id.clone(),
                            );
                        }

                        // Register market in DB so FK constraints on decisions/trades pass
                        let _ = wx_conn.execute(
                            "INSERT OR IGNORE INTO markets (market_id, condition_id, asset, window, token_yes, token_no, open_ts, resolution_ts, open_price)
                             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                            rusqlite::params![
                                wx_market_id,
                                bm.condition_id,
                                format!("WX-{}", event.city),
                                format!("WX-{}-{}", event.city, event.target_date),
                                bm.token_yes,
                                bm.token_no,
                                now_ts,
                                end_ts,
                                bm.midpoint,
                            ],
                        );

                        let p_ens = probs.get(i).copied();
                        let edge_val = p_ens.map(|p| p - bm.midpoint);

                        let _ = db::queries::insert_weather_market(
                            &wx_conn,
                            &event.event_id,
                            &event.city,
                            &event.target_date,
                            bm.bucket.index as i32,
                            &bm.label,
                            bm.bucket.lo,
                            bm.bucket.hi,
                            &bm.token_yes,
                            &bm.token_no,
                            Some(bm.best_bid),
                            Some(bm.best_ask),
                            Some(bm.midpoint),
                            p_ens,
                            edge_val,
                            now_ts,
                        );
                    }

                    // Find tail edges
                    let edges = weather::signal::find_tail_edges(
                        &market_prices,
                        &probs,
                        wx_config.tail_buckets,
                        wx_config.max_tail_price,
                        wx_config.edge_threshold,
                    );

                    for edge in &edges {
                        let bm = &event.buckets[edge.bucket_index as usize];
                        let wx_market_id = weather::types::weather_market_id(
                            &event.city, &event.target_date, edge.bucket_index,
                        );

                        tracing::info!(
                            city = %event.city,
                            date = %event.target_date,
                            bucket = edge.bucket_index,
                            p_ensemble = format_args!("{:.3}", edge.p_ensemble),
                            market_price = format_args!("{:.3}", edge.market_price),
                            edge = format_args!("{:.3}", edge.edge),
                            "weather tail edge"
                        );

                        // Call decide() directly — bypass DecisionActor's
                        // crypto-specific filters.
                        let result = actors::decision::decide(
                            edge.p_ensemble,
                            bm.midpoint,
                            config.strategy.tau_min,
                            config.strategy.liquidity_b,
                            (config.strategy.kelly_fraction * 2.0).min(1.0), // half-Kelly → full for ensemble
                            executor.bankroll(),
                            10_000.0, // weather volume placeholder
                            config.strategy.max_volume_pct,
                            config.strategy.max_bet_fraction,
                            0.0, // min_confidence: ensemble is high confidence
                            1.0, // confidence: 1.0 for ensemble-based
                            &wx_market_id,
                            bm.best_bid,
                            bm.best_ask,
                            &format!(
                                "wx-{}-{}",
                                event.city, event.target_date
                            ),
                            0.95, // max_fill_price: tail buckets are cheap
                            0.001, // min_fill_price: Polymarket's tick size
                            false, // direction_guard: multi-outcome, p_hat < 0.5 is normal
                        );

                        match result {
                            Ok(dec) => {
                                // Paper mode: try_fill directly
                                if exec_mode == Mode::Paper {
                                    match executor
                                        .try_fill(&dec, dec.best_ask, dec.best_bid)
                                        .await
                                    {
                                        Ok(fill) => {
                                            trades_placed += 1;
                                            if let Some(ref stats) = tg_stats {
                                                stats.record_fill();
                                            }
                                            if let Some(ref tg) = telegram_tx {
                                                let _ = tg.try_send(
                                                    TelegramAlert::TradeFilled {
                                                        decision: dec.clone(),
                                                        fill_price: fill.fill_price,
                                                    },
                                                );
                                            }
                                            let _ = db_tx.try_send(
                                                DbEvent::SaveOpenPosition {
                                                    decision_id: fill.decision_id,
                                                    market_id: dec.market_id.clone(),
                                                    side: dec.side,
                                                    entry_price: fill.fill_price,
                                                    size: fill.size_shares,
                                                    fee_rate: dec.fee_rate,
                                                    entry_ts: dec.ts,
                                                    estimated_slippage:
                                                        fill.estimated_slippage,
                                                },
                                            );
                                            let _ = db_tx.try_send(
                                                DbEvent::Decision(dec),
                                            );
                                        }
                                        Err(reason) => {
                                            tracing::debug!(
                                                market = %dec.market_id,
                                                reason = %reason,
                                                "weather fill rejected"
                                            );
                                        }
                                    }
                                } else {
                                    // Live mode: GTD order
                                    let res_ts = executor.market_resolution_ts(
                                        &dec.market_id,
                                    );
                                    match executor.try_place_gtd(
                                        &dec,
                                        dec.best_ask,
                                        dec.best_bid,
                                        config.execution.gtd_expiry_secs,
                                        res_ts,
                                        config.execution
                                            .min_time_before_resolution_secs,
                                        config.execution.gtd_price_bump,
                                    ).await {
                                        Ok(GtdResult::InstantFill(fill)) => {
                                            trades_placed += 1;
                                            if let Some(ref stats) = tg_stats {
                                                stats.record_fill();
                                            }
                                            if let Some(ref tg) = telegram_tx {
                                                let _ = tg.try_send(
                                                    TelegramAlert::GtdOrderFilled {
                                                        market_id:
                                                            fill.market_id.clone(),
                                                        side: fill.side,
                                                        price: fill.fill_price,
                                                        maker: true,
                                                    },
                                                );
                                            }
                                            let _ = db_tx.try_send(
                                                DbEvent::SaveOpenPosition {
                                                    decision_id: fill.decision_id,
                                                    market_id:
                                                        fill.market_id.clone(),
                                                    side: fill.side,
                                                    entry_price: fill.fill_price,
                                                    size: fill.size_shares,
                                                    fee_rate: fill.fee_rate,
                                                    entry_ts: fill.entry_ts,
                                                    estimated_slippage: 0.0,
                                                },
                                            );
                                            let _ = db_tx.try_send(
                                                DbEvent::Decision(dec),
                                            );
                                        }
                                        Ok(GtdResult::Resting(_order_id)) => {
                                            let _ = db_tx.try_send(
                                                DbEvent::Decision(dec),
                                            );
                                        }
                                        Err(reason) => {
                                            tracing::debug!(
                                                market = %dec.market_id,
                                                reason = %reason,
                                                "weather GTD rejected"
                                            );
                                        }
                                    }
                                }
                            }
                            Err(nt) => {
                                tracing::info!(
                                    market = %nt.market_id,
                                    reason = ?nt.reason,
                                    edge = format_args!("{:.4}", nt.edge),
                                    eff_edge = format_args!("{:.4}", nt.effective_edge),
                                    fee = format_args!("{:.4}", nt.fee_rate),
                                    "weather decide skip"
                                );
                                trades_skipped += 1;
                                let _ = db_tx.try_send(DbEvent::Skip(nt));
                            }
                        }
                    }

                    // Brief delay between cities to avoid API rate limits
                    tokio::time::sleep(tokio::time::Duration::from_millis(200)).await;
                }

                tracing::info!(
                    events = events.len(),
                    bankroll = format_args!("${:.2}", executor.bankroll()),
                    trades = trades_placed,
                    skips = trades_skipped,
                    "weather poll complete"
                );

                // ── Settlement: check if any open positions have resolved ──
                let open_ids = executor.open_market_ids();
                for mkt_id in &open_ids {
                    let Some(cid) = wx_condition_ids.get(mkt_id) else {
                        continue;
                    };

                    let url = format!("{}/markets/{cid}", clob_url);
                    let resolved_side = match http.get(&url).send().await {
                        Ok(resp) if resp.status().is_success() => {
                            match resp.json::<crate::polymarket::types::ClobMarket>().await {
                                Ok(cm) => {
                                    crate::actors::market_fetcher::determine_outcome_from_clob(&cm)
                                }
                                Err(_) => None,
                            }
                        }
                        _ => None,
                    };

                    if let Some(side) = resolved_side {
                        let now = now_micros();
                        let _ = db_tx.try_send(DbEvent::ClearOpenPositions {
                            market_id: mkt_id.clone(),
                        });
                        let _ = db_tx.try_send(DbEvent::MarketResolution {
                            market_id: mkt_id.clone(),
                            resolved_side: side.to_string(),
                        });
                        let results = executor.settle(mkt_id, side, now);
                        for tr in &results {
                            match tr.outcome {
                                crate::types::Outcome::Win => wins += 1,
                                crate::types::Outcome::Loss => losses += 1,
                            }
                            total_pnl += tr.pnl;
                            tracing::info!(
                                market = %tr.market_id,
                                outcome = %tr.outcome,
                                pnl = format_args!("{:+.2}", tr.pnl),
                                bankroll = format_args!("${:.2}", tr.bankroll_after),
                                "weather settled"
                            );
                            let _ = db_tx.try_send(DbEvent::Trade(tr.clone()));
                            if let Some(ref tg) = telegram_tx {
                                let _ = tg.try_send(TelegramAlert::TradeSettled(tr.clone()));
                            }
                        }
                    }

                    // Small delay between CLOB lookups
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                }
            }

            _ = tokio::signal::ctrl_c() => {
                tracing::info!("weather shutting down...");
                break;
            }
        }
    }

    // Final summary
    let final_bankroll = executor.bankroll();
    let return_pct = (final_bankroll - bankroll) / bankroll * 100.0;

    tracing::info!("══════════════════════════════════════════");
    tracing::info!("       WEATHER TRADING SUMMARY ({mode_str})    ");
    tracing::info!("══════════════════════════════════════════");
    tracing::info!("  Trades placed:      {trades_placed}");
    tracing::info!("  Signals skipped:    {trades_skipped}");
    tracing::info!("  Wins / Losses:      {wins} / {losses}");
    tracing::info!("  Net P&L:            {total_pnl:+.2}");
    tracing::info!("  Starting bankroll:  ${bankroll:.2}");
    tracing::info!("  Final bankroll:     ${final_bankroll:.2}");
    tracing::info!("  Return:             {return_pct:+.2}%");
    tracing::info!("══════════════════════════════════════════");

    // Drop db_tx to close the channel; writer will flush remaining events
    drop(db_tx);
    let _ = writer_handle.await;
    tracing::info!("weather shutdown complete");
    Ok(())
}

// ─── Crypto ──────────────────────────────────────────────────────────────────

#[allow(clippy::too_many_lines)]
async fn run_crypto(
    paths: &AppPaths,
    paper_trade: bool,
    bankroll_override: Option<f64>,
    asset_filter: cli::AssetFilter,
    window_filter: cli::WindowFilter,
) -> Result<(), Box<dyn std::error::Error>> {
    // Ensure app directory and config exist
    paths.ensure_config()?;

    // Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::from_default_env()
                .add_directive("poly=info".parse()?)
                .add_directive("polymarket_bot=info".parse()?),
        )
        .init();

    // Load config
    let mut config = config::Config::load(&paths.config_str())?;
    // Override db_path to use resolved path
    config.general.db_path = paths.db_str();

    let mode_str = if paper_trade { "paper" } else { "real" };

    // Restore bankroll from DB if available
    let startup_conn = db::init(&config.general.db_path)?;
    let bankroll = if let Some(b) = bankroll_override {
        b
    } else if let Ok(Some(last)) = db::queries::last_bankroll(&startup_conn) {
        tracing::info!(
            bankroll = format_args!("${last:.2}"),
            "restored bankroll from DB"
        );
        last
    } else {
        config.bankroll.initial
    };
    let restored_positions = db::queries::load_open_positions(&startup_conn).unwrap_or_default();
    let restored_markets =
        db::queries::load_markets_for_open_positions(&startup_conn).unwrap_or_default();
    let next_decision_id = db::queries::max_decision_id(&startup_conn).unwrap_or(0) + 1;
    drop(startup_conn);

    tracing::info!(
        mode = mode_str,
        bankroll = format_args!("${bankroll:.2}"),
        "loaded config"
    );

    // Build LiveTrader for live mode
    let live_trader: Option<LiveTrader> = if paper_trade {
        None
    } else {
        let private_key =
            get_private_key().map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;

        Some(
            LiveTrader::connect(&private_key)
                .await
                .map_err(|e| -> Box<dyn std::error::Error> { e })?,
        )
    };

    // Shutdown watch channel
    let (shutdown_tx, shutdown_rx) = watch::channel(false);

    // Create all mpsc channels
    let (db_tx, db_rx) = mpsc::channel::<DbEvent>(10_000);
    let (spot_tx, mut spot_rx_fanout) = mpsc::channel::<SpotTick>(5_000);
    let (spot_tx_signal, spot_rx_signal) = mpsc::channel::<SpotTick>(5_000);
    let (market_tx_signal, market_rx_signal) = mpsc::channel::<MarketState>(100);
    let (market_tx_decision, market_rx_decision) = mpsc::channel::<MarketState>(100);
    let (signal_tx, signal_rx) = mpsc::channel::<Signal>(1_000);
    let (decision_in_tx, decision_in_rx) = mpsc::channel::<DecisionInput>(200);
    let (decision_out_tx, mut decision_out_rx) = mpsc::channel::<DecisionOutput>(100);
    let (settle_tx, mut settle_rx) = mpsc::channel::<SettleCommand>(100);
    let (market_reg_tx, mut market_reg_rx) = mpsc::channel::<MarketState>(100);

    // Send config snapshot to DB
    {
        let config_json = serde_json::to_string(&config)?;
        let _ = db_tx
            .send(DbEvent::ConfigSnapshot {
                config_json,
                ts: now_micros(),
            })
            .await;
    }

    // Spawn all actors

    // Writer actor
    let mut writer = WriterActor::new(
        &config.general.db_path,
        config.writer.batch_size,
        config.writer.flush_interval_ms,
    )?;
    let writer_handle = tokio::spawn(async move {
        writer.run(db_rx).await;
    });

    // Ingest actor (Binance spot prices)
    let ingest = IngestActor::new(config.clone());
    let ingest_db_tx = db_tx.clone();
    let ingest_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        ingest.run(spot_tx, ingest_db_tx, ingest_shutdown).await;
    });

    // Spot price fan-out
    tokio::spawn(async move {
        while let Some(sp) = spot_rx_fanout.recv().await {
            let _ = spot_tx_signal.try_send(sp);
        }
    });

    // Signal actor
    let warm_trackers = {
        let warm_conn = db::init(&config.general.db_path)?;
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
                    s.slow_variance,
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
                let _ = market_reg_tx.send(ms.clone()).await;
                if tx.send(DecisionInput::Market(ms)).await.is_err() {
                    break;
                }
            }
        });
    }
    let bankroll_tx = decision_in_tx.clone();
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
        config.strategy.adapt.clone(),
        config.strategy.midpoint_ema_tau_secs,
        config.strategy.min_displacement_pct,
    );
    tokio::spawn(async move {
        decision_actor.run().await;
    });

    // Market fetcher
    let fetcher_client =
        PolymarketClient::new(&config.polymarket.gamma_url, &config.polymarket.clob_url);
    let effective_window = match window_filter {
        cli::WindowFilter::All => cli::WindowFilter::from_enabled(&config.markets.enabled),
        ref w => w.clone(),
    };
    let fetcher = MarketFetcher::new(
        fetcher_client,
        asset_filter.clone(),
        effective_window,
        config.polymarket.poll_interval_secs,
        config.strategy.max_spread,
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
                restored_markets,
            )
            .await;
    });

    // Telegram alert actor
    let (telegram_tx, tg_stats): (
        Option<mpsc::Sender<TelegramAlert>>,
        Option<Arc<TelegramStats>>,
    ) = if let Some(ref tg) = config.telegram {
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
                "telegram alerts enabled"
            );
            (Some(tg_tx), Some(stats))
        } else {
            (None, None)
        }
    } else {
        (None, None)
    };
    let exec_tg_stats = tg_stats.clone();

    // Executor task
    let exec_db_tx = db_tx.clone();
    let mut exec_shutdown = shutdown_rx.clone();
    let exec_bankroll_tx = bankroll_tx;
    let exec_handle = tokio::spawn(async move {
        let exec_mode = if paper_trade { Mode::Paper } else { Mode::Live };
        let mut executor = Executor::new(
            exec_mode,
            bankroll,
            live_trader,
            config.strategy.max_total_exposure,
        );
        executor.set_next_decision_id(next_decision_id);

        for pos in &restored_positions {
            let side = match pos.side.as_str() {
                "YES" => Side::Yes,
                _ => Side::No,
            };
            executor.restore_position(
                pos.decision_id,
                pos.market_id.clone(),
                side,
                pos.entry_price,
                pos.size,
                pos.fee_rate,
                pos.entry_ts,
                pos.estimated_slippage,
            );
            tracing::info!(
                market = %pos.market_id,
                side = %pos.side,
                size = format_args!("${:.2}", pos.size),
                price = format_args!("{:.4}", pos.entry_price),
                "restored open position"
            );
        }

        let execution_config = config.execution.clone();
        let mut poll_interval = tokio::time::interval(tokio::time::Duration::from_secs(
            execution_config.order_poll_interval_secs,
        ));
        poll_interval.tick().await;

        let mut trades_placed: u32 = 0;
        let mut trades_skipped: u32 = 0;
        let mut fill_rejections: u32 = 0;
        let mut markets_resolved: u32 = 0;
        let mut wins: u32 = 0;
        let mut losses: u32 = 0;
        let mut total_fees: f64 = 0.0;
        let mut total_pnl: f64 = 0.0;

        loop {
            tokio::select! {
                msg = market_reg_rx.recv() => {
                    if let Some(ms) = msg {
                        executor.register_market(
                            &ms.market_id,
                            &ms.token_yes,
                            &ms.token_no,
                            ms.resolution_ts,
                        );
                    }
                }

                msg = decision_out_rx.recv() => {
                    match msg {
                        Some(DecisionOutput::Trade(dec)) => {
                            if exec_mode == Mode::Live {
                                let res_ts = executor.market_resolution_ts(&dec.market_id);
                                match executor.try_place_gtd(
                                    &dec,
                                    dec.best_ask,
                                    dec.best_bid,
                                    execution_config.gtd_expiry_secs,
                                    res_ts,
                                    execution_config.min_time_before_resolution_secs,
                                    execution_config.gtd_price_bump,
                                ).await {
                                    Ok(GtdResult::InstantFill(fill)) => {
                                        trades_placed += 1;
                                        if let Some(ref stats) = exec_tg_stats {
                                            stats.record_fill();
                                        }
                                        if let Some(ref tg) = telegram_tx {
                                            let _ = tg.try_send(
                                                TelegramAlert::GtdOrderFilled {
                                                    market_id: fill.market_id.clone(),
                                                    side: fill.side,
                                                    price: fill.fill_price,
                                                    maker: true,
                                                },
                                            );
                                        }
                                        let _ = exec_db_tx.try_send(
                                            DbEvent::SaveOpenPosition {
                                                decision_id: fill.decision_id,
                                                market_id: fill.market_id.clone(),
                                                side: fill.side,
                                                entry_price: fill.fill_price,
                                                size: fill.size_shares,
                                                fee_rate: fill.fee_rate,
                                                entry_ts: fill.entry_ts,
                                                estimated_slippage: 0.0,
                                            },
                                        );
                                        let _ = exec_db_tx.try_send(DbEvent::Decision(dec));
                                        let _ = exec_bankroll_tx.try_send(
                                            DecisionInput::BankrollUpdate(executor.bankroll()),
                                        );
                                    }
                                    Ok(GtdResult::Resting(_order_id)) => {
                                        if let Some(ref tg) = telegram_tx {
                                            let _ = tg.try_send(
                                                TelegramAlert::GtdOrderPosted {
                                                    market_id: dec.market_id.clone(),
                                                    side: dec.side,
                                                    price: dec.price,
                                                    expiry_secs: execution_config.gtd_expiry_secs,
                                                },
                                            );
                                        }
                                        let _ = exec_db_tx.try_send(DbEvent::Decision(dec));
                                    }
                                    Err(reason) => {
                                        fill_rejections += 1;
                                        let _ = exec_bankroll_tx.try_send(
                                            DecisionInput::PositionClosed(dec.market_id.clone()),
                                        );
                                        let _ = exec_db_tx.try_send(
                                            DbEvent::FillRejection {
                                                market_id: dec.market_id.clone(),
                                                side: dec.side,
                                                size: dec.size_usd,
                                                price: dec.price,
                                                reason,
                                                ts: dec.ts,
                                            },
                                        );
                                    }
                                }
                            } else {
                                // Paper mode
                                match executor.try_fill(&dec, dec.best_ask, dec.best_bid).await {
                                    Ok(fill) => {
                                        trades_placed += 1;
                                        if let Some(ref stats) = exec_tg_stats {
                                            stats.record_fill();
                                        }
                                        if let Some(ref tg) = telegram_tx {
                                            let _ = tg.try_send(
                                                TelegramAlert::TradeFilled {
                                                    decision: dec.clone(),
                                                    fill_price: fill.fill_price,
                                                },
                                            );
                                        }
                                        let _ = exec_db_tx.try_send(
                                            DbEvent::SaveOpenPosition {
                                                decision_id: fill.decision_id,
                                                market_id: dec.market_id.clone(),
                                                side: dec.side,
                                                entry_price: fill.fill_price,
                                                size: fill.size_shares,
                                                fee_rate: dec.fee_rate,
                                                entry_ts: dec.ts,
                                                estimated_slippage: fill.estimated_slippage,
                                            },
                                        );
                                        let _ = exec_db_tx.try_send(DbEvent::Decision(dec));
                                        let _ = exec_bankroll_tx.try_send(
                                            DecisionInput::BankrollUpdate(executor.bankroll()),
                                        );
                                    }
                                    Err(reason) => {
                                        fill_rejections += 1;
                                        let _ = exec_bankroll_tx.try_send(
                                            DecisionInput::PositionClosed(dec.market_id.clone()),
                                        );
                                        let _ = exec_db_tx.try_send(
                                            DbEvent::FillRejection {
                                                market_id: dec.market_id.clone(),
                                                side: dec.side,
                                                size: dec.size_usd,
                                                price: dec.price,
                                                reason,
                                                ts: dec.ts,
                                            },
                                        );
                                    }
                                }
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
                        let _ = exec_db_tx.try_send(DbEvent::ClearOpenPositions {
                            market_id: cmd.market_id.clone(),
                        });
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
                                size_shares = format_args!("{:.2}", tr.size_shares),
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
                        let _ = exec_bankroll_tx.try_send(
                            DecisionInput::BankrollUpdate(executor.bankroll()),
                        );
                        let _ = exec_bankroll_tx.try_send(
                            DecisionInput::PositionClosed(cmd.market_id.clone()),
                        );
                    }
                }

                _ = poll_interval.tick(), if exec_mode == Mode::Live => {
                    let completed = executor.poll_active_orders(
                        execution_config.gtd_expiry_secs,
                        execution_config.max_signal_age_secs,
                        execution_config.fok_price_bump,
                    ).await;
                    for completion in completed {
                        if let Some(ref fill) = completion.fill {
                            trades_placed += 1;
                            if let Some(ref stats) = exec_tg_stats {
                                stats.record_fill();
                            }
                            if let Some(ref tg) = telegram_tx {
                                let _ = tg.try_send(
                                    TelegramAlert::GtdOrderFilled {
                                        market_id: fill.market_id.clone(),
                                        side: fill.side,
                                        price: fill.fill_price,
                                        maker: completion.maker,
                                    },
                                );
                            }
                            let _ = exec_db_tx.try_send(
                                DbEvent::SaveOpenPosition {
                                    decision_id: fill.decision_id,
                                    market_id: fill.market_id.clone(),
                                    side: fill.side,
                                    entry_price: fill.fill_price,
                                    size: fill.size_shares,
                                    fee_rate: fill.fee_rate,
                                    entry_ts: fill.entry_ts,
                                    estimated_slippage: 0.0,
                                },
                            );
                            let _ = exec_bankroll_tx.try_send(
                                DecisionInput::BankrollUpdate(executor.bankroll()),
                            );
                        } else {
                            fill_rejections += 1;
                            let _ = exec_bankroll_tx.try_send(
                                DecisionInput::PositionClosed(completion.market_id),
                            );
                        }
                    }
                }

                _ = exec_shutdown.changed() => {
                    executor.cancel_all_active_orders().await;
                    while let Ok(cmd) = settle_rx.try_recv() {
                        markets_resolved += 1;
                        let _ = exec_db_tx.try_send(DbEvent::ClearOpenPositions {
                            market_id: cmd.market_id.clone(),
                        });
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

        (
            executor.bankroll(),
            trades_placed,
            trades_skipped,
            fill_rejections,
            markets_resolved,
            wins,
            losses,
            total_fees,
            total_pnl,
        )
    });

    // Log startup info
    tracing::info!(
        "poly v{} — {mode_str} mode — ${bankroll:.2} bankroll",
        env!("CARGO_PKG_VERSION"),
    );
    tracing::info!("asset={:?} window={:?}", asset_filter, window_filter);
    if paper_trade {
        tracing::info!("paper-trade mode: real market data, simulated execution");
    } else {
        tracing::info!("REAL trading mode: orders will be placed on Polymarket");
    }
    tracing::info!("press Ctrl+C to stop");

    // Wait for ctrl+c
    tokio::signal::ctrl_c().await?;
    tracing::info!("shutting down...");
    let _ = shutdown_tx.send(true);

    drop(db_tx);

    if let Ok((
        final_bankroll,
        trades_placed,
        trades_skipped,
        fill_rejections,
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
        tracing::info!("  Fill rejections:    {fill_rejections}");
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

    let _ = writer_handle.await;
    tracing::info!("shutdown complete");
    Ok(())
}
