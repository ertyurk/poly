use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::time::{self, Duration, Instant};

use crate::types::*;

/// Minimum interval between individual alert messages (rate limiting).
const MIN_ALERT_INTERVAL: Duration = Duration::from_secs(1);

/// Messages the Telegram actor can receive.
#[derive(Debug, Clone)]
pub enum TelegramAlert {
    TradeFilled {
        decision: TradeDecision,
        fill_price: f64,
    },
    TradeSettled(TradeResult),
}

/// Shared stats for the periodic summary, updated atomically by the executor.
pub struct TelegramStats {
    pub trades_placed: AtomicU32,
    pub wins: AtomicU32,
    pub losses: AtomicU32,
    /// Total P&L in cents (i64 atomic via two u32s would be complex; use a Mutex).
    pnl_lock: tokio::sync::Mutex<PnlSnapshot>,
}

struct PnlSnapshot {
    total_pnl: f64,
    total_fees: f64,
    bankroll: f64,
    initial_bankroll: f64,
}

impl TelegramStats {
    pub fn new(initial_bankroll: f64) -> Self {
        Self {
            trades_placed: AtomicU32::new(0),
            wins: AtomicU32::new(0),
            losses: AtomicU32::new(0),
            pnl_lock: tokio::sync::Mutex::new(PnlSnapshot {
                total_pnl: 0.0,
                total_fees: 0.0,
                bankroll: initial_bankroll,
                initial_bankroll,
            }),
        }
    }

    pub async fn record_settlement(&self, tr: &TradeResult) {
        match tr.outcome {
            Outcome::Win => {
                self.wins.fetch_add(1, Ordering::Relaxed);
            }
            Outcome::Loss => {
                self.losses.fetch_add(1, Ordering::Relaxed);
            }
        }
        let mut snap = self.pnl_lock.lock().await;
        snap.total_pnl += tr.pnl;
        snap.total_fees += tr.fee_paid;
        snap.bankroll = tr.bankroll_after;
    }

    pub fn record_fill(&self) {
        self.trades_placed.fetch_add(1, Ordering::Relaxed);
    }

    async fn snapshot(&self) -> SummaryData {
        let snap = self.pnl_lock.lock().await;
        let wins = self.wins.load(Ordering::Relaxed);
        let losses = self.losses.load(Ordering::Relaxed);
        let total = wins + losses;
        SummaryData {
            trades_placed: self.trades_placed.load(Ordering::Relaxed),
            wins,
            losses,
            win_rate: if total > 0 {
                100.0 * f64::from(wins) / f64::from(total)
            } else {
                0.0
            },
            total_pnl: snap.total_pnl,
            total_fees: snap.total_fees,
            bankroll: snap.bankroll,
            initial_bankroll: snap.initial_bankroll,
            return_pct: if snap.initial_bankroll > 0.0 {
                (snap.bankroll - snap.initial_bankroll) / snap.initial_bankroll * 100.0
            } else {
                0.0
            },
        }
    }
}

struct SummaryData {
    trades_placed: u32,
    wins: u32,
    losses: u32,
    win_rate: f64,
    total_pnl: f64,
    total_fees: f64,
    bankroll: f64,
    initial_bankroll: f64,
    return_pct: f64,
}

pub struct TelegramActor {
    bot_token: String,
    chat_id: String,
    client: reqwest::Client,
    summary_interval: Duration,
    stats: Arc<TelegramStats>,
}

impl TelegramActor {
    pub fn new(
        bot_token: String,
        chat_id: String,
        summary_interval_mins: u64,
        stats: Arc<TelegramStats>,
    ) -> Self {
        Self {
            bot_token,
            chat_id,
            client: reqwest::Client::new(),
            summary_interval: Duration::from_secs(summary_interval_mins * 60),
            stats,
        }
    }

    pub async fn run(
        &self,
        mut rx: mpsc::Receiver<TelegramAlert>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) {
        tracing::info!(
            interval_mins = self.summary_interval.as_secs() / 60,
            "telegram alert actor started"
        );

        let mut summary_tick = time::interval(self.summary_interval);
        // Skip the first immediate tick
        summary_tick.tick().await;
        let mut last_alert = Instant::now() - MIN_ALERT_INTERVAL;

        loop {
            tokio::select! {
                biased;

                _ = shutdown.changed() => {
                    // Send final summary on shutdown
                    let summary = self.format_summary("FINAL SESSION SUMMARY").await;
                    let _ = self.send_message(&summary).await;
                    tracing::info!("telegram actor shutting down");
                    return;
                }

                msg = rx.recv() => {
                    match msg {
                        Some(alert) => {
                            // Rate-limit individual alerts to avoid hitting Telegram limits
                            let elapsed = last_alert.elapsed();
                            if elapsed < MIN_ALERT_INTERVAL {
                                time::sleep(MIN_ALERT_INTERVAL - elapsed).await;
                            }
                            let text = format_alert(&alert);
                            if let Err(e) = self.send_message(&text).await {
                                tracing::warn!(error = %e, "failed to send telegram alert");
                            }
                            last_alert = Instant::now();
                        }
                        None => break,
                    }
                }

                _ = summary_tick.tick() => {
                    let summary = self.format_summary("PERIODIC SUMMARY").await;
                    if let Err(e) = self.send_message(&summary).await {
                        tracing::warn!(error = %e, "failed to send telegram summary");
                    }
                }
            }
        }
    }

    async fn format_summary(&self, title: &str) -> String {
        let s = self.stats.snapshot().await;
        format!(
            "\u{1f4ca} *{title}*\n\
             \n\
             Trades placed: {}\n\
             Wins / Losses: {} / {}\n\
             Win rate: {:.1}%\n\
             \n\
             Total P&L: *{:+.2}*\n\
             Total fees: ${:.2}\n\
             \n\
             Starting bankroll: ${:.2}\n\
             Current bankroll: *${:.2}*\n\
             Return: *{:+.2}%*",
            s.trades_placed,
            s.wins,
            s.losses,
            s.win_rate,
            s.total_pnl,
            s.total_fees,
            s.initial_bankroll,
            s.bankroll,
            s.return_pct,
        )
    }

    async fn send_message(&self, text: &str) -> Result<(), String> {
        let url = format!(
            "https://api.telegram.org/bot{}/sendMessage",
            self.bot_token
        );

        let resp = self
            .client
            .post(&url)
            .json(&serde_json::json!({
                "chat_id": self.chat_id,
                "text": text,
                "parse_mode": "Markdown",
                "disable_web_page_preview": true,
            }))
            .send()
            .await
            .map_err(|e| e.to_string())?;

        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(format!("telegram API {status}: {body}"));
        }

        // Check Telegram API-level ok field
        let body: serde_json::Value = resp
            .json()
            .await
            .map_err(|e| format!("bad json: {e}"))?;
        if body.get("ok").and_then(|v| v.as_bool()) != Some(true) {
            let desc = body
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");
            return Err(format!("telegram API error: {desc}"));
        }

        Ok(())
    }
}

fn format_alert(alert: &TelegramAlert) -> String {
    match alert {
        TelegramAlert::TradeFilled { decision, fill_price } => {
            let link = if decision.event_slug.is_empty() {
                String::new()
            } else {
                format!(
                    "\n[View on Polymarket](https://polymarket.com/event/{})",
                    decision.event_slug
                )
            };
            format!(
                "\u{1f4c8} *Trade Filled*\n\
                 Market: `{}`\n\
                 Side: *{}*\n\
                 Size: ${:.2}\n\
                 Fill price: {:.4}\n\
                 Edge: {:.2}%{link}",
                decision.market_id,
                decision.side,
                decision.size_usd,
                fill_price,
                decision.effective_edge * 100.0,
            )
        }
        TelegramAlert::TradeSettled(tr) => {
            let icon = match tr.outcome {
                Outcome::Win => "\u{2705}",
                Outcome::Loss => "\u{274c}",
            };
            let link = if tr.event_slug.is_empty() {
                String::new()
            } else {
                format!(
                    "\n[View on Polymarket](https://polymarket.com/event/{})",
                    tr.event_slug
                )
            };
            format!(
                "{icon} *Trade Settled*\n\
                 Market: `{}`\n\
                 Side: *{}* | Outcome: *{}*\n\
                 Size: ${:.2} @ {:.4}\n\
                 P&L: *{:+.2}*\n\
                 Fees: ${:.4}\n\
                 Bankroll: ${:.2}{link}",
                tr.market_id,
                tr.side,
                tr.outcome,
                tr.size_shares,
                tr.entry_price,
                tr.pnl,
                tr.fee_paid,
                tr.bankroll_after,
            )
        }
    }
}
