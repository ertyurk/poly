use std::str::FromStr;

use alloy::signers::local::PrivateKeySigner;
use alloy::signers::Signer as _;
use chrono::{Duration, Utc};
use polymarket_client_sdk::auth::Normal;
use polymarket_client_sdk::clob::types::{
    OrderStatusType, OrderType, Side as SdkSide, SignatureType,
};
use polymarket_client_sdk::clob::{Client, Config};
use polymarket_client_sdk::types::{Address, Decimal, U256};
use polymarket_client_sdk::POLYGON;

/// Result of an order placement attempt.
#[derive(Debug)]
pub struct LiveFillResult {
    pub order_id: String,
    pub success: bool,
    pub matched: bool,
}

/// Wraps the official Polymarket SDK for authenticated order placement.
pub struct LiveTrader {
    client: Client<polymarket_client_sdk::auth::state::Authenticated<Normal>>,
    signer: PrivateKeySigner,
}

impl LiveTrader {
    /// Authenticate with Polymarket using just the private key.
    /// Uses proxy wallet mode — the SDK auto-derives the proxy address from the EOA
    /// via CREATE2, matching the proxy wallet that Polymarket's website created.
    pub async fn connect(
        private_key: &str,
    ) -> Result<Self, Box<dyn std::error::Error + Send + Sync>> {
        let pk = private_key.strip_prefix("0x").unwrap_or(private_key);
        let signer: PrivateKeySigner = pk
            .parse()
            .map_err(|e| format!("invalid private key: {e}"))?;
        let signer = signer.with_chain_id(Some(POLYGON));

        let config = Config::builder().use_server_time(true).build();

        // Check for explicit proxy wallet address (POLYMARKET_PROXY_ADDRESS env var).
        // If not set, fall back to SDK auto-derivation via CREATE2.
        let proxy_addr = std::env::var("POLYMARKET_PROXY_ADDRESS")
            .ok()
            .and_then(|s| {
                let s = s.trim();
                if s.is_empty() {
                    None
                } else {
                    s.parse::<Address>().ok()
                }
            });

        let mut auth = Client::new("https://clob.polymarket.com", config)?
            .authentication_builder(&signer)
            .signature_type(SignatureType::Proxy);

        if let Some(addr) = proxy_addr {
            tracing::info!(proxy = %addr, "using explicit proxy wallet address");
            auth = auth.funder(addr);
        }

        let client = auth
            .authenticate()
            .await
            .map_err(|e| format!("polymarket auth failed: {e}"))?;

        tracing::info!(
            signer = %signer.address(),
            "authenticated with Polymarket CLOB API (proxy mode)"
        );

        Ok(Self { client, signer })
    }

    /// Place a limit order via the official SDK.
    pub async fn place_order(
        &self,
        token_id: &str,
        side_buy: bool,
        price: f64,
        size: f64,
    ) -> Result<LiveFillResult, Box<dyn std::error::Error + Send + Sync>> {
        let token_id_u256 =
            U256::from_str(token_id).map_err(|e| format!("invalid token_id: {e}"))?;

        let side = if side_buy {
            SdkSide::Buy
        } else {
            SdkSide::Sell
        };

        // Convert f64 to Decimal, rounding to 2 decimal places for size and price
        let price_dec =
            Decimal::from_str(&format!("{price:.2}")).map_err(|e| format!("invalid price: {e}"))?;
        let size_dec =
            Decimal::from_str(&format!("{size:.2}")).map_err(|e| format!("invalid size: {e}"))?;

        let order = self
            .client
            .limit_order()
            .token_id(token_id_u256)
            .order_type(OrderType::FOK)
            .price(price_dec)
            .size(size_dec)
            .side(side)
            .build()
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                format!("order build failed: {e}").into()
            })?;

        let signed_order = self.client.sign(&self.signer, order).await.map_err(
            |e| -> Box<dyn std::error::Error + Send + Sync> {
                format!("order sign failed: {e}").into()
            },
        )?;

        let resp = self.client.post_order(signed_order).await.map_err(
            |e| -> Box<dyn std::error::Error + Send + Sync> {
                format!("order post failed: {e}").into()
            },
        )?;

        let matched = matches!(resp.status, OrderStatusType::Matched);

        Ok(LiveFillResult {
            order_id: resp.order_id.to_string(),
            success: resp.success,
            matched,
        })
    }

    /// Place a GTD (Good-Till-Date) limit order with auto-expiry.
    pub async fn place_gtd_order(
        &self,
        token_id: &str,
        side_buy: bool,
        price: f64,
        size: f64,
        expiry_secs: u64,
    ) -> Result<LiveFillResult, Box<dyn std::error::Error + Send + Sync>> {
        let token_id_u256 =
            U256::from_str(token_id).map_err(|e| format!("invalid token_id: {e}"))?;
        let side = if side_buy {
            SdkSide::Buy
        } else {
            SdkSide::Sell
        };
        let price_dec =
            Decimal::from_str(&format!("{price:.2}")).map_err(|e| format!("invalid price: {e}"))?;
        let size_dec =
            Decimal::from_str(&format!("{size:.2}")).map_err(|e| format!("invalid size: {e}"))?;
        // Polymarket requires expiration >= now + 60s (security threshold).
        // Add 60s buffer so a 15s GTD actually expires at now + 75s.
        let expiry = Utc::now() + Duration::seconds(60 + expiry_secs as i64);

        let order = self
            .client
            .limit_order()
            .token_id(token_id_u256)
            .order_type(OrderType::GTD)
            .expiration(expiry)
            .price(price_dec)
            .size(size_dec)
            .side(side)
            .build()
            .await
            .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                format!("GTD order build failed: {e}").into()
            })?;

        let signed_order = self.client.sign(&self.signer, order).await.map_err(
            |e| -> Box<dyn std::error::Error + Send + Sync> {
                format!("order sign failed: {e}").into()
            },
        )?;

        let resp = self.client.post_order(signed_order).await.map_err(
            |e| -> Box<dyn std::error::Error + Send + Sync> {
                format!("order post failed: {e}").into()
            },
        )?;

        let matched = matches!(resp.status, OrderStatusType::Matched);
        Ok(LiveFillResult {
            order_id: resp.order_id.to_string(),
            success: resp.success,
            matched,
        })
    }

    /// Check order status.
    /// Returns `Some(true)` if matched, `Some(false)` if live/delayed,
    /// `None` if cancelled/expired/unknown.
    pub async fn check_order_status(
        &self,
        order_id: &str,
    ) -> Result<Option<bool>, Box<dyn std::error::Error + Send + Sync>> {
        let resp = self.client.order(order_id).await.map_err(
            |e| -> Box<dyn std::error::Error + Send + Sync> {
                format!("order status check failed: {e}").into()
            },
        )?;
        match resp.status {
            OrderStatusType::Matched => Ok(Some(true)),
            OrderStatusType::Live | OrderStatusType::Delayed => Ok(Some(false)),
            _ => Ok(None),
        }
    }

    /// Cancel a specific order by ID.
    pub async fn cancel_order(
        &self,
        order_id: &str,
    ) -> Result<bool, Box<dyn std::error::Error + Send + Sync>> {
        let resp = self.client.cancel_order(order_id).await.map_err(
            |e| -> Box<dyn std::error::Error + Send + Sync> {
                format!("cancel order failed: {e}").into()
            },
        )?;
        Ok(resp.canceled.contains(&order_id.to_string()))
    }

    /// Cancel ALL open orders. Safety net for shutdown.
    pub async fn cancel_all_orders(
        &self,
    ) -> Result<usize, Box<dyn std::error::Error + Send + Sync>> {
        let resp = self.client.cancel_all_orders().await.map_err(
            |e| -> Box<dyn std::error::Error + Send + Sync> {
                format!("cancel all orders failed: {e}").into()
            },
        )?;
        Ok(resp.canceled.len())
    }
}
