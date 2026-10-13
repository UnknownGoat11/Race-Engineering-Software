//! Opt-in, GET-only probe of LMU's supplementary REST endpoints (docs/LMU_TELEMETRY_RESEARCH.md §9).
//!
//! Safety by construction (D-018, research §9 policy):
//! * **Loopback only.** [`LoopbackEndpoint`] always targets `127.0.0.1` (not `localhost`); there is no way
//!   to give it another host, and [`get`] refuses any address that is not `127.0.0.1` before connecting.
//! * **GET only.** [`build_request`] is the only request builder and writes the `GET` method constant;
//!   paths are fixed constants ([`ENDPOINTS`]) and are validated (no spaces or control characters).
//! * **One-shot and spaced.** Each endpoint is requested once, with at least [`ProbeOptions::spacing`]
//!   between requests and a [`REQUEST_TIMEOUT`] deadline per request (connect + send + receive).
//! * **Pit menu excluded by default.** [`PIT_MENU_ENDPOINT`] can make the in-game pit menu flicker; it is
//!   only requested with `include_pit_menu`. The [`CRASH_SUSPECT_ENDPOINTS`] (garage routes that timed out
//!   or reset the connection right before an LMU crash on 2026-10-13) only with `include_crash_suspects`;
//!   any endpoint can be left out with `skip` (`--skip`, repeatable).
//! * **No personal names saved.** JSON bodies and shape summaries have name-like values redacted, and
//!   object keys that are not schema-like or contain a name seen elsewhere replaced. Every top-level key of
//!   a [`NAME_KEYED_ENDPOINTS`] body (`/rest/strategy/usage` is keyed by the player name) is replaced
//!   unconditionally, independent of the other responses.
//! * **Bounded.** Headers are capped at [`MAX_HEADER_BYTES`], bodies at [`MAX_BODY_BYTES`].
//!
//! The HTTP/1.1 client is a minimal `std::net::TcpStream` implementation (Content-Length, chunked and
//! read-to-close bodies) so no HTTP or TLS dependency is needed.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;
use std::fs;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{IpAddr, Ipv4Addr, SocketAddr, TcpStream};
use std::path::Path;
use std::time::{Duration, Instant};

use serde::Serialize;
use thiserror::Error;

use crate::COMPONENT;
use crate::error::VerifyError;

/// LMU's REST port (`Settings.JSON` "WebUI port", research §9).
pub const LMU_REST_PORT: u16 = 6397;
/// Deadline of one request: connect, send and receive (research §9: 1 s timeout).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(1);
/// Largest body kept (bytes); the rest is not read.
pub const MAX_BODY_BYTES: usize = 2 * 1024 * 1024;
/// Largest status line + header block accepted (bytes).
pub const MAX_HEADER_BYTES: usize = 64 * 1024;
/// Longest chunk-size line (with chunk extensions) or chunk terminator line accepted (bytes).
const MAX_CHUNK_LINE_BYTES: usize = 1024;
/// The only HTTP method this client can send.
const METHOD: &str = "GET";
/// Endpoint that can make the in-game MFD pit menu flicker when polled (research §9).
pub const PIT_MENU_ENDPOINT: &str = "/rest/garage/UIScreen/RepairAndRefuel";

/// Garage routes that timed out or reset the connection right before an LMU crash on 2026-10-13 (owner
/// probe in a session, research §9): crash suspects, excluded by default (`include_crash_suspects`).
pub const CRASH_SUSPECT_ENDPOINTS: &[&str] = &[
    "/rest/garage/getVehicleCondition",
    "/rest/garage/UIScreen/TireManagement",
    "/rest/garage/getPlayerGarageData",
    "/rest/garage/PitMenu/receivePitMenu",
];

/// The GET endpoints of research §9, in probe order.
pub const ENDPOINTS: &[&str] = &[
    "/swagger-schema.json",
    "/rest/watch/sessionInfo",
    "/rest/watch/standings",
    "/rest/watch/standings/history",
    "/rest/watch/trackmap",
    "/rest/sessions/GetGameState",
    "/rest/sessions/weather",
    "/rest/sessions/TireInventory",
    "/rest/strategy/usage",
    "/rest/strategy/pitstop-estimate",
    "/rest/strategy/overall",
    "/rest/garage/getVehicleCondition",
    "/rest/garage/tireinfo",
    "/rest/garage/brakeinfo",
    "/rest/garage/PitMenu/receivePitMenu",
    "/rest/garage/UIScreen/TireManagement",
    "/rest/garage/getPlayerGarageData",
    PIT_MENU_ENDPOINT,
];

/// Response headers kept in the results (lower case).
const KEPT_HEADERS: [&str; 7] = [
    "cache-control",
    "connection",
    "content-length",
    "content-type",
    "date",
    "server",
    "transfer-encoding",
];

/// JSON shape paths recorded per response.
const MAX_SHAPE_PATHS: usize = 2_000;
/// Array elements walked per array (lengths are always exact).
const MAX_ARRAY_ELEMENTS_WALKED: usize = 50;
/// Example strings are cut to this many characters.
const MAX_EXAMPLE_CHARS: usize = 40;
/// Example strings under keys containing one of these words are redacted (personal names).
const NAME_LIKE_KEYS: [&str; 8] = [
    "driver", "player", "name", "user", "steam", "server", "nick", "email",
];

/// Errors of one request.
#[derive(Debug, Error)]
pub enum RestError {
    /// The target is not `127.0.0.1` (refused before connecting).
    #[error("refusing non-loopback target {0}: the REST probe only talks to 127.0.0.1")]
    NotLoopback(SocketAddr),
    /// The path is not a plain absolute path.
    #[error("invalid request path {0:?}")]
    InvalidPath(String),
    /// Connecting, sending or receiving failed.
    #[error("{context}: {source}")]
    Io {
        /// What was being done.
        context: &'static str,
        /// Root cause.
        #[source]
        source: io::Error,
    },
    /// The request deadline passed.
    #[error("no complete response within {0:?}")]
    Timeout(Duration),
    /// The response is not valid HTTP/1.x.
    #[error("malformed HTTP response: {0}")]
    Malformed(String),
}

impl RestError {
    fn io(context: &'static str, source: io::Error) -> Self {
        Self::Io { context, source }
    }

    /// A socket timeout (turned into [`RestError::Timeout`] by [`get`], which knows the deadline).
    fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout(_))
            || matches!(self, Self::Io { source, .. }
                if matches!(source.kind(), io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock))
    }
}

/// A REST target on `127.0.0.1` (the host cannot be anything else).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LoopbackEndpoint {
    port: u16,
}

impl LoopbackEndpoint {
    /// LMU's REST server: `127.0.0.1:6397`.
    pub fn lmu() -> Self {
        Self {
            port: LMU_REST_PORT,
        }
    }

    /// A target from a socket address; refuses anything that is not `127.0.0.1` (tests use this with an
    /// ephemeral port).
    ///
    /// # Errors
    /// [`RestError::NotLoopback`] for every other address, including `localhost` names resolved
    /// elsewhere, `0.0.0.0` and IPv6.
    pub fn from_socket_addr(addr: SocketAddr) -> Result<Self, RestError> {
        if addr.ip() == IpAddr::V4(Ipv4Addr::LOCALHOST) {
            Ok(Self { port: addr.port() })
        } else {
            Err(RestError::NotLoopback(addr))
        }
    }

    /// The socket address (always `127.0.0.1`).
    pub fn addr(&self) -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), self.port)
    }
}

/// Probe settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProbeOptions {
    /// Also request [`PIT_MENU_ENDPOINT`].
    pub include_pit_menu: bool,
    /// Also request the [`CRASH_SUSPECT_ENDPOINTS`].
    pub include_crash_suspects: bool,
    /// Endpoints never requested (exact paths, `--skip`).
    pub skip: Vec<String>,
    /// Minimum pause between the end of one request and the start of the next.
    pub spacing: Duration,
    /// Deadline per request.
    pub timeout: Duration,
}

/// A raw HTTP response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpResponse {
    /// Status code.
    pub status: u16,
    /// Headers (lower-case names; repeated headers joined with ", ").
    pub headers: BTreeMap<String, String>,
    /// Body (at most [`MAX_BODY_BYTES`]).
    pub body: Vec<u8>,
    /// The body was longer than [`MAX_BODY_BYTES`] and was cut.
    pub truncated: bool,
}

/// The request bytes of a GET for `path` (the only request this client can build).
///
/// # Errors
/// [`RestError::InvalidPath`] unless `path` starts with `/` and holds only visible ASCII.
pub fn build_request(endpoint: &LoopbackEndpoint, path: &str) -> Result<String, RestError> {
    if !path.starts_with('/') || !path.bytes().all(|b| b.is_ascii_graphic()) {
        return Err(RestError::InvalidPath(path.to_owned()));
    }
    Ok(format!(
        "{METHOD} {path} HTTP/1.1\r\nHost: {}\r\nUser-Agent: {}/{}\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        endpoint.addr(),
        env!("CARGO_PKG_NAME"),
        env!("CARGO_PKG_VERSION")
    ))
}

/// A reader that fails with `TimedOut` once the deadline has passed.
struct DeadlineReader {
    stream: TcpStream,
    deadline: Instant,
}

/// Smallest socket timeout set (a zero timeout is rejected by the OS API).
const MIN_SOCKET_TIMEOUT: Duration = Duration::from_millis(1);

impl Read for DeadlineReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let remaining = self.deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return Err(io::Error::from(io::ErrorKind::TimedOut));
        }
        self.stream
            .set_read_timeout(Some(remaining.max(MIN_SOCKET_TIMEOUT)))?;
        self.stream.read(buf)
    }
}

/// Reads one CRLF-terminated line (without the terminator), bounded by `budget` bytes.
fn read_line<R: BufRead>(r: &mut R, budget: &mut usize) -> Result<String, RestError> {
    let mut line = Vec::new();
    let mut limited = r.take(u64::try_from(*budget).unwrap_or(u64::MAX).saturating_add(1));
    let n = limited
        .read_until(b'\n', &mut line)
        .map_err(|e| RestError::io("reading the response", e))?;
    if n == 0 {
        return Err(RestError::Malformed("connection closed early".to_owned()));
    }
    if n > *budget {
        return Err(RestError::Malformed("header block too large".to_owned()));
    }
    *budget -= n;
    if line.last() != Some(&b'\n') {
        return Err(RestError::Malformed("incomplete line".to_owned()));
    }
    line.pop();
    if line.last() == Some(&b'\r') {
        line.pop();
    }
    String::from_utf8(line).map_err(|_| RestError::Malformed("non-UTF-8 header".to_owned()))
}

/// Parses a status line (`HTTP/1.1 200 OK`).
fn parse_status(line: &str) -> Result<u16, RestError> {
    let mut parts = line.splitn(3, ' ');
    let version = parts.next().unwrap_or("");
    let code = parts.next().unwrap_or("");
    if !version.starts_with("HTTP/1.") {
        return Err(RestError::Malformed(format!("status line {line:?}")));
    }
    code.parse::<u16>()
        .map_err(|_| RestError::Malformed(format!("status line {line:?}")))
}

/// Reads up to `limit` bytes into `body`; returns `true` if more data followed (body cut).
fn read_capped<R: Read>(r: &mut R, body: &mut Vec<u8>, limit: usize) -> Result<bool, RestError> {
    let room = MAX_BODY_BYTES.saturating_sub(body.len());
    let take = limit.min(room);
    let mut part = Vec::new();
    r.take(u64::try_from(take).unwrap_or(u64::MAX))
        .read_to_end(&mut part)
        .map_err(|e| RestError::io("reading the body", e))?;
    let got = part.len();
    body.extend_from_slice(&part);
    if got < take {
        return Err(RestError::Malformed(
            "body shorter than announced".to_owned(),
        ));
    }
    Ok(limit > room)
}

/// Decodes a chunked body.
///
/// Each chunk-size line and each CRLF after a chunk has its own [`MAX_CHUNK_LINE_BYTES`] budget, so a
/// body sent in many small chunks is not mistaken for an oversized header block. The number of chunks is
/// still bounded: every chunk adds at least one byte to the body, which stops at [`MAX_BODY_BYTES`] (and
/// the request deadline bounds the time). The trailer block shares one [`MAX_HEADER_BYTES`] budget.
fn read_chunked<R: BufRead>(r: &mut R) -> Result<(Vec<u8>, bool), RestError> {
    let mut body = Vec::new();
    loop {
        let mut line_budget = MAX_CHUNK_LINE_BYTES;
        let line = read_line(r, &mut line_budget)?;
        let size_text = line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_text, 16)
            .map_err(|_| RestError::Malformed(format!("chunk size {size_text:?}")))?;
        if size == 0 {
            // Trailers end with an empty line.
            let mut trailer_budget = MAX_HEADER_BYTES;
            while !read_line(r, &mut trailer_budget)?.is_empty() {}
            return Ok((body, false));
        }
        if read_capped(r, &mut body, size)? {
            return Ok((body, true));
        }
        let mut crlf_budget = MAX_CHUNK_LINE_BYTES;
        if !read_line(r, &mut crlf_budget)?.is_empty() {
            return Err(RestError::Malformed(
                "chunk not followed by CRLF".to_owned(),
            ));
        }
    }
}

/// Reads a full response from `r`.
fn read_response<R: BufRead>(r: &mut R) -> Result<HttpResponse, RestError> {
    let mut budget = MAX_HEADER_BYTES;
    let status = parse_status(&read_line(r, &mut budget)?)?;
    let mut headers: BTreeMap<String, String> = BTreeMap::new();
    loop {
        let line = read_line(r, &mut budget)?;
        if line.is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            let name = name.trim().to_ascii_lowercase();
            let value = value.trim().to_owned();
            headers
                .entry(name)
                .and_modify(|v| {
                    v.push_str(", ");
                    v.push_str(&value);
                })
                .or_insert(value);
        }
    }
    let chunked = headers
        .get("transfer-encoding")
        .is_some_and(|v| v.to_ascii_lowercase().contains("chunked"));
    let (body, truncated) = if chunked {
        read_chunked(r)?
    } else if let Some(len) = headers.get("content-length") {
        let len: usize = len
            .parse()
            .map_err(|_| RestError::Malformed(format!("content-length {len:?}")))?;
        let mut body = Vec::new();
        let cut = read_capped(r, &mut body, len)?;
        (body, cut)
    } else {
        let mut body = Vec::new();
        let mut limited = r.take(u64::try_from(MAX_BODY_BYTES).unwrap_or(u64::MAX) + 1);
        limited
            .read_to_end(&mut body)
            .map_err(|e| RestError::io("reading the body", e))?;
        let cut = body.len() > MAX_BODY_BYTES;
        body.truncate(MAX_BODY_BYTES);
        (body, cut)
    };
    Ok(HttpResponse {
        status,
        headers,
        body,
        truncated,
    })
}

/// Sends one `GET path` to the loopback endpoint and reads the response within `timeout`.
///
/// # Errors
/// [`RestError`] for a refused target or path, connection / I/O failures, the deadline and malformed
/// responses.
pub fn get(
    endpoint: &LoopbackEndpoint,
    path: &str,
    timeout: Duration,
) -> Result<HttpResponse, RestError> {
    let addr = endpoint.addr();
    // Defence in depth: the type already guarantees this.
    if addr.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) {
        return Err(RestError::NotLoopback(addr));
    }
    let request = build_request(endpoint, path)?;
    let deadline = Instant::now() + timeout;
    let result = (|| {
        let stream = TcpStream::connect_timeout(&addr, timeout.max(MIN_SOCKET_TIMEOUT))
            .map_err(|e| RestError::io("connecting", e))?;
        let remaining = deadline.saturating_duration_since(Instant::now());
        stream
            .set_write_timeout(Some(remaining.max(MIN_SOCKET_TIMEOUT)))
            .map_err(|e| RestError::io("configuring the socket", e))?;
        (&stream)
            .write_all(request.as_bytes())
            .map_err(|e| RestError::io("sending the request", e))?;
        let mut reader = BufReader::new(DeadlineReader { stream, deadline });
        read_response(&mut reader)
    })();
    match result {
        Err(e) if e.is_timeout() || Instant::now() >= deadline => Err(RestError::Timeout(timeout)),
        other => other,
    }
}

/// One entry of a JSON shape summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ShapeEntry {
    /// Key path (`$.a.b[]`).
    pub path: String,
    /// JSON types seen at the path (`array`, `bool`, `null`, `number`, `object`, `string`).
    pub types: Vec<&'static str>,
    /// Occurrences.
    pub count: u64,
    /// Smallest and largest array length (arrays only).
    pub array_len: Option<(usize, usize)>,
    /// An example scalar value (strings under name-like keys are redacted).
    pub example: Option<String>,
}

/// Shape summary of a JSON body.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Shape {
    /// Paths in sorted order.
    pub entries: Vec<ShapeEntry>,
    /// More than [`MAX_SHAPE_PATHS`] paths existed.
    pub truncated: bool,
}

#[derive(Debug, Default)]
struct ShapeAcc {
    types: BTreeSet<&'static str>,
    count: u64,
    array_len: Option<(usize, usize)>,
    example: Option<String>,
}

fn type_name(v: &serde_json::Value) -> &'static str {
    match v {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

fn name_like(key: &str) -> bool {
    let k = key.to_ascii_lowercase();
    NAME_LIKE_KEYS.iter().any(|n| k.contains(n))
}

fn example_of(v: &serde_json::Value, redact: bool) -> Option<String> {
    match v {
        serde_json::Value::String(s) if redact && !s.is_empty() => {
            Some(crate::redact::REDACTED.to_owned())
        }
        serde_json::Value::String(s) => Some(s.chars().take(MAX_EXAMPLE_CHARS).collect()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::Bool(b) => Some(b.to_string()),
        serde_json::Value::Null | serde_json::Value::Array(_) | serde_json::Value::Object(_) => {
            None
        }
    }
}

fn walk(
    v: &serde_json::Value,
    path: &str,
    redact: bool,
    acc: &mut BTreeMap<String, ShapeAcc>,
    truncated: &mut bool,
) {
    if !acc.contains_key(path) && acc.len() >= MAX_SHAPE_PATHS {
        *truncated = true;
        return;
    }
    let entry = acc.entry(path.to_owned()).or_default();
    entry.types.insert(type_name(v));
    entry.count += 1;
    if entry.example.is_none() {
        entry.example = example_of(v, redact);
    }
    match v {
        serde_json::Value::Array(items) => {
            let len = items.len();
            entry.array_len = Some(
                entry
                    .array_len
                    .map_or((len, len), |(a, b)| (a.min(len), b.max(len))),
            );
            let child = format!("{path}[]");
            for item in items.iter().take(MAX_ARRAY_ELEMENTS_WALKED) {
                walk(item, &child, redact, acc, truncated);
            }
        }
        serde_json::Value::Object(map) => {
            for (k, item) in map {
                walk(
                    item,
                    &format!("{path}.{k}"),
                    redact || name_like(k),
                    acc,
                    truncated,
                );
            }
        }
        _ => {}
    }
}

/// The shape summary of a JSON value.
pub fn shape_of(v: &serde_json::Value) -> Shape {
    let mut acc = BTreeMap::new();
    let mut truncated = false;
    walk(v, "$", false, &mut acc, &mut truncated);
    Shape {
        entries: acc
            .into_iter()
            .map(|(path, a)| ShapeEntry {
                path,
                types: a.types.into_iter().collect(),
                count: a.count,
                array_len: a.array_len,
                example: a.example,
            })
            .collect(),
        truncated,
    }
}

/// Outcome of probing one endpoint.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ProbeResult {
    /// Endpoint path.
    pub endpoint: &'static str,
    /// Why the endpoint was not requested.
    pub skipped: Option<String>,
    /// HTTP status.
    pub status: Option<u16>,
    /// Time from connect to the end of the body (ms).
    pub latency_ms: Option<f64>,
    /// Kept response headers.
    pub headers: BTreeMap<String, String>,
    /// Body length kept (bytes).
    pub body_bytes: usize,
    /// The body was cut at [`MAX_BODY_BYTES`].
    pub truncated: bool,
    /// The body parsed as JSON.
    pub json: bool,
    /// Request error.
    pub error: Option<String>,
    /// JSON shape summary.
    pub shape: Option<Shape>,
    /// File the body was saved to (relative to the output directory).
    pub saved_as: Option<String>,
    /// Why the body was not saved (see [`BodyPolicy`]).
    pub not_saved: Option<String>,
    /// The body (saved to its own file, not serialised).
    #[serde(skip)]
    pub body: Vec<u8>,
}

impl ProbeResult {
    fn empty(endpoint: &'static str) -> Self {
        Self {
            endpoint,
            skipped: None,
            status: None,
            latency_ms: None,
            headers: BTreeMap::new(),
            body_bytes: 0,
            truncated: false,
            json: false,
            error: None,
            shape: None,
            saved_as: None,
            not_saved: None,
            body: Vec::new(),
        }
    }
}

/// Probes `endpoints` once each, in order.
pub fn probe_endpoints(
    endpoint: &LoopbackEndpoint,
    endpoints: &[&'static str],
    options: &ProbeOptions,
) -> Vec<ProbeResult> {
    let mut results = Vec::with_capacity(endpoints.len());
    let mut requested_before = false;
    for &path in endpoints {
        let mut r = ProbeResult::empty(path);
        if let Some(why) = skip_reason(path, options) {
            r.skipped = Some(why.to_owned());
            results.push(r);
            continue;
        }
        if requested_before {
            std::thread::sleep(options.spacing);
        }
        requested_before = true;
        let start = Instant::now();
        match get(endpoint, path, options.timeout) {
            Ok(resp) => {
                r.latency_ms = Some(start.elapsed().as_secs_f64() * 1e3);
                r.status = Some(resp.status);
                r.headers = resp
                    .headers
                    .into_iter()
                    .filter(|(k, _)| KEPT_HEADERS.contains(&k.as_str()))
                    .collect();
                r.body_bytes = resp.body.len();
                r.truncated = resp.truncated;
                r.json = serde_json::from_slice::<serde_json::Value>(&resp.body).is_ok();
                r.body = resp.body;
            }
            Err(e) => {
                tracing::info!(component = COMPONENT, endpoint = path, error = %e, "REST request failed");
                r.error = Some(e.to_string());
            }
        }
        results.push(r);
    }
    // Shapes are built after every body is in: names seen as values in one response (standings) redact
    // the same names used as object keys in another (strategy/usage is keyed by the player name).
    let names = names_in(&results);
    for r in &mut results {
        if let Ok(mut value) = serde_json::from_slice::<serde_json::Value>(&r.body) {
            redact_value_for(r.endpoint, &mut value, &names);
            r.shape = Some(shape_of(&value));
        }
    }
    results
}

/// Why `path` is not requested with `options`, if it is not.
fn skip_reason(path: &str, options: &ProbeOptions) -> Option<&'static str> {
    if options.skip.iter().any(|s| s == path) {
        Some("skipped on request (--skip)")
    } else if path == PIT_MENU_ENDPOINT && !options.include_pit_menu {
        Some("excluded by default (can make the in-game pit menu flicker); use --include-pit-menu")
    } else if CRASH_SUSPECT_ENDPOINTS.contains(&path) && !options.include_crash_suspects {
        Some(
            "excluded by default (timed out or reset the connection right before an LMU crash on 2026-10-13); use --include-crash-suspects",
        )
    } else {
        None
    }
}

/// Probes every endpoint of [`ENDPOINTS`].
pub fn probe(endpoint: &LoopbackEndpoint, options: &ProbeOptions) -> Vec<ProbeResult> {
    probe_endpoints(endpoint, ENDPOINTS, options)
}

/// File name for an endpoint's body.
fn file_name(endpoint: &str, json: bool) -> String {
    let stem: String = endpoint
        .trim_start_matches('/')
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect();
    format!("{stem}.{}", if json { "json" } else { "txt" })
}

/// What [`save`] writes for each response body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BodyPolicy {
    /// JSON bodies are saved pretty-printed with every non-empty string under a name-like key (and
    /// anywhere below such a key) replaced by `<name>`, like the shape summary. Non-JSON bodies cannot be
    /// redacted reliably and are not saved (`index.json` says so).
    #[default]
    Redacted,
    /// Every body verbatim (JSON pretty-printed, raw otherwise). Holds driver, player and server names:
    /// for local inspection only, do not share.
    Raw,
}

/// Shortest collected name: shorter strings would redact ordinary keys by accident.
const MIN_COLLECTED_NAME_CHARS: usize = 2;
/// Shortest collected name matched inside a longer key (shorter ones only redact an equal key). Errs on
/// the side of redaction: a compound or class name used as a value can hide an ordinary key too.
const MIN_CONTAINED_NAME_CHARS: usize = 4;

/// Collects the non-empty strings found under name-like keys of `v` (and below them) into `out`.
fn collect_names(v: &serde_json::Value, under_name: bool, out: &mut BTreeSet<String>) {
    match v {
        serde_json::Value::String(s) if under_name => {
            let t = s.trim();
            if t.chars().count() >= MIN_COLLECTED_NAME_CHARS {
                out.insert(t.to_lowercase());
            }
        }
        serde_json::Value::Array(items) => {
            for item in items {
                collect_names(item, under_name, out);
            }
        }
        serde_json::Value::Object(map) => {
            for (k, item) in map {
                collect_names(item, under_name || name_like(k), out);
            }
        }
        _ => {}
    }
}

/// The names (lower case) seen as values under name-like keys in every JSON body of `results`.
pub fn names_in(results: &[ProbeResult]) -> BTreeSet<String> {
    let mut names = BTreeSet::new();
    for r in results {
        if let Ok(value) = serde_json::from_slice::<serde_json::Value>(&r.body) {
            collect_names(&value, false, &mut names);
        }
    }
    names
}

/// A key that looks like a schema field: ASCII letters, digits, `_`, `-` and `.` only, starting with a
/// letter or `_` (`driverName`, `pit_state`), or digits only (an index or slot id). Anything else (a space,
/// non-ASCII, punctuation) is "unknown": a personal name used as a key (LMU's `/rest/strategy/usage` is
/// keyed by the player name) must never reach a saved file or the shape summary.
fn schema_like_key(key: &str) -> bool {
    let mut chars = key.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if key.chars().all(|c| c.is_ascii_digit()) {
        return true;
    }
    (first.is_ascii_alphabetic() || first == '_')
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.'))
}

/// The key must be replaced: unknown (not schema-like) or equal to / containing a collected name.
fn key_needs_redaction(key: &str, names: &BTreeSet<String>) -> bool {
    if !schema_like_key(key) {
        return true;
    }
    let k = key.to_lowercase();
    names.iter().any(|n| {
        k == *n || (n.chars().count() >= MIN_CONTAINED_NAME_CHARS && k.contains(n.as_str()))
    })
}

/// Endpoints whose top-level object keys are personal names (LMU keys `/rest/strategy/usage` by the
/// player name, probe 2026-10-13). Their top-level keys are always replaced, whatever else the probe saw:
/// a one-token ASCII name (`DriverOne`) is schema-like and would otherwise survive whenever the standings
/// were skipped, failed or empty.
pub const NAME_KEYED_ENDPOINTS: &[&str] = &["/rest/strategy/usage"];

/// `<name>` for the first replaced key of an object, `<name>#2`, `<name>#3`, ... for the next ones.
fn redacted_key(n: usize) -> String {
    if n <= 1 {
        crate::redact::REDACTED.to_owned()
    } else {
        format!("{}#{n}", crate::redact::REDACTED)
    }
}

/// [`redact_value`] for the body of `endpoint`. For a [`NAME_KEYED_ENDPOINTS`] endpoint every top-level
/// object key is replaced unconditionally (and everything below it is treated as name-like); the
/// collected-names rule of [`redact_value`] still applies everywhere as a second layer.
pub fn redact_value_for(endpoint: &str, v: &mut serde_json::Value, names: &BTreeSet<String>) {
    match v {
        serde_json::Value::Object(map) if NAME_KEYED_ENDPOINTS.contains(&endpoint) => {
            let old = std::mem::take(map);
            for (i, (_, mut item)) in old.into_iter().enumerate() {
                redact_value(&mut item, names, true);
                map.insert(redacted_key(i + 1), item);
            }
        }
        _ => redact_value(v, names, false),
    }
}

/// Replaces non-empty strings under name-like keys with `<name>` (`redact`: already below such a key) and
/// renames every key that [`key_needs_redaction`] to `<name>`, `<name>#2`, ... (everything below a
/// redacted key is treated as name-like too). Depth is bounded by serde_json's parser recursion limit.
pub fn redact_value(v: &mut serde_json::Value, names: &BTreeSet<String>, redact: bool) {
    match v {
        serde_json::Value::String(s) if redact && !s.is_empty() => {
            crate::redact::REDACTED.clone_into(s);
        }
        serde_json::Value::Array(items) => {
            for item in items {
                redact_value(item, names, redact);
            }
        }
        serde_json::Value::Object(map) => {
            let old = std::mem::take(map);
            let mut redacted_keys = 0_usize;
            for (k, mut item) in old {
                let hide = key_needs_redaction(&k, names);
                redact_value(&mut item, names, redact || hide || name_like(&k));
                let key = if hide {
                    redacted_keys += 1;
                    redacted_key(redacted_keys)
                } else {
                    k
                };
                map.insert(key, item);
            }
        }
        _ => {}
    }
}

/// Saves the bodies according to `policy` (see [`BodyPolicy`]) and `index.json` to `dir`.
///
/// # Errors
/// [`VerifyError::Write`] when a file cannot be written, [`VerifyError::Json`] when serialisation fails.
pub fn save(results: &[ProbeResult], dir: &Path, policy: BodyPolicy) -> Result<(), VerifyError> {
    let write = |path: &Path, bytes: &[u8]| {
        fs::write(path, bytes).map_err(|source| VerifyError::Write {
            path: path.to_path_buf(),
            source,
        })
    };
    fs::create_dir_all(dir).map_err(|source| VerifyError::Write {
        path: dir.to_path_buf(),
        source,
    })?;
    let names = names_in(results);
    let mut index: Vec<ProbeResult> = Vec::with_capacity(results.len());
    for r in results {
        let mut entry = r.clone();
        entry.body = Vec::new();
        if r.status.is_some() {
            let name = file_name(r.endpoint, r.json);
            let bytes = match (r.json, policy) {
                (true, _) => {
                    let mut value: serde_json::Value = serde_json::from_slice(&r.body)?;
                    if policy == BodyPolicy::Redacted {
                        redact_value_for(r.endpoint, &mut value, &names);
                    }
                    let mut pretty = serde_json::to_vec_pretty(&value)?;
                    pretty.push(b'\n');
                    Some(pretty)
                }
                (false, BodyPolicy::Raw) => Some(r.body.clone()),
                (false, BodyPolicy::Redacted) => None,
            };
            if let Some(bytes) = bytes {
                write(&dir.join(&name), &bytes)?;
                entry.saved_as = Some(name);
            } else {
                entry.not_saved = Some(
                    "non-JSON body cannot be redacted; use --keep-raw-bodies to save it".to_owned(),
                );
            }
        }
        index.push(entry);
    }
    let mut json = serde_json::to_vec_pretty(&index)?;
    json.push(b'\n');
    write(&dir.join("index.json"), &json)
}

/// The probe table printed on stdout.
pub fn table(results: &[ProbeResult]) -> String {
    let mut s = String::new();
    let _ = writeln!(
        s,
        "{:<40} {:>6} {:>9} {:>9}  {:<5} note",
        "endpoint", "status", "ms", "bytes", "json"
    );
    for r in results {
        let note = match (&r.skipped, &r.error) {
            (Some(why), _) => format!("SKIPPED: {why}"),
            (None, Some(e)) => format!("ERROR: {e}"),
            (None, None) => {
                let ct = r.headers.get("content-type").map_or("", String::as_str);
                let shape = r.shape.as_ref().map_or(String::new(), |sh| {
                    format!(", {} key paths", sh.entries.len())
                });
                format!(
                    "{ct}{shape}{}",
                    if r.truncated {
                        ", TRUNCATED at 2 MiB"
                    } else {
                        ""
                    }
                )
            }
        };
        let _ = writeln!(
            s,
            "{:<40} {:>6} {:>9} {:>9}  {:<5} {}",
            r.endpoint,
            r.status.map_or_else(|| "-".to_owned(), |c| c.to_string()),
            r.latency_ms
                .map_or_else(|| "-".to_owned(), |ms| format!("{ms:.1}")),
            r.body_bytes,
            if r.json { "yes" } else { "no" },
            note
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_are_get_only_with_a_loopback_host() {
        let ep = LoopbackEndpoint::lmu();
        let req = build_request(&ep, "/rest/watch/standings").expect("request");
        assert!(req.starts_with("GET /rest/watch/standings HTTP/1.1\r\n"));
        assert!(req.contains("\r\nHost: 127.0.0.1:6397\r\n"));
        assert!(req.ends_with("\r\n\r\n"));
        for bad in ["rest", "/a b", "/a\r\nPOST /x", ""] {
            assert!(matches!(
                build_request(&ep, bad),
                Err(RestError::InvalidPath(_))
            ));
        }
    }

    #[test]
    fn only_127_0_0_1_is_accepted() {
        for addr in [
            "10.0.0.1:6397",
            "0.0.0.0:6397",
            "[::1]:6397",
            "127.0.0.2:6397",
            "192.168.1.5:80",
        ] {
            let a: SocketAddr = addr.parse().expect("addr");
            assert!(LoopbackEndpoint::from_socket_addr(a).is_err(), "{addr}");
        }
        let ok: SocketAddr = "127.0.0.1:4321".parse().expect("addr");
        assert_eq!(
            LoopbackEndpoint::from_socket_addr(ok).expect("ok").addr(),
            ok
        );
        assert_eq!(LoopbackEndpoint::lmu().addr().to_string(), "127.0.0.1:6397");
    }

    #[test]
    fn responses_parse_with_length_chunked_and_close() {
        let fixed = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 7\r\n\r\n{\"a\":1}";
        let r = read_response(&mut &fixed[..]).expect("content-length");
        assert_eq!(
            (r.status, r.body.as_slice(), r.truncated),
            (200, &b"{\"a\":1}"[..], false)
        );

        let chunked = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n4;ext=1\r\n{\"a\"\r\n3\r\n:1}\r\n0\r\nX-Trailer: y\r\n\r\n";
        let r = read_response(&mut &chunked[..]).expect("chunked");
        assert_eq!(r.body, b"{\"a\":1}");

        let close = b"HTTP/1.0 404 Not Found\r\nServer: x\r\n\r\nnope";
        let r = read_response(&mut &close[..]).expect("close");
        assert_eq!((r.status, r.body.as_slice()), (404, &b"nope"[..]));

        for bad in [
            &b"HTTP/2 200\r\n\r\n"[..],
            b"FTP 200 OK\r\n\r\n",
            b"HTTP/1.1 abc\r\n\r\n",
            b"HTTP/1.1 200 OK\r\nContent-Length: 10\r\n\r\nshort",
        ] {
            assert!(matches!(
                read_response(&mut &bad[..]),
                Err(RestError::Malformed(_))
            ));
        }
    }

    #[test]
    fn saved_json_has_name_like_values_redacted_at_any_depth() {
        let mut v: serde_json::Value = serde_json::from_str(
            r#"{"server":{"title":"Club Night","slots":[{"id":1,"label":"Jane"}]},"drivers":[{"driverName":"X Y","lap":3}],"track":"Le Mans","empty":{"userName":""}}"#,
        )
        .expect("json");
        redact_value(&mut v, &BTreeSet::new(), false);
        assert_eq!(
            v.to_string(),
            r#"{"drivers":[{"driverName":"<name>","lap":3}],"empty":{"userName":""},"server":{"slots":[{"id":1,"label":"<name>"}],"title":"<name>"},"track":"Le Mans"}"#
        );
    }

    #[test]
    fn many_small_chunks_are_decoded() {
        // 20 000 chunks of 8 bytes: 100 kB of chunk lines and terminators, more than one header budget.
        const CHUNKS: usize = 20_000;
        let mut raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        for _ in 0..CHUNKS {
            raw.extend_from_slice(b"8\r\n01234567\r\n");
        }
        raw.extend_from_slice(b"0\r\n\r\n");
        let r = read_response(&mut &raw[..]).expect("many chunks");
        assert_eq!((r.body.len(), r.truncated), (8 * CHUNKS, false));

        // A single chunk-size line is still bounded.
        let mut long = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n8;".to_vec();
        long.extend(std::iter::repeat_n(b'x', MAX_CHUNK_LINE_BYTES));
        long.extend_from_slice(b"\r\n01234567\r\n0\r\n\r\n");
        assert!(matches!(
            read_response(&mut &long[..]),
            Err(RestError::Malformed(_))
        ));

        // Endless tiny chunks stop at the body cap.
        let mut endless = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n".to_vec();
        for _ in 0..=MAX_BODY_BYTES / 1024 {
            endless.extend_from_slice(b"400\r\n");
            endless.extend(std::iter::repeat_n(b'a', 1024));
            endless.extend_from_slice(b"\r\n");
        }
        let r = read_response(&mut &endless[..]).expect("capped");
        assert_eq!((r.body.len(), r.truncated), (MAX_BODY_BYTES, true));
    }

    #[test]
    fn shapes_list_paths_types_lengths_and_redact_names() {
        let v: serde_json::Value = serde_json::from_str(
            r#"{"drivers":[{"driverName":"Jane Doe","lap":3},{"driverName":"X Y","lap":4.5}],"ok":true,"none":null}"#,
        )
        .expect("json");
        let shape = shape_of(&v);
        let get = |p: &str| shape.entries.iter().find(|e| e.path == p).expect(p);
        assert_eq!(get("$.drivers").array_len, Some((2, 2)));
        assert_eq!(
            get("$.drivers[].driverName").example.as_deref(),
            Some("<name>")
        );
        assert_eq!(get("$.drivers[].lap").types, vec!["number"]);
        assert_eq!(get("$.drivers[].lap").count, 2);
        assert_eq!(get("$.ok").example.as_deref(), Some("true"));
        assert_eq!(get("$.none").types, vec!["null"]);
        assert!(!shape.truncated);
        assert_eq!(
            file_name("/rest/watch/standings", true),
            "rest_watch_standings.json"
        );
        assert_eq!(
            file_name("/swagger-schema.json", false),
            "swagger-schema_json.txt"
        );
    }

    #[test]
    fn name_like_or_unknown_object_keys_are_redacted() {
        // LMU /rest/strategy/usage is keyed by the player name (probe 2026-10-13); the name is also a value in
        // the standings. Both responses go through one name set.
        let standings = br#"[{"driverName":"Jane","slotID":3,"vehicleName":"Car #7"}]"#;
        let usage = r#"{"Jane":{"fuel":0.5,"tires":[1,2]},"Max Power":{"fuel":0.4},"Über":{"ve":1},"12":{"ok":true},"pit_state":"EXITING","JaneTeam":{"x":1}}"#.as_bytes();
        let results: Vec<ProbeResult> = [standings.to_vec(), usage.to_vec()]
            .into_iter()
            .map(|body| ProbeResult {
                status: Some(200),
                json: true,
                body,
                ..ProbeResult::empty("/rest/test")
            })
            .collect();
        let names = names_in(&results);
        assert!(names.contains("jane") && names.contains("car #7"));
        let mut v: serde_json::Value = serde_json::from_slice(usage).expect("json");
        redact_value(&mut v, &names, false);
        let text = v.to_string();
        for leaked in ["Jane", "Max Power", "Über", "JaneTeam"] {
            assert!(!text.contains(leaked), "{leaked} leaked: {text}");
        }
        let map = v.as_object().expect("object");
        // Schema-like keys and numeric ids stay; the values below a redacted key keep their numbers.
        assert_eq!(map.get("pit_state"), Some(&serde_json::json!("EXITING")));
        assert_eq!(map.get("12"), Some(&serde_json::json!({"ok": true})));
        assert_eq!(map.keys().filter(|k| k.starts_with("<name>")).count(), 4);
        // The shape summary (index.json) is built from the redacted value, so its paths hold no name.
        let shape = shape_of(&v);
        assert!(shape.entries.iter().all(|e| !e.path.contains("Jane")));
        assert!(shape.entries.iter().any(|e| e.path.starts_with("$.<name>")));
    }

    #[test]
    fn name_keyed_endpoint_keys_are_redacted_without_any_collected_names() {
        // Review fix-gate1d: standings skipped/failed/empty means no collected names; a one-token ASCII
        // player name is schema-like, so only the endpoint rule can hide it.
        let usage = br#"{"DriverOne":{"fuel":0.5,"note":"x"},"Driver_Two":{"fuel":0.4}}"#;
        let results = vec![ProbeResult {
            status: Some(200),
            json: true,
            body: usage.to_vec(),
            shape: None,
            ..ProbeResult::empty(NAME_KEYED_ENDPOINTS[0])
        }];
        assert!(names_in(&results).is_empty());
        let dir =
            std::env::temp_dir().join(format!("lmu-verify-rest-namekeyed-{}", std::process::id()));
        save(&results, &dir, BodyPolicy::Redacted).expect("save");
        let body = fs::read_to_string(dir.join(file_name(NAME_KEYED_ENDPOINTS[0], true)))
            .expect("saved body");
        let index = fs::read_to_string(dir.join("index.json")).expect("index");
        let _ = fs::remove_dir_all(&dir);
        for text in [&body, &index] {
            assert!(
                !text.contains("DriverOne") && !text.contains("Driver_Two"),
                "player name leaked: {text}"
            );
        }
        let v: serde_json::Value = serde_json::from_str(&body).expect("json");
        let map = v.as_object().expect("object");
        assert_eq!(map.len(), 2, "{body}");
        assert!(map.contains_key("<name>") && map.contains_key("<name>#2"));
        // Numbers below the replaced key survive; strings below it are treated as name-like.
        assert!(body.contains("0.5") && !body.contains("\"x\""), "{body}");

        // The shape summary goes through the same rule.
        let mut shaped = serde_json::from_slice::<serde_json::Value>(usage).expect("json");
        redact_value_for(NAME_KEYED_ENDPOINTS[0], &mut shaped, &BTreeSet::new());
        assert!(
            shape_of(&shaped)
                .entries
                .iter()
                .all(|e| !e.path.contains("DriverOne") && !e.path.contains("Driver_Two"))
        );
        // Other endpoints keep schema-like keys.
        let mut other = serde_json::json!({"pit_state": "EXITING"});
        redact_value_for("/rest/watch/standings", &mut other, &BTreeSet::new());
        assert_eq!(other, serde_json::json!({"pit_state": "EXITING"}));
    }

    #[test]
    fn skip_list_crash_suspects_and_pit_menu_are_not_requested_by_default() {
        let options = ProbeOptions {
            include_pit_menu: false,
            include_crash_suspects: false,
            skip: vec!["/rest/watch/trackmap".to_owned()],
            spacing: Duration::ZERO,
            timeout: REQUEST_TIMEOUT,
        };
        assert!(skip_reason("/rest/watch/trackmap", &options).is_some());
        assert!(skip_reason(PIT_MENU_ENDPOINT, &options).is_some());
        for path in CRASH_SUSPECT_ENDPOINTS {
            assert!(ENDPOINTS.contains(path), "{path}");
            assert!(skip_reason(path, &options).is_some(), "{path}");
        }
        assert!(skip_reason("/rest/watch/standings", &options).is_none());
        let all = ProbeOptions {
            include_pit_menu: true,
            include_crash_suspects: true,
            skip: Vec::new(),
            ..options
        };
        assert!(ENDPOINTS.iter().all(|p| skip_reason(p, &all).is_none()));
    }
}
