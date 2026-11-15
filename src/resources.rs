use serde::Serialize;
use serde_json::{json, Value};

use crate::client::{data, path_id, query, to_body, Client};
use crate::error::Error;

/// Payments: create one per order, send the customer to its checkout_url, read it back before shipping.
#[derive(Clone, Copy, Debug)]
pub struct Payments<'a> {
    client: &'a Client,
}

impl<'a> Payments<'a> {
    pub(crate) fn new(client: &'a Client) -> Self {
        Payments { client }
    }

    /// Creates a payment from params such as
    /// `json!({"amount": "25.00", "currency": "EUR", "order_id": "1001", "notify_url": "…"})`.
    ///
    /// Without an idempotency key a random one is used, so a retried request never creates a second payment.
    /// Pass your own key (for example `Some("order-1001-1")`) to make repeating the call from your code safe too.
    pub fn create<P: Serialize + ?Sized>(&self, params: &P, idempotency_key: Option<&str>) -> Result<Value, Error> {
        let body = to_body(params)?;
        let key = match idempotency_key.filter(|key| !key.is_empty()) {
            Some(key) => key.to_string(),
            None => random_key()?,
        };
        self.client.request("POST", "/payments", Some(&body), Some(&key), false).map(data)
    }

    pub fn retrieve(&self, id: &str) -> Result<Value, Error> {
        self.client.request("GET", &format!("/payments/{}", path_id(id)), None, None, false).map(data)
    }

    /// One page of payments (50), newest first: `{"data": […], "meta": {"next_cursor": …, "prev_cursor": …}}`.
    ///
    /// Filters: `status`, `order_id`, `cursor`, e.g. `&[("status", "paid")]`. Empty values are left out.
    pub fn list(&self, filters: &[(&str, &str)]) -> Result<Value, Error> {
        self.client.request("GET", &format!("/payments{}", query(filters)), None, None, false)
    }

    /// Every payment matching the filters, fetching pages as you go:
    /// `for payment in coino.payments().all(&[("status", "paid")]) { let payment = payment?; … }`.
    pub fn all(&self, filters: &[(&str, &str)]) -> AllPayments<'a> {
        let cursor = filters.iter().find(|(key, _)| *key == "cursor").map(|(_, value)| value.to_string());
        AllPayments {
            client: self.client,
            filters: filters
                .iter()
                .filter(|(key, _)| *key != "cursor")
                .map(|(key, value)| (key.to_string(), value.to_string()))
                .collect(),
            cursor: cursor.filter(|c| !c.is_empty()),
            page: Vec::new().into_iter(),
            done: false,
        }
    }

    /// Cancels an open payment that has received nothing.
    pub fn cancel(&self, id: &str) -> Result<Value, Error> {
        self.client.request("POST", &format!("/payments/{}/cancel", path_id(id)), None, None, true).map(data)
    }
}

/// The payments of [`Payments::all`], one page at a time. An error ends the iteration.
#[derive(Debug)]
pub struct AllPayments<'a> {
    client: &'a Client,
    filters: Vec<(String, String)>,
    cursor: Option<String>,
    page: std::vec::IntoIter<Value>,
    done: bool,
}

impl Iterator for AllPayments<'_> {
    type Item = Result<Value, Error>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            if let Some(payment) = self.page.next() {
                return Some(Ok(payment));
            }
            if self.done {
                return None;
            }
            let mut params = self.filters.clone();
            if let Some(cursor) = self.cursor.take() {
                params.push(("cursor".to_string(), cursor));
            }
            match self.client.request("GET", &format!("/payments{}", query(&params)), None, None, false) {
                Err(error) => {
                    self.done = true;
                    return Some(Err(error));
                }
                Ok(answer) => {
                    self.cursor = answer
                        .get("meta")
                        .and_then(|meta| meta.get("next_cursor"))
                        .and_then(Value::as_str)
                        .filter(|c| !c.is_empty())
                        .map(str::to_string);
                    self.done = self.cursor.is_none();
                    self.page = match data(answer) {
                        Value::Array(payments) => payments.into_iter(),
                        _ => Vec::new().into_iter(),
                    };
                }
            }
        }
    }
}

/// Custody users: lifetime deposit addresses and balances for the users of your app.
#[derive(Clone, Copy, Debug)]
pub struct Custody<'a> {
    client: &'a Client,
}

impl<'a> Custody<'a> {
    pub(crate) fn new(client: &'a Client) -> Self {
        Custody { client }
    }

    /// Creates a user, or returns the existing one with the same external_id:
    /// `json!({"external_id": "user-42", "name": "Ada"})`. Optional: name, email, metadata.
    pub fn create<P: Serialize + ?Sized>(&self, params: &P) -> Result<Value, Error> {
        let body = to_body(params)?;
        self.client.request("POST", "/custody/users", Some(&body), None, true).map(data)
    }

    pub fn retrieve(&self, id: &str) -> Result<Value, Error> {
        self.client.request("GET", &format!("/custody/users/{}", path_id(id)), None, None, false).map(data)
    }

    /// One page of users (50), newest first: `{"data": […], "meta": {"next_cursor": …, "prev_cursor": …}}`.
    ///
    /// Filters: `external_id`, `cursor`. Empty values are left out.
    pub fn list(&self, filters: &[(&str, &str)]) -> Result<Value, Error> {
        self.client.request("GET", &format!("/custody/users{}", query(filters)), None, None, false)
    }

    /// One page of a user's credited deposits (50), newest first: `{"data": […], "next_cursor": …}`.
    pub fn deposits(&self, id: &str, cursor: Option<&str>) -> Result<Value, Error> {
        let path = format!("/custody/users/{}/deposits{}", path_id(id), query(&[("cursor", cursor.unwrap_or(""))]));
        self.client.request("GET", &path, None, None, false)
    }

    /// Moves a user's balance of one asset to your main balance: all of it (`None`), or just `amount`.
    pub fn transfer(&self, id: &str, asset: &str, amount: Option<&str>) -> Result<Value, Error> {
        let body = match amount {
            Some(amount) => json!({ "asset": asset, "amount": amount }),
            None => json!({ "asset": asset }),
        };
        self.client
            .request("POST", &format!("/custody/users/{}/transfers", path_id(id)), Some(&body), None, false)
            .map(data)
    }
}

/// API withdrawals. Turn them on in the gateway settings first.
#[derive(Clone, Copy, Debug)]
pub struct Withdrawals<'a> {
    client: &'a Client,
}

impl<'a> Withdrawals<'a> {
    pub(crate) fn new(client: &'a Client) -> Self {
        Withdrawals { client }
    }

    /// Sends coins from your balance (or a custody user's balance) to an address:
    /// `json!({"asset": "usdt-tron", "amount": "50", "address": "T…", "reference": "payout-77"})`.
    /// Optional: custody_user, reference.
    ///
    /// Give a reference: the same reference returns the same withdrawal, so a retried request never pays twice.
    pub fn create<P: Serialize + ?Sized>(&self, params: &P) -> Result<Value, Error> {
        let body = to_body(params)?;
        let repeatable = body.get("reference").is_some_and(truthy);
        self.client.request("POST", "/withdrawals", Some(&body), None, repeatable).map(data)
    }

    pub fn retrieve(&self, id: &str) -> Result<Value, Error> {
        self.client.request("GET", &format!("/withdrawals/{}", path_id(id)), None, None, false).map(data)
    }
}

/// A value JavaScript would call true: a reference that is set.
fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(_) | Value::Object(_) => true,
    }
}

/// 32 random hex characters.
fn random_key() -> Result<String, Error> {
    let mut bytes = [0u8; 16];
    getrandom::getrandom(&mut bytes)
        .map_err(|e| Error::InvalidArgument(format!("Could not make a random idempotency key: {e}")))?;
    Ok(hex::encode(bytes))
}
