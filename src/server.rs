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
    if *req.method() != Method::Get {
        return text(405, "GET only");
    }
    let path = req.url().split('?').next().unwrap_or_default();

    if let Some(route) = path.strip_prefix("/og").and_then(|p| p.strip_suffix(".png")) {
        let route = if route.is_empty() { "/" } else { route };
        if !is_route(route) {
            return text(404, "not found");
        }
        return png(render_on(session, worker, cfg, &og_embed(cfg, route), 1.0));
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
