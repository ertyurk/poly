use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::config::Config;
use crate::types::*;

/// Maximum backoff between reconnection attempts (60 seconds).
const MAX_BACKOFF_SECS: u64 = 60;

#[derive(Debug, serde::Deserialize)]
struct BinanceTrade {
    #[serde(rename = "s")]
    symbol: String,
    #[serde(rename = "p")]
    price: String,
    #[serde(rename = "q")]
    qty: String,
    #[serde(rename = "m")]
    buyer_is_maker: bool,
    #[serde(rename = "T")]
    trade_time: i64,
}

pub fn parse_binance_tick(msg: &str) -> Option<SpotTick> {
    let trade: BinanceTrade = serde_json::from_str(msg).ok()?;
    let asset = match trade.symbol.as_str() {
        "BTCUSDT" => Asset::BTC,
        "ETHUSDT" => Asset::ETH,
        _ => return None,
    };
    let price: f64 = trade.price.parse().ok()?;
    let qty: f64 = trade.qty.parse().ok()?;
    Some(SpotTick {
        asset,
        price,
        ts: trade.trade_time * MS_TO_MICROS,
        qty,
        buyer_is_maker: trade.buyer_is_maker,
    })
}

pub struct IngestActor {
    config: Config,
}

impl IngestActor {
    pub const fn new(config: Config) -> Self {
        Self { config }
    }

    pub async fn run(
        &self,
        spot_tx: mpsc::Sender<SpotTick>,
        db_tx: mpsc::Sender<DbEvent>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        let streams = self.config.binance.streams.join("/");
        let url = format!("{}/{}", self.config.binance.ws_url, streams);
        let mut retry_count = 0u32;

        loop {
            if *shutdown.borrow() {
                break;
            }

            tracing::debug!(url = %url, "connecting to Binance WebSocket");
            match connect_async(&url).await {
                Ok((ws_stream, _)) => {
                    retry_count = 0;
                    tracing::debug!("connected to Binance");
                    let (_, mut read) = ws_stream.split();

                    loop {
                        tokio::select! {
                            msg = read.next() => {
                                match msg {
                                    Some(Ok(Message::Text(text))) => {
                                        if let Some(tick) = parse_binance_tick(&text) {
                                            let sp = SpotPrice { asset: tick.asset, price: tick.price, ts: tick.ts };
                                            let _ = db_tx.try_send(DbEvent::SpotPrice(sp));
                                            let _ = spot_tx.try_send(tick);
                                        }
                                    }
                                    Some(Ok(Message::Close(_))) | None => {
                                        tracing::warn!("Binance WS closed");
                                        break;
                                    }
                                    Some(Err(e)) => {
                                        tracing::error!(error = %e, "Binance WS error");
                                        break;
                                    }
                                    _ => {}
                                }
                            }
                            _ = shutdown.changed() => {
                                tracing::debug!("ingest actor shutting down");
                                return;
                            }
                        }
                    }
                }
                Err(e) => {
                    retry_count += 1;
                    let backoff_secs =
                        2u64.pow(retry_count.min(6)).min(MAX_BACKOFF_SECS);
                    tracing::warn!(
                        error = %e,
                        retry = retry_count,
                        backoff_secs,
                        "Binance WS connection failed, retrying"
                    );
                    tokio::time::sleep(std::time::Duration::from_secs(backoff_secs))
                        .await;
                }
            }
        }
    }
}
