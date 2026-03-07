use hmac::{Hmac, Mac};
use sha2::Sha256;

type HmacSha256 = Hmac<Sha256>;

/// Build L2 authentication headers for Polymarket CLOB API.
///
/// Required for order placement and other authenticated endpoints.
/// Read-only endpoints (markets, books, midpoints) do not need auth.
pub fn build_headers(
    api_key: &str,
    api_secret: &str,
    passphrase: &str,
    timestamp: &str,
    method: &str,
    path: &str,
    body: &str,
) -> Vec<(String, String)> {
    let message = format!("{timestamp}\n{method}\n{path}\n{body}");

    let secret_bytes = base64::engine::general_purpose::STANDARD
        .decode(api_secret)
        .unwrap_or_default();

    let signature = sign_hmac(&secret_bytes, message.as_bytes());
    let sig_b64 = base64::engine::general_purpose::STANDARD.encode(signature);

    vec![
        ("POLY-API-KEY".into(), api_key.into()),
        ("POLY-SIGNATURE".into(), sig_b64),
        ("POLY-TIMESTAMP".into(), timestamp.into()),
        ("POLY-PASSPHRASE".into(), passphrase.into()),
    ]
}

fn sign_hmac(key: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac = HmacSha256::new_from_slice(key)
        .unwrap_or_else(|_| HmacSha256::new_from_slice(b"").unwrap_or_else(|_| unreachable!()));
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

use base64::Engine;
