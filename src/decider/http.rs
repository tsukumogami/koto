//! A bounded HTTP `POST` for decider providers.
//!
//! [`post_json_with_deadline`] wraps attohttpc with connect, read, and
//! whole-request timeouts, and runs the request on a worker thread that the
//! caller waits on with `recv_timeout`. attohttpc's own deadline doesn't
//! cover name resolution, so the outer wait is what keeps a hung DNS lookup
//! within the budget. Redirects are never followed, proxies are never used
//! for a loopback host, and the response body is capped at
//! [`MAX_RESPONSE_BYTES`] before anything parses it. Errors carry fixed text
//! and an error kind only: never a body, a header, or the URL.

use std::io::Read;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use attohttpc::{ErrorKind, ProxySettings};
use url::Url;

use crate::config::validate::is_loopback_url;

use super::types::{ApiKey, DeciderError, ErrorClass};

/// Largest response body read, in bytes (1 MiB).
pub const MAX_RESPONSE_BYTES: usize = 1024 * 1024;

/// `User-Agent` sent with every decider request.
pub const USER_AGENT: &str = concat!("koto/", env!("CARGO_PKG_VERSION"));

/// Run `work` on a worker thread and wait at most `budget` for it.
///
/// Returns `timeout` if the worker hasn't answered in time. The worker is
/// left to finish on its own (its own timeouts end it); its late result is
/// discarded. A worker that panics is reported as `connect`.
pub fn run_with_deadline<T, F>(budget: Duration, work: F) -> Result<T, DeciderError>
where
    T: Send + 'static,
    F: FnOnce() -> Result<T, DeciderError> + Send + 'static,
{
    let (tx, rx) = mpsc::sync_channel(1);
    let spawned = thread::Builder::new()
        .name("koto-decider-http".to_string())
        .spawn(move || {
            let _ = tx.send(work());
        });
    if spawned.is_err() {
        return Err(DeciderError::new(
            ErrorClass::Connect,
            "could not start the request thread",
        ));
    }
    match rx.recv_timeout(budget) {
        Ok(result) => result,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(DeciderError::timeout()),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(DeciderError::new(
            ErrorClass::Connect,
            "request thread ended without a result",
        )),
    }
}

/// `POST` `body` as JSON to `url` with `bearer` in the `Authorization`
/// header, bounded by `budget`. Returns the status and the body (at most
/// [`MAX_RESPONSE_BYTES`]).
///
/// Any status is returned as-is, 3xx included: redirects are not followed,
/// so the bearer token can't travel to a `Location` host. Classifying the
/// status is the caller's job.
pub fn post_json_with_deadline(
    url: &Url,
    bearer: &ApiKey,
    body: Vec<u8>,
    budget: Duration,
) -> Result<(u16, Vec<u8>), DeciderError> {
    let url = url.clone();
    let bearer = bearer.clone();
    run_with_deadline(budget, move || send(&url, &bearer, body, budget))
}

fn send(
    url: &Url,
    bearer: &ApiKey,
    body: Vec<u8>,
    budget: Duration,
) -> Result<(u16, Vec<u8>), DeciderError> {
    let proxies = if is_loopback_url(url) {
        // An empty settings value: no proxy for any scheme.
        ProxySettings::builder().build()
    } else {
        ProxySettings::from_env()
    };
    let request = with_headers(
        attohttpc::post(url.as_str())
            .follow_redirects(false)
            .connect_timeout(budget)
            .read_timeout(budget)
            .timeout(budget)
            .proxy_settings(proxies),
        bearer,
    )?;
    let response = request.bytes(body).send().map_err(|e| classify(e.kind()))?;

    let (status, _headers, reader) = response.split();
    let mut buf = Vec::new();
    reader
        .take(MAX_RESPONSE_BYTES as u64 + 1)
        .read_to_end(&mut buf)
        .map_err(|e| classify_io(e.kind()))?;
    if buf.len() > MAX_RESPONSE_BYTES {
        return Err(DeciderError::malformed("response body exceeds 1 MiB"));
    }
    Ok((status.as_u16(), buf))
}

/// Set the request headers with the fallible header API. attohttpc's
/// `header()` panics on a value that isn't a valid header (a key holding a
/// newline, say); here that's a `malformed` error with fixed text, which
/// never includes the value.
fn with_headers<B>(
    builder: attohttpc::RequestBuilder<B>,
    bearer: &ApiKey,
) -> Result<attohttpc::RequestBuilder<B>, DeciderError> {
    let bad_header = |_| DeciderError::malformed("request header value is not valid");
    builder
        .try_header("Authorization", bearer_value(bearer))
        .map_err(bad_header)?
        .try_header("Content-Type", "application/json")
        .map_err(bad_header)?
        .try_header("Accept", "application/json")
        .map_err(bad_header)?
        .try_header("User-Agent", USER_AGENT)
        .map_err(bad_header)
}

fn bearer_value(key: &ApiKey) -> String {
    format!("Bearer {}", key.expose_for_transport())
}

fn classify_io(kind: std::io::ErrorKind) -> DeciderError {
    use std::io::ErrorKind as K;
    match kind {
        K::TimedOut | K::WouldBlock => DeciderError::timeout(),
        k => DeciderError::with_kind(ErrorClass::Connect, "request failed", k),
    }
}

/// Map an attohttpc error to a class using only its kind. The error's own
/// text is never used: some kinds carry a proxy's body.
fn classify(kind: &ErrorKind) -> DeciderError {
    match kind {
        ErrorKind::Io(e) => classify_io(e.kind()),
        ErrorKind::InvalidResponse(_) => DeciderError::malformed("response is not valid HTTP"),
        ErrorKind::StatusCode(s) => DeciderError::http_status(s.as_u16()),
        ErrorKind::TooManyRedirections => {
            DeciderError::new(ErrorClass::Connect, "unexpected redirect handling")
        }
        ErrorKind::InvalidBaseUrl | ErrorKind::InvalidUrlHost | ErrorKind::InvalidUrlPort => {
            DeciderError::new(ErrorClass::Connect, "decider endpoint is not usable")
        }
        ErrorKind::ConnectError { .. } | ErrorKind::ConnectNotSupported => {
            DeciderError::new(ErrorClass::Connect, "proxy connection failed")
        }
        ErrorKind::Tls(_) | ErrorKind::InvalidDNSName(_) | ErrorKind::ServerCertVerifier(_) => {
            DeciderError::new(ErrorClass::Connect, "TLS connection failed")
        }
        _ => DeciderError::new(ErrorClass::Connect, "request failed"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn deadline_returns_timeout_for_a_hung_worker() {
        // Stands in for a DNS lookup that never returns: no network.
        let budget = Duration::from_millis(200);
        let start = Instant::now();
        let got: Result<(), _> = run_with_deadline(budget, || {
            thread::sleep(Duration::from_secs(5));
            Ok(())
        });
        let took = start.elapsed();
        assert_eq!(got.unwrap_err().class, ErrorClass::Timeout);
        assert!(
            took < budget + Duration::from_millis(250),
            "took {:?}",
            took
        );
    }

    #[test]
    fn deadline_passes_through_a_prompt_result() {
        let got = run_with_deadline(Duration::from_secs(2), || Ok(7u8));
        assert_eq!(got.unwrap(), 7);
        let err: Result<u8, _> =
            run_with_deadline(Duration::from_secs(2), || Err(DeciderError::malformed("x")));
        assert_eq!(err.unwrap_err().class, ErrorClass::Malformed);
    }

    #[test]
    fn deadline_reports_a_panicking_worker_as_connect() {
        let got: Result<(), _> =
            run_with_deadline(Duration::from_secs(2), || panic!("worker blew up"));
        assert_eq!(got.unwrap_err().class, ErrorClass::Connect);
    }

    #[test]
    fn io_kinds_map_to_classes() {
        assert_eq!(
            classify_io(std::io::ErrorKind::TimedOut).class,
            ErrorClass::Timeout
        );
        assert_eq!(
            classify_io(std::io::ErrorKind::WouldBlock).class,
            ErrorClass::Timeout
        );
        let e = classify_io(std::io::ErrorKind::ConnectionRefused);
        assert_eq!(e.class, ErrorClass::Connect);
        assert_eq!(e.detail, "request failed: connection refused");
    }

    #[test]
    fn a_key_that_is_not_a_valid_header_is_an_error_not_a_panic() {
        // The key never reaches the network: header building fails first.
        // The URL is loopback on the discard port in case it didn't.
        let url = Url::parse("http://127.0.0.1:9/d").unwrap();
        for raw in ["sk-SECRET\nX", "sk-SECRET\rX", "sk-SECRET\u{0}X"] {
            let key = ApiKey::new(raw);
            let built = with_headers(attohttpc::post(url.as_str()), &key);
            let err = built.expect_err("header refused");
            assert_eq!(err.class, ErrorClass::Malformed);
            assert!(!err.detail.contains("SECRET"), "{}", err.detail);

            let got = post_json_with_deadline(&url, &key, b"{}".to_vec(), Duration::from_secs(2));
            let err = got.unwrap_err();
            assert_eq!(err.class, ErrorClass::Malformed, "{}", err.detail);
            assert!(!err.detail.contains("SECRET"), "{}", err.detail);
        }
    }

    #[test]
    fn user_agent_names_koto_and_its_version() {
        assert_eq!(USER_AGENT, format!("koto/{}", env!("CARGO_PKG_VERSION")));
    }
}
