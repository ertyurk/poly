use std::collections::BTreeSet;
use std::path::Path;

use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;

type BoxError = Box<dyn std::error::Error + Send + Sync>;

#[derive(Debug, Clone, Serialize)]
pub struct DashboardPayload {
    pub generated_at: String,
    pub db_path: String,
    pub initial_bankroll: f64,
    pub realized_bankroll: f64,
    pub estimated_total_equity: f64,
    pub trades: Vec<TradeRow>,
    pub open_positions: Vec<OpenPositionRow>,
    pub skips: Vec<SkipRow>,
    pub fill_rejections: Vec<FillRejectionRow>,
    pub spot_points: Vec<SpotPoint>,
    pub filters: FilterOptions,
}

#[derive(Debug, Clone, Serialize)]
pub struct FilterOptions {
    pub assets: Vec<String>,
    pub windows: Vec<String>,
    pub sides: Vec<String>,
    pub outcomes: Vec<String>,
}

impl FilterOptions {
    fn build(
        trades: &[TradeRow],
        open_positions: &[OpenPositionRow],
        skips: &[SkipRow],
        fill_rejections: &[FillRejectionRow],
        spot_points: &[SpotPoint],
    ) -> Self {
        let mut assets = BTreeSet::new();
        let mut windows = BTreeSet::new();
        let mut sides = BTreeSet::new();
        let mut outcomes = BTreeSet::new();

        for trade in trades {
            assets.insert(trade.asset.clone());
            windows.insert(trade.window.clone());
            sides.insert(trade.side.clone());
            outcomes.insert(trade.outcome.clone());
        }

        for position in open_positions {
            assets.insert(position.asset.clone());
            windows.insert(position.window.clone());
            sides.insert(position.side.clone());
        }

        for skip in skips {
            assets.insert(skip.asset.clone());
            windows.insert(skip.window.clone());
        }

        for rejection in fill_rejections {
            assets.insert(rejection.asset.clone());
            windows.insert(rejection.window.clone());
            sides.insert(rejection.side.clone());
        }

        for spot in spot_points {
            assets.insert(spot.asset.clone());
        }

        Self {
            assets: assets.into_iter().collect(),
            windows: windows.into_iter().collect(),
            sides: sides.into_iter().collect(),
            outcomes: outcomes.into_iter().collect(),
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TradeRow {
    pub decision_id: i64,
    pub market_id: String,
    pub asset: String,
    pub window: String,
    pub side: String,
    pub outcome: String,
    pub entry_price: f64,
    pub size_shares: f64,
    pub size_usd: Option<f64>,
    pub fee_rate: f64,
    pub fee_paid: f64,
    pub gross_pnl: f64,
    pub pnl: f64,
    pub bankroll_after: f64,
    pub entry_ts: i64,
    pub resolved_ts: i64,
    pub duration_secs: f64,
    pub decision_price: Option<f64>,
    pub edge: Option<f64>,
    pub effective_edge: Option<f64>,
    pub kelly_fraction: Option<f64>,
    pub open_price: Option<f64>,
    pub resolved_side: Option<String>,
    pub entry_spread: Option<f64>,
    pub entry_best_bid: Option<f64>,
    pub entry_best_ask: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct OpenPositionRow {
    pub decision_id: i64,
    pub market_id: String,
    pub asset: String,
    pub window: String,
    pub side: String,
    pub entry_price: f64,
    pub size_shares: f64,
    pub fee_rate: f64,
    pub entry_ts: i64,
    pub estimated_slippage: f64,
    pub cost_basis: f64,
    pub estimated_entry_fee: f64,
    pub open_price: Option<f64>,
    pub resolution_ts: Option<i64>,
    pub latest_best_bid: Option<f64>,
    pub latest_best_ask: Option<f64>,
    pub latest_midpoint: Option<f64>,
    pub latest_spread: Option<f64>,
    pub latest_book_ts: Option<i64>,
    pub mark_price: Option<f64>,
    pub current_value: Option<f64>,
    pub unrealized_pnl_gross: Option<f64>,
    pub unrealized_pnl_net: Option<f64>,
    pub minutes_to_resolution: Option<f64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SkipRow {
    pub market_id: String,
    pub asset: String,
    pub window: String,
    pub skip_reason: String,
    pub edge: f64,
    pub effective_edge: f64,
    pub ts: i64,
    pub resolved_side: String,
    pub would_have_side: String,
    pub would_have_won: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct FillRejectionRow {
    pub market_id: String,
    pub asset: String,
    pub window: String,
    pub side: String,
    pub size: f64,
    pub price: f64,
    pub reason: String,
    pub ts: i64,
}

#[derive(Debug, Clone, Serialize)]
pub struct SpotPoint {
    pub asset: String,
    pub price: f64,
    pub ts: i64,
}

pub fn load_dashboard_payload<P: AsRef<Path>>(db_path: P) -> Result<DashboardPayload, BoxError> {
    let path = db_path.as_ref();
    if !path.exists() {
        return Err(format!("database file not found: {}", path.display()).into());
    }

    let conn = crate::db::init(&path.to_string_lossy())?;
    let trades = load_trades(&conn)?;
    let open_positions = load_open_positions(&conn)?;
    let skips = load_skips(&conn)?;
    let fill_rejections = load_fill_rejections(&conn)?;
    let spot_points = load_spot_points(&conn)?;
    let initial_bankroll = infer_initial_bankroll(&conn, &trades)?;
    let realized_bankroll = trades
        .last()
        .map_or(initial_bankroll, |trade| trade.bankroll_after);
    let estimated_total_equity = realized_bankroll
        + open_positions
            .iter()
            .filter_map(|position| position.unrealized_pnl_net)
            .sum::<f64>();
    let filters = FilterOptions::build(
        &trades,
        &open_positions,
        &skips,
        &fill_rejections,
        &spot_points,
    );

    Ok(DashboardPayload {
        generated_at: chrono::Utc::now().to_rfc3339(),
        db_path: path.display().to_string(),
        initial_bankroll,
        realized_bankroll,
        estimated_total_equity,
        trades,
        open_positions,
        skips,
        fill_rejections,
        spot_points,
        filters,
    })
}

fn load_trades(conn: &Connection) -> Result<Vec<TradeRow>, BoxError> {
    let mut stmt = conn.prepare(
        "SELECT
            t.decision_id,
            t.market_id,
            COALESCE(m.asset, 'UNKNOWN') AS asset,
            COALESCE(m.window, 'unknown') AS window,
            t.side,
            t.outcome,
            t.entry_price,
            t.size,
            COALESCE(
                d_by_id.size,
                (
                    SELECT d_market.size
                    FROM decisions d_market
                    WHERE d_market.market_id = t.market_id
                      AND d_market.action = 'TRADE'
                    ORDER BY d_market.id DESC
                    LIMIT 1
                )
            ) AS size_usd,
            t.fee_rate,
            t.fee_paid,
            t.gross_pnl,
            t.pnl,
            t.bankroll_after,
            t.entry_ts,
            t.resolved_ts,
            COALESCE(
                d_by_id.price,
                (
                    SELECT d_market.price
                    FROM decisions d_market
                    WHERE d_market.market_id = t.market_id
                      AND d_market.action = 'TRADE'
                    ORDER BY d_market.id DESC
                    LIMIT 1
                )
            ) AS decision_price,
            COALESCE(
                d_by_id.edge,
                (
                    SELECT d_market.edge
                    FROM decisions d_market
                    WHERE d_market.market_id = t.market_id
                      AND d_market.action = 'TRADE'
                    ORDER BY d_market.id DESC
                    LIMIT 1
                )
            ) AS edge,
            COALESCE(
                d_by_id.effective_edge,
                (
                    SELECT d_market.effective_edge
                    FROM decisions d_market
                    WHERE d_market.market_id = t.market_id
                      AND d_market.action = 'TRADE'
                    ORDER BY d_market.id DESC
                    LIMIT 1
                )
            ) AS effective_edge,
            COALESCE(
                d_by_id.kelly_fraction,
                (
                    SELECT d_market.kelly_fraction
                    FROM decisions d_market
                    WHERE d_market.market_id = t.market_id
                      AND d_market.action = 'TRADE'
                    ORDER BY d_market.id DESC
                    LIMIT 1
                )
            ) AS kelly_fraction,
            m.open_price,
            m.resolved_side,
            (
                SELECT bs.spread
                FROM book_snapshots bs
                WHERE bs.market_id = t.market_id
                ORDER BY bs.ts ASC
                LIMIT 1
            ) AS entry_spread,
            (
                SELECT bs.best_bid
                FROM book_snapshots bs
                WHERE bs.market_id = t.market_id
                ORDER BY bs.ts ASC
                LIMIT 1
            ) AS entry_best_bid,
            (
                SELECT bs.best_ask
                FROM book_snapshots bs
                WHERE bs.market_id = t.market_id
                ORDER BY bs.ts ASC
                LIMIT 1
            ) AS entry_best_ask
        FROM trades t
        LEFT JOIN decisions d_by_id
          ON d_by_id.id = t.decision_id
         AND d_by_id.action = 'TRADE'
        LEFT JOIN markets m ON m.market_id = t.market_id
        ORDER BY t.resolved_ts ASC, t.decision_id ASC",
    )?;

    let rows = stmt.query_map([], |row| {
        let entry_ts: i64 = row.get(14)?;
        let resolved_ts: i64 = row.get(15)?;

        Ok(TradeRow {
            decision_id: row.get(0)?,
            market_id: row.get(1)?,
            asset: row.get(2)?,
            window: row.get(3)?,
            side: row.get(4)?,
            outcome: row.get(5)?,
            entry_price: row.get(6)?,
            size_shares: row.get(7)?,
            size_usd: row.get(8)?,
            fee_rate: row.get(9)?,
            fee_paid: row.get(10)?,
            gross_pnl: row.get(11)?,
            pnl: row.get(12)?,
            bankroll_after: row.get(13)?,
            entry_ts,
            resolved_ts,
            duration_secs: ((resolved_ts - entry_ts) as f64) / 1_000_000.0,
            decision_price: row.get(16)?,
            edge: row.get(17)?,
            effective_edge: row.get(18)?,
            kelly_fraction: row.get(19)?,
            open_price: row.get(20)?,
            resolved_side: row.get(21)?,
            entry_spread: row.get(22)?,
            entry_best_bid: row.get(23)?,
            entry_best_ask: row.get(24)?,
        })
    })?;

    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn load_open_positions(conn: &Connection) -> Result<Vec<OpenPositionRow>, BoxError> {
    if !table_exists(conn, "open_positions")? {
        return Ok(Vec::new());
    }

    let now = chrono::Utc::now().timestamp_micros();
    let mut stmt = conn.prepare(
        "SELECT
            op.decision_id,
            op.market_id,
            COALESCE(m.asset, 'UNKNOWN') AS asset,
            COALESCE(m.window, 'unknown') AS window,
            op.side,
            op.entry_price,
            op.size,
            op.fee_rate,
            op.entry_ts,
            op.estimated_slippage,
            m.open_price,
            m.resolution_ts,
            (
                SELECT bs.best_bid
                FROM book_snapshots bs
                WHERE bs.market_id = op.market_id
                ORDER BY bs.ts DESC
                LIMIT 1
            ) AS latest_best_bid,
            (
                SELECT bs.best_ask
                FROM book_snapshots bs
                WHERE bs.market_id = op.market_id
                ORDER BY bs.ts DESC
                LIMIT 1
            ) AS latest_best_ask,
            (
                SELECT bs.midpoint
                FROM book_snapshots bs
                WHERE bs.market_id = op.market_id
                ORDER BY bs.ts DESC
                LIMIT 1
            ) AS latest_midpoint,
            (
                SELECT bs.spread
                FROM book_snapshots bs
                WHERE bs.market_id = op.market_id
                ORDER BY bs.ts DESC
                LIMIT 1
            ) AS latest_spread,
            (
                SELECT bs.ts
                FROM book_snapshots bs
                WHERE bs.market_id = op.market_id
                ORDER BY bs.ts DESC
                LIMIT 1
            ) AS latest_book_ts
        FROM open_positions op
        LEFT JOIN markets m ON m.market_id = op.market_id
        ORDER BY op.entry_ts DESC",
    )?;

    let rows = stmt.query_map([], |row| {
        let side: String = row.get(4)?;
        let entry_price: f64 = row.get(5)?;
        let size_shares: f64 = row.get(6)?;
        let fee_rate: f64 = row.get(7)?;
        let resolution_ts: Option<i64> = row.get(11)?;
        let latest_best_bid: Option<f64> = row.get(12)?;
        let latest_best_ask: Option<f64> = row.get(13)?;
        let latest_midpoint: Option<f64> = row.get(14)?;
        let latest_spread: Option<f64> = row.get(15)?;
        let latest_book_ts: Option<i64> = row.get(16)?;

        let cost_basis = entry_price * size_shares;
        let estimated_entry_fee = cost_basis * fee_rate;
        let mark_price = match side.as_str() {
            "YES" => latest_best_bid,
            "NO" => latest_best_ask.map(|ask| 1.0 - ask),
            _ => latest_midpoint,
        };
        let current_value = mark_price.map(|price| price * size_shares);
        let unrealized_pnl_gross = current_value.map(|value| value - cost_basis);
        let unrealized_pnl_net = unrealized_pnl_gross.map(|pnl| pnl - estimated_entry_fee);
        let minutes_to_resolution = resolution_ts.map(|ts| (ts - now) as f64 / 60_000_000.0);

        Ok(OpenPositionRow {
            decision_id: row.get(0)?,
            market_id: row.get(1)?,
            asset: row.get(2)?,
            window: row.get(3)?,
            side,
            entry_price,
            size_shares,
            fee_rate,
            entry_ts: row.get(8)?,
            estimated_slippage: row.get(9)?,
            cost_basis,
            estimated_entry_fee,
            open_price: row.get(10)?,
            resolution_ts,
            latest_best_bid,
            latest_best_ask,
            latest_midpoint,
            latest_spread,
            latest_book_ts,
            mark_price,
            current_value,
            unrealized_pnl_gross,
            unrealized_pnl_net,
            minutes_to_resolution,
        })
    })?;

    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn load_skips(conn: &Connection) -> Result<Vec<SkipRow>, BoxError> {
    let mut stmt = conn.prepare(
        "SELECT
            d.market_id,
            COALESCE(m.asset, 'UNKNOWN') AS asset,
            COALESCE(m.window, 'unknown') AS window,
            COALESCE(d.skip_reason, 'UNKNOWN') AS skip_reason,
            d.edge,
            d.effective_edge,
            d.ts,
            m.resolved_side,
            CASE WHEN d.edge > 0 THEN 'YES' ELSE 'NO' END AS would_have_side,
            CASE
                WHEN m.resolved_side = CASE WHEN d.edge > 0 THEN 'YES' ELSE 'NO' END
                THEN 1
                ELSE 0
            END AS would_have_won
        FROM decisions d
        JOIN markets m ON m.market_id = d.market_id
        WHERE d.action = 'SKIP'
          AND m.resolved_side IS NOT NULL
        ORDER BY d.ts DESC",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(SkipRow {
            market_id: row.get(0)?,
            asset: row.get(1)?,
            window: row.get(2)?,
            skip_reason: row.get(3)?,
            edge: row.get(4)?,
            effective_edge: row.get(5)?,
            ts: row.get(6)?,
            resolved_side: row.get(7)?,
            would_have_side: row.get(8)?,
            would_have_won: row.get::<_, i64>(9)? != 0,
        })
    })?;

    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn load_fill_rejections(conn: &Connection) -> Result<Vec<FillRejectionRow>, BoxError> {
    if !table_exists(conn, "fill_rejections")? {
        return Ok(Vec::new());
    }

    let mut stmt = conn.prepare(
        "SELECT
            fr.market_id,
            COALESCE(m.asset, 'UNKNOWN') AS asset,
            COALESCE(m.window, 'unknown') AS window,
            fr.side,
            fr.size,
            fr.price,
            fr.reason,
            fr.ts
        FROM fill_rejections fr
        LEFT JOIN markets m ON m.market_id = fr.market_id
        ORDER BY fr.ts DESC",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(FillRejectionRow {
            market_id: row.get(0)?,
            asset: row.get(1)?,
            window: row.get(2)?,
            side: row.get(3)?,
            size: row.get(4)?,
            price: row.get(5)?,
            reason: row.get(6)?,
            ts: row.get(7)?,
        })
    })?;

    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn load_spot_points(conn: &Connection) -> Result<Vec<SpotPoint>, BoxError> {
    let mut stmt = conn.prepare(
        "SELECT asset, price, ts
         FROM (
            SELECT
                asset,
                price,
                ts,
                ROW_NUMBER() OVER (PARTITION BY asset ORDER BY ts DESC) AS rn
            FROM spot_prices
         ) recent
         WHERE rn <= 240
         ORDER BY asset ASC, ts ASC",
    )?;

    let rows = stmt.query_map([], |row| {
        Ok(SpotPoint {
            asset: row.get(0)?,
            price: row.get(1)?,
            ts: row.get(2)?,
        })
    })?;

    rows.collect::<Result<Vec<_>, _>>().map_err(Into::into)
}

fn infer_initial_bankroll(conn: &Connection, trades: &[TradeRow]) -> Result<f64, BoxError> {
    if let Some(first_trade) = trades.first() {
        return Ok(first_trade.bankroll_after - first_trade.pnl);
    }

    let config_json: Option<String> = conn
        .query_row(
            "SELECT config_json FROM config_snapshots ORDER BY ts DESC LIMIT 1",
            [],
            |row| row.get(0),
        )
        .optional()?;

    if let Some(config_json) = config_json {
        let value: serde_json::Value = serde_json::from_str(&config_json)?;
        if let Some(initial) = value
            .get("bankroll")
            .and_then(|v| v.get("initial"))
            .and_then(serde_json::Value::as_f64)
        {
            return Ok(initial);
        }
    }

    Ok(0.0)
}

fn table_exists(conn: &Connection, table_name: &str) -> Result<bool, rusqlite::Error> {
    let exists = conn.query_row(
        "SELECT EXISTS(
            SELECT 1
            FROM sqlite_master
            WHERE type = 'table' AND name = ?1
        )",
        [table_name],
        |row| row.get::<_, i64>(0),
    )?;
    Ok(exists != 0)
}

#[cfg(test)]
mod tests {
    use super::load_dashboard_payload;
    use crate::db;
    use crate::db::queries::PersistedPosition;
    use crate::types::{
        Asset, MarketState, MarketType, Outcome, Side, SpotPrice, TradeDecision, TradeResult,
        Window,
    };
    use tempfile::NamedTempFile;

    #[test]
    fn dashboard_payload_loads_trade_data() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        let tmp = NamedTempFile::new()?;
        let path = tmp.path().to_string_lossy().to_string();
        let conn = db::init(&path)?;

        let settled_market = MarketState {
            market_id: "btc_5m_settled".into(),
            asset: Asset::BTC,
            window: Window::FiveMin,
            token_yes: "yes-token".into(),
            token_no: "no-token".into(),
            best_bid: 0.48,
            best_ask: 0.52,
            midpoint: 0.50,
            resolution_ts: 2_000_000,
            open_ts: 1_000_000,
            open_price: Some(85_000.0),
            volume_24h: 50_000.0,
            market_type: MarketType::UpDown,
            event_slug: "btc-updown-5m-123".into(),
        };
        db::queries::insert_market(&conn, &settled_market)?;
        db::queries::insert_book_snapshot(
            &conn,
            &settled_market.market_id,
            0.48,
            0.52,
            0.50,
            0.04,
            1_500_000,
        )?;

        let open_market = MarketState {
            market_id: "eth_5m_open".into(),
            asset: Asset::ETH,
            window: Window::FiveMin,
            token_yes: "yes-open".into(),
            token_no: "no-open".into(),
            best_bid: 0.40,
            best_ask: 0.42,
            midpoint: 0.41,
            resolution_ts: 9_000_000,
            open_ts: 3_000_000,
            open_price: Some(3_200.0),
            volume_24h: 45_000.0,
            market_type: MarketType::UpDown,
            event_slug: "eth-updown-5m-456".into(),
        };
        db::queries::insert_market(&conn, &open_market)?;
        db::queries::insert_book_snapshot(
            &conn,
            &open_market.market_id,
            0.40,
            0.42,
            0.41,
            0.02,
            4_500_000,
        )?;

        let decision = TradeDecision {
            market_id: settled_market.market_id.clone(),
            side: Side::No,
            size_usd: 100.0,
            price: 0.52,
            edge: 0.08,
            effective_edge: 0.05,
            fee_rate: 0.015,
            kelly_fraction: 0.10,
            best_bid: 0.48,
            best_ask: 0.52,
            ts: 1_550_000,
            event_slug: settled_market.event_slug.clone(),
        };
        let decision_id = db::queries::insert_decision(&conn, &decision)?;

        let trade = TradeResult {
            decision_id,
            market_id: settled_market.market_id.clone(),
            side: Side::No,
            entry_price: 0.52,
            size_shares: 192.3,
            fee_rate: 0.015,
            fee_paid: 1.50,
            gross_pnl: 12.0,
            outcome: Outcome::Win,
            pnl: 10.5,
            bankroll_after: 1_010.5,
            entry_ts: 1_550_000,
            resolved_ts: 2_000_000,
            estimated_slippage: 0.01,
            event_slug: settled_market.event_slug.clone(),
        };
        db::queries::insert_trade(&conn, &trade)?;

        let open_position = PersistedPosition {
            decision_id: 2,
            market_id: open_market.market_id.clone(),
            side: "NO".into(),
            entry_price: 0.35,
            size: 20.0,
            fee_rate: 0.01,
            entry_ts: 4_000_000,
            estimated_slippage: 0.0,
        };
        db::queries::save_open_position(&conn, &open_position)?;

        db::queries::insert_fill_rejection(
            &conn,
            &settled_market.market_id,
            "NO",
            50.0,
            0.55,
            "price_slippage",
            1_560_000,
        )?;

        db::queries::insert_spot_price(
            &conn,
            &SpotPrice {
                asset: Asset::BTC,
                price: 85_100.0,
                ts: 1_100_000,
            },
        )?;
        db::queries::insert_spot_price(
            &conn,
            &SpotPrice {
                asset: Asset::ETH,
                price: 3_250.0,
                ts: 4_600_000,
            },
        )?;
        drop(conn);

        let payload = load_dashboard_payload(&path)?;

        assert_eq!(payload.trades.len(), 1);
        assert_eq!(payload.open_positions.len(), 1);
        assert_eq!(payload.fill_rejections.len(), 1);
        assert_eq!(payload.spot_points.len(), 2);
        assert_eq!(payload.trades[0].asset, "BTC");
        assert_eq!(payload.open_positions[0].asset, "ETH");
        assert_eq!(payload.fill_rejections[0].window, "5m");
        assert!((payload.initial_bankroll - 1_000.0).abs() < 1e-9);
        assert!((payload.realized_bankroll - 1_010.5).abs() < 1e-9);
        assert!(payload.estimated_total_equity > payload.realized_bankroll);

        Ok(())
    }
}
