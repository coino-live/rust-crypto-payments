//! Offline checks against a stand-in API: a tiny HTTP/1.1 server on 127.0.0.1 that records every request.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use coinolive::{Client, Error, VERSION};
use serde_json::{json, Value};

// ---------------------------------------------------------------------------------------------------------------
// The stand-in API
// ---------------------------------------------------------------------------------------------------------------

#[derive(Clone, Debug)]
struct Seen {
    method: String,
    /// Path and query as sent, e.g. "/api/v2/payments?status=paid".
    url: String,
    /// Lower-case names.
    headers: Vec<(String, String)>,
    body: String,
}

impl Seen {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }

    fn path(&self) -> &str {
        self.url.split('?').next().unwrap_or("")
    }
}

struct Reply {
    status: u16,
    content_type: &'static str,
    body: String,
    location: Option<&'static str>,
    delay: Duration,
}

fn answer(status: u16, body: Value) -> Reply {
    Reply { status, content_type: "application/json", body: body.to_string(), location: None, delay: Duration::ZERO }
}

fn text(status: u16, body: &str) -> Reply {
    Reply { status, content_type: "text/html", body: body.to_string(), location: None, delay: Duration::ZERO }
}

type Handler = dyn Fn(&Seen, usize) -> Reply + Send + Sync;

struct Stub {
    base_url: String,
    seen: Arc<Mutex<Vec<Seen>>>,
}

impl Stub {
    /// `handler(request, n)`: n counts the requests with the same method and URL, this one included.
    fn start(handler: impl Fn(&Seen, usize) -> Reply + Send + Sync + 'static) -> Stub {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let port = listener.local_addr().expect("address").port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let handler: Arc<Handler> = Arc::new(handler);
        let shared = Arc::clone(&seen);
        thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(stream) = stream else { continue };
                let handler = Arc::clone(&handler);
                let seen = Arc::clone(&shared);
                thread::spawn(move || serve(stream, &*handler, &seen));
            }
        });
        Stub { base_url: format!("http://127.0.0.1:{port}/api/v2"), seen }
    }

    fn client(&self) -> Client {
        self.client_with(2)
    }

    fn client_with(&self, max_retries: u32) -> Client {
        Client::builder("ck_test_mock")
            .base_url(self.base_url.clone())
            .timeout(Duration::from_secs(5))
            .max_retries(max_retries)
            .build()
            .expect("client")
    }

    fn seen(&self) -> Vec<Seen> {
        self.seen.lock().expect("lock").clone()
    }
}

fn serve(mut stream: TcpStream, handler: &Handler, seen: &Mutex<Vec<Seen>>) {
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut line = String::new();
    if reader.read_line(&mut line).unwrap_or(0) == 0 {
        return;
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let url = parts.next().unwrap_or("").to_string();

    let mut headers = Vec::new();
    loop {
        let mut header = String::new();
        if reader.read_line(&mut header).unwrap_or(0) == 0 {
            break;
        }
        let header = header.trim_end();
        if header.is_empty() {
            break;
        }
        if let Some((name, value)) = header.split_once(':') {
            headers.push((name.trim().to_ascii_lowercase(), value.trim().to_string()));
        }
    }
    let find = |name: &str| headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());

    let mut body: Vec<u8> = Vec::new();
    if let Some(length) = find("content-length").and_then(|l| l.parse::<usize>().ok()) {
        body.resize(length, 0);
        reader.read_exact(&mut body).expect("body");
    } else if find("transfer-encoding").is_some_and(|te| te.eq_ignore_ascii_case("chunked")) {
        loop {
            let mut size = String::new();
            reader.read_line(&mut size).expect("chunk size");
            let size = usize::from_str_radix(size.trim(), 16).unwrap_or(0);
            let mut chunk = vec![0u8; size + 2]; // the data and its CRLF
            reader.read_exact(&mut chunk).expect("chunk");
            if size == 0 {
                break;
            }
            body.extend_from_slice(&chunk[..size]);
        }
    }

    let request = Seen { method, url, headers, body: String::from_utf8(body).expect("utf-8 body") };
    let n = {
        let mut seen = seen.lock().expect("lock");
        seen.push(request.clone());
        seen.iter().filter(|s| s.method == request.method && s.url == request.url).count()
    };

    let reply = handler(&request, n);
    thread::sleep(reply.delay);
    let mut out = format!(
        "HTTP/1.1 {} Stub\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        reply.status,
        reply.content_type,
        reply.body.len()
    );
    if let Some(location) = reply.location {
        out.push_str("Location: ");
        out.push_str(location);
        out.push_str("\r\n");
    }
    out.push_str("\r\n");
    out.push_str(&reply.body);
    let _ = stream.write_all(out.as_bytes());
    let _ = stream.flush();
}

/// Starts a stand-in that always answers `status`, runs `call` and returns how many requests it made and its error.
fn attempts(status: u16, max_retries: u32, call: impl FnOnce(&Client) -> Result<Value, Error>) -> (usize, Error) {
    let stub = Stub::start(move |_, _| answer(status, json!({ "message": "Busy" })));
    let error = call(&stub.client_with(max_retries)).expect_err("the call should fail");
    (stub.seen().len(), error)
}

fn payment() -> Value {
    json!({ "id": "01pay", "status": "open", "checkout_url": "https://example.test/pay/01pay" })
}

// ---------------------------------------------------------------------------------------------------------------
// Arguments
// ---------------------------------------------------------------------------------------------------------------

#[test]
fn version_is_1_0_0() {
    assert_eq!(VERSION, "1.0.0");
    assert_eq!(coinolive::DEFAULT_BASE_URL, "https://coino.live/api/v2");
}

#[test]
fn refuses_a_key_that_is_not_ck() {
    for key in ["sk_123", "", "ck_live_", "ck_prod_abc", "ck_live_abc-1", "ck_test_abc def"] {
        let error = Client::new(key).expect_err(key);
        assert!(matches!(error, Error::InvalidArgument(_)), "{key}: {error:?}");
        assert!(error.message().contains("ck_live_"));
    }
    assert!(Client::new("ck_live_abc123").is_ok());
    assert!(Client::new("ck_test_ABC789").is_ok());
}

#[test]
fn refuses_a_plain_http_base_url() {
    for url in [
        "http://coino.live/api/v2",
        "http://localhost.example.com/api/v2",
        "http://127.0.0.1:/api/v2",
        "ftp://coino.live/api/v2",
    ] {
        let error = Client::builder("ck_test_abc").base_url(url).build().expect_err(url);
        assert!(matches!(error, Error::InvalidArgument(_)), "{url}: {error:?}");
        assert_eq!(error.message(), "base_url must start with https://.");
    }
    for url in ["https://coino.live/api/v2", "http://127.0.0.1:9/api/v2", "http://localhost", "http://localhost:8000/"] {
        assert!(Client::builder("ck_test_abc").base_url(url).build().is_ok(), "{url}");
    }
}

#[test]
fn defaults_to_the_live_api_and_hides_the_key() {
    let client = Client::builder("ck_live_secret123").base_url("").build().expect("client");
    let debug = format!("{client:?}");
    assert!(debug.contains("https://coino.live/api/v2"), "{debug}");
    assert!(!debug.contains("secret123"), "{debug}");
}

#[test]
fn trims_the_trailing_slash_of_the_base_url() {
    let stub = Stub::start(|_, _| answer(200, json!({ "data": [] })));
    let client = Client::builder("ck_test_mock").base_url(format!("{}//", stub.base_url)).build().expect("client");
    client.currencies().expect("currencies");
    assert_eq!(stub.seen()[0].url, "/api/v2/currencies");
}

// ---------------------------------------------------------------------------------------------------------------
// Requests: paths, methods, headers and bodies
// ---------------------------------------------------------------------------------------------------------------

#[test]
fn create_retries_a_503_with_one_idempotency_key() {
    let stub = Stub::start(|_, n| {
        if n < 3 {
            answer(503, json!({ "message": "Busy" }))
        } else {
            answer(201, json!({ "data": payment() }))
        }
    });
    let params = json!({ "amount": "25.00", "currency": "EUR", "order_id": "1001" });
    let created = stub.client().payments().create(&params, None).expect("create");
    assert_eq!(created, payment());

    let seen = stub.seen();
    assert_eq!(seen.len(), 3);
    let key = seen[0].header("idempotency-key").expect("an idempotency key").to_string();
    assert_eq!(key.len(), 32);
    assert!(key.bytes().all(|b| b.is_ascii_hexdigit()), "{key}");
    for request in &seen {
        assert_eq!(request.method, "POST");
        assert_eq!(request.url, "/api/v2/payments");
        assert_eq!(request.header("idempotency-key"), Some(key.as_str()));
        assert_eq!(request.header("authorization"), Some("Bearer ck_test_mock"));
        assert_eq!(request.header("accept"), Some("application/json"));
        assert_eq!(request.header("content-type"), Some("application/json"));
        let agent = request.header("user-agent").unwrap_or("");
        assert!(agent.starts_with(&format!("coinolive-rust/{VERSION} Rust/")), "{agent}");
        assert_eq!(serde_json::from_str::<Value>(&request.body).expect("JSON body"), params);
    }
}

#[test]
fn create_uses_a_given_idempotency_key() {
    let stub = Stub::start(|_, _| answer(201, json!({ "data": payment() })));
    let client = stub.client();
    client.payments().create(&json!({ "amount": "1.00" }), Some("order-1001-1")).expect("create");
    client.payments().create(&json!({ "amount": "1.00" }), Some("")).expect("create");
    client.payments().create(&json!({ "amount": "1.00" }), None).expect("create");

    let keys: Vec<String> = stub.seen().iter().map(|s| s.header("idempotency-key").unwrap_or("").to_string()).collect();
    assert_eq!(keys[0], "order-1001-1");
    assert_eq!(keys[1].len(), 32, "an empty key is replaced by a random one");
    assert_eq!(keys[2].len(), 32);
    assert_ne!(keys[1], keys[2], "every call gets its own random key");
}

#[test]
fn create_accepts_any_serializable_params() {
    let mut order = std::collections::BTreeMap::new();
    order.insert("amount", "9.99");
    order.insert("order_id", "1002");
    let stub = Stub::start(|_, _| answer(201, json!({ "data": payment() })));
    stub.client().payments().create(&order, None).expect("create");
    let body: Value = serde_json::from_str(&stub.seen()[0].body).expect("JSON body");
    assert_eq!(body, json!({ "amount": "9.99", "order_id": "1002" }));
}

#[test]
fn every_method_calls_its_endpoint() {
    let stub = Stub::start(|_, _| answer(200, json!({ "data": { "ok": true }, "meta": { "next_cursor": null } })));
    let client = stub.client();
    let ok = json!({ "ok": true });
    let page = json!({ "data": { "ok": true }, "meta": { "next_cursor": null } });

    assert_eq!(client.payments().retrieve("01pay").unwrap(), ok);
    assert_eq!(client.payments().retrieve("a b/c?").unwrap(), ok);
    assert_eq!(client.payments().list(&[("status", "paid"), ("order_id", "")]).unwrap(), page);
    assert_eq!(client.payments().list(&[]).unwrap(), page);
    assert_eq!(client.payments().list(&[("order_id", "a b&c=d")]).unwrap(), page);
    assert_eq!(client.payments().cancel("01pay").unwrap(), ok);

    assert_eq!(client.custody().create(&json!({ "external_id": "user-42", "name": "Ada" })).unwrap(), ok);
    assert_eq!(client.custody().retrieve("01usr").unwrap(), ok);
    assert_eq!(client.custody().list(&[("external_id", "user-42")]).unwrap(), page);
    assert_eq!(client.custody().deposits("01usr", None).unwrap(), page);
    assert_eq!(client.custody().deposits("01usr", Some("c2")).unwrap(), page);
    assert_eq!(client.custody().transfer("01usr", "usdt-tron", None).unwrap(), ok);
    assert_eq!(client.custody().transfer("01usr", "usdt-tron", Some("1.5")).unwrap(), ok);

    let withdrawal = json!({ "asset": "usdt-tron", "amount": "50", "address": "T123", "reference": "payout-77" });
    assert_eq!(client.withdrawals().create(&withdrawal).unwrap(), ok);
    assert_eq!(client.withdrawals().retrieve("01wd").unwrap(), ok);

    assert_eq!(client.currencies().unwrap(), ok);
    assert_eq!(client.assets().unwrap(), ok);
    assert_eq!(client.balances().unwrap(), ok);

    let expected: [(&str, &str, Option<Value>); 18] = [
        ("GET", "/api/v2/payments/01pay", None),
        ("GET", "/api/v2/payments/a%20b%2Fc%3F", None),
        ("GET", "/api/v2/payments?status=paid", None),
        ("GET", "/api/v2/payments", None),
        ("GET", "/api/v2/payments?order_id=a+b%26c%3Dd", None),
        ("POST", "/api/v2/payments/01pay/cancel", None),
        ("POST", "/api/v2/custody/users", Some(json!({ "external_id": "user-42", "name": "Ada" }))),
        ("GET", "/api/v2/custody/users/01usr", None),
        ("GET", "/api/v2/custody/users?external_id=user-42", None),
        ("GET", "/api/v2/custody/users/01usr/deposits", None),
        ("GET", "/api/v2/custody/users/01usr/deposits?cursor=c2", None),
        ("POST", "/api/v2/custody/users/01usr/transfers", Some(json!({ "asset": "usdt-tron" }))),
        ("POST", "/api/v2/custody/users/01usr/transfers", Some(json!({ "asset": "usdt-tron", "amount": "1.5" }))),
        ("POST", "/api/v2/withdrawals", Some(withdrawal)),
        ("GET", "/api/v2/withdrawals/01wd", None),
        ("GET", "/api/v2/currencies", None),
        ("GET", "/api/v2/assets", None),
        ("GET", "/api/v2/balances", None),
    ];
    let seen = stub.seen();
    assert_eq!(seen.len(), expected.len());
    for (request, (method, url, body)) in seen.iter().zip(expected) {
        assert_eq!((request.method.as_str(), request.url.as_str()), (method, url));
        assert_eq!(request.header("authorization"), Some("Bearer ck_test_mock"), "{url}");
        assert_eq!(request.header("idempotency-key"), None, "{url}");
        match body {
            Some(body) => {
                assert_eq!(request.header("content-type"), Some("application/json"), "{url}");
                assert_eq!(serde_json::from_str::<Value>(&request.body).expect("JSON body"), body, "{url}");
            }
            None => {
                assert_eq!(request.header("content-type"), None, "{url}");
                assert!(request.body.is_empty(), "{url}");
            }
        }
    }
}

#[test]
fn all_follows_next_cursor() {
    let stub = Stub::start(|request, _| {
        if request.url.contains("cursor=") {
            answer(200, json!({ "data": [{ "id": "p3" }], "meta": { "next_cursor": null, "prev_cursor": "c1" } }))
        } else {
            answer(200, json!({ "data": [{ "id": "p1" }, { "id": "p2" }], "meta": { "next_cursor": "c2", "prev_cursor": null } }))
        }
    });
    let client = stub.client();
    let ids: Vec<String> = client
        .payments()
        .all(&[("status", "paid")])
        .map(|payment| payment.expect("payment")["id"].as_str().unwrap_or("").to_string())
        .collect();
    assert_eq!(ids, ["p1", "p2", "p3"]);
    let urls: Vec<String> = stub.seen().into_iter().map(|s| s.url).collect();
    assert_eq!(urls, ["/api/v2/payments?status=paid", "/api/v2/payments?status=paid&cursor=c2"]);
}

#[test]
fn all_stops_at_an_error() {
    let stub = Stub::start(|_, _| answer(422, json!({ "message": "The status is invalid." })));
    let client = stub.client();
    let mut payments = client.payments().all(&[("status", "nope")]);
    assert!(matches!(payments.next(), Some(Err(Error::Validation(_)))));
    assert!(payments.next().is_none());
    assert_eq!(stub.seen().len(), 1);
}

// ---------------------------------------------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------------------------------------------

#[test]
fn maps_answers_to_errors() {
    let stub = Stub::start(|request, _| match (request.method.as_str(), request.path()) {
        ("GET", "/api/v2/payments/missing") => answer(404, json!({ "message": "Payment not found." })),
        ("POST", "/api/v2/payments/bad/cancel") => answer(
            422,
            json!({ "message": "Payments with a deposit cannot be cancelled.", "errors": { "id": ["has a deposit"] } }),
        ),
        ("GET", "/api/v2/balances") => answer(401, json!({ "message": "Invalid API key." })),
        ("GET", "/api/v2/currencies") => answer(418, json!({ "errors": "not an object" })),
        ("POST", "/api/v2/withdrawals") => answer(403, json!({ "message": "API withdrawals are off." })),
        ("GET", "/api/v2/withdrawals/text") => text(200, "not json"),
        ("GET", "/api/v2/assets") => Reply { location: Some("https://example.test/"), ..text(302, "") },
        _ => text(404, "<html>not here</html>"),
    });
    let client = stub.client();

    let e = client.payments().retrieve("missing").unwrap_err();
    assert!(matches!(e, Error::NotFound(_)), "{e:?}");
    assert_eq!((e.status(), e.message()), (404, "Payment not found."));
    assert_eq!(e.to_string(), "Payment not found.");

    let e = client.payments().cancel("bad").unwrap_err();
    assert!(matches!(e, Error::Validation(_)), "{e:?}");
    assert_eq!(e.status(), 422);
    assert!(e.message().contains("deposit"));
    assert_eq!(e.errors().expect("field errors")["id"][0], "has a deposit");

    let e = client.balances().unwrap_err();
    assert!(matches!(e, Error::Authentication(_)), "{e:?}");
    assert_eq!(e.message(), "Invalid API key.");

    let e = client.currencies().unwrap_err();
    assert!(matches!(e, Error::Api(_)), "{e:?}");
    assert_eq!((e.status(), e.message()), (418, "HTTP 418"));
    assert!(e.errors().expect("an API error").is_empty());

    let e = client.withdrawals().create(&json!({ "asset": "usdt-tron", "amount": "1", "address": "T1" })).unwrap_err();
    assert!(matches!(e, Error::Api(_)), "{e:?}");
    assert_eq!((e.status(), e.message()), (403, "API withdrawals are off."));

    let e = client.withdrawals().retrieve("text").unwrap_err();
    assert!(matches!(e, Error::Api(_)), "{e:?}");
    assert_eq!((e.status(), e.message()), (200, "HTTP 200"));

    let e = client.assets().unwrap_err();
    assert!(matches!(e, Error::Api(_)), "{e:?}");
    assert_eq!(e.status(), 302);
    assert!(e.message().contains("Check base_url"), "{}", e.message());

    let e = client.request("GET", "/nothing", None, None, false).unwrap_err();
    assert!(matches!(e, Error::Api(_)), "{e:?}");
    assert_eq!(e.status(), 404);
    assert!(e.message().contains(&format!("There is no Coino Live API at {}", stub.base_url)), "{}", e.message());

    // None of these is temporary, so each was sent once.
    assert_eq!(stub.seen().len(), 8);
}

#[test]
fn a_timeout_is_a_connection_error() {
    let stub = Stub::start(|_, _| Reply { delay: Duration::from_secs(3), ..answer(200, json!({ "data": [] })) });
    let client = Client::builder("ck_test_mock")
        .base_url(stub.base_url.clone())
        .timeout(Duration::from_millis(500))
        .max_retries(0)
        .build()
        .expect("client");
    let e = client.request("GET", "/slow", None, None, false).unwrap_err();
    assert!(matches!(e, Error::Connection(_)), "{e:?}");
    assert_eq!(e.status(), 0);
    assert!(e.message().starts_with("Could not reach Coino Live: "), "{}", e.message());
}

#[test]
fn an_unreachable_api_is_a_connection_error_after_retrying() {
    let port = TcpListener::bind("127.0.0.1:0").expect("bind").local_addr().expect("address").port();
    // The listener is dropped, so nothing answers on that port.
    let client = Client::builder("ck_test_mock")
        .base_url(format!("http://127.0.0.1:{port}/api/v2"))
        .timeout(Duration::from_secs(2))
        .max_retries(1)
        .build()
        .expect("client");
    let started = Instant::now();
    let e = client.assets().unwrap_err();
    assert!(matches!(e, Error::Connection(_)), "{e:?}");
    assert!(started.elapsed() >= Duration::from_millis(500), "one retry after a 0.5 s pause");
}

// ---------------------------------------------------------------------------------------------------------------
// Retries: network errors, 429 and 5xx, twice, only for requests that are safe to repeat
// ---------------------------------------------------------------------------------------------------------------

#[test]
fn a_429_is_retried_twice_then_returned() {
    let (count, e) = attempts(429, 2, |c| c.currencies());
    assert!(matches!(e, Error::RateLimit(_)), "{e:?}");
    assert_eq!(count, 3);
}

#[test]
fn a_5xx_read_is_retried_twice_then_returned() {
    let (count, e) = attempts(503, 2, |c| c.payments().retrieve("01pay"));
    assert!(matches!(e, Error::Api(_)), "{e:?}");
    assert_eq!((count, e.status(), e.message()), (3, 503, "Busy"));
}

#[test]
fn a_read_recovers_after_a_retry() {
    let stub = Stub::start(|_, n| if n == 1 { answer(500, json!({})) } else { answer(200, json!({ "data": [1] })) });
    assert_eq!(stub.client().assets().unwrap(), json!([1]));
    assert_eq!(stub.seen().len(), 2);
}

#[test]
fn max_retries_zero_sends_once() {
    let (count, _) = attempts(503, 0, |c| c.balances());
    assert_eq!(count, 1);
}

#[test]
fn client_errors_are_not_retried() {
    let (count, e) = attempts(422, 2, |c| c.payments().list(&[]));
    assert!(matches!(e, Error::Validation(_)), "{e:?}");
    assert_eq!(count, 1);
}

#[test]
fn a_transfer_is_not_retried() {
    let (count, e) = attempts(503, 2, |c| c.custody().transfer("01usr", "usdt-tron", None));
    assert_eq!((count, e.status()), (1, 503));
}

#[test]
fn a_withdrawal_without_a_reference_is_not_retried() {
    let (count, _) = attempts(503, 2, |c| c.withdrawals().create(&json!({ "asset": "usdt-tron", "amount": "50", "address": "T1" })));
    assert_eq!(count, 1);
    let (count, _) = attempts(503, 2, |c| {
        c.withdrawals().create(&json!({ "asset": "usdt-tron", "amount": "50", "address": "T1", "reference": "" }))
    });
    assert_eq!(count, 1);
}

#[test]
fn a_withdrawal_with_a_reference_is_retried() {
    let (count, _) = attempts(503, 2, |c| {
        c.withdrawals().create(&json!({ "asset": "usdt-tron", "amount": "50", "address": "T1", "reference": "payout-77" }))
    });
    assert_eq!(count, 3);
}

#[test]
fn a_custody_user_create_is_retried() {
    let (count, _) = attempts(502, 2, |c| c.custody().create(&json!({ "external_id": "user-42" })));
    assert_eq!(count, 3);
}

#[test]
fn a_cancel_is_retried() {
    let (count, _) = attempts(503, 2, |c| c.payments().cancel("01pay"));
    assert_eq!(count, 3);
}

#[test]
fn a_payment_create_is_retried_with_the_same_key() {
    let stub = Stub::start(|_, _| answer(500, json!({ "message": "Oops" })));
    let e = stub.client().payments().create(&json!({ "amount": "5.00" }), Some("order-7-1")).unwrap_err();
    assert_eq!(e.status(), 500);
    let keys: Vec<String> = stub.seen().iter().map(|s| s.header("idempotency-key").unwrap_or("").to_string()).collect();
    assert_eq!(keys, ["order-7-1", "order-7-1", "order-7-1"]);
}

#[test]
fn a_post_without_a_safe_repeat_is_not_retried_by_request() {
    let stub = Stub::start(|_, _| answer(503, json!({ "message": "Busy" })));
    let body = json!({ "a": 1 });
    let e = stub.client().request("POST", "/anything", Some(&body), None, false).unwrap_err();
    assert_eq!(e.status(), 503);
    assert_eq!(stub.seen().len(), 1);
}
