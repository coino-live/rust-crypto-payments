//! Webhook signatures: `X-Coino-Signature: t=<unix time>,v1=<hex HMAC-SHA256(secret, "<t>.<raw body>")>`.
//!
//! Pass the raw request body, not parsed JSON:
//!
//! ```no_run
//! # fn handle(raw_body: &[u8], signature_header: &str, secret: &str, coino: &coinolive::Client) -> Result<(), Box<dyn std::error::Error>> {
//! let event = coinolive::webhooks::verify(raw_body, signature_header, secret)?;
//! let id = event["data"]["id"].as_str().unwrap_or_default();
//! let payment = coino.payments().retrieve(id)?; // Read the payment before acting on it.
//! # Ok(())
//! # }
//! ```

use std::time::{SystemTime, UNIX_EPOCH};

use hmac::{Hmac, Mac};
use serde_json::Value;
use sha2::Sha256;

use crate::error::SignatureVerificationError;

/// Seconds a signature stays valid unless you pass another tolerance.
pub const DEFAULT_TOLERANCE: u64 = 300;

/// Returns the decoded event, or an error when the signature is missing, wrong or older than 300 seconds.
///
/// - `payload`: the raw request body (bytes or str), not parsed JSON
/// - `signature_header`: the X-Coino-Signature header (`""` when it is missing)
/// - `secret`: the gateway's signing secret
pub fn verify(
    payload: impl AsRef<[u8]>,
    signature_header: &str,
    secret: &str,
) -> Result<Value, SignatureVerificationError> {
    verify_with_tolerance(payload, signature_header, secret, DEFAULT_TOLERANCE)
}

/// [`verify`] with your own tolerance: the seconds a signature stays valid.
pub fn verify_with_tolerance(
    payload: impl AsRef<[u8]>,
    signature_header: &str,
    secret: &str,
    tolerance: u64,
) -> Result<Value, SignatureVerificationError> {
    let payload = payload.as_ref();

    let mut t = "";
    let mut v1 = "";
    for pair in signature_header.split(',') {
        if let Some((key, value)) = pair.trim().split_once('=') {
            match key {
                "t" => t = value,
                "v1" => v1 = value,
                _ => {}
            }
        }
    }
    // Like PHP's empty(): "" and "0" count as missing.
    let missing = |value: &str| value.is_empty() || value == "0";
    if missing(t) || missing(v1) || !t.bytes().all(|b| b.is_ascii_digit()) {
        return Err(SignatureVerificationError::new("The X-Coino-Signature header is missing or malformed."));
    }

    let now = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    if !t.parse::<u64>().is_ok_and(|signed_at| now.abs_diff(signed_at) <= tolerance) {
        return Err(SignatureVerificationError::new("The signature is too old."));
    }

    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("HMAC takes a key of any length");
    mac.update(t.as_bytes());
    mac.update(b".");
    mac.update(payload);
    let expected = hex::encode(mac.finalize().into_bytes());
    if !constant_time_eq(expected.as_bytes(), v1.as_bytes()) {
        return Err(SignatureVerificationError::new("The signature does not match."));
    }

    match serde_json::from_slice::<Value>(payload) {
        Ok(event) if event.is_object() || event.is_array() => Ok(event),
        _ => Err(SignatureVerificationError::new("The payload is not JSON.")),
    }
}

/// Compares two byte strings in time that depends only on their length.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |diff, (x, y)| diff | (x ^ y)) == 0
}
