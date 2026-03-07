use polymarket_bot::actors::ingest::parse_binance_trade;
use polymarket_bot::types::Asset;

#[test]
fn test_parse_binance_trade_btc() {
    let msg = r#"{"e":"trade","E":1709800000000,"s":"BTCUSDT","t":123,"p":"85000.50","q":"0.1","T":1709800000000,"m":false}"#;
    let result = parse_binance_trade(msg).unwrap();
    assert_eq!(result.asset, Asset::BTC);
    assert!((result.price - 85000.50).abs() < 0.01);
}

#[test]
fn test_parse_binance_trade_eth() {
    let msg = r#"{"e":"trade","E":1709800000000,"s":"ETHUSDT","t":456,"p":"3200.25","q":"1.0","T":1709800000000,"m":false}"#;
    let result = parse_binance_trade(msg).unwrap();
    assert_eq!(result.asset, Asset::ETH);
    assert!((result.price - 3200.25).abs() < 0.01);
}

#[test]
fn test_parse_binance_trade_unknown_symbol() {
    let msg = r#"{"e":"trade","E":1709800000000,"s":"SOLUSDT","t":789,"p":"100.0","q":"1.0","T":1709800000000,"m":false}"#;
    assert!(parse_binance_trade(msg).is_none());
}
