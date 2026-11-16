use std::{fs, time::Duration};

use anyhow::{Context, Result};
use headless_chrome::protocol::cdp::{Emulation, Page::CaptureScreenshotFormatOption, DOM::RGBA};

use crate::{
    browser::{goto, set_viewport, settle, wait_for_images, Session},
    config::{Config, Embed},
    profile::profile_card,
};

const TEMPLATE: &str = include_str!("template.html");

pub fn render(s: &Session, cfg: &Config, embed: &Embed, scale: f64) -> Result<Vec<u8>> {
    let tab = &s.tab;
    set_viewport(tab, 1.0)?;
    let card = match &embed.user {
        Some(id) => profile_card(tab, id)?,
        None => String::new(),
    };

    if !goto(tab, &format!("{}{}", cfg.base_url, embed.route))? {
        eprintln!("  warning: {} didn't finish loading, continuing", embed.route);
    }
    wait_for_images(tab)?;
    settle(tab, Duration::from_millis(embed.delay_ms.unwrap_or(cfg.delay_ms)))?;

    let title = if embed.title.is_empty() { page_title(s, cfg)? } else { embed.title.clone() };
    clean_page(s, cfg)?;
    fs::write(s.tmp.join("shot.png"), screenshot_page(s, embed.transparent)?)?;

    let html = TEMPLATE
        .replace("{title}", &escape(&title))
        .replace("{subtitle}", &escape(&embed.subtitle))
        .replace("/* {css} */", &embed.css)
        .replace("<!-- {card} -->", &card);
    fs::write(s.tmp.join("embed.html"), html)?;

    set_viewport(tab, scale)?;
    anyhow::ensure!(goto(tab, &format!("file:///{}", s.tmp.join("embed.html").display()))?, "template didn't load");
    tab.evaluate("document.fonts.ready.then(() => true)", true).context("loading fonts")?;
    tab.capture_screenshot(CaptureScreenshotFormatOption::Png, None, None, true).context("screenshotting template")
}

fn page_title(s: &Session, cfg: &Config) -> Result<String> {
    let t = s.tab.evaluate("document.querySelector('meta[property=\"og:title\"]')?.content || document.title", false)?.value;
    let t = t.and_then(|v| v.as_str().map(str::to_string)).unwrap_or_default();
    Ok(strip_site_name(&t, &cfg.site_name).to_string())
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
    }
}
