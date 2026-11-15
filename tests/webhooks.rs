//! Webhook signatures, checked against vectors made with sdk/php (CoinoLive\Webhook, PHP 8.2):
//!
//!     hash_hmac('sha256', '1793000000.' . $body, 'whsec_test')

use std::time::{SystemTime, UNIX_EPOCH};

use coinolive::webhooks::{self, verify, verify_with_tolerance};
use coinolive::SignatureVerificationError;
use hmac::{Hmac, Mac};
use sha2::Sha256;

const SECRET: &str = "whsec_test";
const BODY: &str = r#"{"id":"01evt","event":"payment.paid","created_at":"2026-10-31T10:00:00+00:00","data":{"id":"01pay","status":"paid"}}"#;
/// PHP: hash_hmac('sha256', '1793000000.' . BODY, 'whsec_test')
const PHP_HEADER: &str = "t=1793000000,v1=fa3281e313bd7f228039f14757abbd3b52715f05959382126df7341de5af93da";

/// A body with "café 25€" in it, as UTF-8 bytes.
const UNICODE_BODY_HEX: &str = "7b226964223a22303165767432222c226576656e74223a22637573746f64792e6465706f7369742e637265646974\
6564222c22637265617465645f6174223a22323032362d31302d33315431303a30303a30302b30303a3030222c2264617461223a7b226964223a\
223031757372222c226e6f7465223a22636166c3a9203235e282ac227d7d";
/// PHP: hash_hmac('sha256', '1793000000.' . hex2bin(UNICODE_BODY_HEX), 'whsec_test'), with spaces around the pairs.
const PHP_UNICODE_HEADER: &str = " t=1793000000 , v1=1673bb2aa1cb538b2ffea2939a784e69196bbec7d2409787102c54fc148ae5d5";

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).expect("clock").as_secs()
}

fn sign(t: u64, body: &str, secret: &str) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).expect("key");
    mac.update(format!("{t}.{body}").as_bytes());
    format!("t={t},v1={}", hex::encode(mac.finalize().into_bytes()))
}

fn refused(result: Result<serde_json::Value, SignatureVerificationError>) -> String {
    result.expect_err("the webhook should be refused").message().to_string()
}

#[test]
fn accepts_the_php_vector() {
    let event = verify_with_tolerance(BODY, PHP_HEADER, SECRET, u64::MAX).expect("valid");
    assert_eq!(event["event"], "payment.paid");
    assert_eq!(event["data"]["id"], "01pay");
}

#[test]
fn accepts_the_php_vector_over_utf8_bytes() {
    let body = hex::decode(UNICODE_BODY_HEX).expect("hex");
    let event = verify_with_tolerance(&body, PHP_UNICODE_HEADER, SECRET, u64::MAX).expect("valid");
    assert_eq!(event["data"]["note"], "café 25€");
    let text = String::from_utf8(body).expect("utf-8");
    assert!(verify_with_tolerance(text.as_str(), PHP_UNICODE_HEADER, SECRET, u64::MAX).is_ok());
}

#[test]
fn the_php_vector_is_too_old_for_the_default_tolerance() {
    assert_eq!(webhooks::DEFAULT_TOLERANCE, 300);
    assert_eq!(refused(verify(BODY, PHP_HEADER, SECRET)), "The signature is too old.");
}

#[test]
fn compares_hex_like_php_hash_equals() {
    let upper = PHP_HEADER.replace("v1=fa3281e3", "v1=FA3281E3");
    assert_eq!(refused(verify_with_tolerance(BODY, &upper, SECRET, u64::MAX)), "The signature does not match.");
}

#[test]
fn a_good_signature_returns_the_event() {
    let header = sign(now(), BODY, SECRET);
    assert_eq!(verify(BODY, &header, SECRET).expect("valid")["data"]["id"], "01pay");
    assert_eq!(verify(BODY.as_bytes(), &header, SECRET).expect("valid")["event"], "payment.paid");
    assert_eq!(verify(BODY.to_string(), &header, SECRET).expect("valid")["event"], "payment.paid");
    assert_eq!(verify(BODY.as_bytes().to_vec(), &header, SECRET).expect("valid")["event"], "payment.paid");
}

#[test]
fn other_pairs_in_the_header_are_ignored() {
    let header = format!("v0=abc,{},x=1", sign(now(), BODY, SECRET));
    assert!(verify(BODY, &header, SECRET).is_ok());
}

#[test]
fn a_wrong_secret_is_refused() {
    let header = sign(now(), BODY, SECRET);
    assert_eq!(refused(verify(BODY, &header, "other")), "The signature does not match.");
}

#[test]
fn a_changed_body_is_refused() {
    let header = sign(now(), BODY, SECRET);
    assert_eq!(refused(verify(BODY.replace("paid", "open"), &header, SECRET)), "The signature does not match.");
}

#[test]
fn an_old_or_future_timestamp_is_refused() {
    let t = now();
    assert_eq!(refused(verify(BODY, &sign(t - 301, BODY, SECRET), SECRET)), "The signature is too old.");
    assert_eq!(refused(verify(BODY, &sign(t + 301, BODY, SECRET), SECRET)), "The signature is too old.");
    assert!(verify(BODY, &sign(t - 290, BODY, SECRET), SECRET).is_ok());
    assert!(verify_with_tolerance(BODY, &sign(t - 1000, BODY, SECRET), SECRET, 1200).is_ok());
}

#[test]
fn a_missing_or_malformed_header_is_refused() {
    let malformed = "The X-Coino-Signature header is missing or malformed.";
    let v1 = sign(now(), BODY, SECRET);
    let v1 = v1.split_once(",v1=").expect("v1").1;
    for header in [
        String::new(),
        "t=abc,v1=00".to_string(),
        format!("t=0,v1={v1}"),
        format!("t=-5,v1={v1}"),
        format!("v1={v1}"),
        format!("t={}", now()),
        format!("t={},v1=0", now()),
        format!("t={},v1=", now()),
    ] {
        assert_eq!(refused(verify(BODY, &header, SECRET)), malformed, "{header}");
    }
}

#[test]
fn a_payload_that_is_not_json_is_refused() {
    for body in ["not json", "42", "\"text\""] {
        let header = sign(now(), body, SECRET);
        assert_eq!(refused(verify(body, &header, SECRET)), "The payload is not JSON.", "{body}");
    }
}

#[test]
fn the_error_displays_its_message() {
    let error = verify(BODY, "", SECRET).expect_err("refused");
    assert_eq!(error.to_string(), error.message());
}
