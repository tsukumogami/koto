//! An in-memory S3-compatible endpoint on `std::net`, for integration tests.
//!
//! Include it from a test file with
//!
//! ```ignore
//! #[path = "support/fake_s3.rs"]
//! mod fake_s3;
//! ```
//!
//! [`FakeS3::start`] binds an ephemeral port on 127.0.0.1 and serves one
//! bucket, addressed path-style (`/<bucket>/<key>`), from a map in memory:
//! GET, PUT, HEAD and DELETE of objects, and ListObjectsV2
//! (`?list-type=2&prefix=..&delimiter=..`) answered in the XML rust-s3
//! parses, 1000 entries a page with a continuation token, as S3 pages.
//! Every request is recorded as (method, key) in arrival order, so a
//! test can assert exactly what a command wrote where. [`FakeS3::fail`]
//! makes every matching request answer with a given status instead,
//! [`FakeS3::fail_all`] does so for every request and
//! [`FakeS3::fail_writes`] for every PUT and DELETE,
//! [`FakeS3::garble_listing`] answers a listing with a body that isn't one,
//! and
//! [`FakeS3::put_after`] stores an object as soon as a given request has
//! been handled, the way another writer racing the command would.
//! [`FakeS3::set_latency`] holds every reply for a fixed time, so a test can
//! see what a command's round trips cost against a distant bucket.
//! [`FakeS3::hold`] holds one request's reply after handling it, so a test
//! can stop the command waiting on it at that exact point.
//!
//! A PUT honours `If-Match` and `If-None-Match: *` as S3 does: when the
//! stored object's ETag isn't the one named, or an object is there for
//! `If-None-Match: *`, it answers 412 PreconditionFailed and stores nothing.
//!
//! Authentication is ignored. A missing object or bucket answers 404 with an
//! S3 XML error body, as a real endpoint does. Each connection is read and
//! answered in full on its own thread and then closed, so it holds up under
//! the koto binary run as a subprocess by `assert_cmd`.
//!
//! No async runtime and no dev-dependency: this is the whole server.

#![allow(dead_code)]

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// One request as the endpoint received it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    /// `GET`, `PUT`, `HEAD`, `DELETE`, ...
    pub method: String,
    /// The object key, percent-decoded; empty for a bucket-level request
    /// such as a listing.
    pub key: String,
    /// For a listing, its `prefix` parameter.
    pub list_prefix: Option<String>,
    /// For a listing, its `delimiter` parameter, when it has one.
    pub delimiter: Option<String>,
    /// The access key id the request was signed with, read from the
    /// `Credential=` part of its `Authorization` header; `None` when the
    /// request carried no signature.
    pub access_key: Option<String>,
    /// When the request had been read in full.
    pub at: Instant,
}

#[derive(Debug, Clone)]
struct Fault {
    /// A method, `LIST` for a listing, [`ANY_METHOD`] for every request or
    /// [`WRITE_METHODS`] for every PUT and DELETE.
    method: String,
    /// The exact key (a listing's prefix); `None` matches every key.
    key: Option<String>,
    status: u16,
    /// Store a PUT's body before answering with the failure, as an
    /// endpoint that wrote the object and lost the reply would.
    store: bool,
    /// Answer 200 with a body that isn't a listing, instead of `status`.
    garble: bool,
    /// How many more matching requests fail; `None` fails them all.
    remaining: Option<usize>,
}

/// A [`Fault`] method matching every request, listings included.
const ANY_METHOD: &str = "*";

/// A [`Fault`] method matching every PUT and DELETE.
const WRITE_METHODS: &str = "PUT|DELETE";

impl Fault {
    fn matches(&self, method: &str, key: &str) -> bool {
        let method_matches = match self.method.as_str() {
            ANY_METHOD => true,
            WRITE_METHODS => method == "PUT" || method == "DELETE",
            m => m == method,
        };
        method_matches && self.key.as_deref().is_none_or(|k| k == key)
    }
}

#[derive(Debug, Clone)]
struct Trigger {
    method: String,
    key: String,
    put_key: String,
    bytes: Vec<u8>,
}

#[derive(Debug, Clone)]
struct Hold {
    method: String,
    key: String,
    duration: Duration,
}

#[derive(Debug, Default)]
struct State {
    objects: BTreeMap<String, Vec<u8>>,
    /// When each object was last written, which listings report as its
    /// LastModified.
    modified: BTreeMap<String, SystemTime>,
    requests: Vec<Request>,
    faults: Vec<Fault>,
    triggers: Vec<Trigger>,
    holds: Vec<Hold>,
    latency: Duration,
    shutdown: bool,
}

/// The running endpoint. Dropping it stops the accept loop.
pub struct FakeS3 {
    addr: SocketAddr,
    bucket: String,
    state: Arc<Mutex<State>>,
}

impl FakeS3 {
    /// Bind 127.0.0.1 on an ephemeral port and serve `bucket`.
    pub fn start(bucket: &str) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fake s3");
        let addr = listener.local_addr().expect("fake s3 addr");
        let state = Arc::new(Mutex::new(State::default()));
        let accept_state = Arc::clone(&state);
        let served = bucket.to_string();
        thread::spawn(move || {
            for conn in listener.incoming() {
                if lock(&accept_state).shutdown {
                    break;
                }
                let Ok(stream) = conn else { continue };
                let state = Arc::clone(&accept_state);
                let bucket = served.clone();
                thread::spawn(move || serve(stream, &bucket, &state));
            }
        });
        FakeS3 {
            addr,
            bucket: bucket.to_string(),
            state,
        }
    }

    /// `http://127.0.0.1:<port>`, the value for `session.cloud.endpoint`.
    pub fn endpoint(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    /// The bytes stored under `key`.
    pub fn object(&self, key: &str) -> Option<Vec<u8>> {
        lock(&self.state).objects.get(key).cloned()
    }

    /// Every stored key, sorted.
    pub fn keys(&self) -> Vec<String> {
        lock(&self.state).objects.keys().cloned().collect()
    }

    /// Every stored key starting with `prefix`, sorted.
    pub fn keys_under(&self, prefix: &str) -> Vec<String> {
        lock(&self.state)
            .objects
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect()
    }

    /// Store `bytes` under `key` directly, as another writer would.
    pub fn put(&self, key: &str, bytes: &[u8]) {
        let mut st = lock(&self.state);
        st.modified.insert(key.to_string(), SystemTime::now());
        st.objects.insert(key.to_string(), bytes.to_vec());
    }

    /// Set the LastModified listings report for `key`, as an object written
    /// at `at` would carry.
    pub fn set_last_modified(&self, key: &str, at: SystemTime) {
        lock(&self.state).modified.insert(key.to_string(), at);
    }

    /// Remove `key` directly.
    pub fn remove(&self, key: &str) {
        lock(&self.state).objects.remove(key);
    }

    /// Requests received so far, in arrival order.
    pub fn requests(&self) -> Vec<Request> {
        lock(&self.state).requests.clone()
    }

    /// Forget the requests recorded so far.
    pub fn clear_requests(&self) {
        lock(&self.state).requests.clear();
    }

    /// Answer every `method` request for exactly `key` with `status` (and
    /// an S3 error body) from now on, leaving the stored object alone.
    /// `method` `LIST` with a prefix as `key` fails the listings with
    /// exactly that prefix.
    pub fn fail(&self, method: &str, key: &str, status: u16) {
        lock(&self.state).faults.push(Fault {
            method: method.to_string(),
            key: Some(key.to_string()),
            status,
            store: false,
            garble: false,
            remaining: None,
        });
    }

    /// Answer every listing with exactly `prefix` with 200 and a body that
    /// isn't a ListObjectsV2 result, as an endpoint that answered with
    /// something else would.
    pub fn garble_listing(&self, prefix: &str) {
        lock(&self.state).faults.push(Fault {
            method: "LIST".to_string(),
            key: Some(prefix.to_string()),
            status: 200,
            store: false,
            garble: true,
            remaining: None,
        });
    }

    /// [`FakeS3::fail`] for the next `times` matching requests only; the
    /// ones after are answered normally. rust-s3 retries a failed request
    /// once, so one failed call by koto is two failed requests here.
    pub fn fail_times(&self, method: &str, key: &str, status: u16, times: usize) {
        lock(&self.state).faults.push(Fault {
            method: method.to_string(),
            key: Some(key.to_string()),
            status,
            store: false,
            garble: false,
            remaining: Some(times),
        });
    }

    /// Store every PUT to exactly `key` but answer it with `status`: the
    /// write lands and the client is told it failed.
    pub fn fail_after_storing(&self, key: &str, status: u16) {
        lock(&self.state).faults.push(Fault {
            method: "PUT".to_string(),
            key: Some(key.to_string()),
            status,
            store: true,
            garble: false,
            remaining: None,
        });
    }

    /// Answer every request, listings included, with `status` from now on,
    /// as an endpoint that refuses or throttles everything would.
    pub fn fail_all(&self, status: u16) {
        self.push_fault(ANY_METHOD, status);
    }

    /// Answer every PUT and DELETE with `status` from now on, storing and
    /// removing nothing, as keys that grant only list and get would.
    pub fn fail_writes(&self, status: u16) {
        self.push_fault(WRITE_METHODS, status);
    }

    fn push_fault(&self, method: &str, status: u16) {
        lock(&self.state).faults.push(Fault {
            method: method.to_string(),
            key: None,
            status,
            store: false,
            garble: false,
            remaining: None,
        });
    }

    /// The ETag the endpoint gives the object under `key`, without its
    /// quotes, as a listing, GET or PUT reply carries it.
    pub fn etag_of(&self, key: &str) -> Option<String> {
        lock(&self.state)
            .objects
            .get(key)
            .map(|bytes| etag(bytes).trim_matches('"').to_string())
    }

    /// Stop failing requests: undo every [`FakeS3::fail`].
    pub fn clear_faults(&self) {
        lock(&self.state).faults.clear();
    }

    /// Once, when the next `method` request for exactly `key` has been
    /// handled and before it is answered, store `bytes` under `put_key`.
    pub fn put_after(&self, method: &str, key: &str, put_key: &str, bytes: &[u8]) {
        lock(&self.state).triggers.push(Trigger {
            method: method.to_string(),
            key: key.to_string(),
            put_key: put_key.to_string(),
            bytes: bytes.to_vec(),
        });
    }

    /// Once, when the next `method` request for exactly `key` has been
    /// handled (a PUT's object stored), hold its reply for `duration`: the
    /// command that sent it waits there, with the request's effect already
    /// in the bucket.
    pub fn hold(&self, method: &str, key: &str, duration: Duration) {
        lock(&self.state).holds.push(Hold {
            method: method.to_string(),
            key: key.to_string(),
            duration,
        });
    }

    /// Hold every reply for `latency` before sending it, as a bucket across
    /// a network would. Zero, the default, answers at once.
    pub fn set_latency(&self, latency: Duration) {
        lock(&self.state).latency = latency;
    }
}

impl Drop for FakeS3 {
    fn drop(&mut self) {
        lock(&self.state).shutdown = true;
        // Unblock the accept loop.
        let _ = TcpStream::connect_timeout(&self.addr, Duration::from_millis(200));
    }
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}

/// A parsed request line plus its body.
struct Incoming {
    method: String,
    path: String,
    query: Vec<(String, String)>,
    body: Vec<u8>,
    access_key: Option<String>,
    /// The `If-Match` header, when the request carried one.
    if_match: Option<String>,
    /// The `If-None-Match` header, when the request carried one.
    if_none_match: Option<String>,
}

fn read_request(stream: &TcpStream) -> Option<Incoming> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();

    let mut content_length = 0usize;
    let mut access_key = None;
    let mut if_match = None;
    let mut if_none_match = None;
    loop {
        let mut h = String::new();
        if reader.read_line(&mut h).ok()? == 0 {
            break;
        }
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((name, value)) = h.split_once(':') {
            if name.trim().eq_ignore_ascii_case("content-length") {
                content_length = value.trim().parse().unwrap_or(0);
            }
            if name.trim().eq_ignore_ascii_case("authorization") {
                access_key = value
                    .split("Credential=")
                    .nth(1)
                    .and_then(|rest| rest.split('/').next())
                    .map(str::to_string);
            }
            if name.trim().eq_ignore_ascii_case("if-match") {
                if_match = Some(value.trim().to_string());
            }
            if name.trim().eq_ignore_ascii_case("if-none-match") {
                if_none_match = Some(value.trim().to_string());
            }
        }
    }
    let mut body = vec![0u8; content_length];
    reader.read_exact(&mut body).ok()?;

    let (path, query) = match target.split_once('?') {
        Some((p, q)) => (p.to_string(), q.to_string()),
        None => (target, String::new()),
    };
    let query = url::form_urlencoded::parse(query.as_bytes())
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    Some(Incoming {
        method,
        path: percent_decode(&path),
        query,
        body,
        access_key,
        if_match,
        if_none_match,
    })
}

fn percent_decode(s: &str) -> String {
    url::form_urlencoded::parse(format!("x={}", s.replace('+', "%2B")).as_bytes())
        .next()
        .map(|(_, v)| v.into_owned())
        .unwrap_or_default()
}

/// One response: status, extra headers, body. HEAD responses carry no body
/// and advertise none.
struct Reply {
    status: u16,
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
}

/// An object's ETag, quoted as S3 sends it: a hash of its content (64-bit
/// FNV-1a, which needs no dependency), so a changed object gets a new one
/// in a listing, a GET, a HEAD and a PUT's reply alike.
fn etag(bytes: &[u8]) -> String {
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    format!("\"{:016x}-{}\"", hash, bytes.len())
}

impl Reply {
    fn ok(body: Vec<u8>) -> Self {
        Reply {
            status: 200,
            headers: Vec::new(),
            body,
        }
    }

    fn error(status: u16, code: &str, message: &str) -> Self {
        Reply {
            status,
            headers: vec![("Content-Type", "application/xml".to_string())],
            body: format!(
                "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
                 <Error><Code>{}</Code><Message>{}</Message></Error>",
                code,
                xml_escape(message)
            )
            .into_bytes(),
        }
    }
}

fn serve(stream: TcpStream, bucket: &str, state: &Mutex<State>) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let Some(req) = read_request(&stream) else {
        return;
    };
    let reply = answer(&req, bucket, state);
    // Before the reply goes out, so the client's next request already
    // sees what the trigger stored.
    fire_triggers(&req, bucket, state);
    if let Some(hold) = take_hold(&req, bucket, state) {
        thread::sleep(hold);
    }
    let latency = lock(state).latency;
    if !latency.is_zero() {
        thread::sleep(latency);
    }
    write_reply(stream, &req.method, &reply);
}

/// Forget, and return, the first hold `req` matches.
fn take_hold(req: &Incoming, bucket: &str, state: &Mutex<State>) -> Option<Duration> {
    let rest = req.path.strip_prefix('/').unwrap_or(&req.path);
    let key = match rest.split_once('/') {
        Some((b, k)) if b == bucket => k,
        _ => return None,
    };
    let mut st = lock(state);
    let at = st
        .holds
        .iter()
        .position(|h| h.method == req.method && h.key == key)?;
    Some(st.holds.remove(at).duration)
}

/// Run, and forget, the triggers `req` matches.
fn fire_triggers(req: &Incoming, bucket: &str, state: &Mutex<State>) {
    let rest = req.path.strip_prefix('/').unwrap_or(&req.path);
    let key = match rest.split_once('/') {
        Some((b, k)) if b == bucket => k,
        _ => return,
    };
    let mut st = lock(state);
    let (fired, kept): (Vec<Trigger>, Vec<Trigger>) = std::mem::take(&mut st.triggers)
        .into_iter()
        .partition(|t| t.method == req.method && t.key == key);
    st.triggers = kept;
    for t in fired {
        st.modified.insert(t.put_key.clone(), SystemTime::now());
        st.objects.insert(t.put_key, t.bytes);
    }
}

fn answer(req: &Incoming, bucket: &str, state: &Mutex<State>) -> Reply {
    let rest = req.path.strip_prefix('/').unwrap_or(&req.path);
    let (named_bucket, key) = match rest.split_once('/') {
        Some((b, k)) => (b, k.to_string()),
        None => (rest, String::new()),
    };
    let param = |name: &str| {
        req.query
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.clone())
    };
    let is_list = req.method == "GET" && param("list-type").as_deref() == Some("2");

    let mut st = lock(state);
    st.requests.push(Request {
        method: req.method.clone(),
        key: if is_list { String::new() } else { key.clone() },
        list_prefix: if is_list {
            Some(param("prefix").unwrap_or_default())
        } else {
            None
        },
        delimiter: param("delimiter").filter(|d| is_list && !d.is_empty()),
        access_key: req.access_key.clone(),
        at: Instant::now(),
    });

    if named_bucket != bucket {
        return Reply::error(404, "NoSuchBucket", "The specified bucket does not exist");
    }
    let (fault_method, fault_key) = if is_list {
        ("LIST".to_string(), param("prefix").unwrap_or_default())
    } else {
        (req.method.clone(), key.clone())
    };
    if let Some(at) = st
        .faults
        .iter()
        .position(|f| f.matches(&fault_method, &fault_key))
    {
        let fault = st.faults[at].clone();
        if let Some(n) = fault.remaining {
            if n <= 1 {
                st.faults.remove(at);
            } else {
                st.faults[at].remaining = Some(n - 1);
            }
        }
        if fault.store {
            st.modified.insert(key.clone(), SystemTime::now());
            st.objects.insert(key.clone(), req.body.clone());
        }
        if fault.garble {
            return Reply::ok(b"<ListBucketResult><Contents><Key>".to_vec());
        }
        return Reply::error(fault.status, "InternalError", "injected failure");
    }

    if is_list {
        let prefix = param("prefix").unwrap_or_default();
        let delimiter = param("delimiter").filter(|d| !d.is_empty());
        let after = param("continuation-token").filter(|t| !t.is_empty());
        return Reply::ok(list_xml(
            bucket,
            &st.objects,
            &st.modified,
            &prefix,
            delimiter.as_deref(),
            after.as_deref(),
        ));
    }

    match req.method.as_str() {
        "GET" => match st.objects.get(&key) {
            Some(bytes) => Reply {
                status: 200,
                headers: vec![("ETag", etag(bytes))],
                body: bytes.clone(),
            },
            None => Reply::error(404, "NoSuchKey", "The specified key does not exist."),
        },
        "HEAD" => match st.objects.get(&key) {
            Some(bytes) => Reply {
                status: 200,
                headers: vec![("ETag", etag(bytes))],
                body: Vec::new(),
            },
            None => Reply {
                status: 404,
                headers: Vec::new(),
                body: Vec::new(),
            },
        },
        "PUT" => {
            let current = st.objects.get(&key).map(|bytes| etag(bytes));
            let unquote = |t: &str| t.trim().trim_matches('"').to_string();
            let failed = match (&req.if_match, &req.if_none_match) {
                (Some(want), _) => current.as_deref().map(unquote) != Some(unquote(want)),
                (None, Some(any)) if any.trim() == "*" => current.is_some(),
                _ => false,
            };
            if failed {
                return Reply::error(
                    412,
                    "PreconditionFailed",
                    "At least one of the pre-conditions you specified did not hold",
                );
            }
            let tag = etag(&req.body);
            st.modified.insert(key.clone(), SystemTime::now());
            st.objects.insert(key, req.body.clone());
            Reply {
                status: 200,
                headers: vec![("ETag", tag)],
                body: Vec::new(),
            }
        }
        "DELETE" => {
            st.objects.remove(&key);
            Reply {
                status: 204,
                headers: Vec::new(),
                body: Vec::new(),
            }
        }
        _ => Reply::error(405, "MethodNotAllowed", "method not allowed"),
    }
}

/// Most entries (keys and common prefixes) one listing page holds, as S3
/// answers by default.
const PAGE_KEYS: usize = 1000;

/// A ListObjectsV2 result: keys under `prefix`, with keys that hold
/// `delimiter` past the prefix rolled up into common prefixes, at most
/// [`PAGE_KEYS`] entries a page. A page that stops short says so and names
/// its last entry as the token the next page starts after.
fn list_xml(
    bucket: &str,
    objects: &BTreeMap<String, Vec<u8>>,
    modified: &BTreeMap<String, SystemTime>,
    prefix: &str,
    delimiter: Option<&str>,
    after_token: Option<&str>,
) -> Vec<u8> {
    let mut contents = Vec::new();
    let mut common: Vec<String> = Vec::new();
    let mut entries = 0usize;
    let mut next_token: Option<String> = None;
    for (key, bytes) in objects.range(prefix.to_string()..) {
        let Some(after) = key.strip_prefix(prefix) else {
            break;
        };
        let entry = delimiter
            .and_then(|d| after.find(d).map(|i| (d, i)))
            .map(|(d, i)| format!("{}{}", prefix, &after[..i + d.len()]));
        let name = entry.clone().unwrap_or_else(|| key.clone());
        if after_token.is_some_and(|t| name.as_str() <= t) {
            continue;
        }
        if let Some(rolled) = &entry {
            if common.last() == Some(rolled) {
                continue;
            }
        }
        if entries == PAGE_KEYS {
            next_token = Some(
                common
                    .last()
                    .into_iter()
                    .chain(
                        contents
                            .last()
                            .map(|(k, _, _, _): &(String, usize, String, String)| k),
                    )
                    .max()
                    .cloned()
                    .unwrap_or_default(),
            );
            break;
        }
        entries += 1;
        match entry {
            Some(rolled) => common.push(rolled),
            None => contents.push((
                key.clone(),
                bytes.len(),
                etag(bytes),
                rfc3339_millis(modified.get(key).copied().unwrap_or(UNIX_EPOCH)),
            )),
        }
    }

    let mut xml = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
         <ListBucketResult xmlns=\"http://s3.amazonaws.com/doc/2006-03-01/\">",
    );
    xml.push_str(&format!("<Name>{}</Name>", xml_escape(bucket)));
    xml.push_str(&format!("<Prefix>{}</Prefix>", xml_escape(prefix)));
    if let Some(d) = delimiter {
        xml.push_str(&format!("<Delimiter>{}</Delimiter>", xml_escape(d)));
    }
    xml.push_str(&format!(
        "<KeyCount>{}</KeyCount><MaxKeys>{PAGE_KEYS}</MaxKeys><IsTruncated>{}</IsTruncated>",
        contents.len() + common.len(),
        next_token.is_some()
    ));
    if let Some(token) = &next_token {
        xml.push_str(&format!(
            "<NextContinuationToken>{}</NextContinuationToken>",
            xml_escape(token)
        ));
    }
    for (key, size, tag, modified) in &contents {
        xml.push_str(&format!(
            "<Contents><Key>{}</Key><LastModified>{}</LastModified>\
             <ETag>{}</ETag><Size>{}</Size><StorageClass>STANDARD</StorageClass>\
             </Contents>",
            xml_escape(key),
            modified,
            xml_escape(tag),
            size
        ));
    }
    for p in &common {
        xml.push_str(&format!(
            "<CommonPrefixes><Prefix>{}</Prefix></CommonPrefixes>",
            xml_escape(p)
        ));
    }
    xml.push_str("</ListBucketResult>");
    xml.into_bytes()
}

/// `at` as S3 lists a LastModified: `YYYY-MM-DDTHH:MM:SS.sssZ`, UTC.
fn rfc3339_millis(at: SystemTime) -> String {
    let since = at.duration_since(UNIX_EPOCH).unwrap_or_default();
    let secs = since.as_secs();
    let (days, rem) = ((secs / 86_400) as i64, secs % 86_400);
    // Days since the epoch to a civil date (Howard Hinnant's algorithm).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}.{:03}Z",
        rem / 3_600,
        rem % 3_600 / 60,
        rem % 60,
        since.subsec_millis()
    )
}

fn xml_escape(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn write_reply(mut stream: TcpStream, method: &str, reply: &Reply) {
    let reason = match reply.status {
        200 => "OK",
        204 => "No Content",
        404 => "Not Found",
        405 => "Method Not Allowed",
        500 => "Internal Server Error",
        503 => "Service Unavailable",
        _ => "Status",
    };
    // A HEAD response has no body, and says so: the client must not wait
    // for bytes that never come.
    let body: &[u8] = if method == "HEAD" { &[] } else { &reply.body };
    let mut head = format!(
        "HTTP/1.1 {} {}\r\nContent-Length: {}\r\nConnection: close\r\n",
        reply.status,
        reason,
        body.len()
    );
    for (name, value) in &reply.headers {
        head.push_str(&format!("{}: {}\r\n", name, value));
    }
    head.push_str("\r\n");
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}
