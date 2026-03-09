use polymarket_bot::actors::decision::{decide, polymarket_fee_rate};
use polymarket_bot::actors::executor::{Executor, Mode};
use polymarket_bot::actors::signal::MarketWindow;
use polymarket_bot::types::*;

#[tokio::test]
async fn test_full_pipeline_paper_trade() {
    // 1. Build a MarketWindow and feed 20 positive returns
    let mut window = MarketWindow::new(0.001);
    for i in 0..20 {
        window.update(0.001, 0.003, i as f64);
    }

    let p_hat = window.p_hat();
    assert!(p_hat > 0.6, "expected p_hat > 0.6, got {p_hat}");

    // 2. Run the decision engine with real Polymarket fee formula
    let p_market = 0.50;
    let fee_rate = polymarket_fee_rate(p_market);
    let result = decide(
        p_hat,
        p_market,
        fee_rate,
        0.05,                // tau_min
        100_000.0,           // b (LMSR liquidity)
        0.5,                 // kelly_fraction
        100_000.0,           // bankroll
        50_000.0,            // volume_24h
        0.02,                // max_volume_pct
        0.10,                // max_bet_fraction
        0.10,                // min_confidence (low threshold)
        window.confidence(), // confidence
        "test-mkt",
    );

    let decision = result.expect("expected a trade decision, got skip");
    assert_eq!(decision.side, Side::Yes, "expected Yes side");
    assert!(decision.size > 0.0, "expected positive size");

    // 3. Paper-execute the trade
    let mut executor = Executor::new(Mode::Paper, 100_000.0, None, 0.50);
    let fill_id = executor.try_fill(&decision, 0.52, 0.48).await;
    assert!(fill_id.is_some(), "expected fill to succeed");

    // 4. Settle: BTC went up, resolved YES (correct prediction)
    let results = executor.settle("test-mkt", Side::Yes, now_micros());
    assert_eq!(results.len(), 1);

    let tr = &results[0];
    assert_eq!(tr.outcome, Outcome::Win);
    assert!(tr.pnl > 0.0, "expected positive pnl, got {}", tr.pnl);
    assert!(
        executor.bankroll() > 100_000.0,
        "expected bankroll > 100k, got {}",
        executor.bankroll()
    );
}

#[test]
fn test_full_pipeline_skip_low_edge() {
    // Feed only 2 near-zero returns — edge will be negligible
    let mut window = MarketWindow::new(0.001);
    for i in 0..2 {
        window.update(0.00001, 0.003, i as f64);
    }

    let p_hat = window.p_hat();

    // At p=0.50 the Polymarket fee is ~1.56%, which should swamp the tiny edge
    let p_market = 0.50;
    let fee_rate = polymarket_fee_rate(p_market);
    let result = decide(
        p_hat,
        p_market,
        fee_rate,
        0.05, // tau_min
        100_000.0,
        0.5,
        100_000.0,
        50_000.0,
        0.02,
        0.10, // max_bet_fraction
        0.10,
        window.confidence(),
        "test-mkt-2",
    );

    assert!(
        result.is_err(),
        "expected skip (Err), but got Ok: {:#?}",
        result.unwrap()
    );
}
