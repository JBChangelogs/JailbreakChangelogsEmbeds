use std::{
    fs,
    path::PathBuf,
    sync::Arc,
    thread::sleep,
    time::{Duration, Instant},
};

use anyhow::Result;
use headless_chrome::{protocol::cdp::Emulation, Browser, LaunchOptions, Tab};

pub struct Session {
    _browser: Browser,
    pub tab: Arc<Tab>,
    pub tmp: PathBuf,
}

impl Session {
    pub fn launch(worker: usize) -> Result<Self> {
        let browser = Browser::new(
            LaunchOptions::default_builder()
                .window_size(Some((1920, 1080)))
                .sandbox(std::env::var_os("NO_SANDBOX").is_none())
                .idle_browser_timeout(Duration::from_secs(60 * 60 * 24 * 365))
                // each browser's profile (deleted when it closes) would otherwise grow its HTTP cache for as long as it runs
                .args(vec![std::ffi::OsStr::new("--disk-cache-size=52428800")])
                .build()?,
        )?;
        let tab = browser.new_tab()?;
        tab.set_default_timeout(Duration::from_secs(60));
        tab.call_method(Emulation::SetEmulatedMedia {
            media: None,
            features: Some(vec![Emulation::MediaFeature { name: "prefers-color-scheme".into(), value: "dark".into() }]),
        })?;
        let tmp = std::env::temp_dir().join(format!("embedgen-{worker}"));
        fs::create_dir_all(&tmp)?;
        Ok(Self { _browser: browser, tab, tmp })
    }
}

pub fn set_viewport(tab: &Tab, scale: f64) -> Result<()> {
    tab.call_method(Emulation::SetDeviceMetricsOverride {
        width: 1920,
        height: 1080,
        device_scale_factor: scale,
        mobile: false,
        scale: None,
        screen_width: None,
        screen_height: None,
        position_x: None,
        position_y: None,
        dont_set_visible_size: None,
        screen_orientation: None,
        viewport: None,
        display_feature: None,
        device_posture: None,
    })?;
    Ok(())
}

pub fn goto(tab: &Tab, url: &str) -> Result<bool> {
    let old = tab.evaluate("performance.timeOrigin", false)?.value;
    tab.navigate_to(url)?;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(15) {
        if let Ok(r) = tab.evaluate("document.readyState === 'complete' && performance.timeOrigin", false) {
            if r.value.as_ref().is_some_and(|v| v.is_number()) && r.value != old {
                return Ok(true);
            }
        }
        sleep(Duration::from_millis(25));
    }
    Ok(false)
}

pub fn settle(tab: &Tab, max: Duration) -> Result<()> {
    let start = Instant::now();
    sleep(Duration::from_millis(300));
    while start.elapsed() < max {
        let busy = tab.evaluate(
            "document.getAnimations().some(a => a.playState === 'running' && a.effect?.getComputedTiming().endTime !== Infinity)",
            false,
        )?;
        if busy.value != Some(true.into()) {
            break;
        }
        sleep(Duration::from_millis(100));
    }
    Ok(())
}

pub fn wait_for_content(tab: &Tab, loading: &[String]) -> Result<()> {
    if !loading.is_empty() {
        let selector = serde_json::to_string(&loading.join(", "))?;
        let gone = format!(
            "![...document.querySelectorAll({selector})].some(e => e.checkVisibility() && e.getBoundingClientRect().top < innerHeight && e.getBoundingClientRect().bottom > 0)"
        );
        wait_until(tab, &gone, "content")?;
    }
    wait_until(
        tab,
        "[...document.images].filter(i => i.width > 2 && i.checkVisibility() && i.getBoundingClientRect().top < innerHeight).every(i => i.complete)",
        "images",
    )
}

/// A shared theme is fetched after the page loads, then set as inline CSS variables on `<html>`.
pub fn wait_for_theme(tab: &Tab) -> Result<()> {
    wait_until(tab, "!!document.documentElement.style.getPropertyValue('--color-primary-bg')", "theme")
}

fn wait_until(tab: &Tab, expr: &str, what: &str) -> Result<()> {
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        if tab.evaluate(expr, false)?.value == Some(true.into()) {
            return Ok(());
        }
        sleep(Duration::from_millis(100));
    }
    eprintln!("  warning: {what} still loading after 10s, screenshotting anyway");
    Ok(())
}
