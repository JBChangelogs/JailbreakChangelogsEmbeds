use std::{collections::HashMap, fs, io::Cursor, sync::Arc, thread};

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
    let token = std::env::var("RENDER_TOKEN").ok();
    let server = Arc::new(Server::http(format!("0.0.0.0:{port}")).map_err(|e| anyhow::anyhow!(e))?);
    println!("listening on http://localhost:{port}");

    let cfg = Arc::new(cfg);
    let workers: Vec<_> = (0..cfg.concurrency)
        .map(|worker| {
            let (server, cfg, token) = (server.clone(), cfg.clone(), token.clone());
            thread::spawn(move || {
                let mut session = None;
                for req in server.incoming_requests() {
                    let resp = handle(&req, &cfg, token.as_deref(), worker, &mut session);
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

fn handle(req: &Request, cfg: &Config, token: Option<&str>, worker: usize, session: &mut Option<Session>) -> Resp {
    if *req.method() != Method::Get {
        return text(405, "GET only");
    }
    let (path, query) = req.url().split_once('?').unwrap_or((req.url(), ""));

    if let Some(route) = path.strip_prefix("/og").and_then(|p| p.strip_suffix(".png")) {
        let route = if route.is_empty() { "/" } else { route };
        if !is_route(route) {
            return text(404, "not found");
        }
        return png(render_on(session, worker, cfg, &og_embed(cfg, route), 1.0), true);
    }
    if path == "/render" {
        return render_request(query, cfg, token, worker, session);
    }
    pregenerated(path, cfg)
}

fn og_embed(cfg: &Config, route: &str) -> Embed {
    let listed = |r: &str| cfg.embeds.iter().find(|e| e.route == r);
    let user = route.strip_prefix("/users/").filter(|id| is_discord_id(id));
    let text_from = listed(route).or_else(|| user.and(listed("/")));
    Embed {
        route: route.into(),
        title: text_from.map(|e| e.title.clone()).unwrap_or_default(),
        subtitle: text_from.map(|e| e.subtitle.clone()).unwrap_or_default(),
        delay_ms: listed(route).and_then(|e| e.delay_ms),
        transparent: listed(route).is_some_and(|e| e.transparent),
        css: listed(route).map(|e| e.css.clone()).unwrap_or_default(),
        user: user.map(str::to_string),
        file: String::new(),
    }
}

fn render_request(query: &str, cfg: &Config, token: Option<&str>, worker: usize, session: &mut Option<Session>) -> Resp {
    let q: HashMap<_, _> = form_urlencoded::parse(query.as_bytes()).into_owned().collect();
    if token.is_some_and(|t| q.get("token").map(String::as_str) != Some(t)) {
        return text(401, "bad token");
    }
    let user = q.get("user").cloned();
    if user.as_deref().is_some_and(|u| !is_discord_id(u)) {
        return text(400, "user must be a discord id");
    }
    let route = q.get("route").cloned().or_else(|| user.as_ref().map(|u| format!("/users/{u}"))).unwrap_or_default();
    if !is_route(&route) {
        return text(400, "route must be a path like /values");
    }
    let embed = Embed {
        route,
        title: q.get("title").cloned().unwrap_or_default(),
        subtitle: q.get("subtitle").cloned().unwrap_or_default(),
        delay_ms: q.get("delay_ms").and_then(|d| d.parse().ok()).map(|d: u64| d.min(30_000)),
        user,
        ..Default::default()
    };
    png(render_on(session, worker, cfg, &embed, 2.0), false)
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

fn png(result: Result<Vec<u8>>, cache: bool) -> Resp {
    let Ok(png) = result else { return text(500, "render failed") };
    let resp = with_header(Response::from_data(png), "Content-Type", "image/png");
    if cache { with_header(resp, "Cache-Control", "public, max-age=86400") } else { resp }
}

fn text(code: u16, msg: &str) -> Resp {
    Response::from_string(msg).with_status_code(code)
}

fn with_header(resp: Resp, name: &str, value: &str) -> Resp {
    resp.with_header(Header::from_bytes(name, value).unwrap())
}
