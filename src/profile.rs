use anyhow::{Context, Result};
use headless_chrome::Tab;
use serde_json::Value;

use crate::{browser::goto, render::escape};

const DEFAULT_ACCENT: u32 = 0x1d7cf2;

/// GETs an API path; None when it answers with an error like `{"detail": "Not Found"}`.
fn api(tab: &Tab, path: &str) -> Result<Option<Value>> {
    anyhow::ensure!(goto(tab, &format!("https://api.jailbreakchangelogs.com{path}"))?, "{path} didn't load");
    let body = tab.evaluate("document.body.innerText", false)?.value.context("empty API response")?;
    let v: Value = serde_json::from_str(body.as_str().unwrap_or_default()).with_context(|| format!("parsing {path}"))?;
    if !v.is_object() || v.get("detail").is_some() {
        eprintln!("  no card for {path}: {v}");
        return Ok(None);
    }
    Ok(Some(v))
}

fn display_name(u: &Value) -> Option<&str> {
    let field = |k: &str| u[k].as_str().filter(|s| !s.is_empty() && *s != "None");
    field("global_name").or_else(|| field("username"))
}

pub fn theme_card(tab: &Tab, code: &str) -> Result<String> {
    let Some(t) = api(tab, &format!("/v2/themes/shared/{code}"))? else { return Ok(String::new()) };
    let var = |k: &str, fallback: &str| css_value(t["vars"][k].as_str().unwrap_or_default()).unwrap_or(fallback).to_string();
    let (bg, accent) = (var("--color-primary-bg", "#101014"), var("--color-highlight", "#1d7cf2"));
    let (button, button_text) = (var("--color-form-button", &accent), var("--color-form-button-text", "#fff"));

    let label = t["label"].as_str().unwrap_or(code);
    let author = match t["user_id"].as_str() {
        Some(id) => api(tab, &format!("/v2/users/{id}"))?.and_then(|u| display_name(&u).map(str::to_string)),
        None => None,
    };
    let icon = t["icon"].as_str().filter(|i| i.bytes().all(|b| b.is_ascii_alphanumeric() || b == b':' || b == b'-')).unwrap_or("lucide:palette");
    let icon_color = if accent.starts_with('#') { accent.replace('#', "%23") } else { "white".into() };
    let icon_url = format!("https://api.iconify.design/{}.svg?color={icon_color}", icon.replacen(':', "/", 1));

    Ok(format!(
        "<div class=\"card\" style=\"--underglow:{accent}\"><div class=\"avatar\" style=\"background:{bg};display:grid;place-items:center;outline:2px solid #ffffff14\">\
         <img src=\"{}\" style=\"width:110px;height:110px\"></div><div><div class=\"name\">{}</div><div class=\"handle\">{}</div>\
         <span class=\"pill\" style=\"background:{button};color:{button_text}\">Theme {}</span></div></div>",
        escape(&icon_url),
        escape(label),
        escape(&author.map(|a| format!("by {a}")).unwrap_or_default()),
        escape(code)
    ))
}

/// A theme colour is only used if it's a plain colour value, so it can't inject other CSS.
fn css_value(s: &str) -> Option<&str> {
    let ok = !s.is_empty() && s.bytes().all(|b| b.is_ascii_alphanumeric() || b" #(),.%-".contains(&b));
    ok.then_some(s)
}

pub fn profile_card(tab: &Tab, id: &str) -> Result<String> {
    let Some(u) = api(tab, &format!("/v2/users/{id}"))? else { return Ok(String::new()) };

    let field = |k: &str| u[k].as_str().filter(|s| !s.is_empty() && *s != "None").map(str::to_string);
    let color = |k: &str| u[k].as_u64().or_else(|| u[k].as_str()?.parse().ok()).map(|c| c as u32 & 0xffffff);
    let username = field("username").with_context(|| format!("no user {id}"))?;
    let name = display_name(&u).unwrap_or(&username).to_string();
    let avatar = field("custom_avatar")
        .filter(|_| u["settings_v2"]["custom_avatar"].as_bool() == Some(true))
        .or_else(|| field("avatar").map(|a| format!("https://cdn.discordapp.com/avatars/{id}/{a}.png?size=512")))
        .unwrap_or_else(|| "https://cdn.discordapp.com/embed/avatars/0.png".into());

    let accents: Vec<u32> = match (color("accent_color"), color("accent_gradient")) {
        (Some(a), Some(b)) => vec![a, b],
        (Some(a), None) => vec![a],
        _ => vec![DEFAULT_ACCENT],
    };
    let background = match accents.as_slice() {
        [a, b] => format!("linear-gradient(135deg, {}, {})", hex(*a), hex(*b)),
        _ => hex(accents[0]),
    };
    let badge = u["usernumber"]
        .as_u64()
        .map(|n| format!("<span class=\"pill\" style=\"background:{background};color:{}\">User #{n}</span>", text_color(&accents)))
        .unwrap_or_default();
    let glow = color("underglow").map(|c| format!(" style=\"--underglow:{}\"", hex(c))).unwrap_or_default();

    Ok(format!(
        "<div class=\"card\"{glow}><img class=\"avatar\" src=\"{}\"><div><div class=\"name\">{}</div><div class=\"handle\">@{}</div>{badge}</div></div>",
        escape(&avatar),
        escape(&name),
        escape(&username)
    ))
}

fn hex(c: u32) -> String {
    format!("#{c:06x}")
}

fn text_color(backgrounds: &[u32]) -> &'static str {
    let worst = |text: u32| backgrounds.iter().map(|&b| contrast(text, b)).fold(f64::INFINITY, f64::min);
    if worst(0xffffff) >= 3.0 || worst(0xffffff) >= worst(0x0b0b0f) { "#fff" } else { "#0b0b0f" }
}

fn contrast(a: u32, b: u32) -> f64 {
    let (la, lb) = (luminance(a), luminance(b));
    (la.max(lb) + 0.05) / (la.min(lb) + 0.05)
}

fn luminance(c: u32) -> f64 {
    let channel = |shift: u32| {
        let v = ((c >> shift) & 0xff) as f64 / 255.0;
        if v <= 0.03928 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) }
    };
    0.2126 * channel(16) + 0.7152 * channel(8) + 0.0722 * channel(0)
}

#[cfg(test)]
mod tests {
    use super::{css_value, text_color};

    #[test]
    fn only_plain_css_colours() {
        assert_eq!(css_value("#ff7a1a"), Some("#ff7a1a"));
        assert_eq!(css_value("hsl(260, 3%, 18%)"), Some("hsl(260, 3%, 18%)"));
        assert_eq!(css_value("red;background:url(x)"), None);
        assert_eq!(css_value("\"><script>"), None);
        assert_eq!(css_value(""), None);
    }

    #[test]
    fn picks_readable_text() {
        assert_eq!(text_color(&[0x1d7cf2]), "#fff");
        assert_eq!(text_color(&[0xa8b2b9]), "#0b0b0f");
        assert_eq!(text_color(&[0x101014]), "#fff");
        assert_eq!(text_color(&[0xffff00]), "#0b0b0f");
        assert_eq!(text_color(&[0xa8b2b9, 0x6f79bf]), "#0b0b0f");
    }
}
