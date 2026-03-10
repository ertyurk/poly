use polymarket_bot::actors::decision::decide;
use polymarket_bot::actors::executor::{Executor, Mode};
use polymarket_bot::types::*;

#[tokio::test]
async fn test_full_pipeline_paper_trade() {
    // Use a strong signal: p_hat=0.75 (high conviction YES)
    let p_hat: f64 = 0.75;
    let confidence = (p_hat - 0.5).abs() * 2.0; // 0.50

    let p_market = 0.50;
    let result = decide(
        p_hat,
        p_market,
        0.05,      // tau_min
        100_000.0, // b (LMSR liquidity)
        0.5,       // kelly_fraction
        100_000.0, // bankroll
        50_000.0,  // volume_24h
        0.02,      // max_volume_pct
        0.10,      // max_bet_fraction
        0.10,      // min_confidence
        confidence,
        "test-mkt",
        0.48, // best_bid
        0.52, // best_ask
        "",   // event_slug
    );

    let decision = result.expect("expected a trade decision, got skip");
    assert_eq!(decision.side, Side::Yes, "expected Yes side");
    assert!(decision.size_usd > 0.0, "expected positive size");

    // Paper-execute the trade
    let mut executor = Executor::new(Mode::Paper, 100_000.0, None, 0.50);
    let fill = executor.try_fill(&decision, 0.52, 0.48).await;
    assert!(fill.is_ok(), "expected fill to succeed");

    // Settle: resolved YES (correct prediction)
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
    // Tiny edge: p_hat barely above 0.50
    let p_hat: f64 = 0.5001;
    let confidence = (p_hat - 0.5).abs() * 2.0; // ~0.0002

    let p_market = 0.50;
    let result = decide(
        p_hat,
        p_market,
        0.05, // tau_min
        100_000.0,
        0.5,
        100_000.0,
        50_000.0,
        0.02,
        0.10,
        0.10,
        confidence,
        "test-mkt-2",
        0.48, // best_bid
        0.52, // best_ask
        "",   // event_slug
    );

    assert!(
        result.is_err(),
        "expected skip (Err), but got Ok: {:#?}",
        result.unwrap()
    );
}
