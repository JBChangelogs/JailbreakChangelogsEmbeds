use std::{fs, io::Cursor, sync::Arc, thread};

use anyhow::Result;
use tiny_http::{Header, Method, Request, Response, Server};

use crate::{
    browser::Session,
    config::{is_discord_id, is_route, Config, Embed},
    render::render,
};

type Resp = Response<Cursor<Vec<u8>>>;

pub fn serve(cfg: Config) -> Result<()> {
    let port = std::env::var("PORT").unwrap_or_else(|_| "8080".into());
    let server = Arc::new(Server::http(format!("0.0.0.0:{port}")).map_err(|e| anyhow::anyhow!(e))?);
    println!("listening on http://localhost:{port}");

    let cfg = Arc::new(cfg);
    let workers: Vec<_> = (0..cfg.concurrency)
        .map(|worker| {
            let (server, cfg) = (server.clone(), cfg.clone());
            thread::spawn(move || {
                let mut session = None;
                for req in server.incoming_requests() {
                    let resp = handle(&req, &cfg, worker, &mut session);
                    let _ = req.respond(resp);
                }
            })
        })
        .collect();
    for w in workers {
        let _ = w.join();
    }
    Ok(())
}

fn handle(req: &Request, cfg: &Config, worker: usize, session: &mut Option<Session>) -> Resp {
    // Preserve Content-Length even above tiny_http's default chunking threshold.
    with_header(route(req, cfg, worker, session).with_chunked_threshold(usize::MAX), "Access-Control-Allow-Origin", "*")
}

fn route(req: &Request, cfg: &Config, worker: usize, session: &mut Option<Session>) -> Resp {
    if *req.method() == Method::Options {
        return with_header(text(204, ""), "Access-Control-Allow-Methods", "GET, HEAD, OPTIONS");
    }
    if !matches!(req.method(), Method::Get | Method::Head) {
        return with_header(text(405, "GET or HEAD only"), "Allow", "GET, HEAD, OPTIONS");
    }
    if let Some(route) = og_route(req.url()) {
        if !is_route(&route) {
            return text(404, "not found");
        }
        return png(render_on(session, worker, cfg, &og_embed(cfg, &route), 1.0));
    }
    pregenerated(req.url().split('?').next().unwrap_or_default(), cfg)
}

/// `/og/a/b.png?x=1` or `/og/a/b?x=1.png` -> `/a/b?x=1`. Browsers never send a raw `#`,
/// so fragments come in as `%23` and are turned back into `#` here.
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
        theme: query_param(route, "theme").filter(|c| !c.is_empty() && c.bytes().all(|b| b.is_ascii_alphanumeric())).map(str::to_string),
        file: String::new(),
    }
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

fn render_on(session: &mut Option<Session>, worker: usize, cfg: &Config, embed: &Embed, scale: f64) -> Result<Vec<u8>> {
    if session.is_none() {
        *session = Some(Session::launch(worker)?);
    }
    let result = render(session.as_ref().unwrap(), cfg, embed, scale);
    match &result {
        Ok(_) => println!("rendered {}", embed.route),
        Err(e) => {
            eprintln!("FAIL {}: {e:#}", embed.route);
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
        for (path, content_type) in [("/values.png", "image/png"), ("/embeds.mp4", "video/mp4")] {
            for method in [Method::Get, Method::Head] {
                let head = method == Method::Head;
                let request: Request = TestRequest::new().with_method(method).with_path(path)
                    .with_header(Header::from_bytes("Origin", "http://localhost:5500").unwrap()).into();
                let output = wire(handle(&request, &cfg, 0, &mut session), head);
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
        ] {
            let request: Request = TestRequest::new().with_method(method.clone()).with_path(path).into();
            let output = wire(handle(&request, &cfg, 0, &mut session), method == Method::Head);
            assert!(output.starts_with(&format!("HTTP/1.1 {status}")));
            assert!(output.to_ascii_lowercase().contains("access-control-allow-origin: *"));
            if method == Method::Options {
                assert!(output.contains("Access-Control-Allow-Methods: GET, HEAD, OPTIONS"));
            }
            if method == Method::Post {
                assert!(output.contains("Allow: GET, HEAD, OPTIONS"));
            }
        }
        assert!(session.is_none());
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
