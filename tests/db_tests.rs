use polymarket_bot::db;
use polymarket_bot::types::*;
use tempfile::NamedTempFile;

#[test]
fn test_init_creates_tables() {
    let tmp = NamedTempFile::new().unwrap();
    let conn = db::init(tmp.path().to_str().unwrap()).unwrap();
    let count: i32 = conn
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name IN ('spot_prices','markets','book_snapshots','signals','decisions','trades','config_snapshots')",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 7);
}

#[test]
fn test_wal_mode_enabled() {
    let tmp = NamedTempFile::new().unwrap();
    let conn = db::init(tmp.path().to_str().unwrap()).unwrap();
    let mode: String = conn
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
}

#[test]
fn test_insert_spot_price() {
    let tmp = NamedTempFile::new().unwrap();
    let conn = db::init(tmp.path().to_str().unwrap()).unwrap();
    let sp = SpotPrice {
        asset: Asset::BTC,
        price: 85000.0,
        ts: 1000000,
    };
    db::queries::insert_spot_price(&conn, &sp).unwrap();
    let count: i32 = conn
        .query_row("SELECT COUNT(*) FROM spot_prices", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn test_insert_decision_and_trade() {
    let tmp = NamedTempFile::new().unwrap();
    let conn = db::init(tmp.path().to_str().unwrap()).unwrap();

    let ms = MarketState {
        market_id: "test-mkt-1".into(),
        asset: Asset::BTC,
        window: Window::FiveMin,
        token_yes: "tok_yes".into(),
        token_no: "tok_no".into(),
        best_bid: 0.48,
        best_ask: 0.52,
        midpoint: 0.50,
        resolution_ts: 2000000,
        open_ts: 1000000,
        open_price: Some(85000.0),
        volume_24h: 50000.0,
    };
    db::queries::insert_market(&conn, &ms).unwrap();

    let dec = TradeDecision {
        market_id: "test-mkt-1".into(),
        side: Side::Yes,
        size: 1000.0,
        price: 0.50,
        edge: 0.10,
        effective_edge: 0.07,
        fee_rate: 0.03,
        kelly_fraction: 0.10,
        ts: 1500000,
    };
    let decision_id = db::queries::insert_decision(&conn, &dec).unwrap();
    assert!(decision_id > 0);

    let tr = TradeResult {
        decision_id,
        market_id: "test-mkt-1".into(),
        side: Side::Yes,
        entry_price: 0.50,
        size: 1000.0,
        fee_paid: 30.0,
        outcome: Outcome::Win,
        pnl: 470.0,
        bankroll_after: 100_470.0,
        entry_ts: 1500000,
        resolved_ts: 2000000,
    };
    db::queries::insert_trade(&conn, &tr).unwrap();

    let count: i32 = conn
        .query_row("SELECT COUNT(*) FROM trades", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}
