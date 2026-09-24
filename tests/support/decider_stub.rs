//! A scripted decider endpoint on `std::net`, for integration tests.
//!
//! Include it from a test file with
//!
//! ```ignore
//! #[path = "support/decider_stub.rs"]
//! mod decider_stub;
//! ```
//!
//! [`DeciderStub::start`] binds an ephemeral port on 127.0.0.1 and serves
//! one HTTP/1.1 request per connection on its own thread. Each request is
//! counted and recorded (method, path, headers, body) as soon as it has been
//! read, then answered from a per-request script: [`DeciderStub::push`]
//! queues a [`Reply`] (any status, headers such as `Location`, any body, an
//! optional delay, or a hold until [`DeciderStub::release`]). When the queue
//! is empty the default reply (HTTP 500, empty body, unless changed with
//! [`DeciderStub::set_default`]) is used. [`closed_port`] yields a port with
//! nothing listening on it.
//!
//! No async runtime and no dev-dependency: this is the whole server.

#![allow(dead_code)]

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

/// Path the stub advertises in [`DeciderStub::url`]. The stub answers any
/// path; this one mirrors the real endpoint's shape.
pub const STUB_PATH: &str = "/v1/systemone";

/// One scripted response.
#[derive(Debug, Clone)]
pub struct Reply {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Wait this long before answering.
    pub delay: Duration,
    /// Wait for [`DeciderStub::release`] before answering.
    pub hold: bool,
}

impl Reply {
    /// A reply with `status` and an empty body.
    pub fn status(status: u16) -> Self {
        Reply {
            status,
            headers: Vec::new(),
            body: Vec::new(),
            delay: Duration::ZERO,
            hold: false,
        }
    }

    /// HTTP 200 with `value` as a JSON body.
    pub fn json(value: &serde_json::Value) -> Self {
        Self::status(200)
            .header("Content-Type", "application/json")
            .with_body(value.to_string().into_bytes())
    }

    /// `status` with a raw body.
    pub fn raw(status: u16, body: impl Into<Vec<u8>>) -> Self {
        Self::status(status).with_body(body.into())
    }

    pub fn with_body(mut self, body: Vec<u8>) -> Self {
        self.body = body;
        self
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.headers.push((name.to_string(), value.to_string()));
        self
    }

    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// Hold the request open until [`DeciderStub::release`].
    pub fn hold(mut self) -> Self {
        self.hold = true;
        self
    }
}

/// One request as the stub received it.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    /// Header names lowercased, in arrival order.
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
}

impl Recorded {
    /// First header named `name` (case-insensitive).
    pub fn header(&self, name: &str) -> Option<&str> {
        let name = name.to_ascii_lowercase();
        self.headers
            .iter()
            .find(|(k, _)| *k == name)
            .map(|(_, v)| v.as_str())
    }

    pub fn body_str(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }

    pub fn json(&self) -> serde_json::Value {
        serde_json::from_slice(&self.body).expect("recorded body is JSON")
    }
}

#[derive(Debug)]
struct State {
    script: VecDeque<Reply>,
    default: Reply,
    requests: Vec<Recorded>,
    released: bool,
    shutdown: bool,
}

#[derive(Debug)]
struct Shared {
    state: Mutex<State>,
    cond: Condvar,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// The running stub. Dropping it stops the accept loop and wakes any held
/// or delayed request.
pub struct DeciderStub {
    addr: SocketAddr,
    shared: Arc<Shared>,
}

impl DeciderStub {
    /// Bind 127.0.0.1 on an ephemeral port and start serving.
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub");
        let addr = listener.local_addr().expect("stub addr");
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                script: VecDeque::new(),
                default: Reply::status(500),
                requests: Vec::new(),
                released: false,
                shutdown: false,
            }),
            cond: Condvar::new(),
        });
        let accept_shared = Arc::clone(&shared);
        thread::spawn(move || {
            for conn in listener.incoming() {
                if accept_shared.lock().shutdown {
                    break;
                }
                let Ok(stream) = conn else { continue };
                let shared = Arc::clone(&accept_shared);
                thread::spawn(move || serve(stream, &shared));
            }
        });
        DeciderStub { addr, shared }
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn port(&self) -> u16 {
        self.addr.port()
    }

    /// `http://127.0.0.1:<port>/v1/systemone`.
    pub fn url(&self) -> String {
        format!("http://127.0.0.1:{}{}", self.port(), STUB_PATH)
    }

    /// `http://localhost:<port>/v1/systemone`.
    pub fn localhost_url(&self) -> String {
        format!("http://localhost:{}{}", self.port(), STUB_PATH)
    }

    /// Queue the reply for the next unanswered request.
    pub fn push(&self, reply: Reply) -> &Self {
        self.shared.lock().script.push_back(reply);
        self
    }

    /// Reply used once the queue is empty.
    pub fn set_default(&self, reply: Reply) -> &Self {
        self.shared.lock().default = reply;
        self
    }

    /// Requests received so far (counted once fully read).
    pub fn request_count(&self) -> usize {
        self.shared.lock().requests.len()
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.shared.lock().requests.clone()
    }

    pub fn last_request(&self) -> Option<Recorded> {
        self.shared.lock().requests.last().cloned()
    }

    /// Block until at least `n` requests have arrived, or `timeout` passes.
    /// Returns whether `n` was reached.
    pub fn wait_for_requests(&self, n: usize, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        let mut state = self.shared.lock();
        while state.requests.len() < n {
            let now = Instant::now();
            if now >= deadline {
                return false;
            }
            state = self
                .shared
                .cond
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        true
    }

    /// Let every held request (current and future) answer.
    pub fn release(&self) {
        self.shared.lock().released = true;
        self.shared.cond.notify_all();
    }
}

impl Drop for DeciderStub {
    fn drop(&mut self) {
        self.shared.lock().shutdown = true;
        self.shared.cond.notify_all();
        // Unblock the accept loop.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(200));
    }
}

/// A loopback port with nothing listening on it.
pub fn closed_port() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    port
}

/// `http://127.0.0.1:<closed port>/v1/systemone`.
pub fn closed_url() -> String {
    format!("http://127.0.0.1:{}{}", closed_port(), STUB_PATH)
}

fn serve(stream: TcpStream, shared: &Shared) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let Some(recorded) = read_request(&stream) else {
        return;
    };

    let reply = {
        let mut state = shared.lock();
        if state.shutdown {
            return;
        }
        state.requests.push(recorded);
        shared.cond.notify_all();
        state
            .script
            .pop_front()
            .unwrap_or_else(|| state.default.clone())
    };

    if reply.hold {
        let mut state = shared.lock();
        while !state.released && !state.shutdown {
            state = shared.cond.wait(state).unwrap_or_else(|e| e.into_inner());
        }
    }
    if !reply.delay.is_zero() {
        let deadline = Instant::now() + reply.delay;
        let mut state = shared.lock();
        while !state.shutdown {
            let now = Instant::now();
            if now >= deadline {
                break;
            }
            state = shared
                .cond
                .wait_timeout(state, deadline - now)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        if state.shutdown {
            return;
        }
    }

    write_reply(stream, &reply);
}

fn read_request(stream: &TcpStream) -> Option<Recorded> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let path = parts.next()?.to_string();

    let mut headers = Vec::new();
    let mut content_length = 0usize;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).ok()? == 0 {
            return None;
        }
        let h = h.trim_end_matches(['\r', '\n']);
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            let k = k.trim().to_ascii_lowercase();
            let v = v.trim().to_string();
            if k == "content-length" {
                content_length = v.parse().ok()?;
            }
            headers.push((k, v));
        }
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).ok()?;
    Some(Recorded {
        method,
        path,
        headers,
        body,
    })
}

fn write_reply(mut stream: TcpStream, reply: &Reply) {
    let mut head = format!(
        "HTTP/1.1 {} Stub\r\nContent-Length: {}\r\nConnection: close\r\n",
        reply.status,
        reply.body.len()
    );
    for (k, v) in &reply.headers {
        head.push_str(&format!("{}: {}\r\n", k, v));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&reply.body);
    let _ = stream.flush();
}
