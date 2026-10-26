//! Test-only helpers: a tiny local HTTP server with canned responses.

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

/// One canned response (or a sequence: the n-th matching request gets the n-th entry,
/// the last entry repeats).
#[derive(Clone)]
pub struct Route {
    /// Substring of "<path>?<query>" that selects this route.
    pub contains: String,
    pub method: Option<&'static str>,
    pub seq: Vec<(u16, Vec<u8>)>,
    pub hits: Arc<AtomicUsize>,
    pub delay_ms: u64,
}

pub fn route(contains: &str, status: u16, body: impl Into<Vec<u8>>) -> Route {
    Route { contains: contains.to_string(), method: None, seq: vec![(status, body.into())], hits: Arc::new(AtomicUsize::new(0)), delay_ms: 0 }
}

pub fn route_seq(contains: &str, seq: Vec<(u16, Vec<u8>)>) -> Route {
    Route { contains: contains.to_string(), method: None, seq, hits: Arc::new(AtomicUsize::new(0)), delay_ms: 0 }
}

pub struct Mock {
    addr: SocketAddr,
    routes: Arc<Mutex<Vec<Route>>>,
    log: Arc<Mutex<Vec<String>>>,
}

impl Mock {
    pub fn start() -> Mock {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let routes: Arc<Mutex<Vec<Route>>> = Arc::new(Mutex::new(Vec::new()));
        let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let (r2, l2) = (routes.clone(), log.clone());
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { continue };
                let (routes, log) = (r2.clone(), l2.clone());
                std::thread::spawn(move || handle(&mut stream, &routes, &log));
            }
        });
        // requests to 127.0.0.1 must never go through a system proxy
        unsafe {
            std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
            std::env::set_var("no_proxy", "127.0.0.1,localhost");
        }
        Mock { addr, routes, log }
    }

    pub fn url(&self) -> String {
        format!("http://{}", self.addr)
    }

    pub fn set(&self, routes: Vec<Route>) {
        *self.routes.lock().unwrap() = routes;
    }

    /// "<METHOD> <path+query> <body>" for every request so far.
    pub fn requests(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }
}

fn handle(stream: &mut TcpStream, routes: &Arc<Mutex<Vec<Route>>>, log: &Arc<Mutex<Vec<String>>>) {
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    let header_end = loop {
        let n = stream.read(&mut tmp).unwrap_or(0);
        if n == 0 {
            return;
        }
        buf.extend_from_slice(&tmp[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).to_string();
    let content_len: usize = head
        .lines()
        .find_map(|l| l.to_lowercase().strip_prefix("content-length:").map(|v| v.trim().parse().unwrap_or(0)))
        .unwrap_or(0);
    while buf.len() < header_end + content_len {
        let n = stream.read(&mut tmp).unwrap_or(0);
        if n == 0 {
            break;
        }
        buf.extend_from_slice(&tmp[..n]);
    }
    let body_text = String::from_utf8_lossy(&buf[header_end..]).to_string();
    let mut parts = head.lines().next().unwrap_or("").split_whitespace();
    let method = parts.next().unwrap_or("").to_string();
    let path = parts.next().unwrap_or("").to_string();
    log.lock().unwrap().push(format!("{method} {path} {body_text}"));

    let found = routes
        .lock()
        .unwrap()
        .iter()
        .find(|r| path.contains(&r.contains) && r.method.map(|m| m == method).unwrap_or(true))
        .cloned();
    let (status, body, delay) = match found {
        Some(r) => {
            let n = r.hits.fetch_add(1, Ordering::Relaxed);
            let (s, b) = r.seq[n.min(r.seq.len() - 1)].clone();
            (s, b, r.delay_ms)
        }
        None => (404, b"{\"error\":\"no mock route\"}".to_vec(), 0),
    };
    if delay > 0 {
        std::thread::sleep(std::time::Duration::from_millis(delay));
    }
    let head = format!(
        "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&body);
}
