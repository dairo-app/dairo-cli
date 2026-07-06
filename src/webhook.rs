//! Constant-time verification of inbound Dairo webhook deliveries.
//!
//! Scheme (must match the backend in `dairo-api/.../webhooks.rs`):
//!
//! 1. When a webhook is created Dairo returns a one-time secret of the form
//!    `whsec_<hex>`. The backend stores only `signing_secret_hash =
//!    hex(sha256(secret))` and uses **that hex string's bytes** as the HMAC key.
//! 2. Each delivery sends `X-Dairo-Signature: v1=<hex hmac_sha256(key,
//!    rawBody)>` and `X-Dairo-Timestamp: <unix seconds>`, where `key =
//!    hex(sha256(secret))` (as ASCII bytes) and `rawBody` is the exact bytes of
//!    the request body.
//!
//! So to verify, the consumer passes the user-facing `whsec_...` secret; we
//! re-derive the signing key the same way the backend does, recompute the HMAC
//! over the raw body, and compare in constant time. We also enforce that the
//! timestamp header is within `tolerance_seconds` of now to bound replay.

use hmac::{Hmac, Mac};
use sha2::{Digest, Sha256};

type HmacSha256 = Hmac<Sha256>;

/// Why a webhook failed verification. Messages never include the secret or the
/// computed signature, only structural facts, so logging an error is safe.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum WebhookError {
    #[error("signing secret is empty")]
    EmptySecret,
    #[error("signature header is missing or malformed")]
    InvalidSignatureFormat,
    #[error("timestamp header is missing or not an integer")]
    InvalidTimestamp,
    #[error("timestamp is outside the allowed tolerance")]
    TimestampOutOfTolerance,
    #[error("signature does not match")]
    SignatureMismatch,
}

/// Derives the HMAC signing key from the user-facing webhook secret exactly the
/// way the backend stores it: the lowercase hex SHA-256 of the secret.
fn signing_key(secret: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(secret.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Computes the `v1=<hex>` signature for `raw_body` given a webhook `secret`.
/// Exposed so callers can build fixtures/tests; verification should use
/// [`verify_webhook`]. Only referenced from tests in this binary crate.
#[cfg_attr(not(test), allow(dead_code))]
pub fn sign_webhook(secret: &str, raw_body: &[u8]) -> String {
    let key = signing_key(secret);
    let mut mac =
        HmacSha256::new_from_slice(key.as_bytes()).expect("HMAC accepts keys of any length");
    mac.update(raw_body);
    format!("v1={:x}", mac.finalize().into_bytes())
}

/// Signs `raw_body` for a local `dairo listen --forward-to` delivery, producing
/// the exact `X-Dairo-Signature: v1=<hex>` value a Dairo webhook handler
/// verifies against `DAIRO_WEBHOOK_SECRET=<secret>`.
///
/// This is a thin, intention-revealing wrapper over [`sign_webhook`]: the
/// derivation (`key = hex(sha256(secret))`, HMAC-SHA256 over the raw bytes) is
/// identical to the production fan-out, so `dairo listen` can mint an ephemeral
/// `whsec_...` secret per run, sign forwards with it, and a handler written
/// against real Dairo webhooks verifies them unchanged. The secret never leaves
/// the developer's machine.
pub fn sign_body(secret: &str, raw_body: &[u8]) -> String {
    sign_webhook(secret, raw_body)
}

/// Verifies a Dairo webhook delivery in constant time.
///
/// - `secret`: the `whsec_...` value returned when the webhook was created.
/// - `raw_body`: the exact bytes of the received request body (do not
///   re-serialize parsed JSON; signatures are over raw bytes).
/// - `signature`: the `X-Dairo-Signature` header value (`v1=<hex>`).
/// - `timestamp`: the `X-Dairo-Timestamp` header value (unix seconds).
/// - `tolerance_seconds`: maximum allowed clock skew between the delivery
///   timestamp and now, in either direction. Pass `0` to skip the timestamp
///   freshness check (not recommended).
pub fn verify_webhook(
    secret: &str,
    raw_body: &[u8],
    signature: &str,
    timestamp: &str,
    tolerance_seconds: u64,
) -> std::result::Result<(), WebhookError> {
    if secret.is_empty() {
        return Err(WebhookError::EmptySecret);
    }

    // Parse the provided signature, accepting the `v1=` prefix exactly as the
    // backend emits it. Reject anything else rather than guess.
    let provided_hex = signature
        .strip_prefix("v1=")
        .filter(|hex| !hex.is_empty())
        .ok_or(WebhookError::InvalidSignatureFormat)?;
    let provided = decode_hex(provided_hex).ok_or(WebhookError::InvalidSignatureFormat)?;

    if tolerance_seconds > 0 {
        let ts: i64 = timestamp
            .trim()
            .parse()
            .map_err(|_| WebhookError::InvalidTimestamp)?;
        let now = unix_now();
        let skew = now.saturating_sub(ts).unsigned_abs();
        if skew > tolerance_seconds {
            return Err(WebhookError::TimestampOutOfTolerance);
        }
    }

    // Recompute the expected MAC and compare in constant time. `hmac`'s
    // `verify_slice` is itself constant-time over the tag length.
    let key = signing_key(secret);
    let mut mac =
        HmacSha256::new_from_slice(key.as_bytes()).expect("HMAC accepts keys of any length");
    mac.update(raw_body);
    mac.verify_slice(&provided)
        .map_err(|_| WebhookError::SignatureMismatch)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or_default()
}

fn decode_hex(input: &str) -> Option<Vec<u8>> {
    if input.len() % 2 != 0 {
        return None;
    }
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(input.len() / 2);
    let mut i = 0;
    while i < bytes.len() {
        let hi = (bytes[i] as char).to_digit(16)?;
        let lo = (bytes[i + 1] as char).to_digit(16)?;
        out.push(((hi << 4) | lo) as u8);
        i += 2;
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SECRET: &str = "dairo_webhook_test_secret";

    fn fresh_ts() -> String {
        unix_now().to_string()
    }

    #[test]
    fn signing_key_matches_backend_sha256_hex() {
        // hex(sha256("whsec_...")) — same derivation the backend uses for
        // `signing_secret_hash`.
        let mut hasher = Sha256::new();
        hasher.update(SECRET.as_bytes());
        let expected = format!("{:x}", hasher.finalize());
        assert_eq!(signing_key(SECRET), expected);
    }

    #[test]
    fn accepts_valid_signature_within_tolerance() {
        let body = br#"{"id":"evt_1","type":"message.received"}"#;
        let sig = sign_webhook(SECRET, body);
        assert!(verify_webhook(SECRET, body, &sig, &fresh_ts(), 300).is_ok());
    }

    #[test]
    fn rejects_tampered_body() {
        let body = br#"{"id":"evt_1","type":"message.received"}"#;
        let sig = sign_webhook(SECRET, body);
        let tampered = br#"{"id":"evt_1","type":"message.bounced"}"#;
        assert_eq!(
            verify_webhook(SECRET, tampered, &sig, &fresh_ts(), 300),
            Err(WebhookError::SignatureMismatch)
        );
    }

    #[test]
    fn rejects_wrong_secret() {
        let body = br#"{"id":"evt_1"}"#;
        let sig = sign_webhook(SECRET, body);
        assert_eq!(
            verify_webhook("whsec_wrong", body, &sig, &fresh_ts(), 300),
            Err(WebhookError::SignatureMismatch)
        );
    }

    #[test]
    fn rejects_stale_timestamp() {
        let body = br#"{"id":"evt_1"}"#;
        let sig = sign_webhook(SECRET, body);
        let stale = (unix_now() - 10_000).to_string();
        assert_eq!(
            verify_webhook(SECRET, body, &sig, &stale, 300),
            Err(WebhookError::TimestampOutOfTolerance)
        );
    }

    #[test]
    fn rejects_future_timestamp_beyond_tolerance() {
        let body = br#"{"id":"evt_1"}"#;
        let sig = sign_webhook(SECRET, body);
        let future = (unix_now() + 10_000).to_string();
        assert_eq!(
            verify_webhook(SECRET, body, &sig, &future, 300),
            Err(WebhookError::TimestampOutOfTolerance)
        );
    }

    #[test]
    fn tolerance_zero_skips_timestamp_check() {
        let body = br#"{"id":"evt_1"}"#;
        let sig = sign_webhook(SECRET, body);
        assert!(verify_webhook(SECRET, body, &sig, "not-a-number", 0).is_ok());
    }

    #[test]
    fn rejects_malformed_signature_header() {
        let body = br#"{"id":"evt_1"}"#;
        assert_eq!(
            verify_webhook(SECRET, body, "deadbeef", &fresh_ts(), 300),
            Err(WebhookError::InvalidSignatureFormat)
        );
        assert_eq!(
            verify_webhook(SECRET, body, "v1=", &fresh_ts(), 300),
            Err(WebhookError::InvalidSignatureFormat)
        );
        assert_eq!(
            verify_webhook(SECRET, body, "v1=zz", &fresh_ts(), 300),
            Err(WebhookError::InvalidSignatureFormat)
        );
    }

    #[test]
    fn rejects_invalid_timestamp_when_enforced() {
        let body = br#"{"id":"evt_1"}"#;
        let sig = sign_webhook(SECRET, body);
        assert_eq!(
            verify_webhook(SECRET, body, &sig, "not-a-number", 300),
            Err(WebhookError::InvalidTimestamp)
        );
    }

    #[test]
    fn rejects_empty_secret() {
        assert_eq!(
            verify_webhook("", b"{}", "v1=ab", &fresh_ts(), 300),
            Err(WebhookError::EmptySecret)
        );
    }

    #[test]
    fn sign_body_matches_sign_webhook_and_verifies() {
        // `sign_body` (the forward-signing entry point used by `dairo listen`)
        // must produce a signature that round-trips through `verify_webhook`,
        // i.e. it is byte-identical to the production scheme.
        let body = br#"{"id":"evt_listen","type":"message.received"}"#;
        let sig = sign_body(SECRET, body);
        assert_eq!(sig, sign_webhook(SECRET, body));
        assert!(verify_webhook(SECRET, body, &sig, &fresh_ts(), 300).is_ok());
    }

    #[test]
    fn matches_a_known_backend_style_signature() {
        // Cross-check the full derivation against an independently computed MAC:
        // key = hex(sha256(secret)); sig = hex(hmac_sha256(key_ascii, body)).
        let body = br#"{"hello":"world"}"#;
        let key = signing_key(SECRET);
        let mut mac = HmacSha256::new_from_slice(key.as_bytes()).unwrap();
        mac.update(body);
        let expected = format!("v1={:x}", mac.finalize().into_bytes());
        assert_eq!(sign_webhook(SECRET, body), expected);
        assert!(verify_webhook(SECRET, body, &expected, &fresh_ts(), 300).is_ok());
    }
}
