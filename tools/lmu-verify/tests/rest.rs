//! The REST probe against an in-process HTTP server on 127.0.0.1: only GET is ever sent, loopback is
//! enforced, the timeout is honoured, the pit-menu endpoint is skipped by default, and Content-Length,
//! chunked and read-to-close bodies are parsed (and capped at 2 MiB).

use std::io::{BufRead, BufReader, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use lmu_verify::rest::{
    self, BodyPolicy, ENDPOINTS, LoopbackEndpoint, MAX_BODY_BYTES, PIT_MENU_ENDPOINT, ProbeOptions,
    RestError,
};

const SLOW_PATH: &str = "/slow";
const CHUNKED_PATH: &str = "/rest/watch/standings";
const CLOSE_PATH: &str = "/rest/watch/trackmap";
const BIG_PATH: &str = "/rest/strategy/usage";
const HANG_S: u64 = 3;

/// What the test server received: the full request head of every connection.
type Requests = Arc<Mutex<Vec<String>>>;

fn json_for(path: &str) -> String {
    format!(r#"{{"path":"{path}","driverName":"Jane Doe","laps":[1,2,3],"ok":true}}"#)
}

fn respond(stream: &mut TcpStream, path: &str) {
    let json = json_for(path);
    let response = match path {
        SLOW_PATH => {
            thread::sleep(Duration::from_secs(HANG_S));
            return;
        }
        CHUNKED_PATH => {
            let (a, b) = json.split_at(10);
            format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nTransfer-Encoding: chunked\r\n\r\n{:x}\r\n{a}\r\n{:x}\r\n{b}\r\n0\r\n\r\n",
                a.len(),
                b.len()
            )
        }
        CLOSE_PATH => {
            "HTTP/1.0 200 OK\r\nContent-Type: text/plain\r\n\r\nnot json, read to close".to_owned()
        }
        BIG_PATH => {
            let len = MAX_BODY_BYTES + 1_000;
            let _ = stream.write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\nContent-Length: {len}\r\n\r\n"
                )
                .as_bytes(),
            );
            let _ = stream.write_all(&vec![b'x'; len]);
            return;
        }
        _ => format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nX-Other: dropped\r\n\r\n{json}",
            json.len()
        ),
    };
    let _ = stream.write_all(response.as_bytes());
}

/// Starts a server on 127.0.0.1 with an ephemeral port; every connection gets its own thread.
fn server() -> (SocketAddr, Requests) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = listener.local_addr().expect("addr");
    let requests: Requests = Arc::default();
    let seen = Arc::clone(&requests);
    thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let seen = Arc::clone(&seen);
            thread::spawn(move || {
                let mut head = String::new();
                let mut reader = BufReader::new(stream.try_clone().expect("clone"));
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                        break;
                    }
                    head.push_str(&line);
                }
                let path = head.split(' ').nth(1).unwrap_or("").to_owned();
                seen.lock().expect("lock").push(head);
                respond(&mut stream, &path);
            });
        }
    });
    (addr, requests)
}

fn endpoint(addr: SocketAddr) -> LoopbackEndpoint {
    LoopbackEndpoint::from_socket_addr(addr).expect("loopback")
}

fn options(include_pit_menu: bool, spacing_ms: u64) -> ProbeOptions {
    ProbeOptions {
        include_pit_menu,
        include_crash_suspects: false,
        skip: Vec::new(),
        spacing: Duration::from_millis(spacing_ms),
        timeout: rest::REQUEST_TIMEOUT,
    }
}

#[test]
fn probe_sends_one_get_per_endpoint_and_skips_the_pit_menu_by_default() {
    let (addr, requests) = server();
    let results = rest::probe(&endpoint(addr), &options(false, 0));
    assert_eq!(results.len(), ENDPOINTS.len());
    let requests = requests.lock().expect("lock").clone();
    // The pit menu and the four crash-suspect garage routes are skipped by default.
    let skipped = 1 + rest::CRASH_SUSPECT_ENDPOINTS.len();
    assert_eq!(requests.len(), ENDPOINTS.len() - skipped, "{requests:#?}");
    for head in &requests {
        assert!(head.starts_with("GET /"), "{head}");
        assert!(head.contains(&format!("\r\nHost: {addr}\r\n")), "{head}");
        assert!(
            !head.to_ascii_lowercase().contains("content-length"),
            "a GET carries no body: {head}"
        );
        assert!(!head.contains(PIT_MENU_ENDPOINT), "{head}");
        for suspect in rest::CRASH_SUSPECT_ENDPOINTS {
            assert!(!head.contains(&format!(" {suspect} ")), "{head}");
        }
    }
    // Each endpoint once.
    let mut paths: Vec<&str> = requests
        .iter()
        .filter_map(|h| h.split(' ').nth(1))
        .collect();
    paths.sort_unstable();
    paths.dedup();
    assert_eq!(paths.len(), ENDPOINTS.len() - skipped);

    let pit = results
        .iter()
        .find(|r| r.endpoint == PIT_MENU_ENDPOINT)
        .expect("pit menu row");
    assert!(pit.skipped.is_some() && pit.status.is_none());
    let table = rest::table(&results);
    assert!(table.contains("SKIPPED"), "{table}");
}

#[test]
fn pit_menu_is_requested_only_when_asked() {
    let (addr, requests) = server();
    let results = rest::probe_endpoints(&endpoint(addr), &[PIT_MENU_ENDPOINT], &options(true, 0));
    assert_eq!(results[0].status, Some(200));
    assert_eq!(requests.lock().expect("lock").len(), 1);
}

#[test]
fn bodies_are_parsed_shaped_capped_and_saved() {
    let (addr, _) = server();
    let paths = [
        "/rest/watch/sessionInfo",
        CHUNKED_PATH,
        CLOSE_PATH,
        BIG_PATH,
    ];
    let results = rest::probe_endpoints(&endpoint(addr), &paths, &options(false, 0));
    let by = |p: &str| results.iter().find(|r| r.endpoint == p).expect(p);

    let fixed = by("/rest/watch/sessionInfo");
    assert_eq!(
        (fixed.status, fixed.json, fixed.truncated),
        (Some(200), true, false)
    );
    assert!(fixed.headers.contains_key("content-type"));
    assert!(
        !fixed.headers.contains_key("x-other"),
        "only the kept header subset is recorded"
    );
    let shape = fixed.shape.as_ref().expect("shape");
    let name = shape
        .entries
        .iter()
        .find(|e| e.path == "$.driverName")
        .expect("name path");
    assert_eq!(name.example.as_deref(), Some("<name>"));
    let laps = shape
        .entries
        .iter()
        .find(|e| e.path == "$.laps")
        .expect("laps path");
    assert_eq!(laps.array_len, Some((3, 3)));

    let chunked = by(CHUNKED_PATH);
    assert!(chunked.json, "chunked body decoded: {chunked:?}");
    assert_eq!(chunked.body, json_for(CHUNKED_PATH).into_bytes());
    assert_eq!(fixed.body, json_for("/rest/watch/sessionInfo").into_bytes());

    let close = by(CLOSE_PATH);
    assert_eq!((close.status, close.json), (Some(200), false));
    assert_eq!(close.body, b"not json, read to close");

    let big = by(BIG_PATH);
    assert!(big.truncated);
    assert_eq!(big.body_bytes, MAX_BODY_BYTES);

    let dir = std::env::temp_dir().join(format!("lmu-verify-rest-{}", std::process::id()));
    // Default: names redacted in JSON bodies, non-JSON bodies (not redactable) not written.
    rest::save(&results, &dir, BodyPolicy::Redacted).expect("save");
    let saved =
        std::fs::read_to_string(dir.join("rest_watch_sessionInfo.json")).expect("saved body");
    assert!(saved.contains("\n  \"laps\""), "pretty-printed: {saved}");
    assert!(!saved.contains("Jane Doe"), "a driver name leaked: {saved}");
    assert!(saved.contains("\"driverName\": \"<name>\""), "{saved}");
    let index = std::fs::read_to_string(dir.join("index.json")).expect("index");
    assert!(!index.contains("Jane Doe"));
    assert!(!dir.join("rest_watch_trackmap.txt").exists());
    assert!(
        index.contains("non-JSON body cannot be redacted"),
        "{index}"
    );
    std::fs::remove_dir_all(&dir).expect("clean up");

    // Opt-in raw bodies keep everything verbatim.
    rest::save(&results, &dir, BodyPolicy::Raw).expect("save raw");
    let saved =
        std::fs::read_to_string(dir.join("rest_watch_sessionInfo.json")).expect("saved body");
    assert!(saved.contains("Jane Doe"));
    assert!(dir.join("rest_watch_trackmap.txt").exists());
    std::fs::remove_dir_all(&dir).expect("clean up");
}

#[test]
fn the_timeout_is_honoured() {
    let (addr, _) = server();
    let timeout = Duration::from_millis(300);
    let start = Instant::now();
    let result = rest::get(&endpoint(addr), SLOW_PATH, timeout);
    let elapsed = start.elapsed();
    assert!(
        matches!(result, Err(RestError::Timeout(t)) if t == timeout),
        "{result:?}"
    );
    // Windows socket receive timeouts can fire up to one scheduler tick (~15.6 ms) early under load.
    const TIMER_SLACK: Duration = Duration::from_millis(20);
    assert!(elapsed + TIMER_SLACK >= timeout, "{elapsed:?}");
    assert!(
        elapsed < Duration::from_secs(HANG_S),
        "returned after {elapsed:?}"
    );
}

#[test]
fn requests_are_spaced() {
    let (addr, _) = server();
    let paths = [
        "/rest/watch/sessionInfo",
        "/rest/sessions/weather",
        "/rest/garage/tireinfo",
    ];
    let start = Instant::now();
    let results = rest::probe_endpoints(&endpoint(addr), &paths, &options(false, 150));
    assert!(results.iter().all(|r| r.status == Some(200)));
    assert!(
        start.elapsed() >= Duration::from_millis(300),
        "{:?}",
        start.elapsed()
    );
}

#[test]
fn only_127_0_0_1_is_a_valid_target() {
    for addr in [
        "0.0.0.0:6397",
        "10.1.2.3:6397",
        "[::1]:6397",
        "127.0.0.53:6397",
    ] {
        let a: SocketAddr = addr.parse().expect("addr");
        assert!(
            matches!(
                LoopbackEndpoint::from_socket_addr(a),
                Err(RestError::NotLoopback(_))
            ),
            "{addr}"
        );
    }
    assert_eq!(LoopbackEndpoint::lmu().addr().to_string(), "127.0.0.1:6397");
    // Nothing listening: a clean connection error, never a panic.
    let closed = TcpListener::bind("127.0.0.1:0").expect("bind");
    let addr = closed.local_addr().expect("addr");
    drop(closed);
    let r = rest::get(
        &endpoint(addr),
        "/rest/watch/sessionInfo",
        Duration::from_millis(500),
    );
    assert!(
        matches!(r, Err(RestError::Io { .. } | RestError::Timeout(_))),
        "{r:?}"
    );
}
