use polymarket_bot::actors::executor::PaperExecutor;
use polymarket_bot::types::*;

#[test]
fn test_executor_fill_win() {
    let mut exec = PaperExecutor::new(100_000.0);
    let dec = TradeDecision {
        market_id: "mkt-1".into(),
        side: Side::Yes,
        size: 1000.0,
        price: 0.50,
        edge: 0.15,
        effective_edge: 0.12,
        fee_rate: 0.03,
        kelly_fraction: 0.10,
        ts: 1000000,
    };
    let best_ask = 0.52;
    let fill = exec.try_fill(&dec, best_ask, 0.48);
    assert!(fill.is_some());
    assert_eq!(exec.position_count(), 1);
}

#[test]
fn test_executor_fill_rejected_price_slipped() {
    let mut exec = PaperExecutor::new(100_000.0);
    let dec = TradeDecision {
        market_id: "mkt-1".into(),
        side: Side::Yes,
        size: 1000.0,
        price: 0.50,
        edge: 0.15,
        effective_edge: 0.12,
        fee_rate: 0.03,
        kelly_fraction: 0.10,
        ts: 1000000,
    };
    let fill = exec.try_fill(&dec, 0.90, 0.10);
    assert!(fill.is_none());
}

#[test]
fn test_executor_settle_win() {
    let mut exec = PaperExecutor::new(100_000.0);
    let dec = TradeDecision {
        market_id: "mkt-1".into(),
        side: Side::Yes,
        size: 1000.0,
        price: 0.50,
        edge: 0.15,
        effective_edge: 0.12,
        fee_rate: 0.03,
        kelly_fraction: 0.10,
        ts: 1000000,
    };
    exec.try_fill(&dec, 0.52, 0.48);
    let results = exec.settle("mkt-1", Side::Yes, 2000000);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, Outcome::Win);
    assert!(results[0].pnl > 0.0);
    assert!(exec.bankroll() > 100_000.0);
}

#[test]
fn test_executor_settle_loss() {
    let mut exec = PaperExecutor::new(100_000.0);
    let dec = TradeDecision {
        market_id: "mkt-1".into(),
        side: Side::Yes,
        size: 1000.0,
        price: 0.50,
        edge: 0.15,
        effective_edge: 0.12,
        fee_rate: 0.03,
        kelly_fraction: 0.10,
        ts: 1000000,
    };
    exec.try_fill(&dec, 0.52, 0.48);
    let results = exec.settle("mkt-1", Side::No, 2000000);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, Outcome::Loss);
    assert!(results[0].pnl < 0.0);
    assert!(exec.bankroll() < 100_000.0);
}
