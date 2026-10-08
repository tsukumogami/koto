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
//! parses. Every request is recorded as (method, key) in arrival order, so a
//! test can assert exactly what a command wrote where. [`FakeS3::fail`]
//! makes every matching request answer with a given status instead.
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
use std::time::Duration;

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
}

#[derive(Debug, Clone)]
struct Fault {
    method: String,
    key: String,
    status: u16,
}

#[derive(Debug, Default)]
struct State {
    objects: BTreeMap<String, Vec<u8>>,
    requests: Vec<Request>,
    faults: Vec<Fault>,
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
        lock(&self.state)
            .objects
            .insert(key.to_string(), bytes.to_vec());
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
    pub fn fail(&self, method: &str, key: &str, status: u16) {
        lock(&self.state).faults.push(Fault {
            method: method.to_string(),
            key: key.to_string(),
            status,
        });
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
}

fn read_request(stream: &TcpStream) -> Option<Incoming> {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let mut parts = line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();

    let mut content_length = 0usize;
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
    write_reply(stream, &req.method, &reply);
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
    });

    if named_bucket != bucket {
        return Reply::error(404, "NoSuchBucket", "The specified bucket does not exist");
    }
    if let Some(fault) = st
        .faults
        .iter()
        .find(|f| f.method == req.method && f.key == key)
    {
        return Reply::error(fault.status, "InternalError", "injected failure");
    }

    if is_list {
        let prefix = param("prefix").unwrap_or_default();
        let delimiter = param("delimiter").filter(|d| !d.is_empty());
        return Reply::ok(list_xml(bucket, &st.objects, &prefix, delimiter.as_deref()));
    }

    match req.method.as_str() {
        "GET" => match st.objects.get(&key) {
            Some(bytes) => Reply::ok(bytes.clone()),
            None => Reply::error(404, "NoSuchKey", "The specified key does not exist."),
        },
        "HEAD" => match st.objects.get(&key) {
            Some(_) => Reply::ok(Vec::new()),
            None => Reply {
                status: 404,
                headers: Vec::new(),
                body: Vec::new(),
            },
        },
        "PUT" => {
            st.objects.insert(key, req.body.clone());
            Reply {
                status: 200,
                headers: vec![("ETag", "\"fake-etag\"".to_string())],
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

/// A ListObjectsV2 result: keys under `prefix`, with keys that hold
/// `delimiter` past the prefix rolled up into common prefixes.
fn list_xml(
    bucket: &str,
    objects: &BTreeMap<String, Vec<u8>>,
    prefix: &str,
    delimiter: Option<&str>,
) -> Vec<u8> {
    let mut contents = Vec::new();
    let mut common: Vec<String> = Vec::new();
    for (key, bytes) in objects.range(prefix.to_string()..) {
        let Some(after) = key.strip_prefix(prefix) else {
            break;
        };
        match delimiter.and_then(|d| after.find(d).map(|i| (d, i))) {
            Some((d, i)) => {
                let rolled = format!("{}{}", prefix, &after[..i + d.len()]);
                if common.last() != Some(&rolled) {
                    common.push(rolled);
                }
            }
            None => contents.push((key.clone(), bytes.len())),
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
        "<KeyCount>{}</KeyCount><MaxKeys>1000</MaxKeys><IsTruncated>false</IsTruncated>",
        contents.len() + common.len()
    ));
    for (key, size) in &contents {
        xml.push_str(&format!(
            "<Contents><Key>{}</Key><LastModified>2026-01-01T00:00:00.000Z</LastModified>\
             <ETag>\"fake-etag\"</ETag><Size>{}</Size><StorageClass>STANDARD</StorageClass>\
             </Contents>",
            xml_escape(key),
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
