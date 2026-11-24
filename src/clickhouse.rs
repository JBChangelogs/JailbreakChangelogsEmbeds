use std::{
    collections::hash_map::RandomState,
    hash::BuildHasher,
    io::{Read, Write},
    net::TcpStream,
    sync::{
        mpsc::{sync_channel, Receiver, SyncSender},
        OnceLock,
    },
    thread,
    time::Duration,
};

use serde_json::{json, Value};
use tiny_http::Request;

static TX: OnceLock<SyncSender<Value>> = OnceLock::new();

struct Target {
    addr: String,
    user: String,
    pass: String,
    db: String,
}

fn parse(url: &str) -> Option<Target> {
    let rest = url.strip_prefix("http://")?;
    let (auth, hostpath) = rest.rsplit_once('@').unwrap_or(("", rest));
    let (user, pass) = auth.split_once(':').unwrap_or((auth, ""));
    let (host, db) = hostpath.split_once('/').unwrap_or((hostpath, ""));
    let addr = if host.contains(':') { host.to_string() } else { format!("{host}:8123") };
    let user = if user.is_empty() { "default" } else { user };
    let db = db.trim_end_matches('/');
    Some(Target { addr, user: decode(user), pass: decode(pass), db: if db.is_empty() { "default".into() } else { decode(db) } })
}

fn decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match (b[i], s.get(i + 1..i + 3).and_then(|h| u8::from_str_radix(h, 16).ok())) {
            (b'%', Some(v)) => {
                out.push(v);
                i += 3;
            }
            (c, _) => {
                out.push(c);
                i += 1;
            }
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn init() {
    let Ok(url) = std::env::var("CLICKHOUSE_URL") else { return };
    let Some(target) = parse(&url) else {
        eprintln!("clickhouse: CLICKHOUSE_URL must be http://user:pass@host:port/db, logging off");
        return;
    };
    let (tx, rx) = sync_channel(10_000);
    let _ = TX.set(tx);
    thread::spawn(move || flush_loop(&target, &rx));
    println!("clickhouse request logging on");
}

pub fn enabled() -> bool {
    TX.get().is_some()
}

// ponytail: drops rows when ClickHouse is down and the queue fills; no retry, add one if gaps matter.
fn flush_loop(t: &Target, rx: &Receiver<Value>) {
    while let Ok(first) = rx.recv() {
        thread::sleep(Duration::from_millis(500));
        let mut body = first.to_string();
        for row in rx.try_iter().take(5_000) {
            body.push('\n');
            body.push_str(&row.to_string());
        }
        if let Err(e) = post(t, &body) {
            eprintln!("clickhouse insert failed: {e}");
        }
    }
}

fn post(t: &Target, body: &str) -> std::io::Result<()> {
    let mut s = TcpStream::connect(&t.addr)?;
    s.set_read_timeout(Some(Duration::from_secs(10)))?;
    let query = "INSERT%20INTO%20request_logs%20FORMAT%20JSONEachRow";
    let req = format!(
        "POST /?database={}&query={query} HTTP/1.1\r\nHost: {}\r\nX-ClickHouse-User: {}\r\nX-ClickHouse-Key: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        t.db, t.addr, t.user, t.pass, body.len()
    );
    s.write_all(req.as_bytes())?;
    let mut resp = String::new();
    s.read_to_string(&mut resp)?;
    if resp.starts_with("HTTP/1.1 200") {
        Ok(())
    } else {
        Err(std::io::Error::other(resp.chars().take(500).collect::<String>()))
    }
}

pub fn row(req: &Request, status: u16) -> Value {
    let header = |name: &'static str| req.headers().iter().find(|h| h.field.equiv(name)).map(|h| h.value.as_str().to_string());
    let ip = header("CF-Connecting-IP")
        .or_else(|| header("X-Forwarded-For").and_then(|v| v.split(',').next().map(|s| s.trim().to_string())))
        .or_else(|| req.remote_addr().map(|a| a.ip().to_string()))
        .unwrap_or_default();
    let (path, query) = req.url().split_once('?').unwrap_or((req.url(), ""));
    let id = |seed| RandomState::new().hash_one(seed);
    json!({
        "method": req.method().as_str(),
        "path": path,
        "query": query,
        "status_code": status,
        "ip": ip,
        "user_agent": header("User-Agent").unwrap_or_default(),
        "request_id": format!("{:016x}{:016x}", id(1), id(2)),
        "instance": std::env::var("HOSTNAME").unwrap_or_default(),
        "service": "embeds",
    })
}

pub fn send(row: Value) {
    if let Some(tx) = TX.get() {
        let _ = tx.try_send(row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_url() {
        let t = parse("http://bob:p%40ss@ch.example:8124/logs").unwrap();
        assert_eq!((t.addr.as_str(), t.user.as_str(), t.pass.as_str(), t.db.as_str()), ("ch.example:8124", "bob", "p@ss", "logs"));
        let t = parse("http://ch").unwrap();
        assert_eq!((t.addr.as_str(), t.user.as_str(), t.db.as_str()), ("ch:8123", "default", "default"));
        assert!(parse("https://ch").is_none());
    }

    #[test]
    fn posts_insert() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let t = parse(&format!("http://u:k@{}/db", listener.local_addr().unwrap())).unwrap();
        let server = thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            let (mut got, mut buf) = (Vec::new(), [0; 4096]);
            while !got.ends_with(b"{\"a\":1}") {
                let n = s.read(&mut buf).unwrap();
                got.extend_from_slice(&buf[..n]);
            }
            s.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").unwrap();
            String::from_utf8_lossy(&got).into_owned()
        });
        post(&t, "{\"a\":1}").unwrap();
        let req = server.join().unwrap();
        assert!(req.starts_with("POST /?database=db&query=INSERT%20INTO%20request_logs%20FORMAT%20JSONEachRow HTTP/1.1\r\n"));
        assert!(req.contains("X-ClickHouse-User: u\r\n") && req.contains("X-ClickHouse-Key: k\r\n") && req.ends_with("\r\n\r\n{\"a\":1}"));
    }
}
