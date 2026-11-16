mod browser;
mod config;
mod profile;
mod render;
mod server;
mod video;

use std::{
    fs,
    sync::atomic::{AtomicUsize, Ordering},
    thread,
};

use anyhow::{Context, Result};

use browser::Session;
use config::Config;

fn main() -> Result<()> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = matches!(args.first().map(String::as_str), Some("video" | "serve")).then(|| args.remove(0));
    let path = args.first().cloned().unwrap_or_else(|| "embeds.json".into());
    let cfg: Config = serde_json::from_str(&fs::read_to_string(&path).with_context(|| format!("reading {path}"))?)
        .with_context(|| format!("parsing {path}"))?;
    match cmd.as_deref() {
        Some("video") => video::make_video(&cfg),
        Some("serve") => server::serve(cfg),
        _ => pregenerate(&cfg),
    }
}

fn pregenerate(cfg: &Config) -> Result<()> {
    fs::create_dir_all(&cfg.out_dir)?;
    let next = AtomicUsize::new(0);
    let failed = AtomicUsize::new(0);
    thread::scope(|s| {
        for worker in 0..cfg.concurrency.min(cfg.embeds.len()) {
            let (next, failed) = (&next, &failed);
            s.spawn(move || {
                let run = || -> Result<()> {
                    let session = Session::launch(worker)?;
                    while let Some(embed) = cfg.embeds.get(next.fetch_add(1, Ordering::Relaxed)) {
                        let result = render::render(&session, cfg, embed, 2.0)
                            .and_then(|png| Ok(fs::write(cfg.out_dir.join(&embed.file), png)?));
                        match result {
                            Ok(()) => println!("ok   {} -> {}", embed.route, embed.file),
                            Err(e) => {
                                failed.fetch_add(1, Ordering::Relaxed);
                                eprintln!("FAIL {}: {e:#}", embed.route);
                            }
                        }
                    }
                    Ok(())
                };
                if let Err(e) = run() {
                    failed.fetch_add(1, Ordering::Relaxed);
                    eprintln!("worker {worker} died: {e:#}");
                }
            });
        }
    });
    let failed = failed.into_inner();
    anyhow::ensure!(failed == 0, "{failed} failure(s)");
    Ok(())
}
