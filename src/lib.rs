//! Coino Live API v2 client for Rust (blocking, built on ureq).
//!
//! ```no_run
//! use serde_json::json;
//!
//! # fn main() -> Result<(), coinolive::Error> {
//! let coino = coinolive::Client::new("ck_live_abc123")?; // Dashboard → Developers
//! let payment = coino.payments().create(
//!     &json!({ "amount": "25.00", "currency": "EUR", "order_id": "1001" }),
//!     None,
//! )?;
//! println!("Send the customer to {}", payment["checkout_url"]);
//! # Ok(())
//! # }
//! ```
//!
//! Every answer is a [`serde_json::Value`], like the plain objects the other Coino Live libraries return.

#![forbid(unsafe_code)]

mod client;
mod error;
mod resources;
pub mod webhooks;

pub use client::{Client, ClientBuilder};
pub use error::{ApiError, Error, SignatureVerificationError};
pub use resources::{AllPayments, Custody, Payments, Withdrawals};

/// The library version, sent in the User-Agent header.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// The API every client talks to unless given another `base_url`.
pub const DEFAULT_BASE_URL: &str = "https://coino.live/api/v2";
