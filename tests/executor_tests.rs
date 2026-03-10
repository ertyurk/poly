use polymarket_bot::actors::executor::{Executor, Mode};
use polymarket_bot::types::*;

#[tokio::test]
async fn test_executor_fill_win() {
    let mut exec = Executor::new(Mode::Paper, 100_000.0, None, 0.50);
    let dec = TradeDecision {
        market_id: "mkt-1".into(),
        side: Side::Yes,
        size_usd: 1000.0,
        price: 0.50,
        edge: 0.15,
        effective_edge: 0.12,
        fee_rate: 0.03,
        kelly_fraction: 0.10,
        best_bid: 0.48,
        best_ask: 0.52,
        ts: 1000000,
    };
    let best_ask = 0.52;
    let fill = exec.try_fill(&dec, best_ask, 0.48).await;
    assert!(fill.is_ok());
}

#[tokio::test]
async fn test_executor_fill_rejected_price_slipped() {
    let mut exec = Executor::new(Mode::Paper, 100_000.0, None, 0.50);
    let dec = TradeDecision {
        market_id: "mkt-1".into(),
        side: Side::Yes,
        size_usd: 1000.0,
        price: 0.50,
        edge: 0.15,
        effective_edge: 0.12,
        fee_rate: 0.03,
        kelly_fraction: 0.10,
        best_bid: 0.48,
        best_ask: 0.52,
        ts: 1000000,
    };
    let fill = exec.try_fill(&dec, 0.90, 0.10).await;
    assert!(fill.is_err());
}

#[tokio::test]
async fn test_executor_settle_win() {
    let mut exec = Executor::new(Mode::Paper, 100_000.0, None, 0.50);
    let dec = TradeDecision {
        market_id: "mkt-1".into(),
        side: Side::Yes,
        size_usd: 1000.0,
        price: 0.50,
        edge: 0.15,
        effective_edge: 0.12,
        fee_rate: 0.03,
        kelly_fraction: 0.10,
        best_bid: 0.48,
        best_ask: 0.52,
        ts: 1000000,
    };
    let _ = exec.try_fill(&dec, 0.52, 0.48).await;
    let results = exec.settle("mkt-1", Side::Yes, 2000000);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, Outcome::Win);
    assert!(results[0].pnl > 0.0);
    assert!(exec.bankroll() > 100_000.0);
}

#[tokio::test]
async fn test_executor_settle_loss() {
    let mut exec = Executor::new(Mode::Paper, 100_000.0, None, 0.50);
    let dec = TradeDecision {
        market_id: "mkt-1".into(),
        side: Side::Yes,
        size_usd: 1000.0,
        price: 0.50,
        edge: 0.15,
        effective_edge: 0.12,
        fee_rate: 0.03,
        kelly_fraction: 0.10,
        best_bid: 0.48,
        best_ask: 0.52,
        ts: 1000000,
    };
    let _ = exec.try_fill(&dec, 0.52, 0.48).await;
    let results = exec.settle("mkt-1", Side::No, 2000000);
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].outcome, Outcome::Loss);
    assert!(results[0].pnl < 0.0);
    assert!(exec.bankroll() < 100_000.0);
}
