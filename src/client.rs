use std::fmt;
use std::fmt::Write as _;
use std::io::Read;
use std::thread;
use std::time::Duration;

use serde::Serialize;
use serde_json::{Map, Value};

use crate::error::{ApiError, Error};
use crate::resources::{Custody, Payments, Withdrawals};
use crate::{DEFAULT_BASE_URL, VERSION};

const RUST_VERSION: &str = env!("COINOLIVE_RUSTC_VERSION");

/// Coino Live API client.
///
/// ```no_run
/// # fn main() -> Result<(), coinolive::Error> {
/// let coino = coinolive::Client::new("ck_live_abc123")?;
/// let balances = coino.balances()?;
/// # Ok(())
/// # }
/// ```
#[derive(Clone)]
pub struct Client {
    api_key: String,
    base_url: String,
    max_retries: u32,
    agent: ureq::Agent,
}

/// Options for a [`Client`]: `Client::builder(key).timeout(…).max_retries(…).base_url(…).build()`.
#[derive(Clone)]
pub struct ClientBuilder {
    api_key: String,
    base_url: String,
    timeout: Duration,
    max_retries: u32,
}

impl ClientBuilder {
    /// The API to talk to; `https://coino.live/api/v2` unless you were given another.
    pub fn base_url(mut self, base_url: impl Into<String>) -> Self {
        self.base_url = base_url.into();
        self
    }

    /// Time allowed per request (30 seconds).
    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    /// Retries for network errors, 429 and 5xx (2), only for requests that are safe to repeat.
    pub fn max_retries(mut self, max_retries: u32) -> Self {
        self.max_retries = max_retries;
        self
    }

    /// Checks the key and base URL and makes the client.
    pub fn build(self) -> Result<Client, Error> {
        if !valid_api_key(&self.api_key) {
            return Err(Error::InvalidArgument(
                "An API key looks like ck_live_… (create one in your dashboard under Developers).".to_string(),
            ));
        }
        let base_url = if self.base_url.is_empty() { DEFAULT_BASE_URL } else { self.base_url.as_str() };
        let base_url = base_url.trim_end_matches('/').to_string();
        if !allowed_base_url(&base_url) {
            return Err(Error::InvalidArgument("base_url must start with https://.".to_string()));
        }

        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout(self.timeout)
            .redirects(0)
            .user_agent(&format!("coinolive-rust/{VERSION} Rust/{RUST_VERSION}"))
            .build();

        Ok(Client { api_key: self.api_key, base_url, max_retries: self.max_retries, agent })
    }
}

impl fmt::Debug for ClientBuilder {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ClientBuilder")
            .field("base_url", &self.base_url)
            .field("timeout", &self.timeout)
            .field("max_retries", &self.max_retries)
            .finish_non_exhaustive()
    }
}

impl Client {
    /// A client with the default options.
    ///
    /// `api_key`: ck_live_… (Dashboard → Developers)
    pub fn new(api_key: &str) -> Result<Client, Error> {
        Client::builder(api_key).build()
    }

    /// Options: `Client::builder(key).timeout(Duration::from_secs(30)).max_retries(2).base_url(…).build()`.
    pub fn builder(api_key: &str) -> ClientBuilder {
        ClientBuilder {
            api_key: api_key.to_string(),
            base_url: DEFAULT_BASE_URL.to_string(),
            timeout: Duration::from_secs(30),
            max_retries: 2,
        }
    }

    /// Payments: create one per order, send the customer to its checkout_url, read it back before shipping.
    pub fn payments(&self) -> Payments<'_> {
        Payments::new(self)
    }

    /// Custody users: lifetime deposit addresses and balances for the users of your app.
    pub fn custody(&self) -> Custody<'_> {
        Custody::new(self)
    }

    /// API withdrawals. Turn them on in the gateway settings first.
    pub fn withdrawals(&self) -> Withdrawals<'_> {
        Withdrawals::new(self)
    }

    /// Currencies a payment can be priced in: `[{"code": "EUR", "usd_per_unit": "1.09"}, …]`.
    pub fn currencies(&self) -> Result<Value, Error> {
        self.request("GET", "/currencies", None, None, false).map(data)
    }

    /// Coins customers can pay with right now.
    pub fn assets(&self) -> Result<Value, Error> {
        self.request("GET", "/assets", None, None, false).map(data)
    }

    /// Your account balance per coin, after fees.
    pub fn balances(&self) -> Result<Value, Error> {
        self.request("GET", "/balances", None, None, false).map(data)
    }

    /// Internal: use the resources. Returns the decoded JSON answer.
    ///
    /// `path` is relative to the base URL, e.g. `/payments`.
    pub fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
        idempotency_key: Option<&str>,
        repeatable: bool,
    ) -> Result<Value, Error> {
        let idempotency_key = idempotency_key.filter(|key| !key.is_empty());
        // Retrying a POST is only safe when repeating it returns the same object (an idempotency key, a custody
        // external_id, a withdrawal reference).
        let retriable = method == "GET" || idempotency_key.is_some() || repeatable;
        let mut attempt: u32 = 0;
        loop {
            match self.send(method, path, body, idempotency_key) {
                Ok(answer) => return Ok(answer),
                Err(error) => {
                    let temporary = matches!(error, Error::Connection(_) | Error::RateLimit(_)) || error.status() >= 500;
                    if !retriable || !temporary || attempt >= self.max_retries {
                        return Err(error);
                    }
                    // 0.5 s, 1 s, 2 s, 4 s, then 8 s.
                    thread::sleep(Duration::from_millis(500u64 << attempt.min(4)));
                    attempt += 1;
                }
            }
        }
    }

    fn send(&self, method: &str, path: &str, body: Option<&Value>, idempotency_key: Option<&str>) -> Result<Value, Error> {
        let mut request = self
            .agent
            .request(method, &format!("{}{path}", self.base_url))
            .set("Authorization", &format!("Bearer {}", self.api_key))
            .set("Accept", "application/json");
        if let Some(key) = idempotency_key {
            request = request.set("Idempotency-Key", key);
        }

        let result = match body {
            Some(body) => {
                let json = serde_json::to_vec(body)
                    .map_err(|e| Error::InvalidArgument(format!("The params could not be encoded as JSON: {e}")))?;
                request.set("Content-Type", "application/json").send_bytes(&json)
            }
            None => request.call(),
        };
        let response = match result {
            Ok(response) => response,
            Err(ureq::Error::Status(_, response)) => response,
            Err(e) => return Err(Error::Connection(format!("Could not reach Coino Live: {e}"))),
        };

        let status = response.status();
        let mut raw = Vec::new();
        if let Err(e) = response.into_reader().read_to_end(&mut raw) {
            return Err(Error::Connection(format!("Could not reach Coino Live: {e}")));
        }
        let json = match serde_json::from_slice::<Value>(&raw).ok().filter(|v| v.is_object() || v.is_array()) {
            Some(json) if (200..300).contains(&status) => return Ok(json),
            other => other,
        };

        if (300..400).contains(&status) || (status == 404 && json.is_none()) {
            return Err(Error::Api(ApiError {
                message: format!("There is no Coino Live API at {} (HTTP {status}). Check base_url.", self.base_url),
                status,
                errors: Map::new(),
            }));
        }

        let message = json
            .as_ref()
            .and_then(|j| j.get("message"))
            .and_then(Value::as_str)
            .filter(|m| !m.is_empty())
            .map_or_else(|| format!("HTTP {status}"), str::to_string);
        let errors = json
            .as_ref()
            .and_then(|j| j.get("errors"))
            .and_then(Value::as_object)
            .cloned()
            .unwrap_or_default();
        Err(Error::from_status(status, message, errors))
    }
}

impl fmt::Debug for Client {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Client")
            .field("base_url", &self.base_url)
            .field("max_retries", &self.max_retries)
            .finish_non_exhaustive()
    }
}

/// The `data` member of an answer.
pub(crate) fn data(answer: Value) -> Value {
    match answer {
        Value::Object(mut object) => object.remove("data").unwrap_or(Value::Null),
        _ => Value::Null,
    }
}

/// Params as a JSON value.
pub(crate) fn to_body<P: Serialize + ?Sized>(params: &P) -> Result<Value, Error> {
    serde_json::to_value(params).map_err(|e| Error::InvalidArgument(format!("The params could not be encoded as JSON: {e}")))
}

/// "?a=1&b=2" from the non-empty values, or "".
pub(crate) fn query<K: AsRef<str>, V: AsRef<str>>(params: &[(K, V)]) -> String {
    let mut out = String::new();
    for (key, value) in params {
        let (key, value) = (key.as_ref(), value.as_ref());
        if value.is_empty() {
            continue;
        }
        out.push(if out.is_empty() { '?' } else { '&' });
        out.push_str(&encode(key, true));
        out.push('=');
        out.push_str(&encode(value, true));
    }
    out
}

/// An ID as one path segment.
pub(crate) fn path_id(id: &str) -> String {
    encode(id, false)
}

fn encode(value: &str, space_as_plus: bool) -> String {
    let mut out = String::with_capacity(value.len());
    for &byte in value.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => out.push(char::from(byte)),
            b' ' if space_as_plus => out.push('+'),
            _ => {
                let _ = write!(out, "%{byte:02X}");
            }
        }
    }
    out
}

/// ck_live_… or ck_test_… followed by letters and digits.
fn valid_api_key(key: &str) -> bool {
    let rest = key.strip_prefix("ck_live_").or_else(|| key.strip_prefix("ck_test_"));
    matches!(rest, Some(rest) if !rest.is_empty() && rest.bytes().all(|b| b.is_ascii_alphanumeric()))
}

/// https://…, or plain http to localhost / 127.0.0.1 (for tests).
fn allowed_base_url(url: &str) -> bool {
    if url.starts_with("https://") {
        return true;
    }
    let Some(rest) = url.strip_prefix("http://") else {
        return false;
    };
    let Some(rest) = rest.strip_prefix("localhost").or_else(|| rest.strip_prefix("127.0.0.1")) else {
        return false;
    };
    let rest = match rest.strip_prefix(':') {
        Some(port) => {
            let digits = port.bytes().take_while(u8::is_ascii_digit).count();
            if digits == 0 {
                return false;
            }
            &port[digits..]
        }
        None => rest,
    };
    rest.is_empty() || rest.starts_with('/')
}
