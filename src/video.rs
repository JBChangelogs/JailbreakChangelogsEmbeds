use std::process::Command;

use anyhow::{Context, Result};

use crate::config::{Config, Video};

pub fn make_video(cfg: &Config) -> Result<()> {
    let Video { file, hold_secs: hold, fade_secs: fade } = &cfg.video;
    let mut cmd = Command::new("ffmpeg");
    cmd.args(["-y", "-hide_banner", "-loglevel", "error"]);
    for embed in &cfg.embeds {
        let img = cfg.out_dir.join(&embed.file);
        anyhow::ensure!(img.exists(), "{} is missing, generate the images first", img.display());
        cmd.args(["-loop", "1", "-framerate", "30", "-t", &(hold + fade).to_string(), "-i"]).arg(img);
    }

    let mut filter = String::new();
    let mut last = "0:v".to_string();
    for k in 1..cfg.embeds.len() {
        filter += &format!("[{last}][{k}:v]xfade=transition=fade:duration={fade}:offset={}[v{k}];", k as f64 * hold);
        last = format!("v{k}");
    }
    filter += &format!("[{last}]format=yuv420p[out]");

    let out = cfg.out_dir.join(file);
    let status = cmd
        .args(["-filter_complex", &filter, "-map", "[out]", "-c:v", "libx264", "-crf", "18", "-movflags", "+faststart"])
        .arg(&out)
        .status()
        .context("running ffmpeg (is it installed and on PATH?)")?;
    anyhow::ensure!(status.success(), "ffmpeg failed");
    println!("wrote {}", out.display());
    Ok(())
}
