use std::{
    cell::RefCell,
    collections::{HashSet, VecDeque},
    fs,
    hash::{DefaultHasher, Hash, Hasher},
    io::Cursor,
    path::{Path, PathBuf},
    sync::{Arc, Condvar, LazyLock, Mutex},
    thread,
    time::{Duration, Instant},
};

use anyhow::Result;
use tiny_http::{Header, Method, Request, Response, Server};

use crate::{
    browser::Session,
    clickhouse,
    config::{is_discord_id, is_route, Config, Embed},
    render::render,
};

type Resp = Response<Cursor<Vec<u8>>>;

const PREGEN_WORKERS: usize = 4;
const PREGEN_QUEUE: usize = 512;

#[derive(Default)]
pub struct Pregen {
    state: Mutex<PregenState>,
    ready: Condvar,
}

#[derive(Default)]
struct PregenState {
    queue: VecDeque<String>,
    pending: HashSet<String>,
}

impl Pregen {
    fn enqueue(&self, route: String) -> Resp {
        if is_fresh(&cache_path(&route)) {
            return text(200, "cached");
        }
        let mut s = self.state.lock().unwrap();
        if let Some(i) = s.queue.iter().position(|r| *r == route) {
            s.queue.remove(i);
        } else if !s.pending.insert(route.clone()) {
            return text(202, "queued");
        } else if s.queue.len() >= PREGEN_QUEUE {
            if let Some(stale) = s.queue.pop_front() {
                s.pending.remove(&stale);
            }
        }
        s.queue.push_back(route);
        self.ready.notify_one();
        text(202, "queued")
    }

    fn next(&self) -> String {
        let s = self.state.lock().unwrap();
        let mut s = self.ready.wait_while(s, |s| s.queue.is_empty()).unwrap();
        s.queue.pop_back().unwrap()
    }

    fn done(&self, route: &str) {
        self.state.lock().unwrap().pending.remove(route);
    }
}

pub fn serve(cfg: Config) -> Result<()> {
    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".into());
    let server = Arc::new(Server::http(format!("0.0.0.0:{port}")).map_err(|e| anyhow::anyhow!(e))?);
    println!("listening on http://localhost:{port}");

    thread::spawn(|| loop {
        prune(&cache_dir(), CACHE_TTL, CACHE_MAX_BYTES);
        thread::sleep(Duration::from_secs(300));
    });

    let cfg = Arc::new(cfg);
    let pregen = Arc::new(Pregen::default());
    for i in 0..PREGEN_WORKERS {
        let (cfg, pregen) = (cfg.clone(), pregen.clone());
        thread::spawn(move || {
            let worker = cfg.concurrency + i;
            let mut session = Session::launch(worker).ok();
            loop {
                let route = pregen.next();
                let _ = cached_render(&mut session, worker, &cfg, &route);
                pregen.done(&route);
            }
        });
    }

    let workers: Vec<_> = (0..cfg.concurrency)
        .map(|worker| {
            let (server, cfg, pregen) = (server.clone(), cfg.clone(), pregen.clone());
            thread::spawn(move || {
                let mut session = Session::launch(worker).ok();
                for req in server.incoming_requests() {
                    let start = Instant::now();
                    let resp = handle(&req, &cfg, &pregen, worker, &mut session);
                    let row = clickhouse::enabled().then(|| clickhouse::row(&req, resp.status_code().0));
                    let _ = req.respond(resp);
                    let error = LAST_ERROR.take();
                    if let Some(mut row) = row {
                        row["duration_ms"] = (start.elapsed().as_millis() as u64).into();
                        if row["status_code"].as_u64() >= Some(500) {
                            row["error"] = error.unwrap_or_default().into();
                        }
                        clickhouse::send(row);
                    }
                }
            })
        })
        .collect();
    for w in workers {
        let _ = w.join();
    }
    Ok(())
}

fn handle(req: &Request, cfg: &Config, pregen: &Pregen, worker: usize, session: &mut Option<Session>) -> Resp {
    with_header(route(req, cfg, pregen, worker, session).with_chunked_threshold(usize::MAX), "Access-Control-Allow-Origin", "*")
}

fn route(req: &Request, cfg: &Config, pregen: &Pregen, worker: usize, session: &mut Option<Session>) -> Resp {
    if *req.method() == Method::Options {
        return with_header(text(204, ""), "Access-Control-Allow-Methods", "GET, HEAD, OPTIONS");
    }
    if !matches!(req.method(), Method::Get | Method::Head) {
        return with_header(text(405, "GET or HEAD only"), "Allow", "GET, HEAD, OPTIONS");
    }
    if let Some(rest) = req.url().strip_prefix("/pregenerate") {
        let resp = match og_route(rest) {
            Some(route) if is_route(&route) => pregen.enqueue(route),
            _ => text(404, "not found"),
        };
        return with_header(resp, "Cache-Control", "no-store");
    }
    if let Some(route) = og_route(req.url()) {
        if !is_route(&route) {
            return text(404, "not found");
        }
        return png(cached_render(session, worker, cfg, &route));
    }
    pregenerated(req.url().split('?').next().unwrap_or_default(), cfg)
}

fn cached_render(session: &mut Option<Session>, worker: usize, cfg: &Config, route: &str) -> Result<Vec<u8>> {
    let cached = cache_path(route);
    let read_fresh = || is_fresh(&cached).then(|| fs::read(&cached).ok()).flatten();
    if let Some(data) = read_fresh() {
        return Ok(data);
    }
    let mut render = || {
        let result = render_on(session, worker, cfg, &og_embed(cfg, route), 1.0);
        if let Ok(data) = &result {
            let tmp = cached.with_extension(format!("{worker}.tmp"));
            let _ = fs::create_dir_all(cache_dir()).and_then(|_| fs::write(&tmp, data)).and_then(|_| fs::rename(&tmp, &cached));
        }
        result
    };
    match exclusive(route, || read_fresh().map_or_else(&mut render, Ok)) {
        Some(result) => result,
        None => read_fresh().map_or_else(render, Ok),
    }
}

static RENDERING: LazyLock<(Mutex<HashSet<String>>, Condvar)> = LazyLock::new(Default::default);

fn exclusive<T>(route: &str, f: impl FnOnce() -> T) -> Option<T> {
    let (rendering, finished) = &*RENDERING;
    let mut set = rendering.lock().unwrap();
    if !set.insert(route.to_string()) {
        let _ = finished.wait_timeout_while(set, Duration::from_secs(60), |s| s.contains(route));
        return None;
    }
    drop(set);
    struct Done<'a>(&'a str);
    impl Drop for Done<'_> {
        fn drop(&mut self) {
            let (rendering, finished) = &*RENDERING;
            rendering.lock().unwrap().remove(self.0);
            finished.notify_all();
        }
    }
    let _done = Done(route);
    Some(f())
}

const CACHE_TTL: Duration = Duration::from_secs(3 * 60 * 60);
const CACHE_MAX_BYTES: u64 = 10 << 30;

fn cache_dir() -> PathBuf {
    std::env::temp_dir().join("embedgen-cache")
}

fn cache_path(route: &str) -> PathBuf {
    let mut h = DefaultHasher::new();
    route.hash(&mut h);
    cache_dir().join(format!("{:016x}.png", h.finish()))
}

fn is_fresh(path: &Path) -> bool {
    fs::metadata(path).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age < CACHE_TTL)
}

fn prune(dir: &Path, ttl: Duration, max_bytes: u64) {
    let Ok(entries) = fs::read_dir(dir) else { return };
    let mut files: Vec<_> = entries
        .flatten()
        .filter_map(|e| {
            let m = e.metadata().ok()?;
            Some((m.modified().ok()?, m.len(), e.path()))
        })
        .collect();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    let mut total = 0;
    for (modified, len, path) in files {
        total += len;
        if total > max_bytes || modified.elapsed().map_or(true, |age| age >= ttl) {
            let _ = fs::remove_file(path);
        }
    }
}

fn og_route(url: &str) -> Option<String> {
    let rest = url.strip_prefix("/og")?;
    let (path, query) = rest.split_once('?').unwrap_or((rest, ""));
    let (path, query) = match path.strip_suffix(".png") {
        Some(path) => (path, query),
        None => (path, query.strip_suffix(".png")?),
    };
    let path = if path.is_empty() { "/" } else { path };
    let route = if query.is_empty() { path.to_string() } else { format!("{path}?{query}") };
    Some(route.replace("%23", "#"))
}

fn og_embed(cfg: &Config, route: &str) -> Embed {
    let listed = |r: &str| cfg.embeds.iter().find(|e| e.route == r);
    let theme_page = route.split(['?', '#']).next().unwrap_or(route).strip_prefix("/themes/").filter(|c| !c.is_empty() && c.bytes().all(|b| b.is_ascii_alphanumeric()));
    let route = &theme_page.map_or_else(|| route.to_string(), |c| format!("/?theme={c}"));
    let base = route.split(['?', '#']).next().unwrap_or(route);
    let user = base.strip_prefix("/users/").filter(|id| is_discord_id(id));
    let text_from = listed(route).or_else(|| listed(base)).or_else(|| user.and(listed("/")));
    Embed {
        route: route.into(),
        title: text_from.map(|e| e.title.clone()).unwrap_or_default(),
        subtitle: text_from.map(|e| e.subtitle.clone()).unwrap_or_default(),
        delay_ms: listed(route).and_then(|e| e.delay_ms),
        transparent: listed(route).is_some_and(|e| e.transparent),
        css: listed(route).map(|e| e.css.clone()).unwrap_or_default(),
        user: user.map(str::to_string),
        theme: code_param(route, "theme"),
        share: code_param(route, "share").filter(|_| base == "/values/calculator"),
        trade_ad: base.strip_prefix("/trading/ad/").filter(|id| is_discord_id(id)).map(str::to_string),
        file: String::new(),
    }
}

fn code_param(route: &str, key: &str) -> Option<String> {
    query_param(route, key).filter(|c| !c.is_empty() && c.bytes().all(|b| b.is_ascii_alphanumeric())).map(str::to_string)
}

fn query_param<'a>(route: &'a str, key: &str) -> Option<&'a str> {
    let query = route.split_once('?')?.1.split('#').next()?;
    query.split('&').find_map(|kv| kv.strip_prefix(key)?.strip_prefix('='))
}

fn pregenerated(path: &str, cfg: &Config) -> Resp {
    let name = path.trim_start_matches('/');
    if !cfg.embeds.iter().any(|e| e.file == name) && cfg.video.file != name {
        return text(404, "not found");
    }
    match fs::read(cfg.out_dir.join(name)) {
        Ok(data) => with_header(Response::from_data(data), "Content-Type", if name.ends_with(".mp4") { "video/mp4" } else { "image/png" }),
        Err(_) => text(404, "not generated yet"),
    }
}

thread_local! {
    static LAST_ERROR: RefCell<Option<String>> = const { RefCell::new(None) };
}

fn render_on(session: &mut Option<Session>, worker: usize, cfg: &Config, embed: &Embed, scale: f64) -> Result<Vec<u8>> {
    if session.is_none() {
        *session = Some(Session::launch(worker)?);
    }
    let result = render(session.as_ref().unwrap(), cfg, embed, scale);
    match &result {
        Ok(_) => println!("rendered {}", embed.route),
        Err(e) => {
            eprintln!("FAIL {}: {e:#}", embed.route);
            LAST_ERROR.set(Some(format!("{e:#}")));
            *session = None;
        }
    }
    result
}

fn png(result: Result<Vec<u8>>) -> Resp {
    let Ok(png) = result else { return text(500, "render failed") };
    let resp = with_header(Response::from_data(png), "Content-Type", "image/png");
    with_header(resp, "Cache-Control", "public, max-age=86400")
}

fn text(code: u16, msg: &str) -> Resp {
    Response::from_string(msg).with_status_code(code)
}

fn with_header(resp: Resp, name: &str, value: &str) -> Resp {
    resp.with_header(Header::from_bytes(name, value).unwrap())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tiny_http::{HTTPVersion, TestRequest};

    #[test]
    fn query_params() {
        assert_eq!(query_param("/?theme=4DVB9KBE", "theme"), Some("4DVB9KBE"));
        assert_eq!(query_param("/values?a=1&theme=X#top", "theme"), Some("X"));
        assert_eq!(query_param("/?themes=X", "theme"), None);
        assert_eq!(query_param("/values", "theme"), None);
    }

    #[test]
    fn marketplace_theme_renders_home() {
        let cfg: Config = serde_json::from_value(serde_json::json!({
            "base_url": "https://x", "out_dir": "out", "delay_ms": 0, "dismiss_buttons": [], "embeds": [{"route": "/", "file": "home.png", "title": "Home"}]
        })).unwrap();
        let e = og_embed(&cfg, "/themes/DK32P5WE");
        assert_eq!((e.route.as_str(), e.theme.as_deref(), e.title.as_str()), ("/?theme=DK32P5WE", Some("DK32P5WE"), "Home"));
        assert_eq!(og_embed(&cfg, "/themes/a$b").theme, None);
    }

    fn wire(response: Resp, head: bool) -> String {
        let mut output = Vec::new();
        response.raw_print(&mut output, HTTPVersion(1, 1), &[], head, None).unwrap();
        String::from_utf8(output).unwrap()
    }

    #[test]
    fn public_image_metadata() {
        let out = std::env::temp_dir().join(format!("embedgen-metadata-{}", std::process::id()));
        fs::create_dir_all(&out).unwrap();
        let data = vec![b'x'; 40_000];
        fs::write(out.join("values.png"), &data).unwrap();
        fs::write(out.join("embeds.mp4"), &data).unwrap();
        let cfg: Config = serde_json::from_value(serde_json::json!({
            "base_url": "https://jailbreakchangelogs.com", "out_dir": out,
            "delay_ms": 0, "dismiss_buttons": [],
            "embeds": [{"route": "/values", "file": "values.png"}]
        })).unwrap();
        let mut session = None;
        let pregen = Pregen::default();
        for (path, content_type) in [("/values.png", "image/png"), ("/embeds.mp4", "video/mp4")] {
            for method in [Method::Get, Method::Head] {
                let head = method == Method::Head;
                let request: Request = TestRequest::new().with_method(method).with_path(path)
                    .with_header(Header::from_bytes("Origin", "http://localhost:5500").unwrap()).into();
                let output = wire(handle(&request, &cfg, &pregen, 0, &mut session), head);
                let (headers, body) = output.split_once("\r\n\r\n").unwrap();
                assert!(headers.starts_with("HTTP/1.1 200"));
                let headers = headers.to_ascii_lowercase();
                assert!(headers.contains(&format!("content-type: {content_type}\r\n")));
                assert!(headers.contains("content-length: 40000"));
                assert!(headers.contains("access-control-allow-origin: *"));
                assert!(!headers.contains("transfer-encoding"));
                assert_eq!(body.len(), if head { 0 } else { data.len() });
            }
        }
        for (method, path, status) in [
            (Method::Options, "/og/values.png", 204),
            (Method::Post, "/og/values.png", 405),
            (Method::Head, "/missing.png", 404),
            (Method::Head, "/og//invalid.png", 404),
            (Method::Get, "/pregenerate/og//invalid.png", 404),
            (Method::Get, "/pregenerate/values", 404),
            (Method::Get, "/pregenerate/og/embedgen-test.png?share=A", 202),
            (Method::Get, "/pregenerate/og/embedgen-test.png?share=A", 202),
            (Method::Get, "/pregenerate/og/embedgen-test.png?share=B", 202),
        ] {
            let request: Request = TestRequest::new().with_method(method.clone()).with_path(path).into();
            let output = wire(handle(&request, &cfg, &pregen, 0, &mut session), method == Method::Head);
            assert!(output.starts_with(&format!("HTTP/1.1 {status}")));
            assert!(output.to_ascii_lowercase().contains("access-control-allow-origin: *"));
            if method == Method::Options {
                assert!(output.contains("Access-Control-Allow-Methods: GET, HEAD, OPTIONS"));
            }
            if method == Method::Post {
                assert!(output.contains("Allow: GET, HEAD, OPTIONS"));
            }
        }
        assert_eq!(pregen.next(), "/embedgen-test?share=B");
        assert_eq!(pregen.next(), "/embedgen-test?share=A");
        assert!(session.is_none());
        let cached = cache_path("/embedgen-test?share=C");
        fs::create_dir_all(cache_dir()).unwrap();
        fs::write(&cached, b"png").unwrap();
        let request: Request = TestRequest::new().with_path("/pregenerate/og/embedgen-test.png?share=C").into();
        let output = wire(handle(&request, &cfg, &pregen, 0, &mut session), false);
        assert!(output.starts_with("HTTP/1.1 200") && output.contains("Cache-Control: no-store"));
        fs::remove_file(cached).unwrap();
        fs::remove_dir_all(out).unwrap();
    }

    #[test]
    fn rendered_png_metadata_retains_cache_policy() {
        let data = vec![b'x'; 40_000];
        let response = with_header(png(Ok(data)).with_chunked_threshold(usize::MAX), "Access-Control-Allow-Origin", "*");
        let output = wire(response, true);
        let (headers, body) = output.split_once("\r\n\r\n").unwrap();
        assert!(headers.contains("Content-Type: image/png"));
        assert!(headers.contains("Content-Length: 40000"));
        assert!(headers.contains("Cache-Control: public, max-age=86400"));
        assert!(body.is_empty());
    }

    #[test]
    fn renders_once_while_in_flight() {
        let first = thread::spawn(|| exclusive("/inflight-test", || thread::sleep(Duration::from_millis(300))));
        thread::sleep(Duration::from_millis(50));
        let start = std::time::Instant::now();
        assert_eq!(exclusive("/inflight-test", || "rendered twice"), None);
        assert!(start.elapsed() >= Duration::from_millis(200));
        assert_eq!(first.join().unwrap(), Some(()));
        assert_eq!(exclusive("/inflight-test", || 1), Some(1));
        assert_eq!(exclusive("/other", || 2), Some(2));
    }

    #[test]
    fn prunes_cache() {
        let dir = std::env::temp_dir().join(format!("embedgen-prune-{}", std::process::id()));
        fs::create_dir_all(&dir).unwrap();
        for name in ["old", "mid", "new"] {
            fs::write(dir.join(name), [0u8; 10]).unwrap();
            thread::sleep(Duration::from_millis(20));
        }
        prune(&dir, Duration::from_secs(60), 25);
        let left = |n: &str| dir.join(n).exists();
        assert!(!left("old") && left("mid") && left("new"));
        prune(&dir, Duration::ZERO, u64::MAX);
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 0);
        fs::remove_dir_all(dir).unwrap();
        assert_ne!(cache_path("/values?share=A"), cache_path("/values?share=B"));
    }

    #[test]
    fn non_ascii_request_gets_a_response() {
        use std::io::{Read, Write};
        let server = Server::http("127.0.0.1:0").unwrap();
        let addr = server.server_addr().to_ip().unwrap();
        thread::spawn(move || {
            for r in server.incoming_requests() {
                let url = r.url().to_string();
                let _ = r.respond(Response::from_string(url));
            }
        });
        let mut s = std::net::TcpStream::connect(addr).unwrap();
        s.write_all("GET /item/caf\u{e9} HTTP/1.1\r\nUser-Agent: caf\u{e9}\r\nConnection: close\r\n\r\n".as_bytes()).unwrap();
        let mut out = String::new();
        s.read_to_string(&mut out).unwrap();
        assert!(out.starts_with("HTTP/1.1 200") && out.ends_with("/item/caf%C3%A9"), "{out}");
    }

    #[test]
    fn og_routes() {
        let r = |u| og_route(u);
        assert_eq!(r("/og.png").as_deref(), Some("/"));
        assert_eq!(r("/og/values.png").as_deref(), Some("/values"));
        assert_eq!(r("/og/item/HyperChrome/HyperShift?tab=comments.png").as_deref(), Some("/item/HyperChrome/HyperShift?tab=comments"));
        assert_eq!(r("/og/item/HyperChrome/HyperShift.png?tab=comments").as_deref(), Some("/item/HyperChrome/HyperShift?tab=comments"));
        assert_eq!(r("/og/values%23top.png").as_deref(), Some("/values#top"));
        assert_eq!(r("/og/values?a=1%23top.png").as_deref(), Some("/values?a=1#top"));
        assert_eq!(r("/og/values"), None);
        assert_eq!(r("/og/values?a=1"), None);
        assert_eq!(r("/values.png"), None);
    }
}
