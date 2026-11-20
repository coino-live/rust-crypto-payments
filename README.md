# Coino Live Rust Client: Accept Crypto & USDT Payments

`coinolive` 1.0.0 is the official Rust crate for the [Coino Live](https://coino.live) REST API v2. Use it to accept crypto payments in Rust: create checkouts, confirm paid orders, verify webhooks, give your users lifetime deposit addresses and withdraw to your own wallet. Full endpoint details are in the [API guide](https://coino.live/api-guide).

- **Payments**: create checkouts, read status, list and cancel
- **Webhooks**: verify the `X-Coino-Signature` header against the raw body
- **Custody**: lifetime deposit addresses for your users
- **Balances and withdrawals**: check your balance per coin and withdraw to your own wallet
- **Typed errors and safe retries**, with idempotency keys built in

The client is **blocking** and built on [ureq](https://crates.io/crates/ureq) and [serde_json](https://crates.io/crates/serde_json). Every answer is a `serde_json::Value`, like the plain objects the other Coino Live libraries return. MIT licensed.

> **Upgrading from API v1?** This repository previously held code for the retired Coino Live API v1, whose endpoints no longer answer. Version 1.0.0 targets API v2.

## Supported assets, networks and fees

Coino Live is a crypto payment gateway that accepts **117 assets on 11 networks**: Bitcoin, Litecoin, Ethereum, BNB Smart Chain, opBNB, Polygon, Base, Arbitrum, TRON, TON and Solana. USDT and USDC are available on most of them. Dogecoin and Bitcoin Cash are not accepted.

- **Platform fee:** 0.25% per payment. Blockchain network fees are separate.
- **Signup:** email and password, with no KYC.
- **Payment flow:** payments are credited to your Coino Live balance, and you withdraw them to your own wallet.

## Installation

Add the crate from [crates.io](https://crates.io/crates/coinolive):

```bash
cargo add coinolive
```

Or add it to `Cargo.toml` as a git dependency from this repository:

```toml
[dependencies]
coinolive = { git = "https://github.com/coino-live/rust-crypto-payments", tag = "v1.0.0" }
serde_json = "1"
```

Use `branch = "main"` instead of `tag` to follow the latest code rather than the pinned release.

## Create a crypto payment

Get your API key from **Dashboard → Developers**. Then create a payment and send the customer to its checkout page:

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

Params can be any type that implements `serde::Serialize`, so your own structs work as well as `json!`.

### Confirm the payment before shipping the order

Ship the order only when a fresh read says `paid`:

```rust
let payment = coino.payments().retrieve(payment_id)?;
if payment["status"] == "paid" {
    // …
}
```

Payment statuses: `open`, `confirming`, `partially_paid`, `paid`, `expired`, `cancelled`.

## Verify payment webhooks

The signature covers the raw request body, so read the body unparsed. Verify the signature first. Then retrieve the payment from the API before you act:

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

By default, signatures up to 300 seconds old are accepted. `webhooks::verify_with_tolerance(raw_body, signature, &signing_secret, 600)` accepts signatures up to 600 seconds old instead.

## List and cancel payments

```rust
coino.payments().list(&[("status", "paid"), ("order_id", "1001")])?; // {"data": […], "meta": {"next_cursor", "prev_cursor"}}
for payment in coino.payments().all(&[("status", "paid")]) { let payment = payment?; /* every page */ }
coino.payments().cancel(payment_id)?;                 // only while nothing was received
```

## Check currencies, assets and balances

`currencies()` lists the currencies you can set prices in. `assets()` lists the crypto assets customers can pay with.

```rust
coino.currencies()?;                                  // prices can be in these
coino.assets()?;                                      // customers can pay with these
coino.balances()?;                                    // your balance per coin, with usd_value
```

## Custody: lifetime deposit addresses for your users

Create custody users with lifetime deposit addresses and read their deposits. You can also move their balance to yours, for example USDT on TRON:

```rust
let user = coino.custody().create(&json!({ "external_id": "user-42" }))?; // lifetime deposit addresses for your users
let user_id = user["id"].as_str().unwrap_or("");
coino.custody().retrieve(user_id)?;
coino.custody().list(&[("external_id", "user-42")])?;
coino.custody().deposits(user_id, None)?;             // Some(cursor) for the next page
coino.custody().transfer(user_id, "usdt-tron", None)?; // move the user's balance to yours; Some("10") for part of it
```

## Withdraw to your own wallet

Withdraw funds from your Coino Live balance to your own wallet:

```rust
coino.withdrawals().create(&json!({ "asset": "usdt-tron", "amount": "50", "address": "T…", "reference": "payout-77" }))?;
coino.withdrawals().retrieve(withdrawal_id)?;
```

## Handle API errors

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

`e.message()` says why the request failed. `e.status()` gives the HTTP status (`0` for `Connection`), and `e.errors()` returns the field errors. A refused webhook is a `coinolive::SignatureVerificationError`.

```rust
match coino.payments().retrieve(payment_id) {
    Ok(payment) => { /* … */ }
    Err(coinolive::Error::NotFound(_)) => { /* unknown payment */ }
    Err(e) => eprintln!("Coino Live: {e} (HTTP {})", e.status()),
}
```

## Retries, timeouts and client configuration

Network errors, 429 and 5xx answers are retried twice with a short pause, but only for requests that are safe to repeat:

- reads
- `payments().create()`, which always sends an idempotency key (random unless you pass one as the second argument)
- `custody().create()`, where the same `external_id` returns the same user
- `withdrawals().create()` with a `reference`

Set the timeout, retry count and API base URL with the client builder:

```rust
use std::time::Duration;

let coino = coinolive::Client::builder("ck_live_…")
    .timeout(Duration::from_secs(30))   // per request
    .max_retries(2)
    .base_url("https://coino.live/api/v2")
    .build()?;
```

## Run the tests

```bash
cargo test   # offline checks against a stand-in API on 127.0.0.1
```

## License

MIT, see [LICENSE](LICENSE).
