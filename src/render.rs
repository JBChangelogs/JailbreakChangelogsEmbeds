use std::{fs, time::Duration};

use anyhow::{Context, Result};
use headless_chrome::protocol::cdp::{Emulation, Page::CaptureScreenshotFormatOption, DOM::RGBA};

use crate::{
    browser::{goto, set_viewport, settle, wait_for_content, wait_for_theme, Session},
    config::{Config, Embed},
    profile::{profile_card, theme_card, trade_ad_card, trade_card},
};

const TEMPLATE: &str = include_str!("template.html");
const VEHICLE_FONT: &[u8] = include_bytes!("jbwingdings.ttf");

pub fn render(s: &Session, cfg: &Config, embed: &Embed, scale: f64) -> Result<Vec<u8>> {
    let tab = &s.tab;
    set_viewport(tab, 1.0)?;
    let trade = match (&embed.share, &embed.trade_ad) {
        (Some(code), _) => trade_card(tab, code)?,
        (None, Some(id)) => trade_ad_card(tab, id)?,
        (None, None) => String::new(),
    };
    // A shared trade is drawn on its own, so the page itself isn't needed.
    let (card, title) = if !trade.is_empty() {
        (trade, embed.title.clone())
    } else {
        let card = match (&embed.user, &embed.theme) {
            (Some(id), _) => profile_card(tab, id)?,
            (None, Some(code)) => theme_card(tab, code)?,
            (None, None) => String::new(),
        };
        (card, screenshot_route(s, cfg, embed)?)
    };

    let html = TEMPLATE
        .replace("{title}", &escape(&title))
        .replace("{subtitle}", &escape(&embed.subtitle))
        .replace("/* {css} */", &embed.css)
        .replace("<!-- {card} -->", &card)
        .replace("{seed}", &seed(&embed.route).to_string());
    fs::write(s.tmp.join("embed.html"), html)?;
    fs::write(s.tmp.join("jbwingdings.ttf"), VEHICLE_FONT)?;

    set_viewport(tab, scale)?;
    anyhow::ensure!(goto(tab, &format!("file:///{}", s.tmp.join("embed.html").display()))?, "template didn't load");
    tab.evaluate("Promise.all([window.layoutDone, document.fonts.ready]).then(() => true)", true).context("loading fonts")?;
    tab.capture_screenshot(CaptureScreenshotFormatOption::Png, None, None, true).context("screenshotting template")
}

/// Loads the route, saves its screenshot as shot.png and returns its title.
fn screenshot_route(s: &Session, cfg: &Config, embed: &Embed) -> Result<String> {
    let tab = &s.tab;
    if !goto(tab, &format!("{}{}", cfg.base_url, embed.route))? {
        eprintln!("  warning: {} didn't finish loading, continuing", embed.route);
    }
    wait_for_content(tab, &cfg.loading)?;
    if embed.theme.is_some() {
        wait_for_theme(tab)?;
    }
    settle(tab, Duration::from_millis(embed.delay_ms.unwrap_or(cfg.delay_ms)))?;

    let title = if embed.title.is_empty() { page_title(s, cfg)? } else { embed.title.clone() };
    clean_page(s, cfg)?;
    fs::write(s.tmp.join("shot.png"), screenshot_page(s, embed.transparent)?)?;
    Ok(title)
}

fn seed(route: &str) -> u32 {
    route.bytes().fold(7u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32)) % 2147483646 + 1
}

fn page_title(s: &Session, cfg: &Config) -> Result<String> {
    let t = s.tab.evaluate("document.querySelector('meta[property=\"og:title\"]')?.content || document.title", false)?.value;
    let t = t.and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    let t = strip_site_name(&t, &cfg.site_name);
    Ok(strip_site_name(t, "Roblox Jailbreak").to_string())
}

fn strip_site_name<'a>(title: &'a str, site: &str) -> &'a str {
    if site.is_empty() {
        return title;
    }
    [" - ", " | ", " — "]
        .iter()
        .find_map(|sep| title.strip_suffix(site)?.strip_suffix(sep))
        .unwrap_or(title)
}

fn clean_page(s: &Session, cfg: &Config) -> Result<()> {
    for name in &cfg.dismiss_buttons {
        let clicked = s.tab.evaluate(&format!(
            "(() => {{ const b = [...document.querySelectorAll('button')].find(b => b.textContent.trim() === {0} || b.ariaLabel === {0}); b?.click(); return !!b }})()",
            serde_json::to_string(name)?
        ), false)?;
        if clicked.value == Some(true.into()) {
            settle(&s.tab, Duration::from_millis(1000))?;
        }
    }
    if !cfg.hide.is_empty() {
        let css = format!("{} {{ display: none !important }}", cfg.hide.join(", "));
        s.tab.evaluate(&format!("document.head.append(Object.assign(document.createElement('style'), {{ textContent: {} }}))", serde_json::to_string(&css)?), false)?;
    }
    Ok(())
}

fn screenshot_page(s: &Session, transparent: bool) -> Result<Vec<u8>> {
    let background = |color| s.tab.call_method(Emulation::SetDefaultBackgroundColorOverride { color });
    if transparent {
        background(Some(RGBA { r: 0, g: 0, b: 0, a: Some(0.0) }))?;
    }
    let shot = s.tab.capture_screenshot(CaptureScreenshotFormatOption::Png, None, None, true).context("screenshotting page");
    if transparent {
        background(None)?;
    }
    shot
}

pub fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

#[cfg(test)]
mod tests {
    use super::strip_site_name;

    #[test]
    fn strips_site_name() {
        let site = "Jailbreak Changelogs";
        assert_eq!(strip_site_name("Inventory Checker - Jailbreak Changelogs", site), "Inventory Checker");
        assert_eq!(strip_site_name("Values | Jailbreak Changelogs", site), "Values");
        assert_eq!(strip_site_name("Season 30: Darkest Winter", site), "Season 30: Darkest Winter");
        assert_eq!(strip_site_name("Jailbreak Changelogs", site), "Jailbreak Changelogs");
        assert_eq!(strip_site_name("x - Jailbreak Changelogs", ""), "x - Jailbreak Changelogs");
        assert_eq!(strip_site_name("HyperShift (HyperChrome) | Roblox Jailbreak", "Roblox Jailbreak"), "HyperShift (HyperChrome)");
    }
}
