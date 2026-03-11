use polymarket_bot::actors::ingest::parse_binance_tick;
use polymarket_bot::types::Asset;

#[test]
fn test_parse_binance_tick_btc() {
    let msg = r#"{"e":"trade","E":1709800000000,"s":"BTCUSDT","t":123,"p":"85000.50","q":"0.1","T":1709800000000,"m":false}"#;
    let tick = parse_binance_tick(msg).unwrap();
    assert_eq!(tick.asset, Asset::BTC);
    assert!((tick.price - 85000.50).abs() < 0.01);
    assert!((tick.qty - 0.1).abs() < 1e-6);
    assert!(!tick.buyer_is_maker);
}

#[test]
fn test_parse_binance_tick_eth() {
    let msg = r#"{"e":"trade","E":1709800000000,"s":"ETHUSDT","t":456,"p":"3200.25","q":"1.5","T":1709800000000,"m":true}"#;
    let tick = parse_binance_tick(msg).unwrap();
    assert_eq!(tick.asset, Asset::ETH);
    assert!((tick.price - 3200.25).abs() < 0.01);
    assert!((tick.qty - 1.5).abs() < 1e-6);
    assert!(tick.buyer_is_maker);
}

#[test]
fn test_parse_binance_tick_unknown_symbol() {
    let msg = r#"{"e":"trade","E":1709800000000,"s":"SOLUSDT","t":789,"p":"100.0","q":"1.0","T":1709800000000,"m":false}"#;
    assert!(parse_binance_tick(msg).is_none());
}
