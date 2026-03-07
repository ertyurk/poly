use k256::ecdsa::{signature::hazmat::PrehashSigner, RecoveryId, Signature, SigningKey};
use tiny_keccak::{Hasher, Keccak};

/// Polymarket CTF Exchange contract on Polygon mainnet.
const EXCHANGE_ADDRESS: &str = "4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E";

/// Polygon chain ID.
const CHAIN_ID: u64 = 137;

fn keccak256(data: &[u8]) -> [u8; 32] {
    let mut hasher = Keccak::v256();
    hasher.update(data);
    let mut output = [0u8; 32];
    hasher.finalize(&mut output);
    output
}

fn left_pad_32(data: &[u8]) -> [u8; 32] {
    let mut padded = [0u8; 32];
    let len = data.len().min(32);
    let offset = 32 - len;
    padded[offset..offset + len].copy_from_slice(&data[..len]);
    padded
}

fn u64_to_bytes32(val: u64) -> [u8; 32] {
    left_pad_32(&val.to_be_bytes())
}

fn u128_to_bytes32(val: u128) -> [u8; 32] {
    left_pad_32(&val.to_be_bytes())
}

fn address_to_bytes32(hex_addr: &str) -> [u8; 32] {
    let addr = hex_addr.strip_prefix("0x").unwrap_or(hex_addr);
    let bytes = hex::decode(addr).unwrap_or_else(|_| vec![0u8; 20]);
    left_pad_32(&bytes)
}

fn domain_separator() -> [u8; 32] {
    let type_hash = keccak256(
        b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
    );
    let name_hash = keccak256(b"Polymarket CTF Exchange");
    let version_hash = keccak256(b"1");

    let mut buf = Vec::with_capacity(5 * 32);
    buf.extend_from_slice(&type_hash);
    buf.extend_from_slice(&name_hash);
    buf.extend_from_slice(&version_hash);
    buf.extend_from_slice(&u64_to_bytes32(CHAIN_ID));
    buf.extend_from_slice(&address_to_bytes32(EXCHANGE_ADDRESS));

    keccak256(&buf)
}

fn order_type_hash() -> [u8; 32] {
    keccak256(
        b"Order(uint256 salt,address maker,address signer,address taker,uint256 tokenId,uint256 makerAmount,uint256 takerAmount,uint256 expiration,uint256 nonce,uint256 feeRateBps,uint8 side,uint8 signatureType)",
    )
}

/// Parameters for signing a Polymarket order.
pub struct OrderParams {
    pub salt: u128,
    pub maker: String,
    pub signer: String,
    pub taker: String,
    pub token_id: u128,
    pub maker_amount: u128,
    pub taker_amount: u128,
    pub expiration: u128,
    pub nonce: u128,
    pub fee_rate_bps: u128,
    /// 0 = BUY, 1 = SELL
    pub side: u8,
    /// 2 = POLY_GNOSIS_SAFE (most common)
    pub signature_type: u8,
}

fn struct_hash(params: &OrderParams) -> [u8; 32] {
    let type_hash = order_type_hash();

    let mut buf = Vec::with_capacity(14 * 32);
    buf.extend_from_slice(&type_hash);
    buf.extend_from_slice(&u128_to_bytes32(params.salt));
    buf.extend_from_slice(&address_to_bytes32(&params.maker));
    buf.extend_from_slice(&address_to_bytes32(&params.signer));
    buf.extend_from_slice(&address_to_bytes32(&params.taker));
    buf.extend_from_slice(&u128_to_bytes32(params.token_id));
    buf.extend_from_slice(&u128_to_bytes32(params.maker_amount));
    buf.extend_from_slice(&u128_to_bytes32(params.taker_amount));
    buf.extend_from_slice(&u128_to_bytes32(params.expiration));
    buf.extend_from_slice(&u128_to_bytes32(params.nonce));
    buf.extend_from_slice(&u128_to_bytes32(params.fee_rate_bps));
    buf.extend_from_slice(&u64_to_bytes32(u64::from(params.side)));
    buf.extend_from_slice(&u64_to_bytes32(u64::from(params.signature_type)));

    keccak256(&buf)
}

/// Sign a Polymarket order using EIP-712 typed data signing.
///
/// Returns the hex-encoded signature with recovery id appended (65 bytes).
pub fn sign_order(
    private_key_hex: &str,
    params: &OrderParams,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let key_bytes = hex::decode(
        private_key_hex
            .strip_prefix("0x")
            .unwrap_or(private_key_hex),
    )?;
    let signing_key = SigningKey::from_bytes((&key_bytes[..]).into())?;

    let ds = domain_separator();
    let sh = struct_hash(params);

    // EIP-712 digest: keccak256(0x19 || 0x01 || domainSeparator || structHash)
    let mut digest_input = Vec::with_capacity(2 + 32 + 32);
    digest_input.push(0x19);
    digest_input.push(0x01);
    digest_input.extend_from_slice(&ds);
    digest_input.extend_from_slice(&sh);
    let digest = keccak256(&digest_input);

    let (sig, recovery_id): (Signature, RecoveryId) = signing_key.sign_prehash(&digest)?;

    // Encode as r || s || v (65 bytes)
    let mut sig_bytes = Vec::with_capacity(65);
    sig_bytes.extend_from_slice(&sig.to_bytes());
    sig_bytes.push(recovery_id.to_byte() + 27); // Ethereum convention: v = recovery_id + 27

    Ok(format!("0x{}", hex::encode(sig_bytes)))
}

/// Derive the Ethereum address from a private key.
pub fn address_from_key(
    private_key_hex: &str,
) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
    let key_bytes = hex::decode(
        private_key_hex
            .strip_prefix("0x")
            .unwrap_or(private_key_hex),
    )?;
    let signing_key = SigningKey::from_bytes((&key_bytes[..]).into())?;
    let verifying_key = signing_key.verifying_key();
    let public_key = verifying_key.to_encoded_point(false);
    // Skip the 0x04 prefix byte, hash the remaining 64 bytes
    let hash = keccak256(&public_key.as_bytes()[1..]);
    // Address is last 20 bytes
    Ok(format!("0x{}", hex::encode(&hash[12..])))
}
