use std::path::PathBuf;

use serde::Deserialize;

#[derive(Deserialize)]
pub struct Config {
    pub base_url: String,
    pub out_dir: PathBuf,
    pub delay_ms: u64,
    pub dismiss_buttons: Vec<String>,
    #[serde(default)]
    pub hide: Vec<String>,
    #[serde(default)]
    pub site_name: String,
    #[serde(default = "default_concurrency")]
    pub concurrency: usize,
    #[serde(default)]
    pub video: Video,
    pub embeds: Vec<Embed>,
}

fn default_concurrency() -> usize {
    4
}

#[derive(Deserialize)]
#[serde(default)]
pub struct Video {
    pub file: String,
    pub hold_secs: f64,
    pub fade_secs: f64,
}

impl Default for Video {
    fn default() -> Self {
        Self { file: "embeds.mp4".into(), hold_secs: 3.0, fade_secs: 1.0 }
    }
}

#[derive(Deserialize, Default)]
#[serde(default)]
pub struct Embed {
    pub route: String,
    pub title: String,
    pub subtitle: String,
    pub file: String,
    pub delay_ms: Option<u64>,
    pub transparent: bool,
    pub css: String,
    pub user: Option<String>,
}

pub fn is_discord_id(s: &str) -> bool {
    !s.is_empty() && s.bytes().all(|b| b.is_ascii_digit())
}

pub fn is_route(s: &str) -> bool {
    s.starts_with('/') && !s.starts_with("//")
}
