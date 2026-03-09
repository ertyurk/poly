use tokio::sync::mpsc;

use crate::types::*;

/// Messages the Telegram actor can receive.
#[derive(Debug, Clone)]
pub enum TelegramAlert {
    TradeFilled(TradeDecision),
    TradeSettled(TradeResult),
}

pub struct TelegramActor {
    bot_token: String,
    chat_id: String,
    client: reqwest::Client,
}

impl TelegramActor {
    pub fn new(bot_token: String, chat_id: String) -> Self {
        Self {
            bot_token,
            chat_id,
            client: reqwest::Client::new(),
        }
    }

    pub async fn run(
        &self,
        mut rx: mpsc::Receiver<TelegramAlert>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        tracing::info!("telegram alert actor started");

        loop {
            tokio::select! {
                biased;

                _ = shutdown.changed() => {
                    tracing::info!("telegram actor shutting down");
                    return;
                }

                msg = rx.recv() => {
                    match msg {
                        Some(alert) => {
                            let text = format_alert(&alert);
                            if let Err(e) = self.send_message(&text).await {
                                tracing::warn!(error = %e, "failed to send telegram alert");
                            }
                        }
                        None => break,
                    }
                }
            }
        }
    }

    async fn send_message(&self, text: &str) -> Result<(), reqwest::Error> {
        let url = format!(
            "https://api.telegram.org/bot{}/sendMessage",
            self.bot_token
        );

        self.client
            .post(&url)
            .json(&serde_json::json!({
                "chat_id": self.chat_id,
                "text": text,
                "parse_mode": "Markdown",
                "disable_web_page_preview": true,
            }))
            .send()
            .await?;

        Ok(())
    }
}

fn format_alert(alert: &TelegramAlert) -> String {
    match alert {
        TelegramAlert::TradeFilled(dec) => {
            format!(
                "\u{1f4c8} *Trade Filled*\n\
                 Market: `{}`\n\
                 Side: *{}*\n\
                 Size: ${:.2}\n\
                 Price: {:.4}\n\
                 Edge: {:.2}%\n\
                 Eff. Edge: {:.2}%",
                dec.market_id,
                dec.side,
                dec.size,
                dec.price,
                dec.edge * 100.0,
                dec.effective_edge * 100.0,
            )
        }
        TelegramAlert::TradeSettled(tr) => {
            let icon = match tr.outcome {
                Outcome::Win => "\u{2705}",
                Outcome::Loss => "\u{274c}",
            };
            format!(
                "{icon} *Trade Settled*\n\
                 Market: `{}`\n\
                 Side: *{}* | Outcome: *{}*\n\
                 Size: ${:.2} @ {:.4}\n\
                 P&L: *{:+.2}*\n\
                 Fees: ${:.4}\n\
                 Bankroll: ${:.2}",
                tr.market_id,
                tr.side,
                tr.outcome,
                tr.size,
                tr.entry_price,
                tr.pnl,
                tr.fee_paid,
                tr.bankroll_after,
            )
        }
    }
}
