use futures_util::StreamExt;
use tokio::sync::mpsc;
use tokio_tungstenite::connect_async;
use tokio_tungstenite::tungstenite::Message;

use crate::config::Config;
use crate::types::*;

#[derive(Debug, serde::Deserialize)]
struct BinanceTrade {
    #[serde(rename = "s")]
    symbol: String,
    #[serde(rename = "p")]
    price: String,
    #[serde(rename = "T")]
    trade_time: i64,
}

pub fn parse_binance_trade(msg: &str) -> Option<SpotPrice> {
    let trade: BinanceTrade = serde_json::from_str(msg).ok()?;
    let asset = match trade.symbol.as_str() {
        "BTCUSDT" => Asset::BTC,
        "ETHUSDT" => Asset::ETH,
        _ => return None,
    };
    let price: f64 = trade.price.parse().ok()?;
    Some(SpotPrice {
        asset,
        price,
        ts: trade.trade_time * 1000,
    })
}

pub struct IngestActor {
    config: Config,
}

impl IngestActor {
    pub fn new(config: Config) -> Self {
        Self { config }
    }

    pub async fn run(
        &self,
        spot_tx: mpsc::Sender<SpotPrice>,
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

            tracing::info!(url = %url, "connecting to Binance WebSocket");
            match connect_async(&url).await {
                Ok((ws_stream, _)) => {
                    retry_count = 0;
                    tracing::info!("connected to Binance");
                    let (_, mut read) = ws_stream.split();

                    loop {
                        tokio::select! {
                            msg = read.next() => {
                                match msg {
                                    Some(Ok(Message::Text(text))) => {
                                        if let Some(sp) = parse_binance_trade(&text) {
                                            let _ = db_tx.try_send(DbEvent::SpotPrice(sp.clone()));
                                            let _ = spot_tx.try_send(sp);
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
                                tracing::info!("ingest actor shutting down");
                                return;
                            }
                        }
                    }
                }
                Err(e) => {
                    retry_count += 1;
                    if retry_count > 5 {
                        tracing::error!("Binance WS: max retries exceeded");
                        return;
                    }
                    let backoff = std::time::Duration::from_secs(2u64.pow(retry_count));
                    tracing::warn!(error = %e, retry = retry_count, "Binance WS connection failed, retrying");
                    tokio::time::sleep(backoff).await;
                }
            }
        }
    }
}
