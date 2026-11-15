# Coino Live Rust library

Accept crypto payments with the [Coino Live API](https://coino.live/api-guide). A blocking client built on
[ureq](https://crates.io/crates/ureq) and [serde_json](https://crates.io/crates/serde_json); every answer is a
`serde_json::Value`, like the plain objects the other Coino Live libraries return.

## Install

Add the crate to `Cargo.toml` from this repository:

```toml
[dependencies]
coinolive = { git = "https://github.com/coino-live/rust-crypto-payments", tag = "v1.0.0" }
serde_json = "1"
```

(Use `branch = "main"` instead of `tag` to follow the latest code.)

## Take a payment

```rust
use serde_json::json;

let coino = coinolive::Client::new("ck_live_…")?; // Dashboard → Developers

let payment = coino.payments().create(
    &json!({
        "amount": "25.00",          // a string, at most 2 decimals
        "currency": "EUR",          // any code from coino.currencies(); USD by default
        "order_id": "1001",
        "success_url": "https://shop.example/thanks",
        "cancel_url": "https://shop.example/cart",
        "notify_url": "https://shop.example/coino-webhook",
    }),
    None,                           // idempotency key: random unless you pass Some("…")
)?;

// Send the customer to payment["checkout_url"].
```

Ship the order when a fresh read says `paid`:

```rust
let payment = coino.payments().retrieve(payment_id)?;
if payment["status"] == "paid" {
    // …
}
```

Statuses: `open`, `confirming`, `partially_paid`, `paid`, `expired`, `cancelled`.

Params can be any type that implements `serde::Serialize`, so your own structs work as well as `json!`.

## Webhooks

The signature covers the raw body, so read it unparsed:

```rust
use coinolive::webhooks;

// raw_body: the request body as bytes; signature: the X-Coino-Signature header ("" when it is missing).
let event = match webhooks::verify(raw_body, signature, &signing_secret) {
    Ok(event) => event,
    Err(_) => return 401,
};
if event["event"].as_str().unwrap_or("").starts_with("payment.") {
    let id = event["data"]["id"].as_str().unwrap_or("");
    let payment = coino.payments().retrieve(id)?; // act on what the API says, not on the webhook body
}
return 200;
```

`webhooks::verify_with_tolerance(raw_body, signature, &signing_secret, 600)` accepts signatures up to 600 seconds old
instead of 300.

## Everything else

```rust
coino.payments().list(&[("status", "paid"), ("order_id", "1001")])?; // {"data": […], "meta": {"next_cursor", "prev_cursor"}}
for payment in coino.payments().all(&[("status", "paid")]) { let payment = payment?; /* every page */ }
coino.payments().cancel(payment_id)?;                 // only while nothing was received

coino.currencies()?;                                  // prices can be in these
coino.assets()?;                                      // customers can pay with these
coino.balances()?;                                    // your balance per coin, with usd_value

let user = coino.custody().create(&json!({ "external_id": "user-42" }))?; // lifetime deposit addresses for your users
let user_id = user["id"].as_str().unwrap_or("");
coino.custody().retrieve(user_id)?;
coino.custody().list(&[("external_id", "user-42")])?;
coino.custody().deposits(user_id, None)?;             // Some(cursor) for the next page
coino.custody().transfer(user_id, "usdt-tron", None)?; // move the user's balance to yours; Some("10") for part of it

coino.withdrawals().create(&json!({ "asset": "usdt-tron", "amount": "50", "address": "T…", "reference": "payout-77" }))?;
coino.withdrawals().retrieve(withdrawal_id)?;
```

## Errors and retries

Every failure returns a `coinolive::Error`:

| Error | When |
| --- | --- |
| `Error::Authentication` | Wrong or revoked key, or the gateway is suspended (401) |
| `Error::NotFound` | Unknown ID, or it belongs to another gateway (404) |
| `Error::Validation` | The request was refused; `e.errors()` names the fields (422) |
| `Error::RateLimit` | Too many requests (429) |
| `Error::Connection` | The API could not be reached, or the request timed out |
| `Error::Api` | Anything else, with `e.status()` |
| `Error::InvalidArgument` | The API key or `base_url` is in the wrong form |

`e.message()` says why, `e.status()` gives the HTTP status (0 for `Connection`), and `e.errors()` the field errors.
A refused webhook is a `coinolive::SignatureVerificationError`.

```rust
match coino.payments().retrieve(payment_id) {
    Ok(payment) => { /* … */ }
    Err(coinolive::Error::NotFound(_)) => { /* unknown payment */ }
    Err(e) => eprintln!("Coino Live: {e} (HTTP {})", e.status()),
}
```

Network errors, 429 and 5xx answers are retried twice with a short pause, but only for requests that are safe to repeat:
reads, `payments().create()` (it always sends an idempotency key, random unless you pass one as the second argument),
`custody().create()` (same `external_id`, same user) and `withdrawals().create()` with a `reference`.

```rust
use std::time::Duration;

let coino = coinolive::Client::builder("ck_live_…")
    .timeout(Duration::from_secs(30))   // per request
    .max_retries(2)
    .base_url("https://coino.live/api/v2")
    .build()?;
```

## Tests

```bash
cargo test   # offline checks against a stand-in API on 127.0.0.1
```

## License

MIT, see [LICENSE](LICENSE).
